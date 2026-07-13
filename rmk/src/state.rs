use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex;
use rmk_types::ble::BleState;
#[cfg(feature = "_ble")]
use rmk_types::ble::BleStatus;
use rmk_types::connection::{ConnectionStatus, ConnectionType, UsbState};

use crate::RawMutex;
use crate::event::{ConnectionStatusChangeEvent, publish_event};

#[derive(Clone, Copy)]
struct RuntimeConnectionStatus {
    status: ConnectionStatus,
    route_generation: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct ReportRoute {
    pub(crate) active: Option<ConnectionType>,
    pub(crate) ble_profile: u8,
    pub(crate) generation: u32,
}

/// Single source of truth for transport state and routing. The private route
/// generation prevents a report blocked on a full queue from resurfacing after
/// an A -> B -> A transport/profile transition.
static CONNECTION_STATUS: Mutex<RawMutex, Cell<RuntimeConnectionStatus>> =
    Mutex::new(Cell::new(RuntimeConnectionStatus {
        status: ConnectionStatus::new(),
        route_generation: 0,
    }));

pub(crate) fn active_transport() -> Option<ConnectionType> {
    CONNECTION_STATUS.lock(|c| c.get().status.decide_active())
}

pub(crate) fn current_connection_status() -> ConnectionStatus {
    CONNECTION_STATUS.lock(|c| c.get().status)
}

pub(crate) fn current_usb_state() -> UsbState {
    CONNECTION_STATUS.lock(|c| c.get().status.usb)
}

#[cfg(feature = "_ble")]
pub(crate) fn current_ble_status() -> BleStatus {
    CONNECTION_STATUS.lock(|c| c.get().status.ble)
}

pub(crate) fn report_route_generation() -> u32 {
    CONNECTION_STATUS.lock(|c| c.get().route_generation)
}

pub(crate) fn report_route() -> ReportRoute {
    CONNECTION_STATUS.lock(|c| {
        let runtime = c.get();
        ReportRoute {
            active: runtime.status.decide_active(),
            ble_profile: runtime.status.ble.profile,
            generation: runtime.route_generation,
        }
    })
}

/// Read-modify-write the connection status atomically.
pub(crate) fn update_status(f: impl FnOnce(&mut ConnectionStatus)) {
    let Some((prev, new)) = CONNECTION_STATUS.lock(|c| {
        let mut runtime = c.get();
        let prev = runtime.status;
        let mut new = prev;
        f(&mut new);
        if prev == new {
            return None;
        }
        let prev_active = prev.decide_active();
        let new_active = new.decide_active();
        if prev_active != new_active
            || (prev_active == Some(ConnectionType::Ble)
                && new_active == Some(ConnectionType::Ble)
                && prev.ble.profile != new.ble.profile)
        {
            runtime.route_generation = runtime.route_generation.wrapping_add(1);
        }
        runtime.status = new;
        c.set(runtime);
        Some((prev, new))
    }) else {
        return;
    };

    let prev_active = prev.decide_active();
    let new_active = new.decide_active();

    if prev_active != new_active
        && let Some(prev_active) = prev_active
    {
        // Drain after the commit so any producer racing past the mutex reads
        // the new state and routes to the new channel rather than the one
        // about to be cleared.
        crate::channel::clear_and_release_report_channel(prev_active);
    }

    publish_event(ConnectionStatusChangeEvent(new));
}

pub fn set_usb_state(s: UsbState) {
    update_status(|c| c.usb = s);
}

pub(crate) fn set_ble_state(s: BleState) {
    update_status(|c| c.ble.state = s);
}

/// Change the active BLE slot when no per-slot runtime connection table is
/// available. The BLE connection owner uses `update_status` directly so it can
/// set the selected slot's exact readiness atomically.
pub(crate) fn set_ble_profile(profile: u8) {
    update_status(|c| {
        c.ble.profile = profile;
        c.ble.state = BleState::Inactive;
    });
}

/// Persistence is the caller's responsibility — enqueue
/// `FlashOperationMessage::ConnectionType` on `FLASH_CHANNEL`.
pub(crate) fn set_preferred_connection(t: ConnectionType) {
    update_status(|c| c.preferred = t);
}

/// Load the preferred connection type at startup.
///
/// With the `storage` feature, reads the persisted `ConnectionType` from flash;
/// otherwise falls back to a build-time default — `Ble` when USB is disabled, `Usb` otherwise.
#[cfg(feature = "_ble")]
pub(crate) async fn load_preferred_connection() -> ConnectionType {
    #[cfg(feature = "storage")]
    let stored = crate::storage::read_connection_type().await;
    #[cfg(not(feature = "storage"))]
    let stored: Option<ConnectionType> = None;
    match stored {
        Some(c) => c,
        #[cfg(feature = "_no_usb")]
        None => ConnectionType::Ble,
        #[cfg(not(feature = "_no_usb"))]
        None => ConnectionType::Usb,
    }
}

#[cfg(all(feature = "_ble", not(feature = "_no_usb")))]
pub(crate) async fn toggle_preferred() {
    let mut new = ConnectionType::Usb;
    update_status(|c| {
        c.preferred = match c.preferred {
            ConnectionType::Usb => ConnectionType::Ble,
            ConnectionType::Ble => ConnectionType::Usb,
        };
        new = c.preferred;
    });
    info!("Switching preferred transport to: {:?}", new);
    #[cfg(feature = "storage")]
    crate::channel::FLASH_CHANNEL
        .send(crate::storage::FlashOperationMessage::ConnectionType(new))
        .await;
}

#[cfg(feature = "_ble")]
pub(crate) fn current_profile() -> u8 {
    CONNECTION_STATUS.lock(|c| c.get().status.ble.profile)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use embassy_futures::select::{Either, select};
    use embassy_time::{Duration, Timer};

    use super::{
        CONNECTION_STATUS, ConnectionStatus, ConnectionType, RuntimeConnectionStatus, UsbState,
        set_preferred_connection, set_usb_state,
    };
    use crate::event::{ConnectionStatusChangeEvent, EventSubscriber, SubscribableEvent};
    use crate::hid::{KeyboardReport, Report};
    use crate::test_support::test_block_on as block_on;

    fn state_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn reset_state() {
        CONNECTION_STATUS.lock(|c| {
            c.set(RuntimeConnectionStatus {
                status: ConnectionStatus::default(),
                route_generation: 0,
            })
        });
        #[cfg(not(feature = "_no_usb"))]
        crate::channel::USB_REPORT_CHANNEL.clear();
        #[cfg(feature = "_ble")]
        crate::channel::BLE_REPORT_CHANNEL.clear();
    }

    fn pressed_keyboard_report() -> Report {
        Report::KeyboardReport(KeyboardReport {
            modifier: 0x02,
            reserved: 0,
            leds: 0,
            keycodes: [4, 0, 0, 0, 0, 0],
        })
    }

    fn assert_neutral_report(report: Report, index: usize) {
        match (index, report) {
            (0, Report::KeyboardReport(r)) => {
                assert_eq!(r.modifier, 0);
                assert_eq!(r.reserved, 0);
                assert_eq!(r.leds, 0);
                assert_eq!(r.keycodes, [0; 6]);
            }
            (1, Report::MouseReport(r)) => {
                assert_eq!(r.buttons, 0);
                assert_eq!(r.x, 0);
                assert_eq!(r.y, 0);
                assert_eq!(r.wheel, 0);
                assert_eq!(r.pan, 0);
            }
            (2, Report::MediaKeyboardReport(r)) => {
                let usage_id = r.usage_id;
                assert_eq!(usage_id, 0);
            }
            (3, Report::SystemControlReport(r)) => {
                let usage_id = r.usage_id;
                assert_eq!(usage_id, 0);
            }
            _ => panic!("unexpected neutral report order"),
        }
    }

    #[test]
    fn preferred_transport_change_publishes_status_event() {
        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);
        let mut sub = ConnectionStatusChangeEvent::subscriber();

        set_preferred_connection(ConnectionType::Ble);

        let event = block_on(sub.next_event());
        assert_eq!(event.0.preferred, ConnectionType::Ble);
    }

    #[test]
    fn usb_state_change_publishes_status_event() {
        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        let mut sub = ConnectionStatusChangeEvent::subscriber();

        set_usb_state(UsbState::Configured);

        let event = block_on(sub.next_event());
        assert_eq!(event.0.usb, UsbState::Configured);
    }

    #[test]
    fn unchanged_status_does_not_publish_event() {
        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);
        let mut sub = ConnectionStatusChangeEvent::subscriber();

        // Re-setting the same value should not publish.
        set_usb_state(UsbState::Configured);

        match block_on(select(Timer::after(Duration::from_millis(1)), sub.next_event())) {
            Either::First(_) => {}
            Either::Second(event) => panic!("unexpected status change event: {:?}", event),
        }
    }

    #[cfg(not(feature = "_no_usb"))]
    #[test]
    fn flipping_away_from_active_clears_stale_reports_and_queues_all_up() {
        use crate::channel::USB_REPORT_CHANNEL;

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);
        assert_eq!(super::active_transport(), Some(ConnectionType::Usb));

        // Drain anything left over from earlier tests, then queue a sentinel
        // that would otherwise persist across a flip.
        USB_REPORT_CHANNEL.clear();
        USB_REPORT_CHANNEL
            .try_send(pressed_keyboard_report())
            .expect("channel should have capacity for sentinel");
        assert!(USB_REPORT_CHANNEL.try_receive().is_ok());
        USB_REPORT_CHANNEL
            .try_send(pressed_keyboard_report())
            .expect("channel should have capacity for sentinel");

        set_usb_state(UsbState::Disabled);
        assert!(super::active_transport().is_none());
        for index in 0..4 {
            assert_neutral_report(
                USB_REPORT_CHANNEL
                    .try_receive()
                    .expect("USB_REPORT_CHANNEL should contain all neutral reports"),
                index,
            );
        }
        assert!(
            USB_REPORT_CHANNEL.try_receive().is_err(),
            "USB_REPORT_CHANNEL should contain only the neutral reports"
        );
    }

    #[cfg(not(feature = "_no_usb"))]
    #[test]
    fn blocked_send_drops_report_after_transport_change() {
        use embassy_futures::join::join;

        use crate::channel::{USB_REPORT_CHANNEL, send_hid_report};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);

        for _ in 0..crate::REPORT_CHANNEL_SIZE {
            USB_REPORT_CHANNEL
                .try_send(pressed_keyboard_report())
                .expect("channel should have capacity while filling");
        }

        block_on(join(
            send_hid_report(Report::KeyboardReport(KeyboardReport::default())),
            async {
                Timer::after(Duration::from_millis(1)).await;
                set_usb_state(UsbState::Disabled);
            },
        ));

        for index in 0..4 {
            assert_neutral_report(
                USB_REPORT_CHANNEL
                    .try_receive()
                    .expect("USB_REPORT_CHANNEL should contain all neutral reports"),
                index,
            );
        }
        assert!(
            USB_REPORT_CHANNEL.try_receive().is_err(),
            "USB_REPORT_CHANNEL should contain only the neutral reports"
        );
    }

    #[cfg(all(not(feature = "_no_usb"), feature = "_ble"))]
    #[test]
    fn usb_preference_flip_releases_previous_ble_transport() {
        use crate::channel::BLE_REPORT_CHANNEL;
        use crate::state::{BleState, set_ble_state};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_preferred_connection(ConnectionType::Usb);
        set_ble_state(BleState::Connected);
        assert_eq!(super::active_transport(), Some(ConnectionType::Ble));

        BLE_REPORT_CHANNEL
            .try_send(pressed_keyboard_report())
            .expect("BLE report channel should have capacity for sentinel");

        set_usb_state(UsbState::Configured);
        assert_eq!(super::active_transport(), Some(ConnectionType::Usb));
        for index in 0..4 {
            assert_neutral_report(
                BLE_REPORT_CHANNEL
                    .try_receive()
                    .expect("BLE_REPORT_CHANNEL should contain all neutral reports"),
                index,
            );
        }
        assert!(
            BLE_REPORT_CHANNEL.try_receive().is_err(),
            "BLE_REPORT_CHANNEL should contain only the neutral reports"
        );
    }

    #[cfg(feature = "_ble")]
    #[test]
    fn ble_reports_capture_slot_and_route_generation() {
        use crate::channel::BLE_REPORT_CHANNEL;
        use crate::state::{BleState, set_ble_state};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_ble_state(BleState::Connected);
        BLE_REPORT_CHANNEL.clear();

        BLE_REPORT_CHANNEL
            .try_send(pressed_keyboard_report())
            .expect("BLE report channel should have capacity");
        let routed = BLE_REPORT_CHANNEL
            .inner
            .try_receive()
            .expect("BLE report should retain routing metadata");
        assert_eq!(routed.slot, 0);
        assert_eq!(routed.route_generation, super::report_route_generation());
        assert!(!routed.force);

        let previous_generation = super::report_route_generation();
        super::update_status(|status| {
            status.ble.profile = 1;
            status.ble.state = BleState::Connected;
        });
        assert_ne!(super::report_route_generation(), previous_generation);
    }

    #[cfg(all(feature = "_ble", not(feature = "_no_usb")))]
    #[test]
    fn ble_profile_change_does_not_invalidate_active_usb_route() {
        use crate::state::{BleState, set_ble_state};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_preferred_connection(ConnectionType::Usb);
        set_usb_state(UsbState::Configured);
        set_ble_state(BleState::Connected);
        assert_eq!(super::active_transport(), Some(ConnectionType::Usb));
        let generation = super::report_route_generation();

        super::update_status(|status| status.ble.profile = 1);

        assert_eq!(super::active_transport(), Some(ConnectionType::Usb));
        assert_eq!(super::report_route_generation(), generation);
    }

    #[cfg(feature = "_ble")]
    #[test]
    fn wake_reports_are_captured_only_when_armed() {
        use core::sync::atomic::Ordering;

        use crate::channel::{
            BLE_REPORT_CHANNEL, BLE_WAKE_REPORT_CAPTURE_ARMED, BLE_WAKE_REPORT_CHANNEL, send_hid_report,
        };

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        BLE_WAKE_REPORT_CHANNEL.clear();
        BLE_WAKE_REPORT_CAPTURE_ARMED.store(true, Ordering::Release);

        block_on(send_hid_report(pressed_keyboard_report()));

        assert!(BLE_REPORT_CHANNEL.inner.try_receive().is_err());
        assert!(BLE_WAKE_REPORT_CHANNEL.try_receive().is_ok());
        BLE_WAKE_REPORT_CAPTURE_ARMED.store(false, Ordering::Release);
    }

    #[cfg(feature = "_ble")]
    #[test]
    fn blocked_ble_send_does_not_survive_profile_aba() {
        use embassy_futures::join::join;

        use crate::channel::{BLE_REPORT_CHANNEL, send_hid_report};
        use crate::state::{BleState, set_ble_state};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_ble_state(BleState::Connected);
        BLE_REPORT_CHANNEL.clear();
        for _ in 0..crate::REPORT_CHANNEL_SIZE {
            BLE_REPORT_CHANNEL
                .try_send(pressed_keyboard_report())
                .expect("BLE report channel should have capacity while filling");
        }

        block_on(join(send_hid_report(pressed_keyboard_report()), async {
            Timer::after(Duration::from_millis(1)).await;
            super::update_status(|status| {
                status.ble.profile = 1;
                status.ble.state = BleState::Connected;
            });
            super::update_status(|status| {
                status.ble.profile = 0;
                status.ble.state = BleState::Connected;
            });
            let _ = BLE_REPORT_CHANNEL.inner.try_receive();
        }));

        let mut remaining = 0;
        while BLE_REPORT_CHANNEL.inner.try_receive().is_ok() {
            remaining += 1;
        }
        assert_eq!(remaining, crate::REPORT_CHANNEL_SIZE - 1);
    }

    #[cfg(all(feature = "_ble", not(feature = "_no_usb")))]
    #[test]
    fn active_usb_does_not_duplicate_into_ble_wake_cache() {
        use core::sync::atomic::Ordering;

        use crate::channel::{
            BLE_WAKE_REPORT_CAPTURE_ARMED, BLE_WAKE_REPORT_CHANNEL, USB_REPORT_CHANNEL, send_hid_report,
        };

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);
        BLE_WAKE_REPORT_CHANNEL.clear();
        BLE_WAKE_REPORT_CAPTURE_ARMED.store(true, Ordering::Release);

        block_on(send_hid_report(pressed_keyboard_report()));

        assert!(USB_REPORT_CHANNEL.try_receive().is_ok());
        assert!(BLE_WAKE_REPORT_CHANNEL.try_receive().is_err());
        BLE_WAKE_REPORT_CAPTURE_ARMED.store(false, Ordering::Release);
    }

    #[cfg(not(feature = "_no_usb"))]
    #[test]
    fn blocked_send_enqueues_when_transport_stays_active() {
        use embassy_futures::join::join;

        use crate::channel::{USB_REPORT_CHANNEL, send_hid_report};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);

        for _ in 0..crate::REPORT_CHANNEL_SIZE {
            USB_REPORT_CHANNEL
                .try_send(Report::KeyboardReport(KeyboardReport::default()))
                .expect("channel should have capacity while filling");
        }

        block_on(join(
            send_hid_report(Report::KeyboardReport(KeyboardReport::default())),
            async {
                Timer::after(Duration::from_millis(1)).await;
                let _ = USB_REPORT_CHANNEL.try_receive();
            },
        ));

        assert_eq!(USB_REPORT_CHANNEL.len(), crate::REPORT_CHANNEL_SIZE);
    }

    #[cfg(not(feature = "_no_usb"))]
    #[test]
    fn blocked_usb_send_does_not_survive_transport_aba() {
        use embassy_futures::join::join;

        use crate::channel::{USB_REPORT_CHANNEL, send_hid_report};

        let _guard = state_test_lock().lock().unwrap();
        reset_state();
        set_usb_state(UsbState::Configured);
        for _ in 0..crate::REPORT_CHANNEL_SIZE {
            USB_REPORT_CHANNEL
                .try_send(pressed_keyboard_report())
                .expect("USB report channel should have capacity while filling");
        }

        block_on(join(send_hid_report(pressed_keyboard_report()), async {
            Timer::after(Duration::from_millis(1)).await;
            set_usb_state(UsbState::Disabled);
            set_usb_state(UsbState::Configured);
        }));

        for index in 0..4 {
            assert_neutral_report(
                USB_REPORT_CHANNEL
                    .try_receive()
                    .expect("USB_REPORT_CHANNEL should contain all neutral reports"),
                index,
            );
        }
        assert!(USB_REPORT_CHANNEL.try_receive().is_err());
    }

    #[cfg(all(feature = "host", feature = "_ble"))]
    #[test]
    fn ble_host_replies_preserve_origin_slot_and_generation() {
        use crate::channel::{HOST_BLE_REPLY, HostRequestOrigin, try_send_host_reply};

        let _guard = state_test_lock().lock().unwrap();
        HOST_BLE_REPLY.clear();
        try_send_host_reply(
            HostRequestOrigin::Ble {
                slot: 2,
                generation: 17,
            },
            [0x5a; 32],
        );

        let reply = HOST_BLE_REPLY.try_receive().expect("BLE host reply should be queued");
        assert_eq!(reply.slot, 2);
        assert_eq!(reply.generation, 17);
        assert_eq!(reply.data, [0x5a; 32]);
    }
}
