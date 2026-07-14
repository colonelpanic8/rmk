mod behavior;
#[cfg(rmk_ble)]
mod ble_battery;
mod device;
#[cfg(rmk_rynk)]
mod lock;
mod positional;
mod storage;
mod vial;

pub use behavior::{
    AutoMouseLayerConfig, BehaviorConfig, CombosConfig, ForksConfig, KeyboardMacrosConfig, MorsesConfig,
    MouseKeyConfig, OneShotConfig, OneShotModifiersConfig, TapConfig,
};
#[cfg(rmk_ble)]
pub use ble_battery::BleBatteryConfig;
pub use device::DeviceConfig;
#[cfg(rmk_rynk)]
pub use lock::LockConfig;
pub use positional::{Hand, PositionalConfig};
pub use storage::StorageConfig;
pub use vial::VialConfig;

/// Internal configurations for RMK keyboard.
#[derive(Default)]
pub struct RmkConfig<'a> {
    pub device_config: DeviceConfig<'a>,
    #[cfg(rmk_vial)]
    pub vial_config: VialConfig<'a>,
    #[cfg(rmk_rynk)]
    pub lock_config: LockConfig,
    /// Opaque, compressed physical-layout blob served over rynk's `GetLayout`.
    /// Baked at build time from `[layout].map`; empty when there's no layout.
    #[cfg(rmk_rynk)]
    pub layout_blob: &'a [u8],
    #[cfg(rmk_storage)]
    pub storage_config: StorageConfig,
    #[cfg(rmk_ble)]
    pub ble_battery_config: BleBatteryConfig<'a>,
}
