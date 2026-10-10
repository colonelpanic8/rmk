//! Shared context for the Vial and Rynk host services.

use embassy_time::Duration;
use rmk_types::action::{EncoderAction, KeyAction};
use rmk_types::auto_mouse::AutoMouseLayerConfig;
#[cfg(feature = "_ble")]
use rmk_types::battery::BatteryStatus;
use rmk_types::combo::{Combo as ComboConfig, ComboDefinition};
use rmk_types::connection::{ConnectionStatus, ConnectionType};
use rmk_types::fork::Fork;
use rmk_types::led_indicator::LedIndicator;
#[cfg(feature = "storage")]
use rmk_types::morse::MorseProfileName;
use rmk_types::morse::{Morse, MorseProfile};
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::AutoMouseLayerConfigs;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::BehaviorConfig;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::BehaviorOptions;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::LAYER_STATE_BITMAP_SIZE;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::LAYER_STATE_CAPACITY;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::LayerState;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::MORSE_PROFILE_ENTRY_CHUNK;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::MorseHoldTriggerPosition;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::MorseHoldTriggerPositionState;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::MorseHoldTriggerPositions;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::MorseProfileEntry;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::MorseProfileState;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::SetMorseHoldTriggerPositionsRequest;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::SetMorseProfileEntryRequest;

#[cfg(feature = "rynk")]
use crate::config::HoldTriggerPositions;
#[cfg(feature = "rynk")]
use crate::config::OneShotModifiersConfig;
use crate::event::{AutoMouseLayerConfigChangeEvent, KeyboardEventPos, publish_event};
use crate::keyboard::combo::Combo;
use crate::keymap::KeyMap;
#[cfg(feature = "storage")]
use crate::storage::{StorageItem, store};

/// Context shared between Vial and Rynk host services.
pub(crate) struct KeyboardContext<'a> {
    pub keymap: &'a KeyMap<'a>,
    pub(crate) layout_blob: &'static [u8],
}

impl<'a> KeyboardContext<'a> {
    pub fn new(keymap: &'a KeyMap<'a>) -> Self {
        Self {
            keymap,
            layout_blob: &[],
        }
    }

    pub fn get_action(&self, layer: u8, row: u8, col: u8) -> KeyAction {
        self.keymap
            .get_action_at(KeyboardEventPos::key_pos(col, row), layer as usize)
    }

    pub fn get_action_flat(&self, index: usize) -> KeyAction {
        self.keymap.get_action_by_flat_index(index)
    }

    /// `(rows, cols, num_layers)`.
    pub fn keymap_dimensions(&self) -> (usize, usize, usize) {
        self.keymap.get_keymap_config()
    }

    /// The opaque, compressed physical-layout blob served by `GetLayout`.
    pub fn layout_blob(&self) -> &'static [u8] {
        self.layout_blob
    }

    pub async fn set_action(&self, layer: u8, row: u8, col: u8, action: KeyAction) -> Result<(), ()> {
        self.keymap
            .set_action_at(KeyboardEventPos::key_pos(col, row), layer as usize, action);
        #[cfg(feature = "storage")]
        store(StorageItem::Keymap {
            layer,
            row,
            col,
            action,
        })
        .await?;
        Ok(())
    }

    pub fn get_encoder(&self, layer: u8, idx: u8) -> Option<EncoderAction> {
        self.keymap.get_encoder_action(layer as usize, idx as usize)
    }

    /// Number of encoders per layer.
    pub fn num_encoders(&self) -> usize {
        self.keymap.num_encoders()
    }

    /// Write one encoder direction and persist the updated pair.
    pub async fn set_encoder_direction(
        &self,
        layer: u8,
        idx: u8,
        clockwise: bool,
        action: KeyAction,
    ) -> Result<(), ()> {
        let updated = if clockwise {
            self.keymap.set_encoder_clockwise(layer as usize, idx as usize, action)
        } else {
            self.keymap
                .set_encoder_counter_clockwise(layer as usize, idx as usize, action)
        };
        #[cfg(feature = "storage")]
        if let Some(encoder) = updated {
            store(StorageItem::Encoder {
                layer,
                idx,
                action: encoder,
            })
            .await?;
        }
        #[cfg(not(feature = "storage"))]
        let _ = updated;
        Ok(())
    }

    /// Write both encoder directions in one synchronous RAM update, then persist
    /// once.
    pub async fn set_encoder(&self, layer: u8, idx: u8, action: EncoderAction) -> Result<(), ()> {
        let written = self.keymap.set_encoder(layer as usize, idx as usize, action);
        #[cfg(feature = "storage")]
        if written {
            store(StorageItem::Encoder { layer, idx, action }).await?;
        }
        #[cfg(not(feature = "storage"))]
        let _ = written;
        Ok(())
    }

    pub fn with_combos<R>(&self, f: impl FnOnce(&[Option<Combo>]) -> R) -> R {
        self.keymap.with_combos(f)
    }

    /// Replace the combo at `idx` with `config` (or remove it if `config` is
    /// empty) and persist. No-op if `idx` is out of range.
    /// Returns `false` when `idx` is out of range (no slot written).
    /// `Ok(false)` when `idx` is out of range (no slot written).
    pub async fn set_combo(&self, idx: u8, config: ComboConfig) -> Result<bool, ()> {
        let valid = self.keymap.with_combos_mut(|combos| {
            if (idx as usize) >= combos.len() {
                return false;
            }
            combos[idx as usize] = if config.actions.is_empty() && config.output == KeyAction::No {
                None
            } else {
                Some(Combo::new(config.clone()))
            };
            true
        });
        if !valid {
            return Ok(false);
        }
        #[cfg(feature = "storage")]
        store(StorageItem::Combo { idx, config }).await?;
        #[cfg(not(feature = "storage"))]
        let _ = config;
        Ok(true)
    }

    /// Validate a versioned combo against this keyboard's matrix. Legacy
    /// action combos retain the validation behavior of `SetCombo`; position
    /// combos reject out-of-range layers/coordinates and duplicate positions.
    pub fn combo_definition_is_valid(&self, definition: &ComboDefinition) -> bool {
        let ComboDefinition::Positions(config) = definition else {
            return true;
        };
        let (rows, cols, layers) = self.keymap_dimensions();
        if config.layer.is_some_and(|layer| layer as usize >= layers) {
            return false;
        }
        config.positions.iter().enumerate().all(|(idx, position)| {
            (position.row as usize) < rows
                && (position.col as usize) < cols
                && !config.positions[..idx].contains(position)
        })
    }

    /// Replace a combo slot using the additive action-or-position definition.
    pub async fn set_combo_definition(&self, idx: u8, definition: ComboDefinition) -> Result<bool, ()> {
        if !self.combo_definition_is_valid(&definition) {
            return Ok(false);
        }
        let valid = self.keymap.with_combos_mut(|combos| {
            if (idx as usize) >= combos.len() {
                return false;
            }
            combos[idx as usize] = if definition.is_empty() {
                None
            } else {
                Some(Combo::from_definition(definition.clone()))
            };
            true
        });
        if !valid {
            return Ok(false);
        }
        #[cfg(feature = "storage")]
        store(match definition {
            definition if definition.is_empty() => StorageItem::Combo {
                idx,
                config: ComboConfig::empty(),
            },
            ComboDefinition::Actions(config) => StorageItem::Combo { idx, config },
            ComboDefinition::Positions(config) => StorageItem::PositionCombo { idx, config },
        })
        .await?;
        Ok(true)
    }

    pub fn get_morse(&self, idx: u8) -> Option<Morse> {
        self.keymap.get_morse(idx as usize)
    }

    pub fn morses_len(&self) -> usize {
        self.keymap.morses_len()
    }

    /// Mutate the morse at `idx` and persist. No-op if `idx` is out of range.
    pub async fn update_morse(&self, idx: u8, f: impl FnOnce(&mut Morse)) -> Result<(), ()> {
        #[cfg(feature = "storage")]
        {
            let updated = self.keymap.with_morse_mut(idx as usize, |morse| {
                f(morse);
                morse.clone()
            });
            if let Some(morse) = updated {
                store(StorageItem::Morse { idx, morse }).await?;
            }
        }
        #[cfg(not(feature = "storage"))]
        {
            self.keymap.with_morse_mut(idx as usize, f);
        }
        Ok(())
    }

    pub fn combo_timeout(&self) -> Duration {
        self.keymap.combo_timeout()
    }

    pub fn one_shot_timeout(&self) -> Duration {
        self.keymap.one_shot_timeout()
    }

    pub fn tap_interval(&self) -> u16 {
        self.keymap.tap_interval()
    }

    pub fn tap_capslock_interval(&self) -> u16 {
        self.keymap.tap_capslock_interval()
    }

    pub fn morse_default_profile(&self) -> MorseProfile {
        self.keymap.morse_default_profile()
    }

    pub fn morse_profiles_capacity(&self) -> usize {
        self.keymap.morse_profiles_capacity()
    }

    /// Profile a key bound to `idx` resolves to. `None` if `idx` is past the
    /// table's capacity.
    pub fn get_morse_profile(&self, idx: u8) -> Option<MorseProfile> {
        ((idx as usize) < self.keymap.morse_profiles_capacity()).then(|| self.keymap.morse_profile(idx))
    }

    #[cfg(feature = "rynk")]
    pub fn morse_profile_state(&self, offset: u8) -> MorseProfileState {
        let total = (0..self.keymap.morse_profiles_capacity())
            .filter(|index| self.keymap.morse_profile_name(*index as u8).is_some())
            .count();
        let entries = (0..self.keymap.morse_profiles_capacity())
            .filter(|index| self.keymap.morse_profile_name(*index as u8).is_some())
            .skip(offset as usize)
            .take(MORSE_PROFILE_ENTRY_CHUNK)
            .map(|index| {
                let index = index as u8;
                MorseProfileEntry {
                    index,
                    name: self.keymap.morse_profile_name(index).expect("filtered occupied slot"),
                    profile: self.keymap.morse_profile(index),
                }
            })
            .collect();
        MorseProfileState {
            capacity: self.keymap.morse_profiles_capacity() as u8,
            total: total as u8,
            entries,
        }
    }

    /// Replace the profile at `idx` and persist it. `Ok(false)` for an index
    /// past the table's capacity, which changes nothing.
    pub async fn set_morse_profile(&self, idx: u8, profile: MorseProfile) -> Result<bool, ()> {
        if !self.keymap.set_morse_profile(idx, profile) {
            return Ok(false);
        }
        #[cfg(feature = "storage")]
        {
            store(StorageItem::MorseProfile { idx, profile }).await?;
            store(StorageItem::MorseProfileName {
                idx,
                name: self.keymap.morse_profile_name(idx).expect("setter occupies the slot"),
            })
            .await?;
        }
        Ok(true)
    }

    /// Create, rename, or update one slot. `Ok(false)` for an index past the table, an
    /// empty name, or a name another slot already carries; nothing changes then.
    #[cfg(feature = "rynk")]
    pub async fn set_morse_profile_entry(&self, request: SetMorseProfileEntryRequest) -> Result<bool, ()> {
        let capacity = self.keymap.morse_profiles_capacity();
        let entry = request.entry;
        if entry.index as usize >= capacity || entry.name.trim().is_empty() {
            return Ok(false);
        }
        for index in 0..capacity {
            if index != entry.index as usize
                && self
                    .keymap
                    .morse_profile_name(index as u8)
                    .is_some_and(|name| name == entry.name)
            {
                return Ok(false);
            }
        }
        if !self
            .keymap
            .set_named_morse_profile(entry.index, entry.name.clone(), entry.profile)
        {
            return Ok(false);
        }

        #[cfg(feature = "storage")]
        {
            store(StorageItem::MorseProfile {
                idx: entry.index,
                profile: entry.profile,
            })
            .await?;
            store(StorageItem::MorseProfileName {
                idx: entry.index,
                name: entry.name,
            })
            .await?;
        }
        Ok(true)
    }

    /// Vacate slot `idx`. `Ok(false)` for an index past the table.
    pub async fn delete_morse_profile(&self, idx: u8) -> Result<bool, ()> {
        if !self.keymap.delete_morse_profile(idx) {
            return Ok(false);
        }
        #[cfg(feature = "storage")]
        let hold_trigger_positions = self.keymap.morse_hold_trigger_positions();
        #[cfg(feature = "storage")]
        {
            store(StorageItem::MorseProfile {
                idx,
                profile: MorseProfile::default(),
            })
            .await?;
            store(StorageItem::MorseProfileName {
                idx,
                name: MorseProfileName::new(),
            })
            .await?;
            store(StorageItem::MorseHoldTriggerPositions(hold_trigger_positions)).await?;
        }
        Ok(true)
    }

    pub fn morse_prior_idle_time(&self) -> Duration {
        self.keymap.morse_prior_idle_time()
    }

    #[cfg(feature = "rynk")]
    pub fn morse_hold_trigger_positions(&self) -> MorseHoldTriggerPositionState {
        // `Extend` works for both the firmware's bounded table and a host build's `Vec`.
        let mut positions = MorseHoldTriggerPositions::new();
        positions.extend(
            self.keymap
                .morse_hold_trigger_positions()
                .iter()
                .map(|&(profile, row, col)| MorseHoldTriggerPosition { profile, row, col }),
        );
        MorseHoldTriggerPositionState {
            capacity: crate::HOLD_TRIGGER_KEY_POSITION_MAX_NUM as u8,
            positions,
        }
    }

    #[cfg(feature = "rynk")]
    pub async fn set_morse_hold_trigger_positions(
        &self,
        request: SetMorseHoldTriggerPositionsRequest,
    ) -> Result<(), ()> {
        let mut stored = HoldTriggerPositions::new();
        for position in request.positions {
            stored
                .push((position.profile, position.row, position.col))
                .expect("Rynk decoder enforces the compiled hold-trigger capacity");
        }
        self.keymap.set_morse_hold_trigger_positions(stored.clone());
        #[cfg(feature = "storage")]
        store(StorageItem::MorseHoldTriggerPositions(stored)).await?;
        Ok(())
    }

    pub fn behavior_options(&self) -> BehaviorOptions {
        let one_shot = self.keymap.one_shot_modifiers_config();
        BehaviorOptions {
            tri_layer: self.keymap.tri_layer(),
            combo_prior_idle_ms: self
                .keymap
                .combo_prior_idle_time()
                .map(|duration| duration.as_millis() as u16),
            oneshot_activate_on_keypress: one_shot.activate_on_keypress,
            oneshot_quick_release: one_shot.quick_release,
            morse_enable_flow_tap: self.keymap.morse_enable_flow_tap(),
            morse_prior_idle_ms: self.keymap.morse_prior_idle_time().as_millis() as u16,
            morse_default_profile: self.keymap.morse_default_profile(),
        }
    }

    /// Replace the global behavior options and persist them. Invalid layer
    /// indices reject the whole update before any field changes: `Ok(false)`.
    #[cfg(feature = "rynk")]
    pub async fn set_behavior_options(&self, options: BehaviorOptions) -> Result<bool, ()> {
        let (_, _, layers) = self.keymap.get_keymap_config();
        if options
            .tri_layer
            .is_some_and(|tri_layer| tri_layer.into_iter().any(|layer| layer as usize >= layers))
        {
            return Ok(false);
        }

        self.keymap.set_tri_layer(options.tri_layer);
        self.keymap
            .set_combo_prior_idle_time(options.combo_prior_idle_ms.map(|ms| Duration::from_millis(ms as u64)));
        self.keymap.set_one_shot_modifiers_config(OneShotModifiersConfig {
            activate_on_keypress: options.oneshot_activate_on_keypress,
            quick_release: options.oneshot_quick_release,
        });
        self.keymap.set_morse_enable_flow_tap(options.morse_enable_flow_tap);
        self.keymap
            .set_morse_prior_idle_time(Duration::from_millis(options.morse_prior_idle_ms as u64));
        self.keymap.set_morse_default_profile(options.morse_default_profile);

        // The prior-idle time and default profile live in the older `BehaviorConfig` item.
        #[cfg(feature = "storage")]
        {
            store(StorageItem::BehaviorOptions(options.into())).await?;
            store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        }
        Ok(true)
    }

    pub fn auto_mouse_layer_configs(&self) -> AutoMouseLayerConfigs {
        self.keymap.auto_mouse_layer_configs().into_iter().collect()
    }

    /// Atomically replace the auto mouse layer table after validating every
    /// entry against this firmware's compiled resources. `Ok(false)` rejects
    /// the whole table without changing anything.
    pub async fn set_auto_mouse_layer_configs(&self, configs: AutoMouseLayerConfigs) -> Result<bool, ()> {
        if configs.len() > crate::AUTO_MOUSE_LAYER_MAX_NUM {
            return Ok(false);
        }
        let (_, _, layers) = self.keymap.get_keymap_config();
        for (index, config) in configs.iter().enumerate() {
            if config.target_layer as usize >= layers
                || config.timeout_ms == 0
                || config.threshold == 0
                || config.extra_mouse_keys.len() > rmk_types::auto_mouse::AUTO_MOUSE_LAYER_EXTRA_KEY_MAX_NUM
            {
                return Ok(false);
            }
            if (config.deactivate_on_key || config.reset_timeout_on_key) && crate::ACTION_EVENT_SUB_SIZE == 0 {
                return Ok(false);
            }
            if configs[..index]
                .iter()
                .any(|existing| existing.device_id == config.device_id)
            {
                return Ok(false);
            }
        }

        let firmware_configs: heapless::Vec<AutoMouseLayerConfig, { crate::AUTO_MOUSE_LAYER_MAX_NUM }> =
            configs.into_iter().collect();
        self.keymap.set_auto_mouse_layer_configs(firmware_configs.clone());
        publish_event(AutoMouseLayerConfigChangeEvent);
        #[cfg(feature = "storage")]
        store(StorageItem::AutoMouseLayerConfigs(firmware_configs)).await?;
        Ok(true)
    }

    pub async fn set_combo_timeout(&self, ms: u16) -> Result<(), ()> {
        self.keymap.set_combo_timeout(Duration::from_millis(ms as u64));
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    pub async fn set_one_shot_timeout(&self, ms: u16) -> Result<(), ()> {
        self.keymap.set_one_shot_timeout(Duration::from_millis(ms as u64));
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    pub async fn set_tap_interval(&self, ms: u16) -> Result<(), ()> {
        self.keymap.set_tap_interval(ms);
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    pub async fn set_tap_capslock_interval(&self, ms: u16) -> Result<(), ()> {
        self.keymap.set_tap_capslock_interval(ms);
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    pub async fn set_morse_default_profile(&self, profile: MorseProfile) -> Result<(), ()> {
        self.keymap.set_morse_default_profile(profile);
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    pub async fn set_morse_prior_idle_time(&self, ms: u16) -> Result<(), ()> {
        self.keymap.set_morse_prior_idle_time(Duration::from_millis(ms as u64));
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    #[cfg(feature = "rynk")]
    pub async fn set_behavior_config(&self, cfg: BehaviorConfig) -> Result<(), ()> {
        self.keymap
            .set_combo_timeout(Duration::from_millis(cfg.combo_timeout_ms as u64));
        self.keymap
            .set_one_shot_timeout(Duration::from_millis(cfg.oneshot_timeout_ms as u64));
        self.keymap.set_tap_interval(cfg.tap_interval_ms);
        self.keymap.set_tap_capslock_interval(cfg.tap_capslock_interval_ms);
        self.keymap.set_morse_default_profile(cfg.morse_default_profile);
        self.keymap
            .set_morse_prior_idle_time(Duration::from_millis(cfg.morse_prior_idle_time_ms as u64));
        #[cfg(feature = "storage")]
        store(StorageItem::BehaviorConfig(self.keymap.behavior_snapshot())).await?;
        Ok(())
    }

    pub async fn set_layout_options(&self, opts: u32) -> Result<(), ()> {
        self.keymap.set_layout_option(opts);
        #[cfg(feature = "storage")]
        store(StorageItem::LayoutOption(opts)).await?;
        Ok(())
    }

    pub fn layout_options(&self) -> u32 {
        self.keymap.layout_option()
    }

    pub async fn reset_storage(&self) {
        #[cfg(feature = "storage")]
        crate::storage::reset().await;
    }

    pub fn led_indicator(&self) -> LedIndicator {
        crate::keyboard::current_led_indicator()
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        crate::state::current_connection_status()
    }

    #[cfg(feature = "_ble")]
    pub fn battery_status(&self) -> BatteryStatus {
        crate::input_device::battery::current_battery_status()
    }

    pub fn active_layer(&self) -> u8 {
        self.keymap.active_layer()
    }

    /// Snapshot the complete active-layer set for host consumers.
    ///
    /// The mutable keymap mask does not contain the default layer, so its bit
    /// is added explicitly to make this snapshot authoritative.
    #[cfg(feature = "rynk")]
    pub fn layer_state(&self) -> LayerState {
        let default_layer = self.keymap.get_default_layer();
        let mut active_bitmap = [0; LAYER_STATE_BITMAP_SIZE];

        if (default_layer as usize) < LAYER_STATE_CAPACITY {
            active_bitmap[default_layer as usize / 8] |= 1_u8 << (default_layer as usize % 8);
        }
        for layer in 0..self.keymap.num_layer().min(LAYER_STATE_CAPACITY) {
            if self.keymap.is_layer_active(layer as u8) {
                active_bitmap[layer / 8] |= 1_u8 << (layer % 8);
            }
        }

        LayerState {
            default_layer,
            active_bitmap,
        }
    }

    pub fn default_layer(&self) -> u8 {
        self.keymap.get_default_layer()
    }

    pub async fn set_default_layer(&self, layer: u8) -> Result<(), ()> {
        self.keymap.set_default_layer(layer);
        #[cfg(feature = "storage")]
        store(StorageItem::DefaultLayer(layer)).await?;
        Ok(())
    }

    /// Tiebreaker connection currently chosen as preferred — independent
    /// of which transport is actively routable.
    pub fn preferred_connection(&self) -> ConnectionType {
        crate::state::current_connection_status().preferred
    }

    pub fn get_fork(&self, idx: u8) -> Option<Fork> {
        self.keymap.with_forks(|forks| forks.get(idx as usize).copied())
    }

    /// Replace the fork at `idx` with `fork` and persist.
    /// `Ok(false)` when `idx` is out of range (no slot written).
    pub async fn set_fork(&self, idx: u8, fork: Fork) -> Result<bool, ()> {
        let valid = self.keymap.with_forks_mut(|forks| {
            if let Some(slot) = forks.get_mut(idx as usize) {
                *slot = fork;
                true
            } else {
                false
            }
        });
        #[cfg(feature = "storage")]
        if valid {
            store(StorageItem::Fork { idx, fork }).await?;
        }
        Ok(valid)
    }

    #[cfg(feature = "host_lock")]
    pub fn read_matrix_state(&self, target: &mut [u8]) {
        self.keymap.read_matrix_state(target);
    }
}

#[cfg(all(test, feature = "rynk"))]
mod tests {
    use rmk_types::action::KeyAction;

    use super::KeyboardContext;
    use crate::config::{BehaviorConfig, PositionalConfig};
    use crate::keymap::{KeyMap, KeymapData};
    use crate::test_support::test_block_on as block_on;

    #[test]
    fn layer_state_includes_default_explicit_and_tri_layers() {
        let mut data: KeymapData<1, 1, 64> = KeymapData::new([[[KeyAction::No]]; 64]);
        let mut behavior = BehaviorConfig {
            default_layer: 5,
            tri_layer: Some([1, 2, 3]),
            ..Default::default()
        };
        let positional = PositionalConfig::<1, 1>::default();
        let keymap = block_on(KeyMap::new(&mut data, &mut behavior, &positional));
        let context = KeyboardContext::new(&keymap);

        let default_only = context.layer_state();
        assert_eq!(default_only.default_layer, 5);
        assert!(default_only.is_active(5));
        assert!((0..64).all(|layer| layer == 5 || !default_only.is_active(layer)));

        keymap.activate_layer(1);
        keymap.activate_layer(2);
        keymap.activate_layer(63);

        let state = context.layer_state();
        assert_eq!(state.default_layer, 5);
        assert!(!state.is_active(0));
        assert!(state.is_active(1));
        assert!(state.is_active(2));
        assert!(state.is_active(3));
        assert!(state.is_active(5));
        assert!(state.is_active(63));
    }
}
