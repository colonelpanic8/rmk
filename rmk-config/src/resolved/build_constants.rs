use serde::Deserialize;

use crate::resolved::Capabilities;
use crate::{DEFAULT_PASSKEY_ENTRY_TIMEOUT_SECS, MIN_PASSKEY_ENTRY_TIMEOUT_SECS};

const SUBSCRIBER_DEFAULT_CONFIG: &str = include_str!("../default_config/subscriber_default.toml");

/// Parsed representation of `subscriber_default.toml`.
#[derive(Deserialize)]
struct SubscriberConfig {
    subscriber: Vec<SubscriberEntry>,
}

/// A single entry: bump `subs` for each listed event when all `capabilities` are active.
#[derive(Deserialize)]
struct SubscriberEntry {
    capabilities: Vec<String>,
    events: Vec<SubscriberEventEntry>,
}

/// Per-event subscriber bump. `count` defaults to 1.
#[derive(Deserialize)]
struct SubscriberEventEntry {
    name: String,
    #[serde(default = "default_sub_count")]
    count: usize,
}

fn default_sub_count() -> usize {
    1
}

/// Compile-time constants emitted as `pub const` items by `rmk-types/build.rs`.
pub struct BuildConstants {
    pub combo_max_num: usize,
    pub combo_max_length: usize,
    pub fork_max_num: usize,
    pub morse_max_num: usize,
    pub max_patterns_per_key: usize,
    pub macro_space_size: usize,
    pub debounce_time: u16,
    pub mouse_key_interval: u16,
    pub mouse_wheel_interval: u16,
    pub report_channel_size: usize,
    pub vial_channel_size: usize,
    pub flash_channel_size: usize,
    pub split_peripherals_num: usize,
    pub ble_profiles_num: usize,
    pub split_central_sleep_timeout_seconds: u32,
    pub protocol_macro_chunk_size: usize,
    /// Rynk RX/TX buffer size (bytes).
    pub rynk_buffer_size: usize,
    pub events: Vec<EventChannel>,
    pub passkey: Option<Passkey>,
}

pub struct EventChannel {
    pub name: String,
    pub channel_size: usize,
    pub pubs: usize,
    pub subs: usize,
}

pub struct Passkey {
    pub enabled: bool,
    pub timeout_secs: u32,
}

impl crate::KeyboardTomlConfig {
    /// Build compile-time constants from the configuration.
    ///
    /// Active capabilities are matched against `subscriber_default.toml` to
    /// auto-bump event subscriber counts.
    pub fn build_constants(&self, caps: &Capabilities) -> Result<BuildConstants, String> {
        let rmk = &self.rmk;

        // Fix split_peripherals_num: when split is active, ensure at least 1
        let split_peripherals_num = if caps.split && rmk.split_peripherals_num < 1 {
            1
        } else {
            rmk.split_peripherals_num
        };

        // Build event channels
        macro_rules! event_channels {
            ($($field:ident),* $(,)?) => {
                vec![$(
                    EventChannel {
                        name: stringify!($field).to_string(),
                        channel_size: self.event.$field.channel_size,
                        pubs: self.event.$field.pubs,
                        subs: self.event.$field.subs,
                    },
                )*]
            };
        }

        let mut events = event_channels!(
            connection_status_change,
            modifier,
            keyboard,
            layer_change,
            wpm_update,
            led_indicator,
            sleep_state,
            battery_status,
            battery_adc,
            charging_state,
            pointing,
            peripheral_connected,
            central_connected,
            peripheral_battery,
            clear_peer,
            dfu_status,
            action,
        );

        // Auto-bump subscriber counts based on active capabilities.
        // Declarations live in subscriber_default.toml.
        apply_capability_subscriber_bumps(&mut events, &caps.active_names());

        // Only validate passkey settings when the build will emit passkey constants.
        let passkey = if caps.passkey_entry {
            self.ble.as_ref().map(resolve_passkey_enabled).transpose()?
        } else {
            None
        };

        // Validate that config values do not exceed protocol ceilings.
        use crate::protocol_limits;
        if rmk.combo_max_length > protocol_limits::MAX_COMBO_SIZE {
            return Err(format!(
                "combo_max_length ({}) exceeds protocol ceiling MAX_COMBO_SIZE ({})",
                rmk.combo_max_length,
                protocol_limits::MAX_COMBO_SIZE
            ));
        }
        if rmk.max_patterns_per_key > protocol_limits::MAX_MORSE_SIZE {
            return Err(format!(
                "max_patterns_per_key ({}) exceeds protocol ceiling MAX_MORSE_SIZE ({})",
                rmk.max_patterns_per_key,
                protocol_limits::MAX_MORSE_SIZE
            ));
        }
        if rmk.protocol_macro_chunk_size > protocol_limits::MAX_MACRO_DATA_SIZE {
            return Err(format!(
                "protocol_macro_chunk_size ({}) exceeds protocol ceiling MAX_MACRO_DATA_SIZE ({})",
                rmk.protocol_macro_chunk_size,
                protocol_limits::MAX_MACRO_DATA_SIZE
            ));
        }
        // Host capability fields are u8/u16 on the wire; check the values no deserializer bound
        // covers (morse_max_num and split_peripherals_num can also be auto-raised past 255).
        validate_u8_capability("morse_max_num", rmk.morse_max_num)?;
        validate_u8_capability("split_peripherals_num", split_peripherals_num)?;
        validate_u8_capability("ble_profiles_num", rmk.ble_profiles_num)?;
        validate_u16_capability("macro_space_size", rmk.macro_space_size)?;
        validate_u16_capability("rynk_buffer_size", rmk.rynk_buffer_size)?;
        Ok(BuildConstants {
            combo_max_num: rmk.combo_max_num,
            combo_max_length: rmk.combo_max_length,
            fork_max_num: rmk.fork_max_num,
            morse_max_num: rmk.morse_max_num,
            max_patterns_per_key: rmk.max_patterns_per_key,
            macro_space_size: rmk.macro_space_size,
            debounce_time: rmk.debounce_time,
            mouse_key_interval: rmk.mouse_key_interval,
            mouse_wheel_interval: rmk.mouse_wheel_interval,
            report_channel_size: rmk.report_channel_size,
            vial_channel_size: rmk.vial_channel_size,
            flash_channel_size: rmk.flash_channel_size,
            split_peripherals_num,
            ble_profiles_num: rmk.ble_profiles_num,
            split_central_sleep_timeout_seconds: rmk.split_central_sleep_timeout_seconds,
            protocol_macro_chunk_size: rmk.protocol_macro_chunk_size,
            rynk_buffer_size: rmk.rynk_buffer_size,
            events,
            passkey,
        })
    }
}

fn validate_u8_capability(name: &str, value: usize) -> Result<(), String> {
    if value > u8::MAX as usize {
        return Err(format!(
            "{name} ({value}) exceeds the u8 host capability field (max 255)"
        ));
    }
    Ok(())
}

fn validate_u16_capability(name: &str, value: usize) -> Result<(), String> {
    if value > u16::MAX as usize {
        return Err(format!(
            "{name} ({value}) exceeds the u16 host capability field (max 65535)"
        ));
    }
    Ok(())
}

/// Bump event subscriber counts based on capabilities declared in `subscriber_default.toml`.
fn apply_capability_subscriber_bumps(events: &mut [EventChannel], active_names: &[&str]) {
    let sub_config: SubscriberConfig =
        toml::from_str(SUBSCRIBER_DEFAULT_CONFIG).expect("Failed to parse subscriber_default.toml");

    for entry in &sub_config.subscriber {
        let all_enabled = entry.capabilities.iter().all(|c| active_names.contains(&c.as_str()));
        if all_enabled {
            for sub_event in &entry.events {
                if let Some(event) = events.iter_mut().find(|e| e.name == sub_event.name) {
                    event.subs += sub_event.count;
                } else {
                    println!(
                        "cargo:warning=subscriber_default.toml: unknown event \"{}\"",
                        sub_event.name
                    );
                }
            }
        }
    }
}

fn resolve_passkey_enabled(ble: &crate::BleConfig) -> Result<Passkey, String> {
    let enabled = ble.passkey_entry.unwrap_or(false);
    let timeout_secs = ble.passkey_entry_timeout.unwrap_or(DEFAULT_PASSKEY_ENTRY_TIMEOUT_SECS);
    if timeout_secs < MIN_PASSKEY_ENTRY_TIMEOUT_SECS {
        return Err(format!(
            "keyboard.toml: [ble.passkey_entry_timeout] must be at least {} seconds, got {}",
            MIN_PASSKEY_ENTRY_TIMEOUT_SECS, timeout_secs
        ));
    }
    Ok(Passkey { enabled, timeout_secs })
}

#[cfg(test)]
mod tests {
    use super::{resolve_passkey_enabled, validate_u8_capability, validate_u16_capability};
    use crate::{BleConfig, DEFAULT_PASSKEY_ENTRY_TIMEOUT_SECS, KeyboardTomlConfig, MIN_PASSKEY_ENTRY_TIMEOUT_SECS};

    #[test]
    fn reserves_led_subscribers_for_display_split_and_dual_rynk_sessions() {
        let config: KeyboardTomlConfig = toml::from_str("").unwrap();
        let caps = crate::resolved::Capabilities {
            display: true,
            split: true,
            rynk: true,
            ble: true,
            ..Default::default()
        };
        let constants = config.build_constants(&caps).unwrap();
        let led_indicator = constants
            .events
            .iter()
            .find(|event| event.name == "led_indicator")
            .unwrap();

        // Three indicator processors, the display, two split peripherals, and USB/BLE Rynk sessions.
        assert_eq!(led_indicator.subs, 8);
    }

    #[test]
    fn validates_passkey_timeout() {
        let ble = BleConfig {
            passkey_entry_timeout: Some(MIN_PASSKEY_ENTRY_TIMEOUT_SECS - 1),
            ..Default::default()
        };

        let err = match resolve_passkey_enabled(&ble) {
            Ok(_) => panic!("expected passkey timeout validation failure"),
            Err(err) => err,
        };
        assert_eq!(
            err,
            format!(
                "keyboard.toml: [ble.passkey_entry_timeout] must be at least {} seconds, got {}",
                MIN_PASSKEY_ENTRY_TIMEOUT_SECS,
                MIN_PASSKEY_ENTRY_TIMEOUT_SECS - 1
            )
        );
    }

    #[test]
    fn uses_default_timeout() {
        let ble = BleConfig::default();
        let passkey = resolve_passkey_enabled(&ble).unwrap();

        assert!(!passkey.enabled);
        assert_eq!(passkey.timeout_secs, DEFAULT_PASSKEY_ENTRY_TIMEOUT_SECS);
    }

    #[test]
    fn validates_capability_wire_widths() {
        assert!(validate_u8_capability("ble_profiles_num", 255).is_ok());
        assert_eq!(
            validate_u8_capability("ble_profiles_num", 256),
            Err("ble_profiles_num (256) exceeds the u8 host capability field (max 255)".to_string())
        );

        assert!(validate_u16_capability("macro_space_size", 65535).is_ok());
        assert_eq!(
            validate_u16_capability("macro_space_size", 65536),
            Err("macro_space_size (65536) exceeds the u16 host capability field (max 65535)".to_string())
        );
    }
}
