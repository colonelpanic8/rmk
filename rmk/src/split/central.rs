use embedded_io_async::{Read, Write};

#[cfg(feature = "dfu_split")]
pub use crate::split::driver::UpdatePolicy;

/// Run the manager task of one serial split peripheral.
///
/// BLE split peripherals are managed inside the BLE transport — pass one
/// [`PeripheralMatrixConfig`](crate::split::PeripheralMatrixConfig) per
/// peripheral to `BleTransport::new`.
pub async fn run_peripheral_manager<S: Read + Write>(
    id: usize,
    receiver: S,
    matrix_config: crate::split::PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: crate::split::driver::UpdatePolicy,
) {
    crate::split::serial::run_serial_peripheral_manager(
        id,
        receiver,
        matrix_config,
        #[cfg(feature = "dfu_split")]
        policy,
    )
    .await;
}

/// Run the manager task for one peripheral on a polled half-duplex bus.
pub async fn run_half_duplex_peripheral_manager<S: Read + Write>(
    id: usize,
    serial: S,
    matrix_config: crate::split::PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: crate::split::driver::UpdatePolicy,
) {
    crate::split::serial::run_half_duplex_peripheral_manager(
        id,
        serial,
        matrix_config,
        #[cfg(feature = "dfu_split")]
        policy,
    )
    .await;
}

/// Run a polled half-duplex manager while automatic selection prefers wired.
pub async fn run_auto_half_duplex_peripheral_manager<S: Read + Write>(
    id: usize,
    serial: S,
    matrix_config: crate::split::PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: crate::split::driver::UpdatePolicy,
) {
    crate::split::serial::run_auto_half_duplex_peripheral_manager(
        id,
        serial,
        matrix_config,
        #[cfg(feature = "dfu_split")]
        policy,
    )
    .await;
}
