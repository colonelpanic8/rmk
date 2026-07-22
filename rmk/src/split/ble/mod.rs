pub mod central;
pub mod peripheral;

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};
use trouble_host::types::gatt_traits::AsGatt;

use super::SplitMessage;
use super::driver::SplitDriverError;

// A peripheral briefly prefers its remembered central before falling back to
// discoverable advertising. Keep that grace period shorter than the central's
// remembered-peer connection attempt: otherwise a missed startup rendezvous
// leaves the central scanning for advertisements that the peripheral will not
// emit until several seconds later.
pub(super) const DIRECTED_ADVERTISING_GRACE_MS: u64 = 750;
pub(super) const KNOWN_PEER_CONNECT_TIMEOUT_MS: u64 = 1_500;
const _: [(); 1] = [(); (DIRECTED_ADVERTISING_GRACE_MS < KNOWN_PEER_CONNECT_TIMEOUT_MS) as usize];

#[derive(Clone, Debug, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PeerAddress {
    pub peer_id: u8,
    pub is_valid: bool,
    pub address: [u8; 6],
}

impl PeerAddress {
    pub(crate) fn new(peer_id: u8, is_valid: bool, address: [u8; 6]) -> Self {
        Self {
            peer_id,
            is_valid,
            address,
        }
    }
}

#[derive(Default, Clone)]
pub(crate) struct GattSplitMessage {
    buf: [u8; SplitMessage::POSTCARD_MAX_SIZE],
    len: usize,
}

impl TryFrom<&SplitMessage> for GattSplitMessage {
    type Error = SplitDriverError;

    fn try_from(value: &SplitMessage) -> Result<Self, Self::Error> {
        let mut buf = [0; SplitMessage::POSTCARD_MAX_SIZE];
        let encoded = postcard::to_slice(value, &mut buf).map_err(|e| {
            error!("Postcard serialize split message error: {}", e);
            SplitDriverError::SerializeError
        })?;

        let len = encoded.len();

        // Check if slice starts at the beginning of buffer
        if encoded.as_ptr() != buf.as_ptr() {
            error!("Postcard serialize split message did not use the buffer correctly!");
            return Err(SplitDriverError::SerializeError);
        }

        Ok(Self { buf, len })
    }
}

impl AsGatt for GattSplitMessage {
    const MIN_SIZE: usize = 0;

    const MAX_SIZE: usize = SplitMessage::POSTCARD_MAX_SIZE;

    fn as_gatt(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
