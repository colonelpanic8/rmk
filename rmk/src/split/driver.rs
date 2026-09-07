//! The abstracted driver layer of the split keyboard.
//!
use core::cell::Cell;

#[cfg(feature = "dfu_split")]
use embassy_futures::select::{Either, select};
use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::{Duration, Instant, Timer};
use futures::FutureExt;
use rmk_types::battery::BatteryStatus;
#[cfg(feature = "rynk")]
use rmk_types::protocol::rynk::PeripheralStatus;

use super::{PeripheralMatrixConfig, SplitMessage};
#[cfg(feature = "dfu_split")]
use crate::event::DfuCmdEvent;
#[cfg(feature = "_ble")]
use crate::event::{BatteryStatusEvent, PeripheralBatteryEvent};
use crate::event::{
    KeyboardEvent, KeyboardEventPos, PeripheralConnectedEvent, SubscribableEvent, publish_event, publish_event_async,
};

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum SplitDriverError {
    SerialError,
    EmptyMessage,
    DeserializeError,
    SerializeError,
    BleError(u8),
    Disconnected,
}

/// Split message reader from other split devices
pub(crate) trait SplitReader {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError>;
}

/// Split message writer to other split devices
pub(crate) trait SplitWriter {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError>;

    /// Wait until the last written control message has reached the peer, as
    /// far as the transport can tell. Transports whose `write` already
    /// delivers keep the default.
    async fn flush(&mut self) -> Result<(), SplitDriverError> {
        Ok(())
    }
}

/// How long the central waits for a peripheral to acknowledge a transport
/// force. Long enough for a sleeping BLE link's peripheral latency.
const FORCE_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Live per-peripheral status. Latched here in the transport-agnostic split
/// layer so host services can read a current snapshot at any time, even when
/// no host session was active when the change happened. Wired peripherals
/// never report a battery, so theirs stays `Unavailable`.
#[derive(Copy, Clone, PartialEq, Eq)]
struct PeripheralSlot {
    connected: bool,
    battery: BatteryStatus,
}

static PERIPHERAL_SLOTS: BlockingMutex<crate::RawMutex, Cell<[PeripheralSlot; crate::SPLIT_PERIPHERALS_NUM]>> =
    BlockingMutex::new(Cell::new(
        [PeripheralSlot {
            connected: false,
            battery: BatteryStatus::Unavailable,
        }; crate::SPLIT_PERIPHERALS_NUM],
    ));

/// Read-modify-write peripheral `id`'s slot. Returns `false` when `id` is out
/// of range or the slot didn't change, so callers skip publishing.
fn update_slot(id: usize, f: impl FnOnce(&mut PeripheralSlot)) -> bool {
    PERIPHERAL_SLOTS.lock(|slots| {
        let mut all = slots.get();
        let Some(slot) = all.get_mut(id) else {
            return false;
        };
        let prev = *slot;
        f(slot);
        if *slot == prev {
            return false;
        }
        slots.set(all);
        true
    })
}

/// Latch peripheral `id`'s connected state and broadcast the change.
pub(crate) fn set_peripheral_connected(id: usize, connected: bool) {
    if update_slot(id, |s| s.connected = connected) {
        publish_event(PeripheralConnectedEvent { id, connected });
    }
}

/// Latch peripheral `id`'s battery status and broadcast the change.
#[cfg(feature = "_ble")]
pub(crate) fn set_peripheral_battery(id: usize, battery: BatteryStatus) {
    if update_slot(id, |s| s.battery = battery) {
        publish_event(PeripheralBatteryEvent {
            id,
            state: BatteryStatusEvent(battery),
        });
    }
}

/// Latest battery status reported by peripheral `id`.
#[cfg(feature = "_ble")]
pub(crate) fn current_peripheral_battery_status(id: usize) -> Option<BatteryStatus> {
    PERIPHERAL_SLOTS.lock(|slots| slots.get().get(id).map(|slot| slot.battery))
}

/// Any peripheral session currently up.
pub(crate) fn any_peripheral_connected() -> bool {
    PERIPHERAL_SLOTS.lock(|slots| slots.get().iter().any(|s| s.connected))
}

/// Latest snapshot for peripheral `id`, or `None` when `id` is out of range.
#[cfg(feature = "rynk")]
pub(crate) fn current_peripheral_status(id: usize) -> Option<PeripheralStatus> {
    PERIPHERAL_SLOTS.lock(|slots| {
        slots.get().get(id).map(|s| PeripheralStatus {
            connected: s.connected,
            battery: s.battery,
        })
    })
}

#[cfg(test)]
mod force_tests {
    use std::collections::VecDeque;

    use embassy_futures::select::select;
    use embassy_time::Timer;

    use super::*;
    use crate::split::selector::{self, FORCE_AUTO, FORCE_BLE, FORCE_WIRED};
    use crate::test_support::test_block_on;

    /// A transport whose scripted replies arrive only once the manager has
    /// written a given number of messages, so an acknowledgement can be made
    /// to follow the request it answers.
    struct FakeLink {
        reads: VecDeque<(usize, SplitMessage)>,
        writes: Vec<SplitMessage>,
    }

    impl FakeLink {
        fn new<I: IntoIterator<Item = (usize, SplitMessage)>>(reads: I) -> Self {
            Self {
                reads: reads.into_iter().collect(),
                writes: Vec::new(),
            }
        }
    }

    impl SplitReader for FakeLink {
        async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
            core::future::poll_fn(|_| match self.reads.front() {
                Some((after_writes, _)) if self.writes.len() >= *after_writes => {
                    core::task::Poll::Ready(Ok(self.reads.pop_front().unwrap().1))
                }
                _ => core::task::Poll::Pending,
            })
            .await
        }
    }

    impl SplitWriter for FakeLink {
        async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
            self.writes.push(*message);
            Ok(0)
        }
    }

    fn new_manager(link: FakeLink) -> PeripheralManager<FakeLink> {
        PeripheralManager::new(
            link,
            0,
            PeripheralMatrixConfig {
                rows: 1,
                cols: 1,
                row_offset: 0,
                col_offset: 0,
            },
        )
    }

    fn run_for(manager: &mut PeripheralManager<FakeLink>, secs: u64) {
        test_block_on(async {
            let _ = select(manager.run(), Timer::after_secs(secs)).await;
        });
    }

    fn overrides(link: &FakeLink) -> Vec<(u8, u8)> {
        link.writes
            .iter()
            .filter_map(|m| match m {
                SplitMessage::TransportOverride { generation, mode } => Some((*generation, *mode)),
                _ => None,
            })
            .collect()
    }

    // Each test owns the process (nextest), so the selector statics start fresh.

    #[test]
    fn force_is_applied_only_after_the_peripheral_acknowledges_it() {
        selector::initialize(true);
        set_peripheral_connected(0, true);

        crate::split::request_transport_force(FORCE_BLE);
        let generation = selector::desired_force().unwrap().generation;
        assert_eq!(
            selector::forced_mode(),
            FORCE_AUTO,
            "a connected peripheral must ack first"
        );

        // The ack follows the ConnectionStatus sync and the override.
        let mut manager = new_manager(FakeLink::new([(2, SplitMessage::TransportOverrideAck(generation))]));
        run_for(&mut manager, 1);

        assert_eq!(overrides(&manager.transceiver), vec![(generation, FORCE_BLE)]);
        assert_eq!(selector::forced_mode(), FORCE_BLE);
    }

    #[test]
    fn force_is_not_applied_when_the_acknowledgement_never_comes() {
        selector::initialize(true);
        set_peripheral_connected(0, true);

        crate::split::request_transport_force(FORCE_BLE);
        let generation = selector::desired_force().unwrap().generation;

        let mut manager = new_manager(FakeLink::new([]));
        run_for(&mut manager, FORCE_ACK_TIMEOUT.as_secs() + 2);

        assert_eq!(overrides(&manager.transceiver), vec![(generation, FORCE_BLE)]);
        assert_eq!(
            selector::forced_mode(),
            FORCE_AUTO,
            "timeout leaves the transport alone"
        );
    }

    #[test]
    fn acknowledgement_of_a_superseded_generation_is_ignored() {
        selector::initialize(true);
        set_peripheral_connected(0, true);

        crate::split::request_transport_force(FORCE_BLE);
        let stale = selector::desired_force().unwrap().generation;
        crate::split::request_transport_force(FORCE_WIRED);
        let current = selector::desired_force().unwrap().generation;
        assert_ne!(stale, current);

        let mut manager = new_manager(FakeLink::new([(2, SplitMessage::TransportOverrideAck(stale))]));
        run_for(&mut manager, 1);
        assert_eq!(overrides(&manager.transceiver), vec![(current, FORCE_WIRED)]);
        assert_eq!(selector::forced_mode(), FORCE_AUTO, "a stale ack proves nothing");

        // A reconnecting manager re-sends the current generation.
        let mut manager = new_manager(FakeLink::new([(2, SplitMessage::TransportOverrideAck(current))]));
        run_for(&mut manager, 1);
        assert_eq!(overrides(&manager.transceiver), vec![(current, FORCE_WIRED)]);
        assert_eq!(selector::forced_mode(), FORCE_WIRED);
    }

    #[test]
    fn force_applies_locally_at_once_without_a_peripheral() {
        selector::initialize(true);

        crate::split::request_transport_force(FORCE_BLE);

        assert_eq!(selector::forced_mode(), FORCE_BLE);
    }
}

#[cfg(all(test, feature = "_ble"))]
mod tests {
    use rmk_types::battery::ChargeState;

    use super::{current_peripheral_battery_status, set_peripheral_battery};

    #[test]
    fn caches_latest_peripheral_battery_status() {
        let status = rmk_types::battery::BatteryStatus::Available {
            charge_state: ChargeState::Discharging,
            level: Some(73),
        };

        set_peripheral_battery(0, status);

        assert_eq!(current_peripheral_battery_status(0), Some(status));
        assert_eq!(current_peripheral_battery_status(crate::SPLIT_PERIPHERALS_NUM), None);
    }
}

/// PeripheralManager runs in central.
/// It reads split message from peripheral and updates key matrix cache of the peripheral.
///
/// When the central scans the matrix, the scanning thread sends sync signal and gets key state cache back.
///
pub(crate) struct PeripheralManager<T: SplitReader + SplitWriter> {
    /// Receiver
    pub(crate) transceiver: T,
    /// Peripheral id
    pub(crate) id: usize,
    /// This peripheral's matrix size and placement in the central's keymap
    matrix_config: PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")]
    pub(crate) passthrough_crc: crate::crc32::Crc32,
    /// Whether to skip hash comparison and always flash firmware.
    #[cfg(feature = "dfu_split")]
    pub(crate) policy: crate::split::dfu::UpdatePolicy,
    /// Set after a chunk fails all retries — aborts subsequent writes.
    #[cfg(feature = "dfu_split")]
    pub(crate) dfu_aborted: bool,
}

impl<T: SplitReader + SplitWriter> PeripheralManager<T> {
    pub(crate) fn new(
        transceiver: T,
        id: usize,
        matrix_config: PeripheralMatrixConfig,
        #[cfg(feature = "dfu_split")] policy: crate::split::dfu::UpdatePolicy,
    ) -> Self {
        Self {
            transceiver,
            matrix_config,
            id,
            #[cfg(feature = "dfu_split")]
            passthrough_crc: crate::crc32::Crc32::new(),
            #[cfg(feature = "dfu_split")]
            policy,
            #[cfg(feature = "dfu_split")]
            dfu_aborted: false,
        }
    }

    /// The transport, for transport-specific upkeep while the manager is
    /// parked (e.g. draining a deselected serial port).
    pub(crate) fn transceiver_mut(&mut self) -> &mut T {
        &mut self.transceiver
    }

    /// Send a message to the peripheral, returning Err on disconnect.
    pub(crate) async fn send(&mut self, msg: &SplitMessage) -> Result<(), ()> {
        debug!("Sending message to peripheral {}: {:?}", self.id, msg);
        match self.transceiver.write(msg).await {
            Ok(_) => Ok(()),
            Err(SplitDriverError::Disconnected) => Err(()),
            Err(e) => {
                error!("SplitDriver write error: {:?}", e);
                Err(())
            }
        }
    }

    /// Replicate a force request. Its write failing is not smoothed over
    /// like [`Self::send`]: the force stays pending on the peripheral's
    /// acknowledgement either way, and the log says why it may never come.
    /// Returns the acknowledgement deadline, or `Err` on disconnect.
    async fn send_force(&mut self, request: crate::split::selector::ForceRequest) -> Result<Instant, ()> {
        let message = SplitMessage::TransportOverride {
            generation: request.generation,
            mode: request.mode,
        };
        match self.transceiver.write(&message).await {
            Ok(_) => {}
            Err(SplitDriverError::Disconnected) => return Err(()),
            Err(e) => error!(
                "Transport force {} not delivered to peripheral {}: {:?}",
                request.generation, self.id, e
            ),
        }
        Ok(Instant::now() + FORCE_ACK_TIMEOUT)
    }

    /// The peripheral applied force `generation`; apply it here too if it
    /// is still the desired one. Returns whether it was.
    fn on_force_acknowledged(&self, generation: u8) -> bool {
        match crate::split::selector::desired_force() {
            Some(request) if request.generation == generation => {
                info!("Peripheral {} acknowledged transport force {}", self.id, generation);
                crate::split::selector::set_forced(request.mode);
                true
            }
            _ => {
                warn!(
                    "Peripheral {} acknowledged stale transport force {}",
                    self.id, generation
                );
                false
            }
        }
    }

    /// Run the manager.
    ///
    /// The manager receives from the peripheral and publishes input events.
    /// It also syncs the central's `ConnectionStatus` to the peripheral on every
    /// change as an informational signal
    pub(crate) async fn run(&mut self) {
        use crate::event::EventSubscriber;

        let mut indicator_sub = crate::event::LedIndicatorEvent::subscriber();
        let mut layer_sub = crate::event::LayerChangeEvent::subscriber();
        // Subscribe before the initial send so any change racing past the
        // snapshot is still delivered to us.
        let mut connection_sub = crate::event::ConnectionStatusChangeEvent::subscriber();
        #[cfg(feature = "_ble")]
        let mut clear_peer_sub = crate::event::ClearPeerEvent::subscriber();
        #[cfg(feature = "display")]
        let mut wpm_sub = crate::event::WpmUpdateEvent::subscriber();
        #[cfg(feature = "display")]
        let mut modifier_sub = crate::event::ModifierEvent::subscriber();
        let mut sleep_sub = crate::event::SleepStateEvent::subscriber();
        #[cfg(feature = "dfu_split")]
        let mut dfu_sub = DfuCmdEvent::subscriber();

        // Send the current state once on startup so the peripheral matches us
        // even when no transition has happened since the central booted.
        if self
            .send(&SplitMessage::ConnectionStatus(
                crate::state::current_connection_status(),
            ))
            .await
            .is_err()
        {
            return;
        }

        // The desired force is part of the initial sync: a reconnecting
        // peripheral may have rebooted since it was applied.
        let mut force_requests = crate::split::selector::force_requests();
        let mut force_ack_deadline = None;
        if let Some(request) = force_requests.try_get() {
            match self.send_force(request).await {
                Ok(deadline) => force_ack_deadline = Some(deadline),
                Err(()) => return,
            }
        }

        #[cfg(feature = "dfu_split")]
        self.check_firmware_update().await;

        loop {
            // Use select_biased_with_feature to handle feature-gated subscriber arms
            let next_event_to_peri = async {
                crate::select_biased_with_feature! {
                    e = indicator_sub.next_event().fuse() => SplitMessage::KeyboardIndicator(e.0.into_bits()),
                    e = layer_sub.next_event().fuse() => SplitMessage::Layer(e.0),
                    e = connection_sub.next_event().fuse() => SplitMessage::ConnectionStatus(e.0),
                    with_feature("_ble"): _ = clear_peer_sub.next_event().fuse() => {
                        #[cfg(feature = "storage")]
                        {
                            use {crate::split::ble::PeerAddress, crate::storage::{StorageItem, store_unchecked}};
                            store_unchecked(StorageItem::PeerAddress(PeerAddress::new(self.id as u8, false, [0; 6]))).await;
                        }
                        SplitMessage::ClearPeer
                    },
                    e = sleep_sub.next_event().fuse() => SplitMessage::SleepState(e.0),
                    with_feature("display"): e = wpm_sub.next_event().fuse() => SplitMessage::Wpm(e.0),
                    with_feature("display"): e = modifier_sub.next_event().fuse() => SplitMessage::Modifier(e.modifier.into_bits()),
                    r = force_requests.changed().fuse() => SplitMessage::TransportOverride { generation: r.generation, mode: r.mode },
                }
            };

            #[cfg(feature = "dfu_split")]
            let event_or_signal = select(next_event_to_peri, dfu_sub.next_message_pure()).fuse();
            #[cfg(not(feature = "dfu_split"))]
            let event_or_signal = next_event_to_peri;

            let force_ack_timeout = async {
                match force_ack_deadline {
                    Some(deadline) => Timer::at(deadline).await,
                    None => core::future::pending().await,
                }
            };

            match select3(self.transceiver.read(), event_or_signal, force_ack_timeout).await {
                Either3::First(read_result) => match read_result {
                    #[cfg(feature = "dfu_split")]
                    Ok(SplitMessage::FirmwareHashResponse(hash)) => {
                        self.handle_proactive_hash(hash).await;
                    }
                    Ok(SplitMessage::TransportOverrideAck(generation)) => {
                        if self.on_force_acknowledged(generation) {
                            force_ack_deadline = None;
                        }
                    }
                    Ok(split_message) => self.process_peripheral_message(split_message).await,
                    Err(e) => error!("Peripheral message read error: {:?}", e),
                },
                #[cfg(feature = "dfu_split")]
                Either3::Second(Either::First(msg)) => {
                    if self.send_event(msg, &mut force_ack_deadline).await.is_err() {
                        return;
                    }
                }
                #[cfg(feature = "dfu_split")]
                Either3::Second(Either::Second(cmd_event)) => {
                    self.handle_dfu_event(cmd_event).await;
                }
                #[cfg(not(feature = "dfu_split"))]
                Either3::Second(msg) => {
                    if self.send_event(msg, &mut force_ack_deadline).await.is_err() {
                        return;
                    }
                }
                Either3::Third(()) => {
                    force_ack_deadline = None;
                    warn!(
                        "Peripheral {} did not acknowledge the transport force; keeping the current transport",
                        self.id
                    );
                }
            }
        }
    }

    /// Forward one central-side event; a force request also arms the wait
    /// for its acknowledgement. `Err` on disconnect.
    async fn send_event(&mut self, msg: SplitMessage, force_ack_deadline: &mut Option<Instant>) -> Result<(), ()> {
        if let SplitMessage::TransportOverride { generation, mode } = msg {
            let request = crate::split::selector::ForceRequest { generation, mode };
            *force_ack_deadline = Some(self.send_force(request).await?);
            return Ok(());
        }
        self.send(&msg).await
    }

    /// Process a single message from the peripheral.
    async fn process_peripheral_message(&self, split_message: SplitMessage) {
        trace!("Got message from peripheral: {:?}", split_message);
        match split_message {
            SplitMessage::Key(e) => match e.pos {
                KeyboardEventPos::Key(key_pos) => {
                    // Verify the row/col
                    if key_pos.row >= self.matrix_config.rows || key_pos.col >= self.matrix_config.cols {
                        error!("Invalid peripheral row/col: {} {}", key_pos.row, key_pos.col);
                        return;
                    }
                    publish_event_async(KeyboardEvent::key(
                        key_pos.row + self.matrix_config.row_offset,
                        key_pos.col + self.matrix_config.col_offset,
                        e.pressed,
                    ))
                    .await;
                }
                _ => publish_event_async(e).await,
            },
            // Non-key events are drop-on-full to keep the split read loop responsive.
            SplitMessage::Pointing(e) => publish_event(e),
            #[cfg(feature = "_ble")]
            SplitMessage::BatteryStatus(state) => set_peripheral_battery(self.id, state.0),
            #[cfg(feature = "dfu_split")]
            SplitMessage::FirmwareHashResponse(hash) => {
                info!("dfu_split: stale hash response ({:#x}) in event loop", hash);
            }
            #[cfg(feature = "dfu_split")]
            SplitMessage::FirmwareChunkAck { offset, crc: _ } => {
                info!("dfu_split: stale chunk ack (offset {}) in event loop, ignoring", offset);
            }
            #[cfg(feature = "dfu_split")]
            SplitMessage::FirmwareUpdateConfirm => {
                info!("dfu_split: stale update confirm in event loop, ignoring");
            }
            _ => warn!("{:?} should not come from peripheral", split_message),
        }
    }
}
