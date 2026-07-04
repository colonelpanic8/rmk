use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, de};
use serde_inline_default::serde_inline_default;

use crate::behavior::BehaviorConfig;
use crate::board::{MatrixConfig, OutputConfig, SplitConfig};
use crate::chip::{ChipConfig, KeyboardInfo};
use crate::communication::BleConfig;
use crate::display::DisplayConfig;
use crate::duration::{de_millis, de_secs};
use crate::event::{EVENT_DEFAULT_CONFIG, EventConfig};
use crate::host::HostConfig;
use crate::input_device::InputDeviceConfig;
use crate::keymap::KeymapTomlConfig;
use crate::layout::LayoutTomlConfig;
use crate::light::LightConfig;
use crate::storage::StorageConfig;

pub mod resolved;

#[rustfmt::skip]
pub mod usb_interrupt_map;
pub(crate) mod behavior;
pub(crate) mod board;
pub(crate) mod chip;
pub(crate) mod communication;
pub(crate) mod display;
pub(crate) mod duration;
pub(crate) mod event;
pub(crate) mod host;
pub(crate) mod input_device;
pub(crate) mod keycode_alias;
pub(crate) mod keymap;
pub(crate) mod layout;
pub use layout::layout_blob_from_toml;
pub(crate) mod light;
pub(crate) mod storage;

/// Protocol-level capacity ceilings for wire-format Vec sizes.
///
/// These define the maximum values any firmware may use for protocol
/// Vec capacities (`COMBO_SIZE`, `MORSE_SIZE`, etc.). The host tool compiles
/// against these as upper bounds. Any firmware with `rynk` enabled
/// must satisfy `value <= ceiling` at compile time.
///
/// Constant names mirror the generated constants with a `MAX_` prefix:
/// `COMBO_SIZE` is bounded by `MAX_COMBO_SIZE`, etc.
pub mod protocol_limits {
    /// Max keys in a combo trigger — ceiling for `COMBO_SIZE`
    pub const MAX_COMBO_SIZE: usize = 16;
    /// Max pattern entries per morse key — ceiling for `MORSE_SIZE`
    pub const MAX_MORSE_SIZE: usize = 32;
    /// Max bytes per macro data chunk — ceiling for `MACRO_DATA_SIZE`
    pub const MAX_MACRO_DATA_SIZE: usize = 256;
    /// Max items per bulk transfer message — ceiling for `BULK_SIZE`
    pub const MAX_BULK_SIZE: usize = 16;
}

/// Configurations for RMK keyboard.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(unused)]
pub struct KeyboardTomlConfig {
    /// Basic keyboard info
    keyboard: Option<KeyboardInfo>,
    /// Matrix of the keyboard, only for non-split keyboards
    matrix: Option<MatrixConfig>,
    // Aliases for key maps
    aliases: Option<HashMap<String, String>>,
    /// Keymap config: layer count and the per-layer key actions (`[[keymap.layer]]`).
    keymap: Option<KeymapTomlConfig>,
    /// Layout config: the physical key arrangement (`map`) plus the rendered layout.
    /// For split keyboards, the total row/col is defined in this section.
    layout: Option<LayoutTomlConfig>,
    /// Behavior config
    behavior: Option<BehaviorConfig>,
    /// Light config
    light: Option<LightConfig>,
    /// Storage config
    storage: Option<StorageConfig>,
    /// Ble config
    pub(crate) ble: Option<BleConfig>,
    /// Chip-specific configs (e.g., [chip.nrf52840])
    chip: Option<HashMap<String, ChipConfig>>,
    /// Dependency config
    dependency: Option<DependencyConfig>,
    /// Split config
    split: Option<SplitConfig>,
    /// Input device config
    input_device: Option<InputDeviceConfig>,
    /// Display config
    display: Option<DisplayConfig>,
    /// Output Pin config
    output: Option<Vec<OutputConfig>>,
    /// Set host configurations
    pub(crate) host: Option<HostConfig>,
    /// RMK config constants
    #[serde(default)]
    pub(crate) rmk: RmkConstantsConfig,
    /// Event channel configuration
    /// Default values are loaded from event_default.toml in new_from_toml_path()
    /// build.rs also loads event defaults via new_from_toml_path_with_event_defaults()
    #[serde(default)]
    pub(crate) event: EventConfig,
}

/// Deep-merge `overlay` into `base`: tables merge key-by-key recursively; any
/// other value (including arrays) replaces the base value wholesale.
fn merge_toml_value(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, value) in overlay_table {
                match base_table.get_mut(&key) {
                    Some(existing) => merge_toml_value(existing, value),
                    None => {
                        base_table.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

impl KeyboardTomlConfig {
    fn parse_from_toml_path<P: AsRef<Path>>(config_toml_path: P, chip_default_config: Option<&str>) -> Self {
        let path = config_toml_path.as_ref();
        let user_toml = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
        Self::parse_from_sources(&user_toml, chip_default_config)
            .unwrap_or_else(|e| panic!("❌ Error in {}:\n{}", path.display(), e))
    }

    /// Parse the user's TOML over the default layers.
    ///
    /// The user source is deserialized on its own first: every root section is
    /// optional and no required leaf is sourced from a default layer, so any
    /// structural error here is the user's own — and reporting it from their
    /// document keeps toml's line/column caret. The value-tree merge below
    /// erases spans, so its error path only covers default-layer interplay.
    fn parse_from_sources(user_toml: &str, chip_default_config: Option<&str>) -> Result<Self, String> {
        let _: Self = toml::from_str(user_toml).map_err(|e| e.to_string())?;

        let mut merged: toml::Value = toml::from_str(EVENT_DEFAULT_CONFIG).expect("event_default.toml is valid TOML");
        if let Some(default_config) = chip_default_config {
            merge_toml_value(
                &mut merged,
                toml::from_str(default_config).expect("chip default config is valid TOML"),
            );
        }
        merge_toml_value(&mut merged, toml::from_str(user_toml).expect("user TOML parsed above"));
        merged
            .try_into()
            .map_err(|e| format!("(after merging default config layers) {e}"))
    }

    /// Load keyboard.toml without requiring `[keyboard].board`/`[keyboard].chip`.
    ///
    /// This is used in rmk-types/build.rs, which must also work for pure-Rust-API
    /// users that have no chip declared. When a chip *is* resolvable, its default
    /// layer is applied so this path sees the same merge as the proc macro.
    pub fn new_from_toml_path_with_event_defaults<P: AsRef<Path>>(config_toml_path: P) -> Self {
        let mut config = Self::parse_from_toml_path(&config_toml_path, None);
        if let Ok(chip_defaults) = config.get_chip_model().and_then(|chip| chip.get_default_config_str()) {
            config = Self::parse_from_toml_path(&config_toml_path, Some(chip_defaults));
        }
        config.auto_calculate_parameters();
        config
    }

    pub fn new_from_toml_path<P: AsRef<Path>>(config_toml_path: P) -> Self {
        let path = config_toml_path.as_ref();

        // First pass: load user config with event defaults to get chip model.
        // This allows user's keyboard.toml to omit [event] section.
        let user_config = Self::parse_from_toml_path(path, None);

        let default_config_str = user_config
            .get_chip_model()
            .and_then(|chip| chip.get_default_config_str())
            .unwrap_or_else(|e| panic!("❌ keyboard.toml error: {e}"));

        // Second pass: load with all three config sources
        // Config priority (later sources override earlier ones):
        // 1. Event default config (lowest priority)
        // 2. Chip-specific default config
        // 3. User config (highest priority)
        let mut config = Self::parse_from_toml_path(path, Some(default_config_str));

        config.auto_calculate_parameters();

        config
    }

    /// Auto calculate some parameters in toml:
    /// - Update morse_max_num to fit all configured morses
    /// - Update max_patterns_per_key to fit the max number of configured (pattern, action) pairs per morse key
    /// - Update peripheral number based on the number of split boards
    /// - TODO: Update controller number based on the number of split boards
    pub(crate) fn auto_calculate_parameters(&mut self) {
        // Update the number of peripherals
        if let Some(split) = &self.split
            && split.peripheral.len() > self.rmk.split_peripherals_num
        {
            // eprintln!(
            //     "The number of split peripherals is updated to {} from {}",
            //     split.peripheral.len(),
            //     self.rmk.split_peripherals_num
            // );
            self.rmk.split_peripherals_num = split.peripheral.len();
        }

        if let Some(behavior) = &self.behavior {
            // Update the max_patterns_per_key
            if let Some(morse) = &behavior.morse
                && let Some(morses) = &morse.morses
            {
                let mut max_required_patterns = self.rmk.max_patterns_per_key;

                for morse in morses {
                    let tap_actions_len = morse.tap_actions.as_ref().map(|v| v.len()).unwrap_or(0);
                    let hold_actions_len = morse.hold_actions.as_ref().map(|v| v.len()).unwrap_or(0);

                    let n = tap_actions_len.max(hold_actions_len);
                    if n > 15 {
                        panic!("The number of taps per morse is too large, the max number of taps is 15, got {n}");
                    }

                    let morse_actions_len = morse.morse_actions.as_ref().map(|v| v.len()).unwrap_or(0);

                    max_required_patterns =
                        max_required_patterns.max(tap_actions_len + hold_actions_len + morse_actions_len);
                }
                self.rmk.max_patterns_per_key = max_required_patterns;

                // Update the morse_max_num
                self.rmk.morse_max_num = self.rmk.morse_max_num.max(morses.len());
            }
        }
    }
}

/// Keyboard constants configuration for performance and hardware limits
#[serde_inline_default]
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RmkConstantsConfig {
    /// Mouse key interval (integer ms, or a "20ms"/"1s" string) - controls mouse movement speed
    #[serde_inline_default(20)]
    #[serde(deserialize_with = "de_millis")]
    pub mouse_key_interval: u16,
    /// Mouse wheel interval (integer ms, or a "20ms"/"1s" string) - controls scrolling speed
    #[serde_inline_default(80)]
    #[serde(deserialize_with = "de_millis")]
    pub mouse_wheel_interval: u16,
    /// Maximum number of combos keyboard can store
    #[serde_inline_default(8)]
    #[serde(deserialize_with = "check_combo_max_num")]
    pub combo_max_num: usize,
    /// Maximum number of keys pressed simultaneously in a combo
    #[serde_inline_default(4)]
    pub combo_max_length: usize,
    /// Maximum number of forks for conditional key actions
    #[serde_inline_default(8)]
    #[serde(deserialize_with = "check_fork_max_num")]
    pub fork_max_num: usize,
    /// Maximum number of morses keyboard can store
    #[serde_inline_default(8)]
    #[serde(deserialize_with = "check_morse_max_num")]
    pub morse_max_num: usize,
    /// Maximum number of patterns a morse key can handle
    #[serde_inline_default(8)]
    #[serde(deserialize_with = "check_max_patterns_per_key")]
    pub max_patterns_per_key: usize,
    /// Macro space size in bytes for storing sequences
    #[serde_inline_default(256)]
    pub macro_space_size: usize,
    /// Default debounce time (integer ms, or a "20ms" string)
    #[serde_inline_default(20)]
    #[serde(deserialize_with = "de_millis")]
    pub debounce_time: u16,
    /// Report channel size
    #[serde_inline_default(16)]
    pub report_channel_size: usize,
    /// Vial channel size
    #[serde_inline_default(4)]
    pub vial_channel_size: usize,
    /// Flash channel size
    #[serde_inline_default(4)]
    pub flash_channel_size: usize,
    /// The number of the split peripherals
    #[serde_inline_default(0)]
    pub split_peripherals_num: usize,
    /// The number of available BLE profiles
    #[serde_inline_default(3)]
    pub ble_profiles_num: usize,
    /// BLE Split Central sleep timeout (integer seconds or a "300s" string; 0 = disabled)
    #[serde_inline_default(0)]
    #[serde(deserialize_with = "de_secs")]
    pub split_central_sleep_timeout_seconds: u32,
    /// Maximum number of key actions in a bulk keymap transfer (protocol).
    /// Smaller values reduce firmware RAM usage but require more round-trips.
    #[serde_inline_default(8)]
    pub protocol_max_bulk_size: usize,
    /// Maximum macro data chunk size for protocol transfers (bytes).
    /// Smaller values reduce firmware RAM usage but require more round-trips.
    #[serde_inline_default(64)]
    pub protocol_macro_chunk_size: usize,
    /// Optional override for the Rynk RX/TX buffer size (bytes). When `None`,
    /// the build emits `RYNK_BUFFER_SIZE = RYNK_MIN_BUFFER_SIZE`. The const
    /// assertion in `rmk/src/host/rynk` rejects user values below the floor.
    #[serde(default)]
    pub rynk_buffer_size: Option<usize>,
}

fn check_combo_max_num<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = Deserialize::deserialize(deserializer)?;
    if value > 256 {
        return Err(de::Error::custom(format!(
            "combo_max_num must be between 0 and 256, got {value}"
        )));
    }
    Ok(value)
}

fn check_morse_max_num<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = Deserialize::deserialize(deserializer)?;
    if value > 256 {
        return Err(de::Error::custom(format!(
            "morse_max_num must be between 0 and 256, got {value}"
        )));
    }
    Ok(value)
}

fn check_max_patterns_per_key<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = Deserialize::deserialize(deserializer)?;
    if !(4..=65536).contains(&value) {
        return Err(de::Error::custom(format!(
            "max_patterns_per_key must be between 4 and 65536, got {value}"
        )));
    }
    Ok(value)
}

fn check_fork_max_num<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: de::Deserializer<'de>,
{
    let value = Deserialize::deserialize(deserializer)?;
    if value > 256 {
        return Err(de::Error::custom(format!(
            "fork_max_num must be between 0 and 256, got {value}"
        )));
    }
    Ok(value)
}

/// Used when the `[rmk]` section is absent. Deserializing an empty table keeps
/// the `serde_inline_default` attributes as the single source of default values.
impl Default for RmkConstantsConfig {
    fn default() -> Self {
        toml::from_str("").expect("inline defaults fill every RmkConstantsConfig field")
    }
}

/// Configurations for dependencies
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyConfig {
    /// Enable defmt log or not
    #[serde(default = "default_true")]
    pub defmt_log: bool,
}

impl Default for DependencyConfig {
    fn default() -> Self {
        Self { defmt_log: true }
    }
}

pub(crate) const fn default_true() -> bool {
    true
}

pub(crate) const fn default_false() -> bool {
    false
}

impl KeyboardTomlConfig {
    pub(crate) fn get_output_config(&self) -> Result<Vec<OutputConfig>, String> {
        let output_config = self.output.clone();
        let split = self.split.clone();
        match (output_config, split) {
            (None, Some(s)) => Ok(s.central.output.unwrap_or_default()),
            (Some(c), None) => Ok(c),
            (None, None) => Ok(Default::default()),
            _ => Err("Use [[split.output]] to define outputs for split in your keyboard.toml!".to_string()),
        }
    }

    pub(crate) fn get_dependency_config(&self) -> DependencyConfig {
        self.dependency.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{MatrixConfig, SplitConfig};
    use crate::chip::ChipConfig;
    use crate::input_device::EncoderConfig;

    #[test]
    fn time_fields_accept_integers_and_duration_strings() {
        let config = KeyboardTomlConfig::parse_from_sources(
            "[rmk]\ndebounce_time = \"5ms\"\nmouse_wheel_interval = \"1s\"\nsplit_central_sleep_timeout_seconds = \"5m\"\n",
            None,
        );
        // "5m" is not a valid unit
        assert!(config.is_err(), "minutes should be rejected");

        let config = KeyboardTomlConfig::parse_from_sources(
            "[rmk]\ndebounce_time = \"5ms\"\nmouse_wheel_interval = \"1s\"\nsplit_central_sleep_timeout_seconds = \"300s\"\n",
            None,
        )
        .unwrap();
        assert_eq!(config.rmk.debounce_time, 5);
        assert_eq!(config.rmk.mouse_wheel_interval, 1000);
        assert_eq!(config.rmk.split_central_sleep_timeout_seconds, 300);

        // Bare integers keep their native unit
        let config = KeyboardTomlConfig::parse_from_sources("[rmk]\ndebounce_time = 7\n", None).unwrap();
        assert_eq!(config.rmk.debounce_time, 7);

        // Sub-second strings are rejected for whole-second fields
        let config =
            KeyboardTomlConfig::parse_from_sources("[rmk]\nsplit_central_sleep_timeout_seconds = \"500ms\"\n", None);
        assert!(
            config.is_err(),
            "sub-second value for a seconds field should be rejected"
        );
    }

    #[test]
    fn mode_typos_are_rejected_as_unknown_variants() {
        // These were free strings whose typos silently fell back to defaults
        let err = toml::from_str::<MatrixConfig>("debouncer = \"faast\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown variant"), "{err}");
        let err = toml::from_str::<EncoderConfig>("pin_a = \"a\"\npin_b = \"b\"\nphase = \"resolutoin\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown variant"), "{err}");
        let err = toml::from_str::<ChipConfig>("dcdc_reg0_voltage = \"5V\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown variant"), "{err}");
        let err = toml::from_str::<SplitConfig>("connection = \"usb\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown variant"), "{err}");
    }

    #[test]
    fn test_event_config_default_values() {
        let config = EventConfig::default();

        // Check some key default values from event_default.toml
        assert_eq!(config.keyboard.channel_size, 16);
        assert_eq!(config.keyboard.pubs, 2);
        assert_eq!(config.keyboard.subs, 3);

        assert_eq!(config.modifier.channel_size, 8);
        assert_eq!(config.modifier.pubs, 1);
        assert_eq!(config.modifier.subs, 2);

        assert_eq!(config.layer_change.channel_size, 1);
        assert_eq!(config.layer_change.subs, 1);

        assert_eq!(config.led_indicator.channel_size, 2);
        assert_eq!(config.led_indicator.pubs, 2);
        assert_eq!(config.led_indicator.subs, 4);

        assert_eq!(config.pointing.channel_size, 8);
        assert_eq!(config.pointing.subs, 2);

        assert_eq!(config.action.channel_size, 16);
        assert_eq!(config.action.pubs, 1);
        assert_eq!(config.action.subs, 0);
    }

    #[test]
    fn test_event_config_user_override() {
        // Simulate user config that overrides some event settings
        let user_toml = r#"
[event.keyboard]
channel_size = 32
"#;
        // Parse with event defaults first, then user config
        let config = KeyboardTomlConfig::parse_from_sources(user_toml, None).unwrap();

        // User-overridden values
        assert_eq!(config.event.keyboard.channel_size, 32);
        assert_eq!(config.event.keyboard.pubs, 2);
        assert_eq!(config.event.keyboard.subs, 3);

        // Non-overridden values should use defaults
        assert_eq!(config.event.modifier.channel_size, 8);
        assert_eq!(config.event.modifier.subs, 2);
        assert_eq!(config.event.layer_change.subs, 1);
    }

    #[test]
    fn test_event_config_partial_override_with_event_defaults_loader() {
        let user_toml = r#"
[event.layer_change]
subs = 2
"#;
        let path = std::env::temp_dir().join(format!(
            "rmk-event-defaults-loader-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, user_toml).unwrap();

        let config = KeyboardTomlConfig::new_from_toml_path_with_event_defaults(&path);
        std::fs::remove_file(path).unwrap();

        assert_eq!(config.event.layer_change.channel_size, 1);
        assert_eq!(config.event.layer_change.pubs, 2);
        assert_eq!(config.event.layer_change.subs, 2);
    }
}
