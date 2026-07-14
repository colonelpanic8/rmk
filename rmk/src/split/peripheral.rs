#[cfg(rmk_ble)]
use bt_hci::{cmd::le::LeSetPhy, controller::ControllerCmdAsync};
use embassy_futures::select::{Either, select};
#[cfg(not(rmk_ble))]
use embedded_io_async::{Read, Write};
use futures::FutureExt;
#[cfg(all(rmk_ble, rmk_storage))]
use {super::ble::PeerAddress, crate::channel::FLASH_CHANNEL};
#[cfg(rmk_ble)]
use {
    crate::event::{BatteryStatusEvent, ChargingStateEvent, EventSubscriber},
    rmk_types::battery::BatteryStatus,
    trouble_host::prelude::*,
};

use super::SplitMessage;
use super::driver::{SplitReader, SplitWriter};
use crate::event::{
    KeyboardEvent, LayerChangeEvent, LedIndicatorEvent, PointingEvent, SubscribableEvent, publish_event,
};
#[cfg(rmk_display)]
use crate::event::{ModifierEvent, SleepStateEvent, WpmUpdateEvent};
#[cfg(not(rmk_ble))]
use crate::split::serial::SerialSplitDriver;
use crate::state::update_status;

/// Run the split peripheral service.
///
/// # Arguments
///
/// * `id` - (optional) The id of the peripheral
/// * `stack` - (optional) The TrouBLE stack
/// * `serial` - (optional) serial port used to send peripheral split message. This argument is enabled only for serial split now
/// * `storage` - (optional) The storage to save the central address
#[allow(clippy::extra_unused_lifetimes)]
pub async fn run_rmk_split_peripheral<
    'b,
    's,
    #[cfg(rmk_ble)] C: Controller + ControllerCmdAsync<LeSetPhy>,
    #[cfg(not(rmk_ble))] S: Write + Read,
>(
    #[cfg(rmk_ble)] id: usize,
    #[cfg(rmk_ble)] stack: &'b Stack<'s, C, DefaultPacketPool>,
    #[cfg(not(rmk_ble))] serial: S,
) where
    's: 'b,
{
    #[cfg(not(rmk_ble))]
    {
        let mut peripheral = SplitPeripheral::new(SerialSplitDriver::new(serial));
        loop {
            peripheral.run().await;
        }
    }

    #[cfg(rmk_ble)]
    crate::split::ble::peripheral::initialize_nrf_ble_split_peripheral_and_run(id, stack).await;
}

/// The split peripheral instance.
pub(crate) struct SplitPeripheral<S: SplitWriter + SplitReader> {
    split_driver: S,
}

impl<S: SplitWriter + SplitReader> SplitPeripheral<S> {
    pub(crate) fn new(split_driver: S) -> Self {
        Self { split_driver }
    }

    /// Run the peripheral keyboard service.
    ///
    /// The peripheral uses the general matrix, does scanning and send the key events through `SplitWriter`.
    /// If also receives split messages from the central through `SplitReader`.
    pub(crate) async fn run(&mut self) {
        let mut key_sub = KeyboardEvent::subscriber();
        #[cfg(rmk_ble)]
        let mut charging_state_sub = ChargingStateEvent::subscriber();
        let mut pointing_sub = PointingEvent::subscriber();
        #[cfg(rmk_ble)]
        let mut battery_sub = BatteryStatusEvent::subscriber();

        loop {
            let read_message_to_send = async {
                crate::select_biased_with_cfg! {
                    e = key_sub.next_message_pure().fuse() => SplitMessage::Key(e),
                    with_cfg(rmk_ble): e = charging_state_sub.next_message_pure().fuse() => {
                        SplitMessage::BatteryStatus(BatteryStatus::Available {
                            charge_state: e.charging.into(),
                            level: None,
                        }.into())
                    },
                    e = pointing_sub.next_message_pure().fuse() => SplitMessage::Pointing(e),
                    with_cfg(rmk_ble): e = battery_sub.next_event().fuse() => SplitMessage::BatteryStatus(e),
                }
            };

            match select(self.split_driver.read(), read_message_to_send).await {
                Either::First(m) => match m {
                    // Process split messages from the central
                    Ok(split_message) => match split_message {
                        SplitMessage::ConnectionStatus(status) => {
                            trace!("Received central connection status: {:?}", status);
                            update_status(|c| *c = status);
                        }
                        #[cfg(all(rmk_ble, rmk_storage))]
                        SplitMessage::ClearPeer => {
                            // Clear the peer address
                            FLASH_CHANNEL
                                .send(crate::storage::FlashOperationMessage::PeerAddress(PeerAddress::new(
                                    0, false, [0; 6],
                                )))
                                .await;
                        }
                        SplitMessage::KeyboardIndicator(indicator) => {
                            // Publish KeyboardIndicator event
                            publish_event(LedIndicatorEvent::new(
                                rmk_types::led_indicator::LedIndicator::from_bits(indicator),
                            ));
                        }
                        SplitMessage::Layer(layer) => {
                            // Publish Layer event
                            publish_event(LayerChangeEvent::new(layer));
                        }
                        #[cfg(rmk_display)]
                        SplitMessage::Wpm(wpm) => {
                            publish_event(WpmUpdateEvent::new(wpm));
                        }
                        #[cfg(rmk_display)]
                        SplitMessage::Modifier(bits) => {
                            publish_event(ModifierEvent {
                                modifier: rmk_types::modifier::ModifierCombination::from_bits(bits),
                            });
                        }
                        #[cfg(rmk_display)]
                        SplitMessage::SleepState(sleeping) => {
                            publish_event(SleepStateEvent::new(sleeping));
                        }
                        _ => (),
                    },
                    Err(e) => {
                        error!("Split message read error: {:?}", e);
                        if let crate::split::driver::SplitDriverError::Disconnected = e {
                            break;
                        }
                    }
                },
                Either::Second(e) => {
                    debug!("Writing split message {:?} to central", e);
                    self.split_driver.write(&e).await.ok();
                }
            }
        }
    }
}
