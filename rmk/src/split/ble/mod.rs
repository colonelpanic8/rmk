pub mod central;
pub mod peripheral;

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

// A peripheral briefly prefers its remembered central before falling back to
// discoverable advertising, so a central that lost or never had the address
// can still find it by payload.
pub(super) const DIRECTED_ADVERTISING_GRACE_MS: u64 = 750;
// While a peripheral's address is known, the central keeps a connection
// request armed so the peripheral's first advertisement after power-on is
// caught immediately. Re-arming on this period picks up refreshed connection
// parameters (e.g. a changed latency policy); a timeout does not invalidate
// the address, which on nRF is FICR-derived and stable for the device's
// lifetime. Only an empty slot (fresh storage) triggers scanning.
pub(super) const KNOWN_PEER_CONNECT_REARM_MS: u64 = 30_000;

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
