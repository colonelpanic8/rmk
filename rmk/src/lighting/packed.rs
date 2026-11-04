//! Conditional lighting rules packed as variable-length records.
//!
//! A [`ConditionSet`] has room for every predicate and is 36 bytes, while a
//! typical rule names one or two. A record holds the rule's slot, a style
//! index, a mask of the predicates it names, and only those predicates: a
//! layer-gated rule packs to 7 bytes and a layer-gated battery or connection
//! rule to 11. The runtime rule table and a board's compiled rules share the
//! format, and the encoder is `const` so compiled rules pack at build time.

use rmk_types::ble::BleState;

use super::Rgb8;
use super::compositor::{Contribution, LightingSource, RenderInput};
use super::context::{LightingContextProvider, SplitForce};
use super::effect::{BuiltinEffect, LightingEffect};
use super::source::{
    ActiveTransport, BatteryCondition, BatteryStatusProvider, BondedSlotCondition, ChargeCondition, ConditionSet,
    ConditionalSceneCell, ConnectionCondition, EffectsCondition, IndicatorCondition, LayerCondition, LayersCondition,
    MaintenanceCondition, OutputMode, SplitLink, SplitTransportCondition,
};
use super::topology::LedSlot;

/// Bytes each predicate packs to, in predicate-mask bit order: layer, battery,
/// connection, output mode, effects, layer set, indicators, maintenance, and
/// split transport.
const PREDICATE_LEN: [usize; 9] = [2, 4, 4, 1, 1, 8, 1, 1, 1];
/// Every predicate at a fixed offset; see [`canonical`].
const CANONICAL_LEN: usize = 23;
/// Slot (2), style index, and the predicate mask (2).
const RECORD_HEADER: usize = 5;

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

const fn level(value: Option<u8>) -> u8 {
    match value {
        Some(level) => level,
        None => 0,
    }
}

/// `conditions` with every predicate at a fixed offset, plus the mask of the
/// predicates it names. Fixed offsets keep the field conversions free of
/// bounds checks; only the record loops deal with the variable-length form.
const fn canonical(conditions: &ConditionSet) -> (u16, [u8; CANONICAL_LEN]) {
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
        b[4] = level(battery.min_level);
        b[5] = level(battery.max_level);
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
        b[7] = match connection.profile {
            Some(profile) => profile.wrapping_add(1),
            None => 0,
        };
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
        let active = layers.active.to_le_bytes();
        let inactive = layers.inactive.to_le_bytes();
        let mut i = 0;
        while i < 4 {
            b[12 + i] = active[i];
            b[16 + i] = inactive[i];
            i += 1;
        }
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

/// Bytes the record for `conditions` occupies.
pub(crate) const fn record_len(conditions: &ConditionSet) -> usize {
    let (mask, _) = canonical(conditions);
    let mut len = RECORD_HEADER;
    let mut bit = 0;
    while bit < PREDICATE_LEN.len() {
        if mask & (1 << bit) != 0 {
            len += PREDICATE_LEN[bit];
        }
        bit += 1;
    }
    len
}

/// Write the record for one rule at `out[at..]`, returning its length. The
/// caller has checked that [`record_len`] bytes fit.
pub(crate) const fn pack_record(
    out: &mut [u8],
    at: usize,
    slot: LedSlot,
    style: u8,
    conditions: &ConditionSet,
) -> usize {
    let (mask, fields) = canonical(conditions);
    let slot = slot.0.to_le_bytes();
    let mask_bytes = mask.to_le_bytes();
    out[at] = slot[0];
    out[at + 1] = slot[1];
    out[at + 2] = style;
    out[at + 3] = mask_bytes[0];
    out[at + 4] = mask_bytes[1];
    let mut len = RECORD_HEADER;
    let mut field = 0;
    let mut bit = 0;
    while bit < PREDICATE_LEN.len() {
        let size = PREDICATE_LEN[bit];
        if mask & (1 << bit) != 0 {
            let mut i = 0;
            while i < size {
                out[at + len + i] = fields[field + i];
                i += 1;
            }
            len += size;
        }
        field += size;
        bit += 1;
    }
    len
}

/// One decoded record. `len` is the number of bytes it occupies.
pub(crate) struct Record {
    pub(crate) slot: LedSlot,
    pub(crate) style: u8,
    pub(crate) conditions: ConditionSet,
    pub(crate) len: usize,
}

/// Decode the record starting at `bytes[0]`.
pub(crate) fn unpack_record(bytes: &[u8]) -> Record {
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
        slot: record_slot(bytes, 0),
        style: bytes[2],
        conditions: from_canonical(mask, &fields),
        len,
    }
}

/// The records packed in `bytes`, each with its offset.
pub(crate) fn records(bytes: &[u8]) -> impl Iterator<Item = (usize, Record)> + '_ {
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

pub(crate) fn record_slot(bytes: &[u8], offset: usize) -> LedSlot {
    LedSlot(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

/// Offsets of the records whose conditions hold for one render.
///
/// The compositor addresses a source by match index several times per
/// contribution, so the matches are evaluated once, before it does, rather
/// than by re-filtering the rules for every index.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Matches<const N: usize> {
    offsets: [u16; N],
    len: usize,
}

impl<const N: usize> Matches<N> {
    pub(crate) const fn new() -> Self {
        Self {
            offsets: [0; N],
            len: 0,
        }
    }

    pub(crate) fn evaluate<Context, Batteries>(
        &mut self,
        bytes: &[u8],
        batteries: &Batteries,
        input: &RenderInput<'_, Context>,
    ) where
        Context: LightingContextProvider,
        Batteries: BatteryStatusProvider + ?Sized,
    {
        self.len = 0;
        for (offset, record) in records(bytes) {
            if self.len < N
                && record.conditions.matches(
                    input.context,
                    batteries,
                    input.policy.output_mode,
                    input.policy.effects_enabled,
                )
            {
                self.offsets[self.len] = offset as u16;
                self.len += 1;
            }
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn offset(&self, index: usize) -> usize {
        self.offsets[..self.len][index] as usize
    }
}

const fn same_color(a: Rgb8, b: Rgb8) -> bool {
    a.r == b.r && a.g == b.g && a.b == b.b
}

const fn same_effect(a: &BuiltinEffect, b: &BuiltinEffect) -> bool {
    match (a, b) {
        (BuiltinEffect::Solid { color: a }, BuiltinEffect::Solid { color: b }) => same_color(*a, *b),
        (
            BuiltinEffect::Blink {
                color: a,
                period_ms: a_period,
                phase_ms: a_phase,
                duty: a_duty,
            },
            BuiltinEffect::Blink {
                color: b,
                period_ms: b_period,
                phase_ms: b_phase,
                duty: b_duty,
            },
        ) => same_color(*a, *b) && *a_period == *b_period && *a_phase == *b_phase && *a_duty == *b_duty,
        (
            BuiltinEffect::Breathe {
                color: a,
                period_ms: a_period,
                phase_ms: a_phase,
                step_ms: a_step,
            },
            BuiltinEffect::Breathe {
                color: b,
                period_ms: b_period,
                phase_ms: b_phase,
                step_ms: b_step,
            },
        ) => same_color(*a, *b) && *a_period == *b_period && *a_phase == *b_phase && *a_step == *b_step,
        _ => false,
    }
}

/// Whether `cells[index]` is the first cell to use its effect.
const fn first_use(cells: &[ConditionalSceneCell<BuiltinEffect>], index: usize) -> bool {
    let mut i = 0;
    while i < index {
        if same_effect(&cells[i].effect, &cells[index].effect) {
            return false;
        }
        i += 1;
    }
    true
}

/// Index of `cells[index]`'s effect among the distinct effects of `cells`, in
/// first-use order.
const fn style_of(cells: &[ConditionalSceneCell<BuiltinEffect>], index: usize) -> usize {
    let mut style = 0;
    let mut i = 0;
    while !same_effect(&cells[i].effect, &cells[index].effect) {
        if first_use(cells, i) {
            style += 1;
        }
        i += 1;
    }
    style
}

/// Bytes `cells` pack to; the length of [`pack_rule_bytes`]'s array.
pub const fn packed_rule_bytes_len(cells: &[ConditionalSceneCell<BuiltinEffect>]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < cells.len() {
        len += record_len(&cells[i].conditions);
        i += 1;
    }
    len
}

/// Distinct effects in `cells`; the length of [`pack_rule_effects`]'s array.
pub const fn packed_rule_effect_count(cells: &[ConditionalSceneCell<BuiltinEffect>]) -> usize {
    let mut count = 0;
    let mut i = 0;
    while i < cells.len() {
        if first_use(cells, i) {
            count += 1;
        }
        i += 1;
    }
    count
}

/// Pack `cells` into records for [`PackedRules`]. Meant for a `static`
/// initializer, where it runs at build time.
pub const fn pack_rule_bytes<const BYTES: usize>(cells: &[ConditionalSceneCell<BuiltinEffect>]) -> [u8; BYTES] {
    core::assert!(BYTES <= u16::MAX as usize, "packed rules must stay u16-addressable");
    core::assert!(packed_rule_bytes_len(cells) == BYTES);
    let mut out = [0; BYTES];
    let mut at = 0;
    let mut i = 0;
    while i < cells.len() {
        let style = style_of(cells, i);
        core::assert!(
            style <= u8::MAX as usize,
            "packed rules hold at most 256 distinct effects"
        );
        at += pack_record(&mut out, at, cells[i].slot, style as u8, &cells[i].conditions);
        i += 1;
    }
    out
}

/// The distinct effects of `cells`, indexed by the records' style bytes.
pub const fn pack_rule_effects<const EFFECTS: usize>(
    cells: &[ConditionalSceneCell<BuiltinEffect>],
) -> [BuiltinEffect; EFFECTS] {
    core::assert!(packed_rule_effect_count(cells) == EFFECTS);
    let mut out = [BuiltinEffect::Solid { color: Rgb8::BLACK }; EFFECTS];
    let mut i = 0;
    while i < cells.len() {
        out[style_of(cells, i)] = cells[i].effect;
        i += 1;
    }
    out
}

/// Immutable conditional rules packed by [`pack_rule_bytes`] and
/// [`pack_rule_effects`], typically compiled from `keyboard.toml`.
#[derive(Copy, Clone, Debug)]
pub struct PackedRules<'a> {
    bytes: &'a [u8],
    effects: &'a [BuiltinEffect],
    len: usize,
}

impl<'a> PackedRules<'a> {
    pub const EMPTY: PackedRules<'static> = PackedRules {
        bytes: &[],
        effects: &[],
        len: 0,
    };

    /// `bytes` and `effects` must come from packing the same `len` cells.
    pub const fn new(bytes: &'a [u8], effects: &'a [BuiltinEffect], len: usize) -> Self {
        Self { bytes, effects, len }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn effect(&self, style: u8) -> BuiltinEffect {
        self.effects
            .get(style as usize)
            .copied()
            .unwrap_or(BuiltinEffect::Solid { color: Rgb8::BLACK })
    }

    /// The rules in their semantic order.
    pub fn iter(&self) -> impl Iterator<Item = ConditionalSceneCell<BuiltinEffect>> + '_ {
        records(self.bytes).map(|(_, record)| ConditionalSceneCell {
            conditions: record.conditions,
            slot: record.slot,
            effect: self.effect(record.style),
        })
    }
}

/// [`ConditionalScenes`](super::ConditionalScenes) over [`PackedRules`].
/// `N` bounds the rules that can match one render; the rule count is enough.
pub struct PackedConditionalScenes<'a, Batteries: ?Sized, const N: usize> {
    rules: PackedRules<'a>,
    batteries: &'a Batteries,
    matches: Matches<N>,
}

impl<'a, Batteries: ?Sized, const N: usize> PackedConditionalScenes<'a, Batteries, N> {
    pub const fn new(rules: PackedRules<'a>, batteries: &'a Batteries) -> Self {
        Self {
            rules,
            batteries,
            matches: Matches::new(),
        }
    }

    pub const fn rules(&self) -> PackedRules<'a> {
        self.rules
    }
}

impl<Context, Batteries, const N: usize> LightingSource<Rgb8, Context> for PackedConditionalScenes<'_, Batteries, N>
where
    Context: LightingContextProvider,
    Batteries: BatteryStatusProvider + ?Sized,
{
    fn prepare(&mut self, input: &RenderInput<'_, Context>) {
        self.matches.evaluate(self.rules.bytes, self.batteries, input);
    }

    fn len(&self, _: &RenderInput<'_, Context>) -> usize {
        self.matches.len()
    }

    fn slot(&self, index: usize, _: &RenderInput<'_, Context>) -> LedSlot {
        record_slot(self.rules.bytes, self.matches.offset(index))
    }

    fn contribution(&mut self, index: usize, input: &RenderInput<'_, Context>) -> Contribution<Rgb8> {
        let style = self.rules.bytes[self.matches.offset(index) + 2];
        Contribution::Opaque(self.rules.effect(style).sample(input.now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::compositor::RenderPolicy;
    use crate::lighting::{Compositor, ConditionalScenes, LayerState, LightingContext, LogicalFrame};
    use crate::types::battery::BatteryStatus;

    const RED: Rgb8 = Rgb8::new(200, 0, 0);
    const GREEN: Rgb8 = Rgb8::new(0, 200, 0);

    const fn rule(layer: u8, active: bool, slot: u16, effect: BuiltinEffect) -> ConditionalSceneCell<BuiltinEffect> {
        ConditionalSceneCell {
            conditions: ConditionSet {
                layer: Some(LayerCondition { layer, active }),
                battery: None,
                connection: None,
                output_mode: None,
                effects: None,
                layers: None,
                indicators: None,
                maintenance: None,
                split_transport: None,
            },
            slot: LedSlot(slot),
            effect,
        }
    }

    const BLINK: BuiltinEffect = BuiltinEffect::Blink {
        color: RED,
        period_ms: 800,
        phase_ms: 0,
        duty: 50,
    };
    // Later rules override earlier ones on a shared slot, and two rules share
    // each effect.
    const CELLS: [ConditionalSceneCell<BuiltinEffect>; 4] = [
        rule(1, true, 0, BuiltinEffect::Solid { color: GREEN }),
        rule(1, true, 1, BLINK),
        rule(2, true, 0, BLINK),
        rule(2, false, 1, BuiltinEffect::Solid { color: GREEN }),
    ];
    static BYTES: [u8; packed_rule_bytes_len(&CELLS)] = pack_rule_bytes(&CELLS);
    static EFFECTS: [BuiltinEffect; packed_rule_effect_count(&CELLS)] = pack_rule_effects(&CELLS);
    const RULES: PackedRules<'static> = PackedRules::new(&BYTES, &EFFECTS, CELLS.len());

    struct NoBatteries;

    impl BatteryStatusProvider for NoBatteries {
        fn battery_status(&self, _node: u8) -> BatteryStatus {
            BatteryStatus::Unavailable
        }
    }

    #[test]
    fn rules_packed_at_build_time_read_back_unchanged_with_shared_effects() {
        assert_eq!(EFFECTS.len(), 2);
        assert_eq!(BYTES.len(), 4 * 7, "a layer-gated rule packs to 7 bytes");
        assert!(RULES.iter().eq(CELLS));
    }

    #[test]
    fn packed_rules_render_like_the_unpacked_source() {
        let compositor = Compositor::<Rgb8, 2>::new(Rgb8::BLACK);
        let mut packed = PackedConditionalScenes::<_, { CELLS.len() }>::new(RULES, &NoBatteries);
        let mut unpacked = ConditionalScenes::new(&CELLS, &NoBatteries);
        for active in [0b001, 0b011, 0b101, 0b111] {
            let context = LightingContext {
                layers: LayerState::new(0, 0, active),
                ..LightingContext::default()
            };
            let mut expected = LogicalFrame::new(Rgb8::BLACK);
            let mut tx = compositor.begin(0, &context, RenderPolicy::default(), Rgb8::BLACK, &mut expected);
            tx.apply(0, &mut unpacked).unwrap();
            tx.finish();
            let mut frame = LogicalFrame::new(Rgb8::BLACK);
            let mut tx = compositor.begin(0, &context, RenderPolicy::default(), Rgb8::BLACK, &mut frame);
            tx.apply(0, &mut packed).unwrap();
            tx.finish();
            assert_eq!(frame.as_slice(), expected.as_slice(), "active layers {active:#b}");
        }
    }
}
