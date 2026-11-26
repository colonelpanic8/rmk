//! BLE status types and the limits an identity string has to clear.

use heapless::String;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

/// Longest name a legacy advertisement can carry: flags (3), the battery + HID service
/// UUIDs (6) and the appearance (4) claim the rest of its 31 bytes.
pub const BLE_ADV_NAME_MAX_LEN: usize = 16;

/// Connection and advertising state of one BLE host or dongle profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub enum BleState {
    /// The profile is advertising.
    Advertising,
    /// The profile has an encrypted connection that can carry input.
    Connected,
    /// The profile has no connection ready for input.
    #[default]
    Inactive,
}

/// Connection state and bond of one BLE host or dongle profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct BleStatus {
    pub state: BleState,
    /// Whether the profile holds a bond, independent of connection state. False while it re-pairs.
    pub bonded: bool,
}

/// Maximum UTF-8 byte length of the name included in legacy BLE advertising.
///
/// The 31-byte advertising payload also carries flags, HID/battery service
/// UUIDs, and the keyboard appearance, leaving 16 bytes for the local name.
pub const BLE_NAME_MAX_LEN: usize = BLE_ADV_NAME_MAX_LEN;

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
