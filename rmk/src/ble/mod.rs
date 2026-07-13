use core::sync::atomic::{AtomicBool, Ordering};

use bt_hci::cmd::le::{LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures, LeSetPhy, LeSetScanResponseData};
use bt_hci::controller::{ControllerCmdAsync, ControllerCmdSync};
use bt_hci::param::AdvFilterPolicy;
use embassy_futures::join::join;
use embassy_futures::select::select;
use embassy_sync::pubsub::Subscriber;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use rmk_types::battery::BatteryStatus;
use rmk_types::ble::BleState;
use rmk_types::connection::ConnectionType;
use rmk_types::led_indicator::LedIndicator;
use trouble_host::prelude::appearance::human_interface_device::KEYBOARD;
use trouble_host::prelude::service::{BATTERY, HUMAN_INTERFACE_DEVICE};
use trouble_host::prelude::*;

use crate::ble::ble_server::{BleHidServer, Server};
use crate::ble::device_info::{PnPID, VidSource};
#[cfg(feature = "passkey_entry")]
use crate::ble::passkey::{PasskeyInputState, next_gatt_event};
use crate::ble::profile::{
    ProfileCccdTable, ProfileInfo, ProfileInfoUpdate, ProfileManager, UPDATED_CCCD_TABLE, UPDATED_PROFILE,
};
use crate::channel::{
    BLE_PROFILE_CHANNEL, BLE_REPORT_CHANNEL, BLE_WAKE_REPORT_CAPTURE_ARMED, BLE_WAKE_REPORT_CHANNEL, LED_SIGNAL,
};
use crate::config::{BleBatteryConfig, RmkConfig};
use crate::core_traits::Runnable;
use crate::event::{BatteryStatusEvent, LedIndicatorEvent, SubscribableEvent, publish_event};
use crate::hid::{HidWriterTrait, Report, neutral_reports};
use crate::keyboard::{LAST_KEY_TIMESTAMP, LOCK_LED_STATES};
#[cfg(feature = "split")]
use crate::split::ble::central::CENTRAL_SLEEP;
use crate::state::set_ble_state;

pub(crate) mod battery_service;
pub(crate) mod ble_server;
pub(crate) mod device_info;
pub(crate) mod led;
#[cfg(feature = "_nrf_ble")]
pub(crate) mod nrf;
pub mod passkey;
pub(crate) mod profile;

/// Global state of sleep management
/// - `true`: Indicates central is sleeping
/// - `false`: Indicates central is awake
pub(crate) static SLEEPING_STATE: AtomicBool = AtomicBool::new(false);

/// Max number of connections. Host BLE profiles may be connected at the same
/// time, and split peripherals still need their own connection slots.
pub(crate) const CONNECTIONS_MAX: usize = crate::SPLIT_PERIPHERALS_NUM + crate::NUM_BLE_PROFILE;

/// Max number of L2CAP channels
pub(crate) const L2CAP_CHANNELS_MAX: usize = CONNECTIONS_MAX * 4; // Signal + att + smp + hid

/// Build the BLE stack.
pub async fn build_ble_stack<'a, C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool>(
    controller: C,
    host_address: [u8; 6],
    resources: &'a mut HostResources<C, P, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>,
) -> Stack<'a, C, P> {
    // Initialize trouble host stack
    trouble_host::new(controller, resources)
        .set_random_address(Address::random(host_address))
        .build()
}

/// BLE transport runnable. Owns the trouble-host server and profile manager;
/// `run` joins the background `ble_task` runner with the advertise→connect→serve
/// loop and runs forever.
//
pub struct BleTransport<'b, 's, C>
where
    's: 'b,
    C: Controller + ControllerCmdAsync<LeSetPhy> + ControllerCmdSync<LeReadLocalSupportedFeatures>,
{
    stack: &'b Stack<'s, C, DefaultPacketPool>,
    server: Server<'static>,
    profile_manager: ProfileManager<'b, 's, C, DefaultPacketPool>,
    product_name: &'static str,
    config: BleBatteryConfig<'b>,
}

impl<'b, 's, C> BleTransport<'b, 's, C>
where
    's: 'b,
    C: Controller + ControllerCmdAsync<LeSetPhy> + ControllerCmdSync<LeReadLocalSupportedFeatures>,
{
    pub async fn new(stack: &'b Stack<'s, C, DefaultPacketPool>, rmk_config: RmkConfig<'static>) -> Self {
        #[cfg(feature = "_nrf_ble")]
        let serial_number = crate::ble::nrf::get_serial_number();
        #[cfg(not(feature = "_nrf_ble"))]
        let serial_number = rmk_config.device_config.serial_number;

        let profile_manager = ProfileManager::new(stack);

        info!("Starting advertising and GATT service");
        let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
            name: rmk_config.device_config.product_name,
            appearance: &appearance::human_interface_device::KEYBOARD,
        }))
        .unwrap();

        server
            .set(
                &server.device_config_service.pnp_id,
                &PnPID {
                    vid_source: VidSource::UsbIF,
                    vendor_id: rmk_config.device_config.vid,
                    product_id: rmk_config.device_config.pid,
                    product_version: 0x0001,
                },
            )
            .unwrap();
        server
            .set(
                &server.device_config_service.serial_number,
                &heapless::String::try_from(serial_number).unwrap(),
            )
            .unwrap();
        server
            .set(
                &server.device_config_service.manufacturer_name,
                &heapless::String::try_from(rmk_config.device_config.manufacturer).unwrap(),
            )
            .unwrap();

        Self {
            stack,
            server,
            profile_manager,
            product_name: rmk_config.device_config.product_name,
            config: rmk_config.ble_battery_config,
        }
    }
}

impl<'b, 's, C> Runnable for BleTransport<'b, 's, C>
where
    's: 'b,
    C: Controller + ControllerCmdAsync<LeSetPhy> + ControllerCmdSync<LeReadLocalSupportedFeatures>,
{
    async fn run(&mut self) -> ! {
        // Load the preferred connection from storage
        let preferred = crate::state::load_preferred_connection().await;
        crate::state::set_preferred_connection(preferred);
        // Load the bonded devices from storage
        #[cfg(feature = "storage")]
        self.profile_manager.load_bonded_devices().await;
        self.profile_manager.update_stack_bonds();

        // Copy the &Stack reference so it doesn't tie a borrow to &mut self.
        let stack: &'b Stack<'s, C, DefaultPacketPool> = self.stack;
        let mut peripheral = stack.peripheral();
        let runner = stack.runner();

        let server = &self.server;
        let profile_manager = &mut self.profile_manager;
        let product_name = self.product_name;
        let battery_enabled = self.config.enabled;

        let connection_loop = async {
            let mut connections = heapless::Vec::<BleHostConnection<'b, '_>, { crate::NUM_BLE_PROFILE }>::new();
            let mut advertiser = None;
            let mut advertising_started_at = None;
            let mut advertising_mode: Option<BleAdvertisingMode> = None;
            let mut hidden_reconnect_started_at = None;
            let mut forced_visible_pairing_slot = None;
            let mut slot_generations = [0u32; crate::NUM_BLE_PROFILE];
            let mut pending_clears = [false; crate::NUM_BLE_PROFILE];
            let mut pending_rejections = [false; crate::NUM_BLE_PROFILE];
            let mut profile_intent_generation = 0u32;
            let mut pending_visible_intents = [None; crate::NUM_BLE_PROFILE];
            let mut pending_bond_removals = [None; crate::NUM_BLE_PROFILE];
            let mut advertising_paused = false;
            let mut last_key_timestamp = None;
            let mut last_led_source = None;
            let mut wake_report_capture: Option<WakeReportCapture> = None;
            let mut pending_ble_reports = heapless::Vec::<PendingBleReport, 4>::new();
            let mut keyboard_activity = crate::event::KeyboardEvent::subscriber();
            let mut pointing_activity = crate::event::PointingEvent::subscriber();

            loop {
                let now = Instant::now();
                let mut user_activity = false;
                while keyboard_activity.try_next_message_pure().is_some() {
                    user_activity = true;
                }
                while pointing_activity.try_next_message_pure().is_some() {
                    user_activity = true;
                }

                if wake_report_capture.is_some_and(|capture| now >= capture.expires_at) {
                    clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                }
                pending_ble_reports.retain(|pending| now < pending.expires_at);
                if crate::state::active_transport() == Some(ConnectionType::Usb) {
                    clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                }

                if advertising_paused && user_activity {
                    let active_profile = crate::state::current_profile();
                    let active_ready = connections.iter().any(|connection| {
                        connection.slot_num == active_profile
                            && connection.ready
                            && connection.conn.raw().is_connected()
                            && !pending_clears[active_profile as usize]
                    });
                    if !active_ready {
                        advertising_paused = false;
                        hidden_reconnect_started_at = None;
                        if profile_manager.bond_info(active_profile).is_some() {
                            wake_report_capture = Some(WakeReportCapture {
                                slot: active_profile,
                                generation: slot_generations[active_profile as usize],
                                expires_at: now + Duration::from_secs(2),
                            });
                            BLE_WAKE_REPORT_CAPTURE_ARMED.store(true, Ordering::Release);
                        } else {
                            clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                        }
                    }
                }

                drain_wake_report_channel(now, wake_report_capture, &slot_generations, &mut pending_ble_reports);

                let profile_action_pending = !BLE_PROFILE_CHANNEL.is_empty();
                if profile_action_pending {
                    advertiser = None;
                    advertising_started_at = None;
                    advertising_mode = None;
                    hidden_reconnect_started_at = None;
                    advertising_paused = false;
                }

                if !profile_action_pending
                    && !advertising_paused
                    && advertiser.is_none()
                    && connections.len() < crate::NUM_BLE_PROFILE
                {
                    let active_profile = crate::state::current_profile();
                    let active_connected = connections.iter().any(|connection| {
                        connection.slot_num == active_profile && connection.conn.raw().is_connected()
                    });

                    let mut next_mode = None;
                    if let Some(slot) = forced_visible_pairing_slot {
                        if !pending_clears[slot as usize] && !pending_rejections[slot as usize] {
                            let slot_connected = connections
                                .iter()
                                .any(|connection| connection.slot_num == slot && connection.conn.raw().is_connected());
                            if !slot_connected && profile_manager.bond_info(slot).is_none() {
                                next_mode = Some(BleAdvertisingMode::VisiblePairing { slot });
                            } else {
                                forced_visible_pairing_slot = None;
                            }
                        }
                    }

                    if next_mode.is_none()
                        && !active_connected
                        && !pending_clears[active_profile as usize]
                        && !pending_rejections[active_profile as usize]
                        && profile_manager.bond_info(active_profile).is_none()
                    {
                        next_mode = Some(BleAdvertisingMode::VisiblePairing { slot: active_profile });
                    }

                    if next_mode.is_none() {
                        let mut slots = heapless::Vec::<u8, { crate::NUM_BLE_PROFILE }>::new();
                        if !active_connected
                            && !pending_clears[active_profile as usize]
                            && !pending_rejections[active_profile as usize]
                            && profile_manager.bond_info(active_profile).is_some()
                        {
                            let _ = slots.push(active_profile);
                        }
                        for info in profile_manager.bonded_profiles() {
                            if info.slot_num == active_profile
                                || pending_clears[info.slot_num as usize]
                                || pending_rejections[info.slot_num as usize]
                            {
                                continue;
                            }
                            let slot_connected = connections.iter().any(|connection| {
                                connection.slot_num == info.slot_num && connection.conn.raw().is_connected()
                            });
                            if !slot_connected && slots.push(info.slot_num).is_err() {
                                warn!("Too many BLE reconnect slots");
                                break;
                            }
                        }
                        if !slots.is_empty() {
                            let fast = hidden_reconnect_started_at
                                .map(|started_at| now.duration_since(started_at) < Duration::from_secs(5))
                                .unwrap_or(true);
                            next_mode = Some(BleAdvertisingMode::HiddenReconnect { slots, fast });
                        }
                    }

                    if let Some(mode) = next_mode {
                        if mode.is_visible() {
                            hidden_reconnect_started_at = None;
                            clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                        } else if hidden_reconnect_started_at.is_none() {
                            hidden_reconnect_started_at = Some(now);
                        }

                        match start_advertising(product_name, stack, &mut peripheral, &mode, profile_manager).await {
                            Ok(new_advertiser) => {
                                advertiser = Some(new_advertiser);
                                advertising_started_at = Some(now);
                                advertising_mode = Some(mode);
                            }
                            Err(e) => {
                                if !BLE_PROFILE_CHANNEL.is_empty() {
                                    advertising_mode = None;
                                    continue;
                                }
                                #[cfg(feature = "defmt")]
                                let e = defmt::Debug2Format(&e);
                                error!("Advertise error: {:?}", e);
                                advertising_mode = None;
                                Timer::after_millis(200).await;
                            }
                        }
                    } else {
                        advertising_mode = None;
                        hidden_reconnect_started_at = None;
                    }
                }

                if advertiser.is_some()
                    && let Some(conn) = peripheral.try_accept()
                {
                    advertiser = None;
                    advertising_started_at = None;
                    let Some(mode) = advertising_mode.take() else {
                        conn.disconnect();
                        continue;
                    };
                    match conn.with_attribute_server(server) {
                        Ok(conn) => {
                            info!("[adv] connection established");
                            if let Err(e) = conn.raw().set_bondable(mode.is_visible()) {
                                error!("Set bondable error: {:?}", e);
                                conn.raw().disconnect();
                                continue;
                            }
                            let known_slot = profile_manager
                                .bonded_profiles()
                                .find(|bond_info| bond_info.info.identity.match_identity(&conn.raw().peer_identity()))
                                .map(|bond_info| bond_info.slot_num);
                            let slot_num = match &mode {
                                BleAdvertisingMode::VisiblePairing { slot } => known_slot.unwrap_or(*slot),
                                BleAdvertisingMode::HiddenReconnect { slots, .. } => {
                                    if let Some(slot) = known_slot {
                                        if slots.contains(&slot) {
                                            slot
                                        } else {
                                            warn!("[adv] rejecting peer from non-target BLE slot {}", slot);
                                            conn.raw().disconnect();
                                            clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                                            continue;
                                        }
                                    } else {
                                        warn!("[adv] rejecting unknown peer during hidden reconnect");
                                        conn.raw().disconnect();
                                        clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                                        continue;
                                    }
                                }
                            };
                            if slot_num as usize >= crate::NUM_BLE_PROFILE {
                                error!("Rejecting BLE connection for invalid slot {}", slot_num);
                                conn.raw().disconnect();
                                continue;
                            }
                            if let Some(index) = connections
                                .iter()
                                .position(|connection| connection.slot_num == slot_num)
                            {
                                warn!("Rejecting duplicate BLE connection for slot {}", slot_num);
                                conn.raw().disconnect();
                                let _ = index;
                                continue;
                            }
                            let generation = slot_generations[slot_num as usize];
                            let bond_info = profile_manager.bond_info(slot_num);
                            if bond_info.is_none() {
                                let empty_table = [0u8; 4];
                                if let Ok(view) = ClientAttTableView::try_from_raw(&empty_table) {
                                    server.set_client_att_table(conn.raw(), &view);
                                }
                            }
                            let connection = BleHostConnection::new(
                                server,
                                conn,
                                stack,
                                slot_num,
                                generation,
                                battery_enabled,
                                bond_info,
                            )
                            .await;
                            if connections.push(connection).is_err() {
                                error!("No BLE profile connection slot available");
                            } else if let BleAdvertisingMode::VisiblePairing { slot } = mode
                                && slot == slot_num
                            {
                                forced_visible_pairing_slot = None;
                            }
                        }
                        Err(e) => error!("Attach GATT server error: {:?}", e),
                    }
                }

                let profile_before_update = crate::state::current_profile();
                let profile_effect = profile_manager.poll_profile_update(&slot_generations).await;
                if profile_effect.select_slot.is_some() || profile_effect.clear_slot.is_some() {
                    profile_intent_generation = profile_intent_generation.wrapping_add(1);
                }
                if let Some(selected_slot) = profile_effect.select_slot {
                    if selected_slot as usize >= crate::NUM_BLE_PROFILE {
                        error!("Ignoring invalid BLE profile selection: {}", selected_slot);
                    } else {
                        advertiser = None;
                        advertising_started_at = None;
                        advertising_mode = None;
                        hidden_reconnect_started_at = None;
                        advertising_paused = false;
                        if profile_effect.clear_slot.is_none() {
                            forced_visible_pairing_slot = None;
                        }
                        clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);

                        if profile_effect.profile_changed {
                            BLE_REPORT_CHANNEL.clear();
                            if let Some(old_connection) = connections.iter().find(|connection| {
                                connection.slot_num == profile_before_update
                                    && connection.ready
                                    && connection.conn.raw().is_connected()
                            }) {
                                old_connection.release_all(server).await;
                            }

                            let selected_ready = connections.iter().any(|connection| {
                                profile_effect.clear_slot != Some(selected_slot)
                                    && connection.slot_num == selected_slot
                                    && connection.ready
                                    && connection.conn.raw().is_connected()
                            });
                            crate::state::update_status(|status| {
                                status.ble.profile = selected_slot;
                                status.ble.state = if selected_ready {
                                    BleState::Connected
                                } else {
                                    BleState::Inactive
                                };
                            });
                            // `update_status` performs a transport-level release after
                            // committing the new profile. The old slot was released
                            // explicitly above, so discard that profile-ambiguous item.
                            BLE_REPORT_CHANNEL.clear();

                            if selected_ready && crate::state::active_transport() == Some(ConnectionType::Ble) {
                                let led = connections
                                    .iter()
                                    .find(|connection| connection.slot_num == selected_slot)
                                    .and_then(|connection| connection.led_indicator)
                                    .unwrap_or_else(LedIndicator::new);
                                LED_SIGNAL.signal(led);
                            }
                        }
                    }
                }

                if let Some(clear_slot) = profile_effect.clear_slot {
                    if clear_slot as usize >= crate::NUM_BLE_PROFILE {
                        error!("Ignoring invalid BLE profile clear: {}", clear_slot);
                    } else {
                        let slot_index = clear_slot as usize;
                        slot_generations[slot_index] = slot_generations[slot_index].wrapping_add(1);
                        pending_clears[slot_index] = true;
                        advertiser = None;
                        advertising_started_at = None;
                        advertising_mode = None;
                        hidden_reconnect_started_at = None;
                        advertising_paused = false;
                        forced_visible_pairing_slot = None;
                        clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);

                        if let Some(index) = connections
                            .iter()
                            .position(|connection| connection.slot_num == clear_slot)
                        {
                            pending_rejections[slot_index] = false;
                            pending_visible_intents[slot_index] = Some(profile_intent_generation);
                            if connections[index].ready && connections[index].conn.raw().is_connected() {
                                connections[index].release_all(server).await;
                            }
                            connections[index].ready = false;
                            connections[index].conn.raw().disconnect();
                        } else {
                            pending_clears[slot_index] = false;
                            profile_manager.clear_bond(clear_slot).await;
                            if crate::state::current_profile() == clear_slot {
                                forced_visible_pairing_slot = Some(clear_slot);
                            }
                        }
                    }
                }

                if let Some(reject_slot) = profile_effect.reject_slot {
                    if reject_slot as usize >= crate::NUM_BLE_PROFILE {
                        error!("Ignoring invalid BLE profile rejection: {}", reject_slot);
                    } else {
                        let slot_index = reject_slot as usize;
                        slot_generations[slot_index] = slot_generations[slot_index].wrapping_add(1);
                        advertiser = None;
                        advertising_started_at = None;
                        advertising_mode = None;
                        hidden_reconnect_started_at = None;
                        advertising_paused = false;
                        clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);

                        if let Some(index) = connections
                            .iter()
                            .position(|connection| connection.slot_num == reject_slot)
                        {
                            if connections[index].ready && connections[index].conn.raw().is_connected() {
                                connections[index].release_all(server).await;
                            }
                            connections[index].ready = false;
                            pending_rejections[slot_index] = true;
                            pending_visible_intents[slot_index] = Some(profile_intent_generation);
                            connections[index].conn.raw().disconnect();
                        } else if crate::state::current_profile() == reject_slot {
                            forced_visible_pairing_slot = Some(reject_slot);
                        }
                    }
                }

                while let Ok(routed) = BLE_REPORT_CHANNEL.inner.try_receive() {
                    if routed.force {
                        if let Some(connection) = connections.iter().find(|connection| {
                            connection.slot_num == routed.slot
                                && connection.ready
                                && connection.conn.raw().is_connected()
                        }) {
                            let mut writer = BleHidServer::new(server, &connection.conn);
                            if let Err(e) = writer.write_report(&routed.report).await {
                                error!("Failed to send forced BLE release: {:?}", e);
                            }
                        }
                        continue;
                    }

                    let route = crate::state::report_route();
                    let active_profile = route.ble_profile;
                    if routed.slot != active_profile
                        || routed.route_generation != route.generation
                        || route.active != Some(ConnectionType::Ble)
                        || pending_clears[active_profile as usize]
                    {
                        debug!("Dropping stale BLE report for slot {}", routed.slot);
                        continue;
                    }

                    if let Some(connection) = connections.iter().find(|connection| {
                        connection.slot_num == active_profile
                            && connection.ready
                            && connection.conn.raw().is_connected()
                    }) {
                        if !connection.notifications_enabled(server, &routed.report) {
                            if let Some(capture) = wake_report_capture
                                && capture.slot == active_profile
                                && capture.generation == slot_generations[active_profile as usize]
                                && Instant::now() < capture.expires_at
                            {
                                if pending_ble_reports.is_full() {
                                    pending_ble_reports.remove(0);
                                }
                                let _ = pending_ble_reports.push(PendingBleReport {
                                    slot: capture.slot,
                                    generation: capture.generation,
                                    expires_at: capture.expires_at,
                                    report: routed.report,
                                });
                            } else {
                                warn!("Dropping BLE report: notifications are not enabled");
                            }
                            continue;
                        }
                        let mut ble_hid_server = BleHidServer::new(server, &connection.conn);
                        if let Err(e) = ble_hid_server.write_report(&routed.report).await {
                            error!("Failed to send report: {:?}", e);
                        }
                    } else {
                        warn!("Dropping BLE report: active slot is not ready");
                    }
                }

                #[cfg(feature = "host")]
                while let Ok(reply) = crate::channel::HOST_BLE_REPLY.try_receive() {
                    if !pending_clears[reply.slot as usize]
                        && let Some(connection) = connections.iter().find(|connection| {
                            connection.slot_num == reply.slot
                                && connection.generation == reply.generation
                                && connection.ready
                                && connection.conn.raw().is_connected()
                        })
                    {
                        debug!("Sending Vial reply to BLE slot {}: {:?}", reply.slot, reply.data);
                        if let Err(e) = server
                            .host_service
                            .input_data
                            .notify(&connection.conn, &reply.data, true)
                            .await
                        {
                            error!("Failed to notify via report: {:?}", e);
                        }
                    } else {
                        warn!("Dropping stale BLE host reply for slot {}", reply.slot);
                    }
                }

                while let Some(led_indicator) = LED_SIGNAL.try_take() {
                    if crate::state::active_transport() == Some(ConnectionType::Ble) {
                        LOCK_LED_STATES.store(led_indicator.into_bits(), Ordering::Relaxed);
                        publish_event(LedIndicatorEvent::new(led_indicator));
                    }
                }

                let mut connection_outcome = None;
                for index in 0..connections.len() {
                    let slot = connections[index].slot_num as usize;
                    if pending_clears[slot] || pending_rejections[slot] {
                        match connections[index].poll_disconnect_event().await {
                            DisconnectPollOutcome::Pending => {}
                            DisconnectPollOutcome::Bond(identity) => {
                                pending_bond_removals[slot] = Some(identity);
                            }
                            DisconnectPollOutcome::Disconnected => {
                                connection_outcome = Some((index, GattEventOutcome::Disconnected));
                                break;
                            }
                        }
                        continue;
                    }
                    match connections[index].poll_gatt_event(server).await {
                        Ok(GattEventOutcome::None) => {}
                        Ok(outcome) => {
                            connection_outcome = Some((index, outcome));
                            break;
                        }
                        Err(e) => {
                            error!("[gatt] error: {:?}", e);
                            connection_outcome = Some((index, GattEventOutcome::Disconnected));
                            break;
                        }
                    }
                }
                if let Some((index, outcome)) = connection_outcome {
                    let slot = connections[index].slot_num;
                    let generation = connections[index].generation;
                    match outcome {
                        GattEventOutcome::None => {}
                        GattEventOutcome::Encrypted | GattEventOutcome::CccdUpdated => {
                            if pending_rejections[slot as usize] || !connections[index].conn.raw().is_connected() {
                                connections[index].ready = false;
                                continue;
                            }
                            if outcome == GattEventOutcome::Encrypted {
                                connections[index].ready = true;
                            }
                            if connections[index].ready
                                && !pending_clears[slot as usize]
                                && crate::state::current_profile() == slot
                            {
                                // Publish readiness before flushing. Reports produced
                                // during an awaited wake write then enter the normal
                                // routed queue behind the cached reports.
                                set_ble_state(BleState::Connected);
                            }

                            if let Some(capture) = wake_report_capture
                                && capture.slot == slot
                                && capture.generation == generation
                                && crate::state::current_profile() == slot
                                && Instant::now() < capture.expires_at
                            {
                                drain_wake_report_channel(
                                    Instant::now(),
                                    wake_report_capture,
                                    &slot_generations,
                                    &mut pending_ble_reports,
                                );
                                if crate::state::active_transport() != Some(ConnectionType::Ble) {
                                    clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                                    continue;
                                }

                                let all_notifications_enabled = pending_ble_reports
                                    .iter()
                                    .filter(|pending| pending.slot == slot && pending.generation == generation)
                                    .all(|pending| connections[index].notifications_enabled(server, &pending.report));
                                if all_notifications_enabled {
                                    let mut flush_failed = false;
                                    let mut pending_index = 0;
                                    let mut ble_hid_server = BleHidServer::new(server, &connections[index].conn);
                                    while pending_index < pending_ble_reports.len() {
                                        if pending_ble_reports[pending_index].slot != slot
                                            || pending_ble_reports[pending_index].generation != generation
                                        {
                                            pending_index += 1;
                                            continue;
                                        }

                                        let pending = pending_ble_reports.remove(pending_index);
                                        if crate::state::current_profile() != slot
                                            || crate::state::active_transport() != Some(ConnectionType::Ble)
                                            || Instant::now() >= pending.expires_at
                                        {
                                            flush_failed = true;
                                            break;
                                        }
                                        if let Err(e) = ble_hid_server.write_report(&pending.report).await {
                                            error!("Failed to send pending wake report: {:?}", e);
                                            flush_failed = true;
                                            break;
                                        }
                                    }

                                    let pending_for_connection = pending_ble_reports
                                        .iter()
                                        .any(|pending| pending.slot == slot && pending.generation == generation);
                                    if flush_failed || !pending_for_connection {
                                        clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                                    }
                                }
                            }
                        }
                        GattEventOutcome::Led(led_indicator) => {
                            connections[index].led_indicator = Some(led_indicator);
                            if connections[index].ready
                                && !pending_clears[slot as usize]
                                && crate::state::current_profile() == slot
                                && crate::state::active_transport() == Some(ConnectionType::Ble)
                            {
                                LED_SIGNAL.signal(led_indicator);
                            }
                        }
                        GattEventOutcome::Disconnected => {
                            if pending_clears[slot as usize] {
                                pending_clears[slot as usize] = false;
                                pending_rejections[slot as usize] = false;
                                let peer_identity = connections[index].conn.raw().peer_identity();
                                if let Some(identity) = pending_bond_removals[slot as usize].take()
                                    && let Err(e) = stack.remove_bond_information(identity)
                                {
                                    debug!("Remove rejected BLE bond for slot {}: {:?}", slot, e);
                                }
                                if let Err(e) = stack.remove_bond_information(peer_identity) {
                                    debug!("Remove connected BLE bond for slot {}: {:?}", slot, e);
                                }
                                profile_manager.clear_bond(slot).await;
                                if pending_visible_intents[slot as usize].take() == Some(profile_intent_generation) {
                                    forced_visible_pairing_slot = Some(slot);
                                }
                            } else if pending_rejections[slot as usize] {
                                pending_rejections[slot as usize] = false;
                                if let Some(identity) = pending_bond_removals[slot as usize].take()
                                    && let Err(e) = stack.remove_bond_information(identity)
                                {
                                    debug!("Remove rejected BLE bond for slot {}: {:?}", slot, e);
                                }
                                if pending_visible_intents[slot as usize].take() == Some(profile_intent_generation) {
                                    forced_visible_pairing_slot = Some(slot);
                                }
                            } else {
                                pending_visible_intents[slot as usize] = None;
                                pending_bond_removals[slot as usize] = None;
                            }
                            // Drop after bond removal so trouble-host also erases
                            // this peer's retained client attribute table.
                            connections.remove(index);
                            advertiser = None;
                            advertising_started_at = None;
                            advertising_mode = None;
                            hidden_reconnect_started_at = None;
                            advertising_paused = false;
                            if wake_report_capture.is_some_and(|capture| capture.slot == slot) {
                                clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                            }
                        }
                        GattEventOutcome::BondLost => {
                            warn!("[gatt] bond lost on BLE slot {}, clearing slot", slot);
                            connections[index].conn.raw().disconnect();
                            advertiser = None;
                            advertising_started_at = None;
                            advertising_mode = None;
                            hidden_reconnect_started_at = None;
                            advertising_paused = false;
                            if !pending_clears[slot as usize] && slot_generations[slot as usize] == generation {
                                slot_generations[slot as usize] = slot_generations[slot as usize].wrapping_add(1);
                                pending_clears[slot as usize] = true;
                                pending_visible_intents[slot as usize] = Some(profile_intent_generation);
                            }
                            if wake_report_capture.is_some_and(|capture| capture.slot == slot) {
                                clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                            }
                        }
                        GattEventOutcome::InvalidPairing(invalid_identity) => {
                            warn!(
                                "[gatt] invalid pairing identity on BLE slot {}, retrying visible pairing",
                                slot
                            );
                            connections[index].conn.raw().disconnect();
                            advertiser = None;
                            advertising_started_at = None;
                            advertising_mode = None;
                            hidden_reconnect_started_at = None;
                            advertising_paused = false;
                            if pending_clears[slot as usize] {
                                pending_bond_removals[slot as usize] = invalid_identity;
                            } else {
                                if slot_generations[slot as usize] == generation {
                                    slot_generations[slot as usize] = slot_generations[slot as usize].wrapping_add(1);
                                }
                                connections[index].ready = false;
                                pending_rejections[slot as usize] = true;
                                pending_bond_removals[slot as usize] = invalid_identity;
                                pending_visible_intents[slot as usize] = Some(profile_intent_generation);
                            }
                            if wake_report_capture.is_some_and(|capture| capture.slot == slot) {
                                clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                            }
                        }
                    }
                }

                for connection in connections.iter_mut() {
                    connection.update_conn_params_if_due(stack).await;
                    if let Some(last_press) = LAST_KEY_TIMESTAMP.try_take() {
                        last_key_timestamp = Some(last_press);
                    }
                    if connection.ready
                        && !pending_clears[connection.slot_num as usize]
                        && let Some(notifier) = &mut connection.battery_notifier
                    {
                        notifier.poll(&connection.conn, last_key_timestamp).await;
                    }
                }

                if let Some(started_at) = advertising_started_at
                    && Instant::now().duration_since(started_at) >= Duration::from_secs(300)
                {
                    advertiser = None;
                    advertising_started_at = None;
                    advertising_mode = None;
                    hidden_reconnect_started_at = None;
                    advertising_paused = true;
                    clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);

                    let active_profile = crate::state::current_profile();
                    let active_ready = connections.iter().any(|connection| {
                        connection.slot_num == active_profile
                            && connection.ready
                            && connection.conn.raw().is_connected()
                            && !pending_clears[active_profile as usize]
                    });
                    if !active_ready && profile_manager.bond_info(active_profile).is_some() {
                        BLE_WAKE_REPORT_CHANNEL.clear();
                        BLE_WAKE_REPORT_CAPTURE_ARMED.store(true, Ordering::Release);
                    }

                    if connections.is_empty() && !active_ready {
                        warn!("Advertising timeout, sleep and wait for any key");
                        set_ble_state(BleState::Inactive);

                        #[cfg(feature = "split")]
                        CENTRAL_SLEEP.signal(true);

                        let _ = select(
                            keyboard_activity.next_message_pure(),
                            pointing_activity.next_message_pure(),
                        )
                        .await;
                        advertising_paused = false;
                        hidden_reconnect_started_at = None;
                        let active_profile = crate::state::current_profile();
                        if profile_manager.bond_info(active_profile).is_some() {
                            wake_report_capture = Some(WakeReportCapture {
                                slot: active_profile,
                                generation: slot_generations[active_profile as usize],
                                expires_at: Instant::now() + Duration::from_secs(2),
                            });
                            BLE_WAKE_REPORT_CAPTURE_ARMED.store(true, Ordering::Release);
                        } else {
                            clear_wake_report_cache(&mut wake_report_capture, &mut pending_ble_reports);
                        }

                        #[cfg(feature = "split")]
                        CENTRAL_SLEEP.signal(false);
                    }
                }

                if let Some(BleAdvertisingMode::HiddenReconnect { fast: true, .. }) = &advertising_mode
                    && let Some(started_at) = hidden_reconnect_started_at
                    && Instant::now().duration_since(started_at) >= Duration::from_secs(5)
                {
                    advertiser = None;
                    advertising_started_at = None;
                    advertising_mode = None;
                }

                let active_profile = crate::state::current_profile();
                if connections.iter().any(|connection| {
                    connection.slot_num == active_profile
                        && connection.ready
                        && connection.conn.raw().is_connected()
                        && !pending_clears[active_profile as usize]
                }) {
                    set_ble_state(BleState::Connected);
                } else if advertiser.is_some() {
                    set_ble_state(BleState::Advertising);
                } else {
                    set_ble_state(BleState::Inactive);
                }

                let active_led_source = if crate::state::active_transport() == Some(ConnectionType::Ble) {
                    Some(active_profile)
                } else {
                    None
                };
                if active_led_source != last_led_source {
                    last_led_source = active_led_source;
                    if let Some(slot) = active_led_source {
                        let led = connections
                            .iter()
                            .find(|connection| connection.slot_num == slot)
                            .and_then(|connection| connection.led_indicator)
                            .unwrap_or_else(LedIndicator::new);
                        LED_SIGNAL.signal(led);
                    }
                }

                Timer::after_millis(1).await;
            }
        };

        join(ble_task(runner), connection_loop).await;
        unreachable!("BleTransport sub-tasks must run forever")
    }
}

#[derive(Clone)]
enum BleAdvertisingMode {
    VisiblePairing {
        slot: u8,
    },
    HiddenReconnect {
        slots: heapless::Vec<u8, { crate::NUM_BLE_PROFILE }>,
        fast: bool,
    },
}

impl BleAdvertisingMode {
    fn is_visible(&self) -> bool {
        matches!(self, Self::VisiblePairing { .. })
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum GattEventOutcome {
    None,
    Disconnected,
    Encrypted,
    CccdUpdated,
    Led(LedIndicator),
    BondLost,
    InvalidPairing(Option<Identity>),
}

enum DisconnectPollOutcome {
    Pending,
    Bond(Identity),
    Disconnected,
}

#[derive(Copy, Clone)]
struct WakeReportCapture {
    slot: u8,
    generation: u32,
    expires_at: Instant,
}

struct PendingBleReport {
    slot: u8,
    generation: u32,
    expires_at: Instant,
    report: Report,
}

fn clear_wake_report_cache(
    wake_report_capture: &mut Option<WakeReportCapture>,
    pending_ble_reports: &mut heapless::Vec<PendingBleReport, 4>,
) {
    *wake_report_capture = None;
    pending_ble_reports.clear();
    BLE_WAKE_REPORT_CAPTURE_ARMED.store(false, Ordering::Release);
    BLE_WAKE_REPORT_CHANNEL.clear();
}

fn drain_wake_report_channel(
    now: Instant,
    wake_report_capture: Option<WakeReportCapture>,
    slot_generations: &[u32; crate::NUM_BLE_PROFILE],
    pending_ble_reports: &mut heapless::Vec<PendingBleReport, 4>,
) {
    while let Ok(wake_report) = BLE_WAKE_REPORT_CHANNEL.try_receive() {
        let report_expires_at = wake_report.captured_at + Duration::from_secs(2);
        if let Some(capture) = wake_report_capture
            && capture.slot == crate::state::current_profile()
            && capture.generation == slot_generations[capture.slot as usize]
            && now < capture.expires_at
            && now < report_expires_at
        {
            if pending_ble_reports.is_full() {
                pending_ble_reports.remove(0);
            }
            let _ = pending_ble_reports.push(PendingBleReport {
                slot: capture.slot,
                generation: capture.generation,
                expires_at: capture.expires_at.min(report_expires_at),
                report: wake_report.report,
            });
        }
    }
}

struct BleHostConnection<'stack, 'server> {
    slot_num: u8,
    generation: u32,
    conn: GattConnection<'stack, 'server, DefaultPacketPool>,
    ready: bool,
    led_indicator: Option<LedIndicator>,
    next_conn_param_update_at: Instant,
    conn_param_stage: u8,
    battery_notifier: Option<BleBatteryNotifier>,
    #[cfg(feature = "passkey_entry")]
    passkey_state: PasskeyInputState,
}

impl<'stack, 'server> BleHostConnection<'stack, 'server> {
    async fn new<C: Controller + ControllerCmdAsync<LeSetPhy>>(
        server: &'server Server<'_>,
        conn: GattConnection<'stack, 'server, DefaultPacketPool>,
        stack: &Stack<'_, C, DefaultPacketPool>,
        slot_num: u8,
        generation: u32,
        battery_enabled: bool,
        bond_info: Option<crate::ble::profile::ProfileInfo>,
    ) -> Self {
        if let Some(bond_info) = bond_info
            && bond_info.info.identity.match_identity(&conn.raw().peer_identity())
        {
            info!("Loading CCCD table: {:?}", bond_info.cccd_table);
            match ClientAttTableView::try_from_raw(&bond_info.cccd_table) {
                Ok(view) => server.set_client_att_table(conn.raw(), &view),
                Err(e) => warn!("Invalid stored CCCD table: {:?}", e),
            }
        }

        update_ble_phy(stack, conn.raw()).await;

        Self {
            slot_num,
            generation,
            conn,
            ready: false,
            led_indicator: None,
            next_conn_param_update_at: Instant::now() + Duration::from_secs(5),
            conn_param_stage: 0,
            battery_notifier: battery_enabled.then(|| {
                let now = Instant::now();
                BleBatteryNotifier {
                    battery_level: server.battery_service.level,
                    sub: BatteryStatusEvent::subscriber(),
                    first_report_at: now + Duration::from_secs(2),
                    first_report_sent: false,
                    next_timeout_report_at: now + Duration::from_secs(1800),
                }
            }),
            #[cfg(feature = "passkey_entry")]
            passkey_state: PasskeyInputState::new(),
        }
    }

    async fn poll_gatt_event(&mut self, server: &Server<'_>) -> Result<GattEventOutcome, Error> {
        #[cfg(feature = "passkey_entry")]
        let event = match with_timeout(
            Duration::from_millis(1),
            next_gatt_event(&self.conn, &mut self.passkey_state),
        )
        .await
        {
            Ok(Some(event)) => event,
            Ok(None) | Err(_) => return Ok(GattEventOutcome::None),
        };

        #[cfg(not(feature = "passkey_entry"))]
        let event = match with_timeout(Duration::from_millis(1), self.conn.next()).await {
            Ok(event) => event,
            Err(_) => return Ok(GattEventOutcome::None),
        };

        handle_gatt_event(
            server,
            &self.conn,
            self.slot_num,
            self.generation,
            #[cfg(feature = "passkey_entry")]
            &mut self.passkey_state,
            event,
        )
        .await
    }

    async fn poll_disconnect_event(&mut self) -> DisconnectPollOutcome {
        let event = match with_timeout(Duration::from_millis(1), self.conn.next()).await {
            Ok(event) => event,
            Err(_) => return DisconnectPollOutcome::Pending,
        };

        match event {
            GattConnectionEvent::Disconnected { reason } => {
                #[cfg(feature = "passkey_entry")]
                self.passkey_state.clear();
                info!("[gatt] disconnected while quarantined: {:?}", reason);
                DisconnectPollOutcome::Disconnected
            }
            GattConnectionEvent::PairingComplete { bond: Some(bond), .. } => DisconnectPollOutcome::Bond(bond.identity),
            GattConnectionEvent::Gatt { event } => {
                match event.reject(AttErrorCode::UNLIKELY_ERROR) {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("[gatt] failed to reject quarantined request: {:?}", e),
                }
                DisconnectPollOutcome::Pending
            }
            _ => DisconnectPollOutcome::Pending,
        }
    }

    async fn release_all(&self, server: &Server<'_>) {
        let mut writer = BleHidServer::new(server, &self.conn);
        for report in neutral_reports() {
            if let Err(e) = writer.write_report(&report).await {
                warn!("Failed to release BLE slot {} reports: {:?}", self.slot_num, e);
            }
        }
    }

    fn notifications_enabled(&self, server: &Server<'_>, report: &Report) -> bool {
        let cccd_handle = match report {
            Report::KeyboardReport(_) => server.hid_service.input_keyboard.cccd_handle,
            Report::MouseReport(_) => server.composite_service.mouse_report.cccd_handle,
            Report::MediaKeyboardReport(_) => server.composite_service.media_report.cccd_handle,
            Report::SystemControlReport(_) => server.composite_service.system_report.cccd_handle,
            #[cfg(feature = "steno")]
            Report::StenoReport(_) => return true,
        };
        let Some(cccd_handle) = cccd_handle else {
            return false;
        };
        server.get_client_att_table(self.conn.raw()).is_some_and(|table| {
            table
                .get(cccd_handle)
                .is_some_and(|value| value.len() >= 2 && u16::from_le_bytes([value[0], value[1]]) & 1 != 0)
        })
    }

    async fn update_conn_params_if_due<C: Controller + ControllerCmdSync<LeReadLocalSupportedFeatures>>(
        &mut self,
        stack: &Stack<'_, C, DefaultPacketPool>,
    ) {
        if self.conn_param_stage >= 2 || Instant::now() < self.next_conn_param_update_at {
            return;
        }

        let params = if self.conn_param_stage == 0 {
            RequestedConnParams {
                min_connection_interval: Duration::from_millis(15),
                max_connection_interval: Duration::from_millis(15),
                max_latency: 30,
                min_event_length: Duration::from_secs(0),
                max_event_length: Duration::from_secs(0),
                supervision_timeout: Duration::from_secs(5),
            }
        } else {
            RequestedConnParams {
                min_connection_interval: Duration::from_micros(7500),
                max_connection_interval: Duration::from_micros(7500),
                max_latency: 30,
                min_event_length: Duration::from_secs(0),
                max_event_length: Duration::from_secs(0),
                supervision_timeout: Duration::from_secs(5),
            }
        };

        update_conn_params(stack, self.conn.raw(), &params).await;
        self.conn_param_stage += 1;
        self.next_conn_param_update_at = Instant::now() + Duration::from_secs(5);
    }
}

struct BleBatteryNotifier {
    battery_level: Characteristic<u8>,
    sub: Subscriber<
        'static,
        crate::RawMutex,
        BatteryStatusEvent,
        { crate::BATTERY_STATUS_EVENT_CHANNEL_SIZE },
        { crate::BATTERY_STATUS_EVENT_SUB_SIZE },
        { crate::BATTERY_STATUS_EVENT_PUB_SIZE },
    >,
    first_report_at: Instant,
    first_report_sent: bool,
    next_timeout_report_at: Instant,
}

impl BleBatteryNotifier {
    async fn poll<P: PacketPool>(&mut self, conn: &GattConnection<'_, '_, P>, last_key_timestamp: Option<u32>) {
        let now = Instant::now();

        if !self.first_report_sent {
            if now < self.first_report_at {
                return;
            }

            let mut status = crate::input_device::battery::current_battery_status();
            while let Some(new_status) = self.sub.try_next_message_pure() {
                status = new_status.0;
            }
            if let BatteryStatus::Available { level: Some(level), .. } = status {
                if let Err(e) = self.battery_level.notify(conn, &level, true).await {
                    error!("Failed to notify battery level: {:?}", e);
                } else {
                    self.first_report_sent = true;
                }
            }
            return;
        }

        let mut status = None;
        while let Some(new_status) = self.sub.try_next_message_pure() {
            status = Some(new_status.0);
        }

        let timeout_report_due = now >= self.next_timeout_report_at && !SLEEPING_STATE.load(Ordering::Acquire);
        if timeout_report_due {
            self.next_timeout_report_at = now + Duration::from_secs(1800);
        }

        let recent_key_activity = last_key_timestamp
            .map(|last_press| (now.as_secs() as u32).saturating_sub(last_press) < 60)
            .unwrap_or(false);

        if (status.is_some() && recent_key_activity) || timeout_report_due {
            let status = status.unwrap_or_else(crate::input_device::battery::current_battery_status);
            if let BatteryStatus::Available { level: Some(level), .. } = status
                && let Err(e) = self.battery_level.notify(conn, &level, true).await
            {
                error!("Failed to notify battery level: {:?}", e);
            }
        }
    }
}

/// This is a background task that is required to run forever alongside any other BLE tasks.
pub(crate) async fn ble_task<C: Controller + ControllerCmdAsync<LeSetPhy>, P: PacketPool>(
    mut runner: Runner<'_, C, P>,
) {
    loop {
        #[cfg(not(feature = "split"))]
        if let Err(_e) = runner.run().await {
            error!("[ble_task] runner.run() error");
            embassy_time::Timer::after_millis(100).await;
        }

        #[cfg(feature = "split")]
        {
            // Signal to indicate the stack is started
            crate::split::ble::central::STACK_STARTED.signal(true);
            if let Err(_e) = runner
                .run_with_handler(&crate::split::ble::central::ScanHandler {})
                .await
            {
                error!("[ble_task] runner.run_with_handler error");
                embassy_time::Timer::after_millis(100).await;
            }
        }
    }
}

/// Stream Events until the connection closes.
///
/// This function will handle the GATT events and process them.
/// This is how we interact with read and write requests.
async fn handle_gatt_event<'stack, 'server>(
    server: &Server<'_>,
    conn: &GattConnection<'stack, 'server, DefaultPacketPool>,
    slot_num: u8,
    generation: u32,
    #[cfg(feature = "passkey_entry")] passkey_state: &mut PasskeyInputState,
    event: GattConnectionEvent<'stack, 'server, DefaultPacketPool>,
) -> Result<GattEventOutcome, Error> {
    let level = server.battery_service.level;
    let output_keyboard = server.hid_service.output_keyboard;
    let hid_control_point = server.hid_service.hid_control_point;
    let input_keyboard = server.hid_service.input_keyboard;
    #[cfg(feature = "host")]
    let (output_host, input_host, host_control_point) = (
        server.host_service.output_data,
        server.host_service.input_data,
        server.host_service.hid_control_point,
    );
    let mouse = server.composite_service.mouse_report;
    let media = server.composite_service.media_report;
    let media_control_point = server.composite_service.hid_control_point;
    let system_control = server.composite_service.system_report;

    match event {
        GattConnectionEvent::Disconnected { reason } => {
            #[cfg(feature = "passkey_entry")]
            passkey_state.clear();
            info!("[gatt] disconnected: {:?}", reason);
            return Ok(GattEventOutcome::Disconnected);
        }
        GattConnectionEvent::PairingComplete { security_level, bond } => {
            #[cfg(feature = "passkey_entry")]
            passkey_state.clear();
            info!("[gatt] pairing complete: {:?}", security_level);
            if let Some(bond_info) = bond {
                let identity = bond_info.identity;
                if !security_level.encrypted()
                    || !bond_info.security_level.encrypted()
                    || !bond_info.is_bonded
                    || !is_usable_bond_identity(&bond_info.identity)
                {
                    warn!(
                        "[gatt] pairing bond is not usable for encrypted reconnect: {:?}",
                        bond_info.identity
                    );
                    conn.raw().disconnect();
                    return Ok(GattEventOutcome::InvalidPairing(Some(identity)));
                }
                let cccd_table = server
                    .get_client_att_table(conn.raw())
                    .and_then(|t| heapless::Vec::from_slice(t.raw()).ok())
                    .unwrap_or_default();
                let profile_info = ProfileInfo {
                    slot_num,
                    info: bond_info,
                    removed: false,
                    cccd_table,
                };
                if UPDATED_PROFILE
                    .try_send(ProfileInfoUpdate {
                        generation,
                        profile_info,
                    })
                    .is_err()
                {
                    warn!("[gatt] BLE profile update queue is full");
                    conn.raw().disconnect();
                    return Ok(GattEventOutcome::InvalidPairing(Some(identity)));
                }
            } else {
                warn!("[gatt] pairing completed without a persistent bond");
                conn.raw().disconnect();
                return Ok(GattEventOutcome::InvalidPairing(None));
            }
        }
        GattConnectionEvent::PairingFailed(err) => {
            #[cfg(feature = "passkey_entry")]
            passkey_state.clear();
            error!("[gatt] pairing error: {:?}", err);
            conn.raw().disconnect();
            return Ok(GattEventOutcome::InvalidPairing(None));
        }
        GattConnectionEvent::Encrypted { security_level, .. } => {
            info!("[gatt] encrypted: {:?}", security_level);
            return Ok(GattEventOutcome::Encrypted);
        }
        GattConnectionEvent::Gatt { event: gatt_event } => {
            let mut cccd_updated = false;
            let mut led_update = None;
            let result = match &gatt_event {
                GattEvent::Read(event) => {
                    if event.handle() == level.handle {
                        let value = server.get(&level);
                        debug!("Read GATT Event to Level: {:?}", value);
                    } else {
                        debug!("Read GATT Event to Unknown: {:?}", event.handle());
                    }

                    if conn.raw().security_level()?.encrypted() {
                        None
                    } else {
                        Some(AttErrorCode::INSUFFICIENT_ENCRYPTION)
                    }
                }
                GattEvent::Write(event) => {
                    if !conn.raw().security_level()?.encrypted() {
                        Some(AttErrorCode::INSUFFICIENT_ENCRYPTION)
                    } else {
                        #[cfg(feature = "host")]
                        let host_control_point_match = event.handle() == host_control_point.handle;
                        #[cfg(not(feature = "host"))]
                        let host_control_point_match = false;

                        let mut data_buf = [0u8; 32];
                        let data_len = event.with_data(|_, data| {
                            let n = data.len().min(data_buf.len());
                            data_buf[..n].copy_from_slice(&data[..n]);
                            data.len()
                        });
                        let data = &data_buf[..data_len.min(data_buf.len())];

                        if event.handle() == output_keyboard.handle {
                            if data_len == 1 {
                                let led_indicator = LedIndicator::from_bits(data[0]);
                                debug!("Got keyboard state: {:?}", led_indicator);
                                led_update = Some(led_indicator);
                            } else {
                                warn!("Wrong keyboard state data: {:?}", data);
                            }
                        } else if event.handle() == input_keyboard.cccd_handle.expect("No CCCD for input keyboard")
                            || event.handle() == mouse.cccd_handle.expect("No CCCD for mouse report")
                            || event.handle() == media.cccd_handle.expect("No CCCD for media report")
                            || event.handle() == system_control.cccd_handle.expect("No CCCD for system report")
                            || event.handle() == level.cccd_handle.expect("No CCCD for battery level")
                        {
                            cccd_updated = true;
                        } else if event.handle() == hid_control_point.handle
                            || event.handle() == media_control_point.handle
                            || host_control_point_match
                        {
                            info!("Write GATT Event to Control Point: {:?}", event.handle());
                            #[cfg(feature = "split")]
                            {
                                if data_len == 1 && crate::state::current_profile() == slot_num {
                                    match data[0] {
                                        0 => CENTRAL_SLEEP.signal(true),
                                        1 => CENTRAL_SLEEP.signal(false),
                                        _ => {}
                                    }
                                }
                            }
                        } else {
                            #[cfg(feature = "host")]
                            if event.handle() == output_host.handle {
                                debug!("Got host packet: {:?}", data);
                                if data_len == 32 && crate::state::current_profile() == slot_num {
                                    crate::channel::enqueue_host_request(
                                        crate::channel::HostRequestOrigin::Ble {
                                            slot: slot_num,
                                            generation,
                                        },
                                        data_buf,
                                    )
                                    .await;
                                } else if data_len == 32 {
                                    warn!("Ignoring host packet from inactive BLE profile {}", slot_num);
                                } else {
                                    warn!("Wrong host packet data: {:?}", data);
                                }
                            } else if event.handle() == input_host.cccd_handle.expect("No CCCD for input host") {
                                cccd_updated = true;
                            } else {
                                debug!("Write GATT Event to Unknown: {:?}", event.handle());
                            }
                            #[cfg(not(feature = "host"))]
                            debug!("Write GATT Event to Unknown: {:?}", event.handle());
                        }

                        None
                    }
                }
                GattEvent::Other(_) => None,
                GattEvent::NotAllowed(_) => None,
            };

            let result = if let Some(code) = result {
                gatt_event.reject(code)
            } else {
                gatt_event.accept()
            };
            match result {
                Ok(reply) => reply.send().await,
                Err(e) => warn!("[gatt] error sending response: {:?}", e),
            }

            if cccd_updated {
                #[cfg(feature = "split")]
                if crate::state::current_profile() == slot_num {
                    CENTRAL_SLEEP.signal(false);
                }

                if let Some(table) = server.get_client_att_table(conn.raw())
                    && let Ok(bytes) = heapless::Vec::from_slice(table.raw())
                {
                    if UPDATED_CCCD_TABLE
                        .try_send(ProfileCccdTable {
                            slot_num,
                            generation,
                            table: bytes,
                        })
                        .is_err()
                    {
                        warn!("[gatt] BLE CCCD update queue is full");
                    }
                }

                return Ok(GattEventOutcome::CccdUpdated);
            }

            if let Some(led_indicator) = led_update {
                return Ok(GattEventOutcome::Led(led_indicator));
            }
        }
        GattConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
            info!("[gatt] PhyUpdated: {:?}, {:?}", tx_phy, rx_phy)
        }
        GattConnectionEvent::ConnectionParamsUpdated {
            conn_interval,
            peripheral_latency,
            supervision_timeout,
        } => {
            info!(
                "[gatt] ConnectionParamsUpdated: {:?}ms, {:?}, {:?}ms",
                conn_interval.as_millis(),
                peripheral_latency,
                supervision_timeout.as_millis()
            );
        }
        GattConnectionEvent::RequestConnectionParams(req) => info!(
            "[gatt] RequestConnectionParams: interval: ({:?}, {:?})ms, {:?}, {:?}ms",
            req.params().min_connection_interval.as_millis(),
            req.params().max_connection_interval.as_millis(),
            req.params().max_latency,
            req.params().supervision_timeout.as_millis(),
        ),
        GattConnectionEvent::DataLengthUpdated {
            max_tx_octets,
            max_tx_time,
            max_rx_octets,
            max_rx_time,
        } => {
            info!(
                "[gatt] DataLengthUpdated: tx/rx octets: ({:?}, {:?}), tx/rx time: ({:?}, {:?})",
                max_tx_octets, max_rx_octets, max_tx_time, max_rx_time
            );
        }
        GattConnectionEvent::FrameSpaceUpdated {
            frame_space,
            initiator,
            phys,
            spacing_types,
        } => {
            info!(
                "[gatt] FrameSpaceUpdated: {:?}, {:?}, {:?}, {:?}",
                frame_space, initiator, phys, spacing_types
            );
        }
        GattConnectionEvent::ConnectionRateChanged {
            conn_interval,
            subrate_factor,
            peripheral_latency,
            continuation_number,
            supervision_timeout,
        } => {
            info!(
                "[gatt] ConnectionRateChanged: {:?}ms, {:?}, {:?}, {:?}, {:?}ms",
                conn_interval.as_millis(),
                subrate_factor,
                peripheral_latency,
                continuation_number,
                supervision_timeout.as_millis()
            );
        }
        GattConnectionEvent::PassKeyDisplay(pass_key) => info!("[gatt] PassKeyDisplay: {:?}", pass_key),
        GattConnectionEvent::PassKeyConfirm(pass_key) => info!("[gatt] PassKeyConfirm: {:?}", pass_key),
        GattConnectionEvent::PassKeyInput => {
            #[cfg(feature = "passkey_entry")]
            if crate::PASSKEY_ENTRY_ENABLED {
                info!("[gatt] PassKeyInput: entering passkey entry mode");
                passkey_state.begin();
            } else {
                warn!("[gatt] PassKeyInput: disabled in config, cancelling pairing, this shouldn't happen");
                if let Err(e) = conn.raw().pass_key_cancel() {
                    error!("[gatt] pass_key_cancel error: {:?}", e);
                }
            }
            #[cfg(not(feature = "passkey_entry"))]
            warn!("[gatt] PassKeyInput event, should not happen")
        }
        GattConnectionEvent::BondLost => {
            warn!("[gatt] BondLost");
            conn.raw().disconnect();
            return Ok(GattEventOutcome::BondLost);
        }
        GattConnectionEvent::OobRequest => warn!("[gatt] OobRequest"),
    }

    Ok(GattEventOutcome::None)
}

fn is_usable_bond_identity(identity: &Identity) -> bool {
    if identity.addr.kind.as_raw() & 1 == 0 {
        return true;
    }

    let addr = identity.addr.addr.into_inner();
    match addr[5] & 0b1100_0000 {
        0b1100_0000 => true,
        0b0100_0000 => identity.irk.is_some(),
        _ => false,
    }
}

async fn start_advertising<'a, C: Controller + ControllerCmdAsync<LeSetPhy>>(
    name: &'a str,
    stack: &Stack<'_, C, DefaultPacketPool>,
    peripheral: &mut Peripheral<'a, C, DefaultPacketPool>,
    mode: &BleAdvertisingMode,
    profile_manager: &ProfileManager<'_, '_, C, DefaultPacketPool>,
) -> Result<Advertiser<'a, C, DefaultPacketPool>, BleHostError<C::Error>>
where
    C: ControllerCmdSync<LeReadFilterAcceptListSize> + ControllerCmdSync<LeSetScanResponseData>,
{
    // Wait for 10ms to ensure the USB is checked
    embassy_time::Timer::after_millis(10).await;
    if !BLE_PROFILE_CHANNEL.is_empty() {
        return Err(BleHostError::BleHost(Error::Busy));
    }
    let mut advertiser_data = [0; 31];
    let mut scan_data = [0; 31];
    let (advertiser_data_len, scan_data_len) = if mode.is_visible() {
        let appearance = KEYBOARD.to_le_bytes();
        let services = [BATTERY.to_le_bytes(), HUMAN_INTERFACE_DEVICE.to_le_bytes()];
        let base_ad = [
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids16(&services),
            AdStructure::Unknown {
                ty: 0x19,
                data: &appearance,
            },
        ];
        let complete_ad = [
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids16(&services),
            AdStructure::CompleteLocalName(name.as_bytes()),
            AdStructure::Unknown {
                ty: 0x19,
                data: &appearance,
            },
        ];
        match AdStructure::encode_slice(&complete_ad, &mut advertiser_data[..]) {
            Ok(len) => (len, 0),
            Err(_) => {
                let advertiser_data_len = AdStructure::encode_slice(&base_ad, &mut advertiser_data[..])?;
                let scan_data_len = match AdStructure::encode_slice(
                    &[AdStructure::CompleteLocalName(name.as_bytes())],
                    &mut scan_data[..],
                ) {
                    Ok(len) => len,
                    Err(_) => {
                        let shortened_len = name.len().min(29);
                        AdStructure::encode_slice(
                            &[AdStructure::ShortenedLocalName(&name.as_bytes()[..shortened_len])],
                            &mut scan_data[..],
                        )?
                    }
                };
                (advertiser_data_len, scan_data_len)
            }
        }
    } else {
        (
            AdStructure::encode_slice(&[AdStructure::Flags(BR_EDR_NOT_SUPPORTED)], &mut advertiser_data[..])?,
            0,
        )
    };

    let mut filter_accept_list = heapless::Vec::<Address, { crate::NUM_BLE_PROFILE }>::new();
    if let BleAdvertisingMode::HiddenReconnect { slots, .. } = mode {
        let filter_accept_list_size = stack.command(LeReadFilterAcceptListSize::new()).await?;
        for slot in slots {
            if filter_accept_list.len() >= filter_accept_list_size as usize {
                warn!(
                    "Controller BLE filter accept list is full, skipping hidden reconnect target slot {}",
                    slot
                );
                break;
            }
            if let Some(info) = profile_manager.bond_info(*slot) {
                if !is_usable_bond_identity(&info.info.identity) {
                    warn!("Skipping BLE slot {} with unusable persisted identity", slot);
                    continue;
                }
                if filter_accept_list.push(info.info.identity.addr).is_err() {
                    warn!("Too many BLE filter accept list entries");
                    break;
                }
            }
        }
    }

    peripheral.set_filter_accept_list(&filter_accept_list).await?;
    stack.command(LeSetScanResponseData::new(0, [0; 31])).await?;

    let (interval, filter_policy) = match mode {
        BleAdvertisingMode::VisiblePairing { .. } => (Duration::from_millis(200), AdvFilterPolicy::Unfiltered),
        BleAdvertisingMode::HiddenReconnect { fast, .. } => (
            if *fast {
                Duration::from_millis(20)
            } else {
                Duration::from_millis(1000)
            },
            AdvFilterPolicy::FilterConnAndScan,
        ),
    };

    let advertise_config = AdvertisementParameters {
        primary_phy: PhyKind::Le2M,
        secondary_phy: PhyKind::Le2M,
        tx_power: TxPower::Plus8dBm,
        interval_min: interval,
        interval_max: interval,
        filter_policy,
        ..Default::default()
    };

    info!("[adv] advertising, visible: {}", mode.is_visible());
    peripheral
        .advertise(
            &advertise_config,
            Advertisement::ConnectableScannableUndirected {
                adv_data: &advertiser_data[..advertiser_data_len],
                scan_data: &scan_data[..scan_data_len],
            },
        )
        .await
}

// Update the PHY to 2M
pub(crate) async fn update_ble_phy<P: PacketPool>(
    stack: &Stack<'_, impl Controller + ControllerCmdAsync<LeSetPhy>, P>,
    conn: &Connection<'_, P>,
) {
    loop {
        match conn.set_phy(stack, PhyKind::Le2M).await {
            Err(BleHostError::BleHost(Error::Hci(error))) => {
                if 0x2A == error.to_status().into_inner() {
                    // Busy, retry
                    info!("[update_ble_phy] HCI busy: {:?}", error);
                    continue;
                } else {
                    error!("[update_ble_phy] HCI error: {:?}", error);
                }
            }
            Err(e) => {
                #[cfg(feature = "defmt")]
                let e = defmt::Debug2Format(&e);
                error!("[update_ble_phy] error: {:?}", e);
            }
            Ok(_) => {
                info!("[update_ble_phy] PHY updated");
            }
        }
        break;
    }
}

// Update the connection parameters
pub(crate) async fn update_conn_params<
    'a,
    'b,
    C: Controller + ControllerCmdSync<LeReadLocalSupportedFeatures>,
    P: PacketPool,
>(
    stack: &Stack<'a, C, P>,
    conn: &Connection<'b, P>,
    params: &RequestedConnParams,
) {
    loop {
        match conn.update_connection_params(stack, params).await {
            Err(BleHostError::BleHost(Error::Hci(error))) => {
                if 0x3A == error.to_status().into_inner() {
                    // Busy, retry
                    info!("[update_conn_params] HCI busy: {:?}", error);
                    embassy_time::Timer::after_millis(100).await;
                    continue;
                } else {
                    error!("[update_conn_params] HCI error: {:?}", error);
                }
            }
            Err(e) => {
                #[cfg(feature = "defmt")]
                let e = defmt::Debug2Format(&e);
                error!("[update_conn_params] BLE host error: {:?}", e);
            }
            _ => (),
        }
        break;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use embassy_futures::join::join;
    use embassy_futures::select::select;
    use embassy_time::Timer;
    use rmk_types::ble::{BleState, BleStatus};
    use trouble_host::prelude::{AddrKind, Address, BdAddr, Identity, IdentityResolvingKey};

    use super::is_usable_bond_identity;
    use crate::event::{Axis, AxisEvent, AxisValType, KeyboardEvent, PointingEvent, SubscribableEvent, publish_event};
    use crate::state::{current_ble_status, set_ble_profile, set_ble_state};
    use crate::test_support::test_block_on as block_on;

    fn ble_status_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn set_ble_state_preserves_current_profile() {
        let _guard = ble_status_test_lock().lock().unwrap();

        set_ble_profile(2);
        set_ble_state(BleState::Advertising);

        assert_eq!(
            current_ble_status(),
            BleStatus {
                profile: 2,
                state: BleState::Advertising,
            }
        );
    }

    #[test]
    fn set_ble_profile_clears_unknown_slot_state() {
        let _guard = ble_status_test_lock().lock().unwrap();

        set_ble_profile(1);
        set_ble_state(BleState::Connected);
        set_ble_profile(3);

        assert_eq!(
            current_ble_status(),
            BleStatus {
                profile: 3,
                state: BleState::Inactive,
            }
        );
    }

    fn identity(kind: AddrKind, random_type: u8, with_irk: bool) -> Identity {
        let mut address = [0u8; 6];
        address[5] = random_type;
        Identity {
            addr: Address::new(kind, BdAddr::new(address)),
            irk: with_irk.then(|| IdentityResolvingKey::new(1).unwrap()),
        }
    }

    #[test]
    fn bond_identity_requires_a_stable_or_resolvable_address() {
        assert!(is_usable_bond_identity(&identity(AddrKind::PUBLIC, 0, false)));
        assert!(is_usable_bond_identity(&identity(AddrKind::RANDOM, 0b1100_0000, false)));
        assert!(!is_usable_bond_identity(&identity(
            AddrKind::RANDOM,
            0b0100_0000,
            false
        )));
        assert!(is_usable_bond_identity(&identity(AddrKind::RANDOM, 0b0100_0000, true)));
        assert!(!is_usable_bond_identity(&identity(AddrKind::RANDOM, 0, true)));
        assert!(!is_usable_bond_identity(&identity(AddrKind::RANDOM, 0b1000_0000, true)));
    }

    #[test]
    fn wake_activity_includes_pointing_events() {
        let _guard = ble_status_test_lock().lock().unwrap();

        block_on(async {
            let wake = async {
                let mut key_wake = KeyboardEvent::subscriber();
                let mut pointing_wake = PointingEvent::subscriber();
                let _ = select(key_wake.next_message_pure(), pointing_wake.next_message_pure()).await;
            };
            join(wake, async {
                Timer::after_millis(1).await;
                publish_event(PointingEvent {
                    device_id: 0,
                    axes: [
                        AxisEvent {
                            typ: AxisValType::Rel,
                            axis: Axis::X,
                            value: 1,
                        },
                        AxisEvent {
                            typ: AxisValType::Rel,
                            axis: Axis::Y,
                            value: 0,
                        },
                        AxisEvent {
                            typ: AxisValType::Rel,
                            axis: Axis::Z,
                            value: 0,
                        },
                    ],
                })
            })
            .await;
        });
    }
}
