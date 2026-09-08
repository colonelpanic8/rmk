//! Connection handlers — USB/BLE preferred-transport read, BLE profile management.

#[cfg(feature = "_ble")]
use rmk_types::ble::BleStatus;
use rmk_types::connection::{ConnectionStatus, ConnectionType};
#[cfg(feature = "_ble")]
use rmk_types::protocol::rynk::BleName;
use rmk_types::protocol::rynk::RynkError;
#[cfg(feature = "_ble")]
use rmk_types::protocol::rynk::command::{ClearBleProfile, GetBleName, GetBleStatus, SetBleName, SwitchBleProfile};
use rmk_types::protocol::rynk::command::{GetConnectionStatus, GetConnectionType};
#[cfg(all(feature = "_ble", feature = "split"))]
use rmk_types::protocol::rynk::command::{GetSplitCentralLatency, SetSplitCentralLatency};
#[cfg(feature = "split")]
use rmk_types::protocol::rynk::command::{GetSplitTransport, SetSplitTransportForce};
#[cfg(all(feature = "_ble", feature = "split"))]
use rmk_types::protocol::rynk::{SplitCentralLatencyPolicy, SplitCentralLatencyState};
#[cfg(feature = "split")]
use rmk_types::protocol::rynk::{SplitTransportForce, SplitTransportState};

use super::super::RynkService;
use super::Handle;

#[cfg(all(feature = "_ble", feature = "split"))]
impl From<crate::split::ble::central::LatencyPolicy> for SplitCentralLatencyPolicy {
    fn from(value: crate::split::ble::central::LatencyPolicy) -> Self {
        Self {
            powered: value.powered,
            battery: value.battery,
            override_latency: value.override_latency,
        }
    }
}

#[cfg(all(feature = "_ble", feature = "split"))]
impl From<SplitCentralLatencyPolicy> for crate::split::ble::central::LatencyPolicy {
    fn from(value: SplitCentralLatencyPolicy) -> Self {
        Self {
            powered: value.powered,
            battery: value.battery,
            override_latency: value.override_latency,
        }
    }
}

#[cfg(all(feature = "_ble", feature = "split"))]
impl From<crate::split::ble::central::LatencyState> for SplitCentralLatencyState {
    fn from(value: crate::split::ble::central::LatencyState) -> Self {
        Self {
            policy: value.policy.into(),
            powered: value.powered,
            effective: value.effective,
        }
    }
}

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

#[cfg(all(feature = "_ble", feature = "split"))]
impl Handle<GetSplitCentralLatency> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<SplitCentralLatencyState, RynkError> {
        Ok(crate::split::ble::central::latency_state().into())
    }
}

#[cfg(all(feature = "_ble", feature = "split"))]
impl Handle<SetSplitCentralLatency> for RynkService<'_> {
    async fn handle(&self, policy: SplitCentralLatencyPolicy) -> Result<SplitCentralLatencyState, RynkError> {
        crate::split::ble::central::set_latency_policy(policy.into()).map_err(|_| RynkError::Invalid)?;
        Ok(crate::split::ble::central::latency_state().into())
    }
}

#[cfg(feature = "_ble")]
impl Handle<GetBleStatus> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<BleStatus, RynkError> {
        Ok(self.ctx.connection_status().ble)
    }
}

/// `Cmd::SwitchBleProfile` — payload is the slot index. `try_send` is
/// used so a host hammering this Cmd while the previous switch is still
/// running observes the queue-full error rather than blocking the
/// dispatch loop.
#[cfg(feature = "_ble")]
impl Handle<SwitchBleProfile> for RynkService<'_> {
    async fn handle(&self, slot: u8) -> Result<(), RynkError> {
        Self::check_ble_profile_slot(slot)?;
        crate::channel::BLE_PROFILE_CHANNEL
            .try_send(crate::ble::profile::BleProfileAction::Switch(slot))
            .map_err(|_| RynkError::NotReady)
    }
}

/// `Cmd::ClearBleProfile` — wipes the bond at the given slot without
/// requiring a prior switch (uses [`BleProfileAction::ClearSlot`]).
#[cfg(feature = "_ble")]
impl Handle<ClearBleProfile> for RynkService<'_> {
    async fn handle(&self, slot: u8) -> Result<(), RynkError> {
        Self::check_ble_profile_slot(slot)?;
        crate::channel::BLE_PROFILE_CHANNEL
            .try_send(crate::ble::profile::BleProfileAction::ClearSlot(slot))
            .map_err(|_| RynkError::NotReady)
    }
}

#[cfg(feature = "_ble")]
impl Handle<GetBleName> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<BleName, RynkError> {
        Ok(crate::ble::name::current())
    }
}

#[cfg(feature = "_ble")]
impl Handle<SetBleName> for RynkService<'_> {
    async fn handle(&self, value: BleName) -> Result<(), RynkError> {
        crate::ble::name::set(value.clone()).map_err(|_| RynkError::Invalid)?;
        crate::channel::FLASH_CHANNEL
            .send(crate::storage::FlashOperationMessage::BleName(value))
            .await;
        Ok(())
    }
}

#[cfg(feature = "_ble")]
impl RynkService<'_> {
    /// `Invalid` for a BLE profile slot past the configured profile count.
    fn check_ble_profile_slot(slot: u8) -> Result<(), RynkError> {
        if (slot as usize) >= crate::NUM_BLE_PROFILE {
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
        if !crate::split::request_transport_force(mode) {
            return Err(RynkError::NotReady);
        }
        Ok(split_transport_state())
    }
}
