//! Persistent logical-layer metadata.

use heapless::String;
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

/// Maximum UTF-8 byte length of a layer name.
pub const LAYER_NAME_MAX_LEN: usize = 32;

/// The device-backed logical state of one fixed-capacity firmware layer slot.
///
/// Vacant slots keep their physical storage and keymap capacity. Hosts compact
/// occupied slots when deleting or reordering layers and clear the trailing
/// physical slot instead of changing the firmware's layer count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct LayerMetadata {
    pub occupied: bool,
    #[cfg_attr(feature = "wasm", tsify(type = "string"))]
    pub name: String<LAYER_NAME_MAX_LEN>,
}

impl LayerMetadata {
    pub fn vacant() -> Self {
        Self {
            occupied: false,
            name: String::new(),
        }
    }
}

impl MaxSize for LayerMetadata {
    const POSTCARD_MAX_SIZE: usize = bool::POSTCARD_MAX_SIZE + crate::heapless_vec_max_size::<u8, LAYER_NAME_MAX_LEN>();
}

/// Replace one layer slot's persistent metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct SetLayerMetadataRequest {
    pub layer: u8,
    pub metadata: LayerMetadata,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::rynk::tests::{assert_max_size_bound, round_trip};

    #[test]
    fn layer_metadata_round_trips() {
        let occupied = LayerMetadata {
            occupied: true,
            name: String::try_from("Navigation 🧭").unwrap(),
        };
        round_trip(&occupied);
        assert_max_size_bound(&occupied);

        let vacant = LayerMetadata::vacant();
        round_trip(&vacant);
        assert_max_size_bound(&vacant);
    }
}
