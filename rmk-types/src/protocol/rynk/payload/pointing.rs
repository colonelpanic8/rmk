//! Pointing-device configuration payloads.
//!
//! A pad's behavior is data, not firmware: the whole arrangement — what each
//! device does by default, and what it does while a given layer is on top —
//! travels in one message and is stored on the device. A board that ships no
//! configuration has no pointing policy at all.

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

use crate::pointing::PointingMode;

/// Pointing devices one [`PointingConfig`] can describe.
pub const POINTING_DEVICE_CAPACITY: usize = 4;

/// Layer overrides one [`PointingConfig`] can carry.
pub const POINTING_LAYER_OVERRIDE_CAPACITY: usize = 16;

/// What a device does when no layer override applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct PointingDeviceConfig {
    /// RMK pointing-device id this entry configures.
    pub device_id: u8,
    pub mode: PointingMode,
}

impl Default for PointingDeviceConfig {
    fn default() -> Self {
        Self {
            device_id: 0,
            mode: PointingMode::default(),
        }
    }
}

/// While `layer` is the topmost active layer, `device_id` uses `mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct PointingLayerOverride {
    pub layer: u8,
    pub device_id: u8,
    pub mode: PointingMode,
}

impl Default for PointingLayerOverride {
    fn default() -> Self {
        Self {
            layer: 0,
            device_id: 0,
            mode: PointingMode::default(),
        }
    }
}

/// Every pad's behavior, as one replaceable unit.
///
/// Entries past their counts are padding, so the encoding stays a fixed
/// shape rather than a length-prefixed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct PointingConfig {
    /// Bumped by the device on every accepted write, so a host that read a
    /// stale copy is told to re-read instead of clobbering another's edit.
    pub revision: u16,
    /// Valid entries in `devices`.
    pub device_count: u8,
    pub devices: [PointingDeviceConfig; POINTING_DEVICE_CAPACITY],
    /// Valid entries in `overrides`.
    pub override_count: u8,
    pub overrides: [PointingLayerOverride; POINTING_LAYER_OVERRIDE_CAPACITY],
}

impl Default for PointingConfig {
    fn default() -> Self {
        Self {
            revision: 0,
            device_count: 0,
            devices: [PointingDeviceConfig::default(); POINTING_DEVICE_CAPACITY],
            override_count: 0,
            overrides: [PointingLayerOverride::default(); POINTING_LAYER_OVERRIDE_CAPACITY],
        }
    }
}

impl PointingConfig {
    /// The device entries that carry meaning.
    pub fn devices(&self) -> &[PointingDeviceConfig] {
        &self.devices[..(self.device_count as usize).min(POINTING_DEVICE_CAPACITY)]
    }

    /// The layer overrides that carry meaning.
    pub fn overrides(&self) -> &[PointingLayerOverride] {
        &self.overrides[..(self.override_count as usize).min(POINTING_LAYER_OVERRIDE_CAPACITY)]
    }

    /// The mode `device_id` uses while `layer` is on top.
    ///
    /// An override wins over the device default, and a device with neither
    /// is left alone — an unconfigured board points nothing anywhere.
    pub fn mode_for(&self, device_id: u8, layer: u8) -> Option<PointingMode> {
        self.overrides()
            .iter()
            .find(|o| o.device_id == device_id && o.layer == layer)
            .map(|o| o.mode)
            .or_else(|| self.devices().iter().find(|d| d.device_id == device_id).map(|d| d.mode))
    }
}

/// A write that only lands if the host's `revision` still matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct SetPointingConfigRequest {
    pub config: PointingConfig,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointing::{CursorConfig, DragConfig, ScrollConfig};
    use crate::protocol::rynk::message::RYNK_MAX_PAYLOAD_SIZE;
    use crate::protocol::rynk::tests::{assert_max_size_bound, round_trip};

    fn populated() -> PointingConfig {
        let mut config = PointingConfig {
            revision: 7,
            device_count: 2,
            override_count: 2,
            ..Default::default()
        };
        config.devices[0] = PointingDeviceConfig {
            device_id: 0,
            mode: PointingMode::Scroll(ScrollConfig::default()),
        };
        config.devices[1] = PointingDeviceConfig {
            device_id: 1,
            mode: PointingMode::Cursor(CursorConfig::default()),
        };
        config.overrides[0] = PointingLayerOverride {
            layer: 2,
            device_id: 0,
            mode: PointingMode::Cursor(CursorConfig::default()),
        };
        config.overrides[1] = PointingLayerOverride {
            layer: 2,
            device_id: 1,
            mode: PointingMode::Drag(DragConfig::default()),
        };
        config
    }

    #[test]
    fn round_trip_pointing_config() {
        round_trip(&populated());
        round_trip(&PointingConfig::default());
        assert_max_size_bound(&populated());
        assert_max_size_bound(&SetPointingConfigRequest { config: populated() });
    }

    /// The whole arrangement travels in one message, so it has to fit in one.
    #[test]
    fn pointing_config_fits_a_single_frame() {
        assert!(
            SetPointingConfigRequest::POSTCARD_MAX_SIZE <= RYNK_MAX_PAYLOAD_SIZE,
            "SetPointingConfigRequest needs {} bytes, payload window is {}",
            SetPointingConfigRequest::POSTCARD_MAX_SIZE,
            RYNK_MAX_PAYLOAD_SIZE,
        );
    }

    #[test]
    fn override_wins_over_the_device_default() {
        let config = populated();
        // Layer 2 is overridden for both pads; layer 0 falls back.
        assert_eq!(config.mode_for(1, 2), Some(PointingMode::Drag(DragConfig::default())));
        assert_eq!(
            config.mode_for(1, 0),
            Some(PointingMode::Cursor(CursorConfig::default()))
        );
        // A device nobody configured is left alone.
        assert_eq!(config.mode_for(3, 0), None);
    }
}
