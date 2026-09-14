//! Adapters from authoritative RMK state to lighting snapshots.

#[cfg(feature = "split")]
use super::SplitForce;
use super::{IndicatorState, LayerState, LightingContext, SnapshotProvider, SplitTransportState};
use crate::keymap::KeyMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TooManyLayers {
    pub configured: usize,
    pub supported: usize,
}

/// Authoritative layer snapshot provider backed by the live RMK keymap.
///
/// Layer-change events are only wakeups. Every render and command reads this
/// provider, so startup and coalesced events cannot leave lighting with a
/// reconstructed or stale active-layer set.
#[derive(Clone, Copy)]
pub struct KeymapLightingState<'keymap, 'data> {
    keymap: &'keymap KeyMap<'data>,
}

impl<'keymap, 'data> KeymapLightingState<'keymap, 'data> {
    pub fn new(keymap: &'keymap KeyMap<'data>) -> Result<Self, TooManyLayers> {
        let configured = keymap.num_layer();
        if configured > LayerState::CAPACITY as usize {
            return Err(TooManyLayers {
                configured,
                supported: LayerState::CAPACITY as usize,
            });
        }
        Ok(Self { keymap })
    }

    pub const fn keymap(&self) -> &'keymap KeyMap<'data> {
        self.keymap
    }
}

impl SnapshotProvider for KeymapLightingState<'_, '_> {
    type Snapshot = LightingContext;

    fn snapshot(&self) -> Self::Snapshot {
        let effective = self.keymap.get_activated_layer();
        let default = self.keymap.get_default_layer();
        let mut active = 1_u64 << default;
        for layer in 0..self.keymap.num_layer() as u8 {
            if self.keymap.is_layer_active(layer) {
                active |= 1_u64 << layer;
            }
        }
        let connection = crate::state::current_connection_status();
        let powered = connection.usb.is_powered();
        LightingContext {
            layers: LayerState::new(effective, default, active),
            indicators: indicator_state(),
            powered,
            local_powered: powered,
            connection,
            bonded_slots: bonded_slots(),
            maintenance_unlocked: crate::state::maintenance_mode_enabled(),
            split_transport: split_transport_state(),
        }
    }
}

fn split_transport_state() -> SplitTransportState {
    #[cfg(feature = "split")]
    {
        use crate::split::selector;

        let auto = selector::auto_enabled();
        SplitTransportState {
            auto,
            force: match selector::forced_mode() {
                selector::FORCE_WIRED => SplitForce::Wired,
                selector::FORCE_BLE => SplitForce::Ble,
                _ => SplitForce::Auto,
            },
            wired: auto && selector::wired_selected(),
        }
    }
    #[cfg(not(feature = "split"))]
    {
        SplitTransportState::default()
    }
}

/// Bonded-slot bitmap, or zero on builds with no BLE stack to bond with.
fn bonded_slots() -> u8 {
    #[cfg(feature = "_ble")]
    {
        crate::state::bonded_slots()
    }
    #[cfg(not(feature = "_ble"))]
    {
        0
    }
}

fn indicator_state() -> IndicatorState {
    let indicator = crate::keyboard::current_led_indicator();
    IndicatorState {
        num_lock: indicator.num_lock(),
        caps_lock: indicator.caps_lock(),
        scroll_lock: indicator.scroll_lock(),
        compose: indicator.compose(),
        kana: indicator.kana(),
    }
}

#[cfg(all(test, feature = "split"))]
mod tests {
    use rmk_types::action::KeyAction;

    use super::*;
    use crate::config::{BehaviorConfig, PositionalConfig};
    use crate::keymap::KeymapData;
    use crate::test_support::test_block_on as block_on;

    #[test]
    fn snapshot_reads_live_maintenance_and_split_transport_state() {
        let mut behavior = BehaviorConfig::default();
        let positional: PositionalConfig<1, 1> = PositionalConfig::default();
        let mut data: KeymapData<1, 1, 1, 0> = KeymapData::new([[[KeyAction::No]]]);
        let keymap = block_on(KeyMap::new(&mut data, &mut behavior, &positional));
        let provider = KeymapLightingState::new(&keymap).unwrap();

        crate::state::set_maintenance_mode(false);
        crate::split::selector::initialize(true);
        crate::split::selector::set_forced(crate::split::selector::FORCE_BLE);
        let snapshot = provider.snapshot();
        assert!(!snapshot.maintenance_unlocked);
        assert_eq!(
            snapshot.split_transport,
            SplitTransportState {
                auto: true,
                force: SplitForce::Ble,
                wired: false,
            }
        );

        crate::state::set_maintenance_mode(true);
        crate::split::selector::set_forced(crate::split::selector::FORCE_WIRED);
        let snapshot = provider.snapshot();
        assert!(snapshot.maintenance_unlocked);
        assert_eq!(snapshot.split_transport.force, SplitForce::Wired);
        assert!(snapshot.split_transport.wired);
    }
}
