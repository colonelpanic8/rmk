pub mod central;
pub mod peripheral;

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

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
