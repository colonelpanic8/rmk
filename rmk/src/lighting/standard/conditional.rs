use rmk_types::ble::BleState;

#[allow(unused_imports)]
use super::*;
use crate::lighting::Rgb8;
use crate::lighting::compositor::{Contribution, LightingSource, RenderInput as SourceRenderInput, RenderPolicy};
use crate::lighting::context::{LightingContextProvider, SplitForce};
use crate::lighting::effect::{BuiltinEffect, LightingEffect};
use crate::lighting::source::{
    ActiveTransport, BatteryCondition, BatteryStatusProvider, BondedSlotCondition, ChargeCondition, ConditionSet,
    ConnectionCondition, EffectsCondition, IndicatorCondition, LayerCondition, LayersCondition, MaintenanceCondition,
    OutputMode, SplitLink, SplitTransportCondition,
};
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
/// Rules are packed as variable-length records that carry only the predicates
/// they name, so this is an average budget, not a per-rule limit. A
/// [`ConditionSet`] holds room for every predicate and is 36 bytes, while a
/// typical rule names one or two: a layer-gated highlight packs to 7 bytes and a
/// layer-gated battery or connection indicator to 11 or 12.
pub const CONDITIONAL_RULE_BYTES: usize = 12;

/// Bytes each predicate packs to, in predicate-mask bit order: layer, battery,
/// connection, output mode, effects, layer set, indicators, maintenance, and
/// split transport. A record carries only the predicates its mask names.
const PREDICATE_LEN: [usize; 9] = [2, 4, 4, 1, 1, 8, 1, 1, 1];
/// Every predicate at a fixed offset; see [`canonical`].
const CANONICAL_LEN: usize = 23;
/// Slot (2), style index, and the predicate mask (2).
const RECORD_HEADER: usize = 5;
const MAX_RECORD: usize = RECORD_HEADER + CANONICAL_LEN;

const fn pack_bool(value: Option<bool>) -> u8 {
    match value {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

const fn unpack_bool(code: u8) -> Option<bool> {
    match code & 3 {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// `conditions` with every predicate at a fixed offset, plus the mask of the
/// predicates it names. Fixed offsets keep the field conversions free of
/// bounds checks; only [`pack_record`] and [`unpack_record`] deal with the
/// variable-length form.
fn canonical(conditions: &ConditionSet) -> (u16, [u8; CANONICAL_LEN]) {
    let mut mask = 0;
    let mut b = [0; CANONICAL_LEN];
    if let Some(layer) = conditions.layer {
        mask |= 1 << 0;
        b[0] = layer.layer;
        b[1] = layer.active as u8;
    }
    if let Some(battery) = conditions.battery {
        mask |= 1 << 1;
        let charge = match battery.charge {
            ChargeCondition::Any => 0,
            ChargeCondition::Charging => 1,
            ChargeCondition::Discharging => 2,
            ChargeCondition::Unknown => 3,
        };
        b[2] = battery.node;
        b[3] = battery.min_level.is_some() as u8 | (battery.max_level.is_some() as u8) << 1 | charge << 2;
        b[4] = battery.min_level.unwrap_or(0);
        b[5] = battery.max_level.unwrap_or(0);
    }
    if let Some(connection) = conditions.connection {
        mask |= 1 << 2;
        let transport = match connection.transport {
            None => 0,
            Some(ActiveTransport::Usb) => 1,
            Some(ActiveTransport::Ble) => 2,
            Some(ActiveTransport::NoneActive) => 3,
        };
        let ble_state = match connection.ble_state {
            None => 0,
            Some(BleState::Advertising) => 1,
            Some(BleState::Connected) => 2,
            Some(BleState::Inactive) => 3,
        };
        b[6] = transport | ble_state << 2 | pack_bool(connection.usb_connected) << 4;
        b[7] = connection.profile.map_or(0, |profile| profile.wrapping_add(1));
        if let Some(bonded) = connection.bonded {
            b[8] = bonded.slot;
            b[9] = pack_bool(Some(bonded.bonded));
        }
    }
    if let Some(mode) = conditions.output_mode {
        mask |= 1 << 3;
        b[10] = match mode {
            OutputMode::AlwaysOn => 0,
            OutputMode::AlwaysOff => 1,
            OutputMode::PoweredOnly => 2,
        };
    }
    if let Some(effects) = conditions.effects {
        mask |= 1 << 4;
        b[11] = effects.enabled as u8;
    }
    if let Some(layers) = conditions.layers {
        mask |= 1 << 5;
        b[12..16].copy_from_slice(&layers.active.to_le_bytes());
        b[16..20].copy_from_slice(&layers.inactive.to_le_bytes());
    }
    if let Some(indicators) = conditions.indicators {
        mask |= 1 << 6;
        b[20] = pack_bool(indicators.num_lock)
            | pack_bool(indicators.caps_lock) << 2
            | pack_bool(indicators.scroll_lock) << 4;
    }
    if let Some(maintenance) = conditions.maintenance {
        mask |= 1 << 7;
        b[21] = maintenance.unlocked as u8;
    }
    if let Some(split) = conditions.split_transport {
        mask |= 1 << 8;
        let link = match split.link {
            None => 0,
            Some(SplitLink::Wired) => 1,
            Some(SplitLink::Ble) => 2,
        };
        let force = match split.force {
            None => 0,
            Some(SplitForce::Auto) => 1,
            Some(SplitForce::Wired) => 2,
            Some(SplitForce::Ble) => 3,
        };
        b[22] = link | force << 2;
    }
    (mask, b)
}

fn from_canonical(mask: u16, b: &[u8; CANONICAL_LEN]) -> ConditionSet {
    let named = |bit: u16| mask & (1 << bit) != 0;
    ConditionSet {
        layer: named(0).then(|| LayerCondition {
            layer: b[0],
            active: b[1] != 0,
        }),
        battery: named(1).then(|| BatteryCondition {
            node: b[2],
            min_level: (b[3] & 1 != 0).then_some(b[4]),
            max_level: (b[3] & 2 != 0).then_some(b[5]),
            charge: match b[3] >> 2 {
                1 => ChargeCondition::Charging,
                2 => ChargeCondition::Discharging,
                3 => ChargeCondition::Unknown,
                _ => ChargeCondition::Any,
            },
        }),
        connection: named(2).then(|| ConnectionCondition {
            transport: match b[6] & 3 {
                1 => Some(ActiveTransport::Usb),
                2 => Some(ActiveTransport::Ble),
                3 => Some(ActiveTransport::NoneActive),
                _ => None,
            },
            ble_state: match (b[6] >> 2) & 3 {
                1 => Some(BleState::Advertising),
                2 => Some(BleState::Connected),
                3 => Some(BleState::Inactive),
                _ => None,
            },
            usb_connected: unpack_bool(b[6] >> 4),
            profile: b[7].checked_sub(1),
            bonded: unpack_bool(b[9]).map(|bonded| BondedSlotCondition { slot: b[8], bonded }),
        }),
        output_mode: named(3).then(|| match b[10] {
            1 => OutputMode::AlwaysOff,
            2 => OutputMode::PoweredOnly,
            _ => OutputMode::AlwaysOn,
        }),
        effects: named(4).then(|| EffectsCondition { enabled: b[11] != 0 }),
        layers: named(5).then(|| LayersCondition {
            active: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            inactive: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
        }),
        indicators: named(6).then(|| IndicatorCondition {
            num_lock: unpack_bool(b[20]),
            caps_lock: unpack_bool(b[20] >> 2),
            scroll_lock: unpack_bool(b[20] >> 4),
        }),
        maintenance: named(7).then(|| MaintenanceCondition { unlocked: b[21] != 0 }),
        split_transport: named(8).then(|| SplitTransportCondition {
            link: match b[22] & 3 {
                1 => Some(SplitLink::Wired),
                2 => Some(SplitLink::Ble),
                _ => None,
            },
            force: match b[22] >> 2 {
                1 => Some(SplitForce::Auto),
                2 => Some(SplitForce::Wired),
                3 => Some(SplitForce::Ble),
                _ => None,
            },
        }),
    }
}

/// Encode one rule into `out`, returning the record length.
fn pack_record(slot: LedSlot, style: u8, conditions: &ConditionSet, out: &mut [u8; MAX_RECORD]) -> usize {
    let (mask, fields) = canonical(conditions);
    out[..2].copy_from_slice(&slot.0.to_le_bytes());
    out[2] = style;
    out[3..5].copy_from_slice(&mask.to_le_bytes());
    let mut len = RECORD_HEADER;
    let mut at = 0;
    for (bit, size) in PREDICATE_LEN.into_iter().enumerate() {
        if mask & (1 << bit) != 0 {
            out[len..len + size].copy_from_slice(&fields[at..at + size]);
            len += size;
        }
        at += size;
    }
    len
}

/// One decoded record. `len` is the number of bytes it occupies.
struct Record {
    slot: LedSlot,
    style: u8,
    conditions: ConditionSet,
    len: usize,
}

/// Decode the record starting at `bytes[0]`. Only [`pack_record`] writes the
/// table, so the bytes are always well formed.
fn unpack_record(bytes: &[u8]) -> Record {
    let mask = u16::from_le_bytes([bytes[3], bytes[4]]);
    let mut fields = [0; CANONICAL_LEN];
    let mut len = RECORD_HEADER;
    let mut at = 0;
    for (bit, size) in PREDICATE_LEN.into_iter().enumerate() {
        if mask & (1 << bit) != 0 {
            fields[at..at + size].copy_from_slice(&bytes[len..len + size]);
            len += size;
        }
        at += size;
    }
    Record {
        slot: LedSlot(u16::from_le_bytes([bytes[0], bytes[1]])),
        style: bytes[2],
        conditions: from_canonical(mask, &fields),
        len,
    }
}

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
        let bytes = &self.bytes.as_flattened()[..self.used];
        let mut at = 0;
        core::iter::from_fn(move || {
            (at < bytes.len()).then(|| {
                let record = unpack_record(&bytes[at..]);
                let start = at;
                at += record.len;
                (start, record)
            })
        })
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
        let mut record = [0; MAX_RECORD];
        let len = pack_record(slot, style, conditions, &mut record);
        let bytes = self.bytes.as_flattened_mut();
        let target = bytes
            .get_mut(at..at + len)
            .ok_or(StandardError::ConditionalSceneFull { capacity: CAP })?;
        target.copy_from_slice(&record[..len]);
        self.last = at;
        self.used = at + len;
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

    fn slot_at(&self, offset: u16) -> LedSlot {
        let bytes = self.bytes.as_flattened();
        LedSlot(u16::from_le_bytes([bytes[offset as usize], bytes[offset as usize + 1]]))
    }

    fn effect_at(&self, offset: u16) -> BuiltinEffect {
        self.pool
            .get(self.bytes.as_flattened()[offset as usize + 2])
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

/// The rules matching one render, evaluated once up front: the compositor
/// addresses a source by match index several times per contribution, and
/// re-filtering the table for each of those made a frame quadratic in rules.
pub(super) struct RuntimeConditionalSource<'a, const CAP: usize> {
    table: &'a RuntimeConditionalSceneTable<CAP>,
    matched: [u16; CAP],
    len: usize,
}

impl<'a, const CAP: usize> RuntimeConditionalSource<'a, CAP> {
    pub(super) fn new<Context, Batteries>(
        table: &'a RuntimeConditionalSceneTable<CAP>,
        batteries: &Batteries,
        context: &Context,
        policy: RenderPolicy,
    ) -> Self
    where
        Context: LightingContextProvider,
        Batteries: BatteryStatusProvider + ?Sized,
    {
        let mut matched = [0; CAP];
        let mut len = 0;
        for (offset, record) in table.records() {
            if record
                .conditions
                .matches(context, batteries, policy.output_mode, policy.effects_enabled)
            {
                matched[len] = offset as u16;
                len += 1;
            }
        }
        Self { table, matched, len }
    }
}

impl<Context, const CAP: usize> LightingSource<Rgb8, Context> for RuntimeConditionalSource<'_, CAP>
where
    Context: LightingContextProvider,
{
    fn len(&self, _: &SourceRenderInput<'_, Context>) -> usize {
        self.len
    }

    fn slot(&self, index: usize, _: &SourceRenderInput<'_, Context>) -> LedSlot {
        self.table.slot_at(self.matched[..self.len][index])
    }

    fn contribution(&mut self, index: usize, input: &SourceRenderInput<'_, Context>) -> Contribution<Rgb8> {
        Contribution::Opaque(
            self.table
                .effect_at(self.matched[..self.len][index])
                .sample(input.now_ms),
        )
    }
}

pub(super) struct NoBatteryStatus;

impl BatteryStatusProvider for NoBatteryStatus {
    fn battery_status(&self, _node: u8) -> crate::types::battery::BatteryStatus {
        crate::types::battery::BatteryStatus::Unavailable
    }
}

pub(super) static NO_BATTERY_STATUS: NoBatteryStatus = NoBatteryStatus;
