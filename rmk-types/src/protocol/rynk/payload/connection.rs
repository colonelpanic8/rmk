//! Connection-management endpoint types.

use heapless::String;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

/// Maximum UTF-8 byte length of the name included in legacy BLE advertising.
///
/// The 31-byte advertising payload also carries flags, HID/battery service
/// UUIDs, and the keyboard appearance, leaving 16 bytes for the local name.
pub const BLE_NAME_MAX_LEN: usize = 16;

/// Runtime BLE name template.
///
/// Every `{slot}` token is replaced with the active profile's human-facing,
/// one-based slot number before advertising. A template without the token is
/// a fixed name for every profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct BleName {
    #[cfg_attr(feature = "wasm", tsify(type = "string"))]
    pub template: String<BLE_NAME_MAX_LEN>,
}

impl MaxSize for BleName {
    const POSTCARD_MAX_SIZE: usize = crate::heapless_vec_max_size::<u8, BLE_NAME_MAX_LEN>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rynk::tests::{assert_max_size_bound, round_trip};

    #[test]
    fn round_trip_ble_name() {
        let name = BleName {
            template: String::try_from("Glove80 {slot}").unwrap(),
        };
        round_trip(&name);
        assert_max_size_bound(&name);

        let max = BleName {
            template: String::try_from("1234567890abcdef").unwrap(),
        };
        round_trip(&max);
        assert_max_size_bound(&max);
    }
}
