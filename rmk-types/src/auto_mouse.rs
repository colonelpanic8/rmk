//! Runtime representation of auto mouse layer configuration.

#[cfg(not(feature = "host"))]
use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

use crate::keycode::KeyCode;

/// Maximum number of keycodes that an auto mouse layer entry can exempt from
/// `deactivate_on_key`.
pub const AUTO_MOUSE_LAYER_EXTRA_KEY_MAX_NUM: usize = 16;

#[cfg(not(feature = "host"))]
pub type AutoMouseLayerExtraKeys = heapless::Vec<KeyCode, AUTO_MOUSE_LAYER_EXTRA_KEY_MAX_NUM>;
#[cfg(feature = "host")]
pub type AutoMouseLayerExtraKeys = alloc::vec::Vec<KeyCode>;

/// One auto mouse layer entry in a transport- and storage-friendly form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub struct AutoMouseLayerConfig {
    pub device_id: Option<u8>,
    pub target_layer: u8,
    pub timeout_ms: u32,
    pub threshold: u16,
    pub deactivate_on_key: bool,
    #[cfg_attr(feature = "wasm", tsify(type = "KeyCode[]"))]
    pub extra_mouse_keys: AutoMouseLayerExtraKeys,
    pub reset_timeout_on_key: bool,
}

#[cfg(not(feature = "host"))]
impl MaxSize for AutoMouseLayerConfig {
    const POSTCARD_MAX_SIZE: usize = Option::<u8>::POSTCARD_MAX_SIZE
        + u8::POSTCARD_MAX_SIZE
        + u32::POSTCARD_MAX_SIZE
        + u16::POSTCARD_MAX_SIZE
        + bool::POSTCARD_MAX_SIZE
        + crate::heapless_vec_max_size::<KeyCode, AUTO_MOUSE_LAYER_EXTRA_KEY_MAX_NUM>()
        + bool::POSTCARD_MAX_SIZE;
}
