//! System handlers — handshake, device identity, reboot, bootloader jump, storage reset.

use rmk_types::constants;
use rmk_types::protocol::rynk::command::{
    BootloaderJump, GetCapabilities, GetDeviceInfo, GetLockStatus, GetVersion, Lock, Reboot, StorageReset, UnlockPoll,
};
use rmk_types::protocol::rynk::{
    DEVICE_INFO_STRING_SIZE, DeviceCapabilities, DeviceInfo, LockStatus, ProtocolVersion, RYNK_HEADER_SIZE, RynkError,
    StorageResetMode,
};

use super::super::{RMK_VERSION, RynkService};
use super::Handle;

fn capability_u8(value: usize) -> Result<u8, RynkError> {
    u8::try_from(value).map_err(|_| RynkError::Invalid)
}

fn capability_u16(value: usize) -> Result<u16, RynkError> {
    u16::try_from(value).map_err(|_| RynkError::Invalid)
}

impl Handle<GetVersion> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<ProtocolVersion, RynkError> {
        Ok(ProtocolVersion::CURRENT)
    }
}

impl Handle<GetCapabilities> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<DeviceCapabilities, RynkError> {
        let (rows, cols, num_layers) = self.ctx.keymap_dimensions();
        let max_payload_size = constants::RYNK_BUFFER_SIZE
            .checked_sub(RYNK_HEADER_SIZE)
            .ok_or(RynkError::Invalid)?;
        Ok(DeviceCapabilities {
            // Layout (live, from the configured keymap)
            num_layers: capability_u8(num_layers)?,
            num_rows: capability_u8(rows)?,
            num_cols: capability_u8(cols)?,

            // Input device limits (compile-time from keyboard.toml)
            num_encoders: capability_u8(self.ctx.num_encoders())?,
            max_combos: capability_u8(constants::COMBO_MAX_NUM)?,
            max_combo_keys: capability_u8(constants::COMBO_MAX_LENGTH)?,
            macro_space_size: capability_u16(constants::MACRO_SPACE_SIZE)?,
            max_morse: capability_u8(constants::MORSE_MAX_NUM)?,
            max_patterns_per_key: capability_u8(constants::MAX_PATTERNS_PER_KEY)?,
            max_forks: capability_u8(constants::FORK_MAX_NUM)?,

            // Feature flags
            storage_enabled: cfg!(feature = "storage"),
            lighting_enabled: false, // TODO Phase 6: surface light_service

            // Connectivity
            is_split: cfg!(feature = "split"),
            num_split_peripherals: capability_u8(constants::SPLIT_PERIPHERALS_NUM)?,
            ble_enabled: cfg!(feature = "_ble"),
            num_ble_profiles: capability_u8(constants::NUM_BLE_PROFILE)?,

            // Protocol limits
            max_payload_size: capability_u16(max_payload_size)?,
            macro_chunk_size: capability_u16(constants::MACRO_DATA_SIZE)?,
            // The BULK_* constants only exist under `bulk`, hence #[cfg] over cfg!().
            #[cfg(feature = "bulk")]
            max_bulk_keys: capability_u8(constants::BULK_KEYMAP_SIZE)?,
            #[cfg(not(feature = "bulk"))]
            max_bulk_keys: 0,
            #[cfg(feature = "bulk")]
            max_bulk_configs: capability_u8(constants::BULK_SIZE)?,
            #[cfg(not(feature = "bulk"))]
            max_bulk_configs: 0,
            bulk_transfer_supported: cfg!(feature = "bulk"),
        })
    }
}

impl Handle<Reboot> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<(), RynkError> {
        // Fire-and-forget: synchronous reset never returns on real hardware.
        crate::boot::reboot_keyboard();
        Ok(())
    }
}

impl Handle<BootloaderJump> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<(), RynkError> {
        // Fire-and-forget, same reasoning as `Reboot`.
        crate::boot::jump_to_bootloader();
        Ok(())
    }
}

impl Handle<StorageReset> for RynkService<'_> {
    async fn handle(&self, mode: StorageResetMode) -> Result<(), RynkError> {
        if mode != StorageResetMode::Full {
            // TODO: Reset required storage range
            return Err(RynkError::Unimplemented);
        }
        self.ctx.reset_storage().await;
        Ok(())
    }
}

// Lock endpoints stay dispatchable while locked.

impl Handle<GetLockStatus> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<LockStatus, RynkError> {
        Ok(self.locker.status())
    }
}

impl Handle<UnlockPoll> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<LockStatus, RynkError> {
        Ok(self.locker.poll())
    }
}

impl Handle<Lock> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<(), RynkError> {
        self.locker.lock();
        Ok(())
    }
}

impl Handle<GetDeviceInfo> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<DeviceInfo, RynkError> {
        Ok(DeviceInfo {
            rmk_version: RMK_VERSION,
            vendor_id: self.device.vid,
            product_id: self.device.pid,
            manufacturer: truncated(self.device.manufacturer),
            product_name: truncated(self.device.product_name),
            serial_number: truncated(self.device.serial_number),
        })
    }
}

/// Copy `s` into the bounded wire string; over-long input is cut at the last
/// whole char that fits, so multi-byte content can never panic or split.
fn truncated(s: &str) -> heapless::String<DEVICE_INFO_STRING_SIZE> {
    let mut out = heapless::String::new();
    for c in s.chars() {
        if out.push(c).is_err() {
            break;
        }
    }
    out
}
