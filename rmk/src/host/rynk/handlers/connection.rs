//! Connection handlers — USB/BLE preferred-transport read, BLE profile management.

use rmk_types::connection::{ConnectionStatus, ConnectionType};
use rmk_types::protocol::rynk::RynkError;
#[cfg(feature = "_ble")]
use rmk_types::protocol::rynk::command::{ClearBleProfile, DisconnectBleProfile, SwitchBleProfile};
use rmk_types::protocol::rynk::command::{GetConnectionStatus, GetConnectionType};
#[cfg(feature = "split")]
use rmk_types::protocol::rynk::command::{GetSplitTransport, SetSplitTransportForce};
#[cfg(feature = "split")]
use rmk_types::protocol::rynk::{SplitTransportForce, SplitTransportState};

use super::super::RynkService;
use super::Handle;

impl Handle<GetConnectionType> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<ConnectionType, RynkError> {
        Ok(self.ctx.preferred_connection())
    }
}

/// `Cmd::GetConnectionStatus` — the same payload the `ConnectionChange`
/// topic pushes, so a host can recover a missed push.
impl Handle<GetConnectionStatus> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<ConnectionStatus, RynkError> {
        Ok(self.ctx.connection_status())
    }
}

/// `Cmd::SwitchBleProfile` — payload is the profile index. `try_send` is
/// used so a host hammering this Cmd while the previous switch is still
/// running observes the queue-full error rather than blocking the
/// dispatch loop.
#[cfg(feature = "_ble")]
impl Handle<SwitchBleProfile> for RynkService<'_> {
    async fn handle(&self, profile: u8) -> Result<(), RynkError> {
        Self::check_ble_profile(profile)?;
        crate::channel::BLE_PROFILE_CHANNEL
            .try_send(crate::ble::profile::BleProfileAction::Switch(profile))
            .map_err(|_| RynkError::NotReady)
    }
}

/// `Cmd::ClearBleProfile` — disconnects the given profile and wipes its bond
/// without requiring a prior switch.
#[cfg(feature = "_ble")]
impl Handle<ClearBleProfile> for RynkService<'_> {
    async fn handle(&self, profile: u8) -> Result<(), RynkError> {
        Self::check_ble_profile(profile)?;
        crate::channel::BLE_PROFILE_CHANNEL
            .try_send(crate::ble::profile::BleProfileAction::ClearBond(Some(profile)))
            .map_err(|_| RynkError::NotReady)
    }
}

/// `Cmd::DisconnectBleProfile` — disconnects the given profile and keeps its bond;
/// the selected profile then waits for a new host.
#[cfg(feature = "_ble")]
impl Handle<DisconnectBleProfile> for RynkService<'_> {
    async fn handle(&self, profile: u8) -> Result<(), RynkError> {
        Self::check_ble_profile(profile)?;
        crate::channel::BLE_PROFILE_CHANNEL
            .try_send(crate::ble::profile::BleProfileAction::Disconnect(profile))
            .map_err(|_| RynkError::NotReady)
    }
}

#[cfg(feature = "_ble")]
impl RynkService<'_> {
    /// `Invalid` for a profile past the host and dongle profiles.
    fn check_ble_profile(profile: u8) -> Result<(), RynkError> {
        if (profile as usize) >= crate::TOTAL_BLE_PROFILES {
            Err(RynkError::Invalid)
        } else {
            Ok(())
        }
    }
}

#[cfg(feature = "split")]
fn split_transport_state() -> SplitTransportState {
    use crate::split::selector;
    SplitTransportState {
        auto: selector::auto_enabled(),
        forced: match selector::forced_mode() {
            selector::FORCE_WIRED => SplitTransportForce::Wired,
            selector::FORCE_BLE => SplitTransportForce::Ble,
            _ => SplitTransportForce::Auto,
        },
        cable_detected: selector::detected_wired(),
        wired_active: selector::auto_enabled() && selector::wired_selected(),
    }
}

/// `Cmd::GetSplitTransport` — selector snapshot; meaningful only on boards
/// with an automatic split transport (`auto` is false otherwise).
#[cfg(feature = "split")]
impl Handle<GetSplitTransport> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<SplitTransportState, RynkError> {
        Ok(split_transport_state())
    }
}

/// `Cmd::SetSplitTransportForce` — volatile transport force. While a
/// peripheral is connected the force travels to it over the split link
/// first and the central applies it only after that send, so both halves
/// rendezvous on the forced transport; with no peripheral connected it
/// applies locally at once so a stranded central can still be steered.
/// The echoed state reads the selector immediately and may still show the
/// pre-force selection while the handoff is in flight — poll
/// `GetSplitTransport` for the settled state.
#[cfg(feature = "split")]
impl Handle<SetSplitTransportForce> for RynkService<'_> {
    async fn handle(&self, force: SplitTransportForce) -> Result<SplitTransportState, RynkError> {
        use crate::split::selector;
        if !selector::auto_enabled() {
            return Err(RynkError::Invalid);
        }
        let mode = match force {
            SplitTransportForce::Auto => selector::FORCE_AUTO,
            SplitTransportForce::Wired => selector::FORCE_WIRED,
            SplitTransportForce::Ble => selector::FORCE_BLE,
        };
        let connected = (0..crate::NUM_SPLIT_PERIPHERALS)
            .any(|id| crate::split::driver::current_peripheral_status(id).is_some_and(|s| s.connected));
        if connected {
            crate::channel::SPLIT_TRANSPORT_FORCE_CHANNEL
                .try_send(mode)
                .map_err(|_| RynkError::NotReady)?;
        } else {
            selector::set_forced(mode);
        }
        Ok(split_transport_state())
    }
}
