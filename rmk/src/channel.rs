//! Exposed channels which can be used to share data across devices & processors

use core::future::poll_fn;
#[cfg(feature = "_ble")]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(feature = "_ble", not(feature = "_no_usb")))]
use embassy_sync::channel::TryReceiveError;
use embassy_sync::channel::{Channel, TrySendError};
#[cfg(feature = "_ble")]
use embassy_sync::signal::Signal;
pub use embassy_sync::{blocking_mutex, channel, pubsub, zerocopy_channel};
#[cfg(feature = "_ble")]
use embassy_time::Instant;
use rmk_types::connection::ConnectionType;
#[cfg(feature = "_ble")]
use {crate::ble::profile::BleProfileAction, rmk_types::led_indicator::LedIndicator};

#[cfg(feature = "host")]
use crate::VIAL_CHANNEL_SIZE;
use crate::hid::{Report, neutral_reports};
#[cfg(feature = "storage")]
use crate::{FLASH_CHANNEL_SIZE, storage::FlashOperationMessage};
use crate::{REPORT_CHANNEL_SIZE, RawMutex};

#[cfg(not(feature = "_no_usb"))]
pub(crate) struct RoutedUsbReport {
    pub(crate) route_generation: u32,
    pub(crate) force: bool,
    pub(crate) report: Report,
}

#[cfg(not(feature = "_no_usb"))]
pub struct UsbReportChannel {
    pub(crate) inner: Channel<RawMutex, RoutedUsbReport, REPORT_CHANNEL_SIZE>,
}

#[cfg(not(feature = "_no_usb"))]
impl UsbReportChannel {
    pub async fn send(&self, report: Report) {
        self.send_for_generation(report, crate::state::report_route().generation)
            .await;
    }

    async fn send_for_generation(&self, mut report: Report, route_generation: u32) {
        loop {
            match self.inner.try_send(RoutedUsbReport {
                route_generation,
                force: false,
                report,
            }) {
                Ok(()) => return,
                Err(TrySendError::Full(routed)) => report = routed.report,
            }

            poll_fn(|cx| self.inner.poll_ready_to_send(cx)).await;
            let current_route = crate::state::report_route();
            if current_route.active != Some(ConnectionType::Usb) || current_route.generation != route_generation {
                return;
            }
        }
    }

    pub fn try_send(&self, report: Report) -> Result<(), TrySendError<Report>> {
        self.try_send_for_generation(report, crate::state::report_route().generation)
    }

    fn try_send_for_generation(&self, report: Report, route_generation: u32) -> Result<(), TrySendError<Report>> {
        self.inner
            .try_send(RoutedUsbReport {
                route_generation,
                force: false,
                report,
            })
            .map_err(|TrySendError::Full(routed)| TrySendError::Full(routed.report))
    }

    pub fn try_receive(&self) -> Result<Report, TryReceiveError> {
        self.inner.try_receive().map(|routed| routed.report)
    }

    pub async fn receive(&self) -> Report {
        self.inner.receive().await.report
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn clear(&self) {
        self.inner.clear();
    }
}

#[cfg(feature = "_ble")]
pub(crate) struct RoutedBleReport {
    pub(crate) slot: u8,
    pub(crate) route_generation: u32,
    pub(crate) force: bool,
    pub(crate) report: Report,
}

#[cfg(feature = "_ble")]
pub struct BleReportChannel {
    pub(crate) inner: Channel<RawMutex, RoutedBleReport, REPORT_CHANNEL_SIZE>,
}

#[cfg(feature = "_ble")]
impl BleReportChannel {
    pub async fn send(&self, report: Report) {
        let route = crate::state::report_route();
        self.send_for_route(report, route.ble_profile, route.generation).await;
    }

    async fn send_for_route(&self, mut report: Report, slot: u8, route_generation: u32) {
        loop {
            match self.inner.try_send(RoutedBleReport {
                slot,
                route_generation,
                force: false,
                report,
            }) {
                Ok(()) => return,
                Err(TrySendError::Full(routed)) => report = routed.report,
            }

            poll_fn(|cx| self.inner.poll_ready_to_send(cx)).await;
            let current_route = crate::state::report_route();
            if current_route.active != Some(ConnectionType::Ble)
                || current_route.ble_profile != slot
                || current_route.generation != route_generation
            {
                return;
            }
        }
    }

    pub fn try_send(&self, report: Report) -> Result<(), TrySendError<Report>> {
        let route = crate::state::report_route();
        self.try_send_for_route(report, route.ble_profile, route.generation)
    }

    fn try_send_for_route(&self, report: Report, slot: u8, route_generation: u32) -> Result<(), TrySendError<Report>> {
        let routed = RoutedBleReport {
            slot,
            route_generation,
            force: false,
            report,
        };
        self.inner
            .try_send(routed)
            .map_err(|TrySendError::Full(routed)| TrySendError::Full(routed.report))
    }

    pub fn try_receive(&self) -> Result<Report, TryReceiveError> {
        self.inner.try_receive().map(|routed| routed.report)
    }

    pub async fn receive(&self) -> Report {
        self.inner.receive().await.report
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    pub fn clear(&self) {
        self.inner.clear();
    }
}

/// Signal for LED indicator, used in BLE keyboards only since BLE receiving is not async
#[cfg(feature = "_ble")]
pub(crate) static LED_SIGNAL: Signal<RawMutex, LedIndicator> = Signal::new();

/// Drained by the USB HID writer task. Routed through `send_hid_report`
/// from the keyboard task and ad-hoc producers (e.g. steno chord output).
#[cfg(not(feature = "_no_usb"))]
pub static USB_REPORT_CHANNEL: UsbReportChannel = UsbReportChannel { inner: Channel::new() };

/// Drained by the BLE HID writer task. Routed through `send_hid_report`.
#[cfg(feature = "_ble")]
pub static BLE_REPORT_CHANNEL: BleReportChannel = BleReportChannel { inner: Channel::new() };

/// Reports generated by the input event that wakes a sleeping BLE slot. This
/// channel sits at the routing boundary so the report is captured before the
/// normal active-transport decision would discard it.
#[cfg(feature = "_ble")]
pub(crate) struct WakeBleReport {
    pub(crate) captured_at: Instant,
    pub(crate) report: Report,
}

#[cfg(feature = "_ble")]
pub(crate) static BLE_WAKE_REPORT_CHANNEL: Channel<RawMutex, WakeBleReport, 4> = Channel::new();
#[cfg(feature = "_ble")]
pub(crate) static BLE_WAKE_REPORT_CAPTURE_ARMED: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "_ble")]
fn try_send_ble_wake_report(report: Report) {
    let wake_report = WakeBleReport {
        captured_at: Instant::now(),
        report,
    };
    if let Err(TrySendError::Full(wake_report)) = BLE_WAKE_REPORT_CHANNEL.try_send(wake_report) {
        let _ = BLE_WAKE_REPORT_CHANNEL.try_receive();
        let _ = BLE_WAKE_REPORT_CHANNEL.try_send(wake_report);
    }
}

/// Reports generated while no transport is selected are normally dropped. A
/// sleeping BLE slot may temporarily arm the dedicated wake-report channel.
pub(crate) async fn send_hid_report(report: Report) {
    let route = crate::state::report_route();
    let Some(transport) = route.active else {
        #[cfg(feature = "_ble")]
        if BLE_WAKE_REPORT_CAPTURE_ARMED.load(Ordering::Acquire) {
            try_send_ble_wake_report(report);
        }
        return;
    };

    #[cfg(feature = "_ble")]
    if transport == ConnectionType::Ble {
        BLE_REPORT_CHANNEL
            .send_for_route(report, route.ble_profile, route.generation)
            .await;
        return;
    }

    #[cfg(not(feature = "_no_usb"))]
    if transport == ConnectionType::Usb {
        USB_REPORT_CHANNEL.send_for_generation(report, route.generation).await;
    }
}

/// Drops the report when the active transport's queue is full or no
/// transport is selected. Use for producers where back-pressure would block
/// the matrix scan (e.g. steno chord output).
pub(crate) fn try_send_hid_report(report: Report) {
    let route = crate::state::report_route();
    let Some(transport) = route.active else {
        #[cfg(feature = "_ble")]
        if BLE_WAKE_REPORT_CAPTURE_ARMED.load(Ordering::Acquire) {
            try_send_ble_wake_report(report);
        }
        return;
    };

    #[cfg(feature = "_ble")]
    if transport == ConnectionType::Ble {
        let _ = BLE_REPORT_CHANNEL.try_send_for_route(report, route.ble_profile, route.generation);
        return;
    }

    #[cfg(not(feature = "_no_usb"))]
    if transport == ConnectionType::Usb {
        let _ = USB_REPORT_CHANNEL.try_send_for_generation(report, route.generation);
    }
}

/// Drains queued reports for `transport` and leaves neutral reports for every
/// HID collection. Called on active-transport flips so the previous host
/// releases any pressed state without replaying stale queued reports later.
pub(crate) fn clear_and_release_report_channel(transport: ConnectionType) {
    #[cfg(feature = "_ble")]
    if transport == ConnectionType::Ble {
        BLE_REPORT_CHANNEL.clear();
        let route = crate::state::report_route();
        for report in neutral_reports() {
            let _ = BLE_REPORT_CHANNEL.inner.try_send(RoutedBleReport {
                slot: route.ble_profile,
                route_generation: route.generation,
                force: true,
                report,
            });
        }
        return;
    }

    #[cfg(not(feature = "_no_usb"))]
    if transport == ConnectionType::Usb {
        USB_REPORT_CHANNEL.clear();
        let route = crate::state::report_route();
        for report in neutral_reports() {
            let _ = USB_REPORT_CHANNEL.inner.try_send(RoutedUsbReport {
                route_generation: route.generation,
                force: true,
                report,
            });
        }
    }
}

// Sync messages from server to flash
#[cfg(feature = "storage")]
pub(crate) static FLASH_CHANNEL: Channel<RawMutex, FlashOperationMessage, FLASH_CHANNEL_SIZE> = Channel::new();
#[cfg(feature = "_ble")]
pub(crate) static BLE_PROFILE_CHANNEL: Channel<RawMutex, BleProfileAction, 1> = Channel::new();

/// Vial host requests from any active transport (USB or BLE) to the central `HostService`.
/// Items carry the originating transport tag so replies can be routed back to the right
/// per-transport reply channel.
///
/// Note: `HostService` processes requests strictly serially, so a slow request from one
/// transport (e.g. flash-bound `process_vial`) blocks queries from the other transport
/// queued behind it until it completes.
#[cfg(feature = "host")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum HostRequestOrigin {
    #[cfg(not(feature = "_no_usb"))]
    Usb,
    #[cfg(feature = "_ble")]
    Ble { slot: u8, generation: u32 },
}

#[cfg(all(feature = "host", feature = "_ble"))]
pub(crate) struct BleHostReply {
    pub(crate) slot: u8,
    pub(crate) generation: u32,
    pub(crate) data: [u8; 32],
}

#[cfg(feature = "host")]
pub(crate) static HOST_REQUEST_CHANNEL: Channel<RawMutex, (HostRequestOrigin, [u8; 32]), VIAL_CHANNEL_SIZE> =
    Channel::new();

/// Per-transport reply for USB. Capacity matches the request queue so bursts of
/// host requests can keep their replies queued until the transport drains them.
#[cfg(all(feature = "host", not(feature = "_no_usb")))]
pub(crate) static HOST_USB_REPLY: Channel<RawMutex, [u8; 32], VIAL_CHANNEL_SIZE> = Channel::new();

/// Per-transport reply for BLE. See `HOST_USB_REPLY` for the sizing/draining rationale.
#[cfg(all(feature = "host", feature = "_ble"))]
pub(crate) static HOST_BLE_REPLY: Channel<RawMutex, BleHostReply, VIAL_CHANNEL_SIZE> = Channel::new();

/// Routes a Vial reply back to the channel owned by the originating transport.
/// Drops with a warning when the destination queue already has a pending reply
/// (the `HostService` produced faster than the transport drained it).
#[cfg(feature = "host")]
pub(crate) fn try_send_host_reply(origin: HostRequestOrigin, reply: [u8; 32]) {
    let ok = match origin {
        #[cfg(not(feature = "_no_usb"))]
        HostRequestOrigin::Usb => HOST_USB_REPLY.try_send(reply).is_ok(),
        #[cfg(feature = "_ble")]
        HostRequestOrigin::Ble { slot, generation } => HOST_BLE_REPLY
            .try_send(BleHostReply {
                slot,
                generation,
                data: reply,
            })
            .is_ok(),
        #[allow(unreachable_patterns)]
        _ => false,
    };
    if !ok {
        warn!("Dropping Vial {:?} reply: reply queue full", origin);
    }
}

/// Enqueues a Vial request from a transport into `HOST_REQUEST_CHANNEL`,
/// back-pressuring the transport task when the queue is full.
#[cfg(feature = "host")]
pub(crate) async fn enqueue_host_request(origin: HostRequestOrigin, data: [u8; 32]) {
    HOST_REQUEST_CHANNEL.send((origin, data)).await;
}
