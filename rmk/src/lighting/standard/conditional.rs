#[allow(unused_imports)]
use super::*;
use crate::lighting::Rgb8;
use crate::lighting::compositor::{Contribution, LightingSource, RenderInput as SourceRenderInput};
use crate::lighting::context::LightingContextProvider;
use crate::lighting::effect::{BuiltinEffect, LightingEffect};
use crate::lighting::packed::{Matches, Record, pack_record, record_len, record_slot, records, unpack_record};
use crate::lighting::source::{BatteryStatusProvider, ConditionSet};
use crate::lighting::topology::LedSlot;

/// One ordered, runtime-authored conditional rule.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConditionalSceneCell {
    pub conditions: ConditionSet,
    pub slot: LedSlot,
    pub effect: BuiltinEffect,
}

pub(super) const EMPTY_CONDITIONAL_SCENE_CELL: RuntimeConditionalSceneCell = RuntimeConditionalSceneCell {
    conditions: ConditionSet {
        layer: None,
        battery: None,
        connection: None,
        output_mode: None,
        effects: None,
        layers: None,
        indicators: None,
        maintenance: None,
        split_transport: None,
    },
    slot: LedSlot(0),
    effect: BuiltinEffect::Solid { color: Rgb8::BLACK },
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConditionalSceneChunk {
    cells: [RuntimeConditionalSceneCell; CONDITIONAL_SCENE_CHUNK_SIZE],
    len: usize,
}

impl RuntimeConditionalSceneChunk {
    pub const fn new() -> Self {
        Self {
            cells: [EMPTY_CONDITIONAL_SCENE_CELL; CONDITIONAL_SCENE_CHUNK_SIZE],
            len: 0,
        }
    }

    pub fn push(&mut self, cell: RuntimeConditionalSceneCell) -> Result<(), StandardError> {
        if self.len == CONDITIONAL_SCENE_CHUNK_SIZE {
            return Err(StandardError::InvalidConditionalSceneRequest);
        }
        self.cells[self.len] = cell;
        self.len += 1;
        Ok(())
    }

    pub fn as_slice(&self) -> &[RuntimeConditionalSceneCell] {
        &self.cells[..self.len]
    }
}

impl Default for RuntimeConditionalSceneChunk {
    fn default() -> Self {
        Self::new()
    }
}

/// Bytes a [`RuntimeConditionalSceneTable`] reserves per rule.
///
/// Rules are stored as [packed records](crate::lighting::packed) carrying only
/// the predicates they name, so this is an average budget, not a per-rule
/// limit: a layer-gated highlight packs to 7 bytes and a layer-gated battery or
/// connection indicator to 11 or 12.
pub const CONDITIONAL_RULE_BYTES: usize = 12;

/// Fixed-capacity ordered conditional source. Order is semantic: later
/// matching rules override earlier rules at the same compositor priority.
///
/// `CAP` rules share `CAP * CONDITIONAL_RULE_BYTES` bytes of packed records,
/// with each rule's effect interned in the table's [`StylePool`].
#[derive(Copy, Clone, Debug)]
pub struct RuntimeConditionalSceneTable<const CAP: usize> {
    bytes: [[u8; CONDITIONAL_RULE_BYTES]; CAP],
    used: usize,
    /// Where the most recently pushed record starts.
    last: usize,
    len: usize,
    pub(super) pool: StylePool,
}

impl<const CAP: usize> RuntimeConditionalSceneTable<CAP> {
    pub const fn new() -> Self {
        // Render keeps matched record offsets as u16.
        const { core::assert!(CAP * CONDITIONAL_RULE_BYTES <= u16::MAX as usize) };
        Self {
            bytes: [[0; CONDITIONAL_RULE_BYTES]; CAP],
            used: 0,
            last: 0,
            len: 0,
            pool: StylePool::new(),
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bytes of the packed record budget currently in use.
    pub const fn bytes_used(&self) -> usize {
        self.used
    }

    fn records(&self) -> impl Iterator<Item = (usize, Record)> + '_ {
        records(&self.bytes.as_flattened()[..self.used])
    }

    /// Installed rules in their semantic order, with effects resolved from the
    /// pool. Style indices are an allocation detail and never escape the table.
    pub fn iter(&self) -> impl Iterator<Item = RuntimeConditionalSceneCell> + '_ {
        self.records().filter_map(move |(_, record)| {
            self.pool.get(record.style).map(|effect| RuntimeConditionalSceneCell {
                conditions: record.conditions,
                slot: record.slot,
                effect,
            })
        })
    }

    /// Write a packed record at `at`, replacing everything from there on.
    fn store(&mut self, at: usize, slot: LedSlot, style: u8, conditions: &ConditionSet) -> Result<(), StandardError> {
        let end = at + record_len(conditions);
        let bytes = self.bytes.as_flattened_mut();
        if end > bytes.len() {
            return Err(StandardError::ConditionalSceneFull { capacity: CAP });
        }
        pack_record(bytes, at, slot, style, conditions);
        self.last = at;
        self.used = end;
        Ok(())
    }

    pub fn push(&mut self, cell: RuntimeConditionalSceneCell) -> Result<(), StandardError> {
        if self.len == CAP {
            return Err(StandardError::ConditionalSceneFull { capacity: CAP });
        }
        // Reserve the bytes before interning, so a rule that does not fit leaves
        // the pool exactly as it was.
        let (last, used) = (self.last, self.used);
        self.store(used, cell.slot, 0, &cell.conditions)?;
        let Some(style) = self.pool.intern(cell.effect) else {
            (self.last, self.used) = (last, used);
            return Err(StandardError::ConditionalSceneFull {
                capacity: SCENE_STYLE_CAP,
            });
        };
        self.bytes.as_flattened_mut()[used + 2] = style;
        self.len += 1;
        Ok(())
    }

    /// Amend the most recently pushed rule's conditions, for stream decoders
    /// that complete a cell after its base record arrives.
    pub fn amend_last_conditions(&mut self, amend: impl FnOnce(&mut ConditionSet)) -> Result<(), StandardError> {
        if self.len == 0 {
            return Err(StandardError::InvalidConditionalSceneRequest);
        }
        let last = self.last;
        let mut record = unpack_record(&self.bytes.as_flattened()[last..self.used]);
        amend(&mut record.conditions);
        let previous_used = self.used;
        self.store(last, record.slot, record.style, &record.conditions)
            .inspect_err(|_| self.used = previous_used)
    }

    pub fn clear(&mut self) {
        self.used = 0;
        self.last = 0;
        self.len = 0;
        self.pool.clear();
    }

    pub fn page(&self, offset: u16) -> RuntimeConditionalSceneChunk {
        let start = (offset as usize).min(self.len);
        let end = (start + CONDITIONAL_SCENE_CHUNK_SIZE).min(self.len);
        let mut chunk = RuntimeConditionalSceneChunk::new();
        for cell in self.iter().skip(start).take(end - start) {
            chunk.push(cell).expect("page is chunk-bounded");
        }
        chunk
    }

    fn effect_at(&self, offset: usize) -> BuiltinEffect {
        self.pool
            .get(self.bytes.as_flattened()[offset + 2])
            .unwrap_or(BuiltinEffect::Solid { color: Rgb8::BLACK })
    }
}

impl<const CAP: usize> Default for RuntimeConditionalSceneTable<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> PartialEq for RuntimeConditionalSceneTable<CAP> {
    fn eq(&self, other: &Self) -> bool {
        // Style indices may differ after independent replacement/replication;
        // only the ordered resolved rules are semantic state.
        self.iter().eq(other.iter())
    }
}

impl<const CAP: usize> Eq for RuntimeConditionalSceneTable<CAP> {}

pub(super) struct RuntimeConditionalSource<'a, Batteries: ?Sized, const CAP: usize> {
    pub(super) table: &'a RuntimeConditionalSceneTable<CAP>,
    pub(super) batteries: &'a Batteries,
    pub(super) matches: Matches<CAP>,
}

impl<Context, Batteries, const CAP: usize> LightingSource<Rgb8, Context>
    for RuntimeConditionalSource<'_, Batteries, CAP>
where
    Context: LightingContextProvider,
    Batteries: BatteryStatusProvider + ?Sized,
{
    fn prepare(&mut self, input: &SourceRenderInput<'_, Context>) {
        let bytes = &self.table.bytes.as_flattened()[..self.table.used];
        self.matches.evaluate(bytes, self.batteries, input);
    }

    fn len(&self, _: &SourceRenderInput<'_, Context>) -> usize {
        self.matches.len()
    }

    fn slot(&self, index: usize, _: &SourceRenderInput<'_, Context>) -> LedSlot {
        record_slot(self.table.bytes.as_flattened(), self.matches.offset(index))
    }

    fn contribution(&mut self, index: usize, input: &SourceRenderInput<'_, Context>) -> Contribution<Rgb8> {
        let effect = self.table.effect_at(self.matches.offset(index));
        Contribution::Opaque(effect.sample(input.now_ms))
    }
}

pub(super) struct NoBatteryStatus;

impl BatteryStatusProvider for NoBatteryStatus {
    fn battery_status(&self, _node: u8) -> crate::types::battery::BatteryStatus {
        crate::types::battery::BatteryStatus::Unavailable
    }
}

pub(super) static NO_BATTERY_STATUS: NoBatteryStatus = NoBatteryStatus;
