//! Split-transport payloads: the wired/BLE selector snapshot and its
//! volatile force control, for boards with an automatic split transport.

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

/// Requested split-transport selection policy. `Auto` returns control to the
/// cable-detect input; `Wired`/`Ble` pin the transport until the next force
/// or reboot (the force is volatile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub enum SplitTransportForce {
    Auto,
    Wired,
    Ble,
}

/// Snapshot of the split-transport selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct SplitTransportState {
    /// The board runs an automatic wired/BLE selection policy. When false
    /// the remaining fields carry no information.
    pub auto: bool,
    /// The volatile forced selection currently applied.
    pub forced: SplitTransportForce,
    /// The debounced cable-detect input, independent of any force.
    pub cable_detected: bool,
    /// The transport the split link currently selects.
    pub wired_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rynk::tests::{assert_max_size_bound, round_trip};

    #[test]
    fn round_trip_split_transport_state() {
        let state = SplitTransportState {
            auto: true,
            forced: SplitTransportForce::Ble,
            cable_detected: true,
            wired_active: false,
        };
        round_trip(&state);
        assert_max_size_bound(&state);
        round_trip(&SplitTransportForce::Wired);
    }
}
