use core::cell::{Cell, RefCell};

use bt_hci::cmd::le::{LeReadLocalSupportedFeatures, LeSetPhy, LeSetScanParams};
use bt_hci::controller::{ControllerCmdAsync, ControllerCmdSync};
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};
use heapless::VecView;
use trouble_host::prelude::*;

use crate::ble::{update_ble_phy, update_conn_params};
use crate::channel::FLASH_CHANNEL;
#[cfg(feature = "storage")]
use crate::split::ble::PeerAddress;
use crate::split::driver::{PeripheralManager, SplitDriverError, SplitReader, SplitWriter, set_peripheral_connected};
use crate::split::{SPLIT_MESSAGE_MAX_SIZE, SplitMessage};
use crate::storage::FlashOperationMessage;
use crate::{
    SPLIT_CENTRAL_MAX_LATENCY_BATTERY, SPLIT_CENTRAL_MAX_LATENCY_POWERED, SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS,
};

pub(crate) static STACK_STARTED: Signal<crate::RawMutex, bool> = Signal::new();
pub(crate) static PERIPHERAL_FOUND: Signal<crate::RawMutex, (u8, BdAddr)> = Signal::new();

// Signals and mutex for syncing scanning state between scanning task and peripheral manager
static START_SCANNING: Signal<crate::RawMutex, ()> = Signal::new();
static STOP_SCANNING: Signal<crate::RawMutex, ()> = Signal::new();
static SCANNING_MUTEX: Mutex<crate::RawMutex, ()> = Mutex::new(());

/// Sleep management signal for BLE Split Central
///
/// This signal serves dual purposes for sleep management:
/// - `signal(true)`: Indicates central has entered sleep mode
/// - `signal(false)`: Indicates activity detected, wake up or reset sleep timer
pub(crate) static CENTRAL_SLEEP: Signal<crate::RawMutex, bool> = Signal::new();

/// Runtime active-mode split BLE latency policy.
///
/// Changes are volatile and take effect on connected peripherals immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatencyPolicy {
    pub powered: u16,
    pub battery: u16,
    pub override_latency: Option<u16>,
}

/// Current policy inputs and selected active-mode latency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatencyState {
    pub policy: LatencyPolicy,
    pub powered: bool,
    pub effective: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidLatency;

impl LatencyPolicy {
    fn effective(self, powered: bool) -> u16 {
        self.override_latency
            .unwrap_or(if powered { self.powered } else { self.battery })
    }

    fn is_valid(self) -> bool {
        self.powered < 500 && self.battery < 500 && self.override_latency.is_none_or(|value| value < 500)
    }
}

static LATENCY_POLICY: BlockingMutex<crate::RawMutex, Cell<LatencyPolicy>> =
    BlockingMutex::new(Cell::new(LatencyPolicy {
        powered: SPLIT_CENTRAL_MAX_LATENCY_POWERED,
        battery: SPLIT_CENTRAL_MAX_LATENCY_BATTERY,
        override_latency: None,
    }));
static LATENCY_CHANGED: PubSubChannel<crate::RawMutex, (), 1, 8, 1> = PubSubChannel::new();

fn externally_powered() -> bool {
    crate::state::current_usb_state() != rmk_types::connection::UsbState::Disabled
}

pub fn latency_state() -> LatencyState {
    let policy = LATENCY_POLICY.lock(Cell::get);
    let powered = externally_powered();
    let effective = policy.effective(powered);
    LatencyState {
        policy,
        powered,
        effective,
    }
}

/// Replace the volatile latency policy and update live split connections.
pub fn set_latency_policy(policy: LatencyPolicy) -> Result<(), InvalidLatency> {
    if !policy.is_valid() {
        return Err(InvalidLatency);
    }
    LATENCY_POLICY.lock(|current| current.set(policy));
    LATENCY_CHANGED.immediate_publisher().publish_immediate(());
    Ok(())
}

#[cfg(test)]
mod latency_tests {
    use super::*;

    #[test]
    fn policy_selects_power_source_unless_overridden() {
        let policy = LatencyPolicy {
            powered: 0,
            battery: 4,
            override_latency: None,
        };
        assert_eq!(policy.effective(true), 0);
        assert_eq!(policy.effective(false), 4);
        assert_eq!(
            LatencyPolicy {
                override_latency: Some(2),
                ..policy
            }
            .effective(true),
            2
        );
        assert_eq!(
            LatencyPolicy {
                override_latency: Some(2),
                ..policy
            }
            .effective(false),
            2
        );
    }

    #[test]
    fn policy_rejects_values_outside_the_ble_limit() {
        let valid = LatencyPolicy {
            powered: 499,
            battery: 499,
            override_latency: Some(499),
        };
        assert!(valid.is_valid());
        assert!(!LatencyPolicy { powered: 500, ..valid }.is_valid());
        assert!(!LatencyPolicy { battery: 500, ..valid }.is_valid());
        assert!(
            !LatencyPolicy {
                override_latency: Some(500),
                ..valid
            }
            .is_valid()
        );
    }
}

pub(crate) fn power_source_changed() {
    LATENCY_CHANGED.immediate_publisher().publish_immediate(());
}

/// Gatt service used in split central to send split message to peripheral
#[gatt_service(uuid = "4dd5fbaa-18e5-4b07-bf0a-353698659946")]
struct SplitBleCentralService {
    #[characteristic(uuid = "0e6313e3-bd0b-45c2-8d2e-37a2e8128bc3", read, notify)]
    message_to_central: [u8; SPLIT_MESSAGE_MAX_SIZE],

    #[characteristic(uuid = "4b3514fb-cae4-4d38-a097-3a2a3d1c3b9c", write_without_response, read, notify)]
    message_to_peripheral: [u8; SPLIT_MESSAGE_MAX_SIZE],
}

/// Gatt server in split peripheral
#[gatt_server]
struct BleSplitCentralServer {
    service: SplitBleCentralService,
}

pub async fn scan_peripherals<
    'b,
    's: 'b,
    C: Controller
        + ControllerCmdSync<LeSetScanParams>
        + ControllerCmdAsync<LeSetPhy>
        + ControllerCmdSync<LeReadLocalSupportedFeatures>,
>(
    stack: &'b Stack<'s, C, DefaultPacketPool>,
    addrs: &RefCell<VecView<Option<[u8; 6]>>>,
) {
    loop {
        // Wait unitil `START_SCANNING` is signaled
        START_SCANNING.wait().await;
        // Check whether the scanning is needed, aka there's empty slot in the addr list.
        let need_scan = !addrs.borrow().iter().all(|a| a.is_some());
        if need_scan {
            let scanning_fut = async {
                loop {
                    let mut central = stack.central();
                    wait_for_stack_started().await;
                    let mut scanner = Scanner::new(&mut central);
                    let scan_config = ScanConfig {
                        active: false,
                        interval: Duration::from_millis(100),
                        window: Duration::from_millis(30),
                        ..Default::default()
                    };
                    let _guard = SCANNING_MUTEX.lock().await;
                    match scanner.scan(&scan_config).await {
                        Ok(_session) => {
                            info!("Start scanning peripherals");
                            STOP_SCANNING.wait().await;
                            info!("Stop scanning");
                        }
                        // Throttle retries while the controller refuses to scan
                        Err(_) => embassy_time::Timer::after_millis(500).await,
                    }
                }
            };
            let update_addrs_fut = async {
                loop {
                    let (found_peripheral_id, addr) = PERIPHERAL_FOUND.wait().await;
                    let scanned_addr = addr.into_inner();
                    if let Some(Some(stored_addr)) = addrs.borrow_mut().get_mut(found_peripheral_id as usize)
                        && *stored_addr == scanned_addr
                    {
                        continue;
                    }

                    info!("Scanned new peripheral {:?}", scanned_addr);
                    let mut slot_updated = false;
                    if let Some(slot) = addrs.borrow_mut().get_mut(found_peripheral_id as usize)
                        && slot.is_none()
                    {
                        // Update only when the slot is empty
                        *slot = Some(scanned_addr);
                        slot_updated = true;
                    }

                    // Update stored addr.
                    // This cannot be put inside the `addrs.borrow_mut()` block because the sending is async
                    if slot_updated {
                        FLASH_CHANNEL
                            .send(FlashOperationMessage::PeerAddress(PeerAddress::new(
                                found_peripheral_id,
                                true,
                                scanned_addr,
                            )))
                            .await;
                    }

                    if addrs.borrow().iter().all(|a| a.is_some()) {
                        break;
                    }
                }
            };

            // Scan until all peripherals are scanned
            // TODO: Timeout?
            select(scanning_fut, update_addrs_fut).await;
        }
    }
}

// When no peripheral address is saved, the central should first scan for peripheral.
// This handler is used to handle the scan result.
pub(crate) struct ScanHandler {}

impl EventHandler for ScanHandler {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        while let Some(Ok(report)) = it.next() {
            // Check advertisement data
            if report.data.len() < 25 {
                continue;
            }
            if report.data[4] == 0x07
                && report.data[5..].starts_with(&[
                    // uuid: 4dd5fbaa-18e5-4b07-bf0a-353698659946
                    70u8, 153u8, 101u8, 152u8, 54u8, 53u8, 10u8, 191u8, 7u8, 75u8, 229u8, 24u8, 170u8, 251u8, 213u8,
                    77u8,
                ])
                && report.data[21..25] == [0x04, 0xff, 0x18, 0xe1]
            {
                // Uuid and manufacturer specific data check passed
                let peripheral_id = report.data[25];
                info!("Found split peripheral: id={:?}, addr={:?}", peripheral_id, report.addr);
                PERIPHERAL_FOUND.signal((peripheral_id, report.addr));
                break;
            }
        }
    }
}

pub(crate) async fn run_ble_peripheral_manager<
    'b,
    's: 'b,
    C: Controller
        + ControllerCmdSync<LeSetScanParams>
        + ControllerCmdAsync<LeSetPhy>
        + ControllerCmdSync<LeReadLocalSupportedFeatures>,
    const ROW: usize,
    const COL: usize,
    const ROW_OFFSET: usize,
    const COL_OFFSET: usize,
>(
    peri_id: usize,
    addrs: &RefCell<VecView<Option<[u8; 6]>>>,
    stack: &'b Stack<'s, C, DefaultPacketPool>,
) {
    trace!("SPLIT_MESSAGE_MAX_SIZE: {}", SPLIT_MESSAGE_MAX_SIZE);

    loop {
        // Check until the address is available
        let address = loop {
            if let Some(Some(addr)) = addrs.borrow().get(peri_id) {
                break Address::random(*addr);
            }
            if !START_SCANNING.signaled() {
                START_SCANNING.signal(());
            }
            // Check again after 500ms
            embassy_time::Timer::after_millis(500).await;
        };
        info!("Peripheral peer address: {:?}", address);

        let mut central = stack.central();
        let config = ConnectConfig {
            connect_params: defaul_central_conn_param(),
            scan_config: ScanConfig {
                filter_accept_list: &[address],
                active: false,
                interval: Duration::from_millis(100),
                window: Duration::from_millis(30),
                ..Default::default()
            },
        };
        wait_for_stack_started().await;

        set_peripheral_connected(peri_id, false);

        // Connect to peripheral
        match with_timeout(Duration::from_millis(super::KNOWN_PEER_CONNECT_REARM_MS), async {
            if let Ok(_guard) = SCANNING_MUTEX.try_lock() {
                info!("Start connecting to peripheral {}", peri_id);
                central.connect(&config).await
            } else {
                STOP_SCANNING.signal(());
                let _guard = SCANNING_MUTEX.lock().await;
                // Wait a little bit to ensure that the scanning has been fully stopped
                embassy_time::Timer::after_millis(100).await;
                info!("Start connecting to peripheral {}", peri_id);
                central.connect(&config).await
            }
        })
        .await
        {
            Ok(Ok(conn)) => {
                info!("Connected to peripheral {}", peri_id);

                set_peripheral_connected(peri_id, true);

                if let Err(e) =
                    run_central_manager_task::<_, _, ROW, COL, ROW_OFFSET, COL_OFFSET>(peri_id, stack, &conn).await
                {
                    #[cfg(feature = "defmt")]
                    let e = defmt::Debug2Format(&e);
                    error!("BLE central error: {:?}", e);
                }
            }
            Ok(Err(e)) => {
                #[cfg(feature = "defmt")]
                let e = defmt::Debug2Format(&e);
                error!("Connect to peripheral {} error: {:?}", peri_id, e);
            }
            Err(_) => {
                // The peripheral is off or out of range; its address is still
                // valid. Re-arm the connection request without a pause so the
                // peripheral's first advertisement after power-on is caught.
                debug!("Connect to peripheral {} timed out, re-arming", peri_id);
                continue;
            }
        }
        // Reconnect after 500ms
        embassy_time::Timer::after_millis(500).await;
    }
}

fn defaul_central_conn_param() -> RequestedConnParams {
    let max_latency = latency_state().effective;
    // Supervision must exceed the longest legal radio silence,
    // interval * (1 + latency). Keep three such periods of margin, with a 2 s
    // floor: a powered-off peripheral is only rediscovered after the dead
    // connection times out, so this bounds reconnect latency for fast
    // off/on cycles.
    let latency_period_us = 7_500 * (1 + max_latency as u64);
    RequestedConnParams {
        min_connection_interval: Duration::from_micros(7500),
        max_connection_interval: Duration::from_micros(7500),
        max_latency,
        supervision_timeout: Duration::from_micros((3 * latency_period_us).max(2_000_000)),
        ..Default::default()
    }
}

async fn run_central_manager_task<
    'b,
    's: 'b,
    C: Controller + ControllerCmdAsync<LeSetPhy> + ControllerCmdSync<LeReadLocalSupportedFeatures>,
    P: PacketPool,
    const ROW: usize,
    const COL: usize,
    const ROW_OFFSET: usize,
    const COL_OFFSET: usize,
>(
    id: usize,
    stack: &'b Stack<'s, C, P>,
    conn: &Connection<'b, P>,
) -> Result<(), BleHostError<C::Error>> {
    let client = GattClient::<C, P, 10>::new(stack, conn).await?;

    // Split link uses 2M PHY always.
    update_ble_phy(stack, conn, PhyKind::Le2M).await;

    info!("Updating connection parameters for peripheral");
    update_conn_params(stack, conn, &defaul_central_conn_param()).await;

    match select3(
        ble_central_task(&client, conn),
        run_peripheral_manager::<_, _, ROW, COL, ROW_OFFSET, COL_OFFSET>(id, &client),
        sleep_manager_task(stack, conn),
    )
    .await
    {
        Either3::First(e) => e,
        Either3::Second(e) => e,
        Either3::Third(e) => e,
    }
}

async fn ble_central_task<'a, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool>(
    client: &GattClient<'a, C, P, 10>,
    conn: &Connection<'a, P>,
) -> Result<(), BleHostError<C::Error>> {
    // Simply monitor connection status. Poll quickly: this bounds how long a
    // dead link lingers before reconnection starts.
    let conn_check = async {
        while conn.is_connected() {
            Timer::after_millis(500).await;
        }
    };

    match select(client.task(), conn_check).await {
        Either::First(e) => e,
        Either::Second(_) => {
            info!("Connection lost");
            Ok(())
        }
    }
}

async fn run_peripheral_manager<
    'a,
    C: Controller + ControllerCmdAsync<LeSetPhy>,
    P: PacketPool,
    const ROW: usize,
    const COL: usize,
    const ROW_OFFSET: usize,
    const COL_OFFSET: usize,
>(
    id: usize,
    client: &GattClient<'a, C, P, 10>,
) -> Result<(), BleHostError<C::Error>> {
    let services = client
        .services_by_uuid(&Uuid::new_long([
            70u8, 153u8, 101u8, 152u8, 54u8, 53u8, 10u8, 191u8, 7u8, 75u8, 229u8, 24u8, 170u8, 251u8, 213u8, 77u8,
        ]))
        .await?;
    info!("Services found");
    if let Some(service) = services.first() {
        let message_to_central = client
            .characteristic_by_uuid::<[u8; SPLIT_MESSAGE_MAX_SIZE]>(
                service,
                // uuid: 0e6313e3-bd0b-45c2-8d2e-37a2e8128bc3
                &Uuid::Uuid128([
                    195u8, 139u8, 18u8, 232u8, 162u8, 55u8, 46u8, 141u8, 194u8, 69u8, 11u8, 189u8, 227u8, 19u8, 99u8,
                    14u8,
                ]),
            )
            .await?;
        info!("Message to central found");
        let message_to_peripheral = client
            .characteristic_by_uuid::<[u8; SPLIT_MESSAGE_MAX_SIZE]>(
                service,
                // uuid: 4b3514fb-cae4-4d38-a097-3a2a3d1c3b9c
                &Uuid::Uuid128([
                    156u8, 59u8, 28u8, 61u8, 42u8, 58u8, 151u8, 160u8, 56u8, 77u8, 228u8, 202u8, 251u8, 20u8, 53u8,
                    75u8,
                ]),
            )
            .await?;
        info!("Subscribing notifications");
        let listener = client.subscribe(&message_to_central, false).await?;
        let split_ble_driver = BleSplitCentralDriver::new(listener, message_to_peripheral, client);
        let peripheral_manager = PeripheralManager::<ROW, COL, ROW_OFFSET, COL_OFFSET, _>::new(split_ble_driver, id);
        peripheral_manager.run().await;
        info!("Peripheral manager stopped");
    };
    Ok(())
}

/// Ble central driver which reads and writes the split message.
///
/// Different from serial, BLE split message is processed in a separate service.
/// The BLE service should keep running, it processes the split message in the callback, which is not async.
/// It's impossible to implement `SplitReader` or `SplitWriter` for BLE service,
/// so we need this wrapper to forward split message to channel.
pub(crate) struct BleSplitCentralDriver<'a, 'b, 'c, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool> {
    // Listener for split message from peripheral
    listener: NotificationListener<'b, 512>,
    // Characteristic to send split message to peripheral
    message_to_peripheral: Characteristic<[u8; SPLIT_MESSAGE_MAX_SIZE]>,
    // Client
    client: &'c GattClient<'a, C, P, 10>,
}

impl<'a, 'b, 'c, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool> BleSplitCentralDriver<'a, 'b, 'c, C, P> {
    pub(crate) fn new(
        listener: NotificationListener<'b, 512>,
        message_to_peripheral: Characteristic<[u8; SPLIT_MESSAGE_MAX_SIZE]>,
        client: &'c GattClient<'a, C, P, 10>,
    ) -> Self {
        Self {
            listener,
            message_to_peripheral,
            client,
        }
    }
}

impl<'a, 'b, 'c, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool> SplitReader
    for BleSplitCentralDriver<'a, 'b, 'c, C, P>
{
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        let data = self.listener.next().await;
        let message = postcard::from_bytes(data.as_ref()).map_err(|_| SplitDriverError::DeserializeError)?;
        info!("Received split message: {:?}", message);

        // Update last activity time when receiving key events from peripheral
        if matches!(message, SplitMessage::Key(_) | SplitMessage::Pointing(_)) {
            debug!("Activity {:?} detected from peripheral", &message);
            update_activity_time();
        }

        Ok(message)
    }
}

impl<'a, 'b, 'c, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool> SplitWriter
    for BleSplitCentralDriver<'a, 'b, 'c, C, P>
{
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        let mut buf = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        match postcard::to_slice(&message, &mut buf) {
            Ok(_bytes) => {
                if let Err(e) = self
                    .client
                    .write_characteristic_without_response(&self.message_to_peripheral, &buf)
                    .await
                {
                    if let BleHostError::BleHost(Error::NotFound) = e {
                        error!("Peripheral disconnected");
                        return Err(SplitDriverError::Disconnected);
                    }
                    #[cfg(feature = "defmt")]
                    let e = defmt::Debug2Format(&e);
                    error!("BLE message_to_peripheral_write error: {:?}", e);
                }
            }
            Err(e) => error!("Postcard serialize split message error: {}", e),
        };

        Ok(SPLIT_MESSAGE_MAX_SIZE)
    }
}

/// Wait for the BLE stack to start.
pub(crate) async fn wait_for_stack_started() {
    while !STACK_STARTED.signaled() {
        embassy_time::Timer::after_millis(500).await;
    }
}

/// Sleep manager task for connection between split central and peripheral
/// Handles sleep timeout and connection parameter adjustments using event-driven approach
async fn sleep_manager_task<
    'b,
    's: 'b,
    C: Controller + ControllerCmdAsync<LeSetPhy> + ControllerCmdSync<LeReadLocalSupportedFeatures>,
    P: PacketPool,
>(
    stack: &'b Stack<'s, C, P>,
    conn: &Connection<'b, P>,
) -> Result<(), BleHostError<C::Error>> {
    let mut latency_changes = LATENCY_CHANGED
        .subscriber()
        .expect("split latency policy supports eight peripheral managers");

    // Sleep management may be disabled, but runtime policy and USB-power
    // changes must still update the live connection.
    if SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS == 0 {
        info!("Sleep management disabled (timeout = 0)");
        loop {
            latency_changes.next_message_pure().await;
            update_conn_params(stack, conn, &defaul_central_conn_param()).await;
        }
    }

    info!(
        "Sleep manager started with {}s timeout",
        SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS
    );

    loop {
        if !crate::state::current_sleeping() {
            // Wait for timeout or activity (false signal means activity/wakeup)
            match select3(
                Timer::after_secs(SPLIT_CENTRAL_SLEEP_TIMEOUT_SECONDS.into()),
                CENTRAL_SLEEP.wait(),
                latency_changes.next_message_pure(),
            )
            .await
            {
                Either3::First(_) => {
                    // Timeout: enter sleep mode
                }
                Either3::Second(signal_value) => {
                    // Received signal - if false, it means activity detected
                    if !signal_value {
                        debug!("Activity detected, resetting sleep timeout");
                        continue;
                    }
                    // True, enter sleep mode
                }
                Either3::Third(()) => {
                    update_conn_params(stack, conn, &defaul_central_conn_param()).await;
                    continue;
                }
            }

            // Timeout or received true from CENTRAL_SLEEP signal, enter sleep mode
            info!("Entering sleep mode");

            // `conn` is the split central -> peripheral BLE link. While the
            // central is sleeping, use a longer interval to reduce central-side
            // radio wakeups; normal params are restored on activity.
            let conn_params = RequestedConnParams {
                min_connection_interval: Duration::from_millis(200),
                max_connection_interval: Duration::from_millis(200),
                max_latency: 25, // 5s
                supervision_timeout: Duration::from_secs(11),
                ..Default::default()
            };

            // Update connection parameters
            update_conn_params(stack, conn, &conn_params).await;
            crate::state::set_sleeping(true);
        } else {
            // Wait for activity to wake up (false signal means activity/wakeup)
            match select(CENTRAL_SLEEP.wait(), latency_changes.next_message_pure()).await {
                Either::First(signal_value) if !signal_value => {
                    info!("Waking up from sleep mode due to activity");
                    crate::state::set_sleeping(false);

                    // Restore normal connection parameters using the latest
                    // power source and runtime override.
                    update_conn_params(stack, conn, &defaul_central_conn_param()).await;
                }
                Either::First(_) | Either::Second(()) => {}
            }
        }
    }
}

/// Update the activity time to indicate user activity
/// This function triggers activity wakeup signal for sleep management
pub(crate) fn update_activity_time() {
    CENTRAL_SLEEP.signal(false);
    debug!("Activity detected, signaling wakeup");
}
