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
use rmk_types::protocol::rynk::{PointingConfig, RynkError};

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
pub async fn replace(next: PointingConfig) -> Result<PointingConfig, RynkError> {
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
