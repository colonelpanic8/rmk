//! The live pointing configuration: what each pad does, per layer.
//!
//! Modes are not compiled into a board. The device holds one
//! [`PointingConfig`], a host reads and replaces it over Rynk, and it is
//! restored from storage at boot. Everything that changes a pad's behavior
//! goes through here, so there is one answer to "why is this pad
//! scrolling?" rather than one per call site.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use rmk_macro::{event, processor};
use rmk_types::pointing::PointingMode;
use rmk_types::protocol::rynk::{
    POINTING_DEVICE_CAPACITY, POINTING_LAYER_OVERRIDE_CAPACITY, PointingConfig, RynkError,
};

#[cfg(feature = "storage")]
use crate::channel::FLASH_CHANNEL;
use crate::event::{LayerChangeEvent, PointingProcessorEvent, publish_event};
#[cfg(feature = "storage")]
use crate::storage::FlashOperationMessage;

/// Requests that pointing processors refresh their runtime configuration.
#[event(channel_size = 1, pubs = 1, subs = 4)]
#[derive(Clone, Copy, Debug)]
pub struct PointingConfigChangeEvent;

/// The live configuration.
struct State {
    config: PointingConfig,
}

static STATE: Mutex<CriticalSectionRawMutex, State> = Mutex::new(State {
    config: PointingConfig {
        revision: 0,
        device_count: 0,
        devices: [EMPTY_DEVICE; rmk_types::protocol::rynk::POINTING_DEVICE_CAPACITY],
        override_count: 0,
        overrides: [EMPTY_OVERRIDE; rmk_types::protocol::rynk::POINTING_LAYER_OVERRIDE_CAPACITY],
    },
});

// `Default` is not const, and a static needs one.
const EMPTY_DEVICE: rmk_types::protocol::rynk::PointingDeviceConfig = rmk_types::protocol::rynk::PointingDeviceConfig {
    device_id: 0,
    mode: rmk_types::pointing::PointingMode::Cursor(rmk_types::pointing::CursorConfig {
        multiplier_x: 1,
        multiplier_y: 1,
        invert_x: false,
        invert_y: false,
    }),
};

const EMPTY_OVERRIDE: rmk_types::protocol::rynk::PointingLayerOverride =
    rmk_types::protocol::rynk::PointingLayerOverride {
        layer: 0,
        device_id: 0,
        mode: EMPTY_DEVICE.mode,
    };

/// Seed the configuration from storage at boot and point the pads at it.
///
/// `stored` is `None` on a board that was never configured, which leaves
/// every pad alone rather than inventing a policy for it.
pub async fn init(stored: Option<PointingConfig>) {
    if let Some(config) = stored {
        STATE.lock().await.config = config;
    }
    publish_event(PointingConfigChangeEvent);
}

/// The configuration as the host would read it.
pub async fn get() -> PointingConfig {
    STATE.lock().await.config
}

/// Replace the configuration, persist it, and re-point the pads.
///
/// The write is refused unless `next.revision` matches what the device
/// holds, so a host editing a stale copy is told to re-read rather than
/// silently discarding someone else's change.
pub async fn replace(next: PointingConfig, layers: usize) -> Result<PointingConfig, RynkError> {
    validate(&next, layers)?;
    {
        let mut state = STATE.lock().await;
        if next.revision != state.config.revision {
            return Err(RynkError::Invalid);
        }
        state.config = next;
        state.config.revision = next.revision.wrapping_add(1);
        #[cfg(feature = "storage")]
        FLASH_CHANNEL
            .send(FlashOperationMessage::PointingConfig(state.config))
            .await;
    }
    publish_event(PointingConfigChangeEvent);
    Ok(get().await)
}

/// Refuse a configuration the device could not honour as written: counts
/// past capacity, duplicate device or override entries, overrides for
/// undeclared devices or layers, and modes whose numbers cannot work.
fn validate(config: &PointingConfig, layers: usize) -> Result<(), RynkError> {
    if config.device_count as usize > POINTING_DEVICE_CAPACITY
        || config.override_count as usize > POINTING_LAYER_OVERRIDE_CAPACITY
    {
        return Err(RynkError::Invalid);
    }
    let devices = config.devices();
    for (i, device) in devices.iter().enumerate() {
        if devices[..i].iter().any(|d| d.device_id == device.device_id) {
            return Err(RynkError::Invalid);
        }
        validate_mode(&device.mode)?;
    }
    let overrides = config.overrides();
    for (i, entry) in overrides.iter().enumerate() {
        if entry.layer as usize >= layers
            || !devices.iter().any(|d| d.device_id == entry.device_id)
            || overrides[..i]
                .iter()
                .any(|o| o.layer == entry.layer && o.device_id == entry.device_id)
        {
            return Err(RynkError::Invalid);
        }
        validate_mode(&entry.mode)?;
    }
    Ok(())
}

fn validate_mode(mode: &PointingMode) -> Result<(), RynkError> {
    let ok = match mode {
        PointingMode::Caret(caret) => caret.threshold > 0,
        PointingMode::Keypad(keypad) => keypad.threshold_x > 0 && keypad.threshold_y > 0,
        _ => true,
    };
    if ok { Ok(()) } else { Err(RynkError::Invalid) }
}

/// Re-points the pads whenever the active layer changes.
///
/// `LayerChangeEvent` carries the topmost active layer, so releasing a
/// momentary layer key announces the layer underneath and restores the
/// pads without any separate bookkeeping.
#[processor(subscribe = [LayerChangeEvent])]
pub struct PointingLayerModes;

impl PointingLayerModes {
    async fn on_layer_change_event(&mut self, LayerChangeEvent(layer): LayerChangeEvent) {
        let config = get().await;
        for device in config.devices() {
            if let Some(mode) = config.mode_for(device.device_id, layer) {
                publish_event(PointingProcessorEvent {
                    device_id: device.device_id,
                    mode,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rmk_types::pointing::CaretConfig;
    use rmk_types::protocol::rynk::{PointingDeviceConfig, PointingLayerOverride};

    use super::*;

    fn configured(device_count: u8, override_count: u8) -> PointingConfig {
        let mut config = PointingConfig {
            device_count,
            override_count,
            ..Default::default()
        };
        for (i, device) in config.devices.iter_mut().enumerate() {
            device.device_id = i as u8;
        }
        for (i, entry) in config.overrides.iter_mut().enumerate() {
            *entry = PointingLayerOverride {
                layer: (i % 2) as u8,
                device_id: (i / 2) as u8,
                mode: PointingMode::default(),
            };
        }
        config
    }

    #[test]
    fn accepts_a_consistent_configuration() {
        assert_eq!(validate(&configured(2, 4), 2), Ok(()));
    }

    #[test]
    fn rejects_counts_past_capacity() {
        let mut config = configured(1, 0);
        config.device_count = POINTING_DEVICE_CAPACITY as u8 + 1;
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
        let mut config = configured(1, 0);
        config.override_count = POINTING_LAYER_OVERRIDE_CAPACITY as u8 + 1;
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
    }

    #[test]
    fn rejects_duplicate_devices_and_overrides() {
        let mut config = configured(2, 0);
        config.devices[1].device_id = 0;
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
        let mut config = configured(1, 2);
        config.overrides[1] = config.overrides[0];
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
    }

    #[test]
    fn rejects_overrides_for_unknown_devices_or_layers() {
        let mut config = configured(1, 1);
        config.overrides[0].device_id = 3;
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
        let mut config = configured(1, 1);
        config.overrides[0].layer = 2;
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
    }

    #[test]
    fn rejects_non_positive_thresholds() {
        let mut config = configured(1, 0);
        config.devices[0] = PointingDeviceConfig {
            device_id: 0,
            mode: PointingMode::Caret(CaretConfig {
                threshold: 0,
                ..Default::default()
            }),
        };
        assert_eq!(validate(&config, 2), Err(RynkError::Invalid));
    }

    #[test]
    fn rejected_writes_keep_the_revision() {
        crate::test_support::test_block_on(async {
            let before = get().await;
            let mut config = configured(2, 0);
            config.revision = before.revision;
            config.devices[1].device_id = 0;
            assert_eq!(replace(config, 2).await, Err(RynkError::Invalid));
            assert_eq!(get().await.revision, before.revision);
        });
    }
}
