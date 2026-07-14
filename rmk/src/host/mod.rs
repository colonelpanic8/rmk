pub(crate) mod context;
// Physical-presence unlock gate, shared by the Vial (`vial` + `host_lock`)
// and Rynk (`rynk` ⇒ `host_lock`) services.
#[cfg(rmk_host_lock)]
pub(crate) mod lock;
#[cfg(rmk_rynk)]
pub(crate) mod rynk;
#[cfg(rmk_storage)]
pub(crate) mod storage;
// Shared transport-adapter error, used by the USB/BLE Vial and BLE Rynk
// adapters. Gated to exactly the feature combos that compile an adapter.
#[cfg(any(
    all(rmk_vial, rmk_usb),
    all(rmk_vial, rmk_ble),
    all(rmk_rynk, rmk_ble),
))]
pub(crate) mod transport;
#[cfg(rmk_vial)]
pub(crate) mod via;

/// The active host-protocol service. Resolves to [`via::VialService`]
/// under the `vial` feature and [`rynk::RynkService`] under `rynk` (the
/// two are mutually exclusive).
#[cfg(rmk_rynk)]
pub use rynk::RynkService as HostService;
/// UART-backed rynk transport helper.
#[cfg(rmk_rynk)]
pub use rynk::run_rynk_uart;
#[cfg(rmk_vial)]
pub use via::VialService as HostService;
