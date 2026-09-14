//! Shared context for the Vial and Rynk host services.

use embassy_time::Duration;
use rmk_types::action::{EncoderAction, KeyAction};
#[cfg(feature = "_ble")]
use rmk_types::battery::BatteryStatus;
use rmk_types::combo::Combo as ComboConfig;
use rmk_types::connection::{ConnectionStatus, ConnectionType};
use rmk_types::fork::Fork;
use rmk_types::led_indicator::LedIndicator;
use rmk_types::morse::{Morse, MorseProfile};
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::BehaviorConfig;

use crate::event::KeyboardEventPos;
use crate::keyboard::combo::Combo;
use crate::keymap::KeyMap;
#[cfg(feature = "storage")]
use crate::storage::{self, StorageItem, store};

/// How long a Rynk keymap write may wait for flash-queue room and its writes
/// before the host hears `Busy`. Short enough to sit inside any host reply
/// timeout; long enough that a host retrying on `Busy` polls a migrating store
/// at a gentle rate instead of hammering it.
#[cfg(feature = "storage")]
const PERSIST_ROOM_WAIT: Duration = Duration::from_millis(500);
#[cfg(feature = "storage")]
const PERSIST_ROOM_POLL: Duration = Duration::from_millis(20);

/// Whether `count` persist messages can enter a queue with `free` of
/// `capacity` slots open without parking the sender.
#[cfg(feature = "storage")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PersistRoom {
    /// Every message fits right now.
    Fits,
    /// The queue is draining; ask again shortly.
    Wait,
    /// More messages than the queue holds at once: they can only stream in.
    Oversize,
}

#[cfg(feature = "storage")]
pub(crate) fn persist_room(count: usize, free: usize, capacity: usize) -> PersistRoom {
    if count > capacity {
        PersistRoom::Oversize
    } else if count <= free {
        PersistRoom::Fits
    } else {
        PersistRoom::Wait
    }
}

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

    /// Run `persist`, which writes `count` items to flash, without parking the
    /// caller on a busy store. `None` means the host should hear `Busy` and retry.
    ///
    /// The storage task drains its queue in order, and a single item can hold
    /// it for tens of seconds while sequential-storage migrates a page through
    /// radio-scheduled flash timeslots. A Rynk handler waiting on such a write
    /// parks the session, and a parked session reads no requests, so the host's
    /// USB write times out and the keyboard looks dead. So `persist` only
    /// starts once the queue has room for all `count` items, and must land
    /// within the same bound. A timeout can leave earlier items applied; the
    /// host resends the whole page, which rewrites them unchanged. More items
    /// than the queue holds can never fit at once and stream in unbounded, so
    /// hosts that page by payload size alone still work.
    pub async fn persist_bounded<T>(&self, count: usize, persist: impl Future<Output = T>) -> Option<T> {
        #[cfg(feature = "storage")]
        {
            let deadline = embassy_time::Instant::now() + PERSIST_ROOM_WAIT;
            loop {
                match persist_room(count, storage::free_capacity(), crate::FLASH_CHANNEL_SIZE) {
                    PersistRoom::Fits => return embassy_time::with_deadline(deadline, persist).await.ok(),
                    PersistRoom::Oversize => return Some(persist.await),
                    PersistRoom::Wait if embassy_time::Instant::now() >= deadline => return None,
                    PersistRoom::Wait => embassy_time::Timer::after(PERSIST_ROOM_POLL).await,
                }
            }
        }
        #[cfg(not(feature = "storage"))]
        {
            let _ = count;
            Some(persist.await)
        }
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

    pub fn morse_prior_idle_time(&self) -> Duration {
        self.keymap.morse_prior_idle_time()
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

#[cfg(all(test, feature = "storage"))]
mod tests {
    use super::{PersistRoom, persist_room};

    #[test]
    fn persist_room_boundaries() {
        assert_eq!(persist_room(4, 4, 4), PersistRoom::Fits);
        assert_eq!(persist_room(4, 3, 4), PersistRoom::Wait);
        assert_eq!(persist_room(0, 0, 4), PersistRoom::Fits);
        assert_eq!(persist_room(5, 4, 4), PersistRoom::Oversize);
    }
}
