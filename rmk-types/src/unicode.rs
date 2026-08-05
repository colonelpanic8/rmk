//! Unicode codepoint input.

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

/// How a codepoint is spelled out to the host.
///
/// Every mode types the codepoint as hex digits surrounded by whatever the OS
/// input method uses to delimit them, so the keyboard needs to know which one
/// the host is running.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Serialize, Deserialize, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[cfg_attr(feature = "wasm", tsify(into_wasm_abi, from_wasm_abi))]
pub enum UnicodeMode {
    /// IBus: `Ctrl+Shift+U`, hex digits, `Enter`.
    #[default]
    Linux,
    /// macOS "Unicode Hex Input" layout: hex digits typed with `LeftAlt` held.
    MacOs,
    /// WinCompose: `RightAlt`, `u`, hex digits, `Enter`.
    Windows,
}

impl UnicodeMode {
    /// The next mode in the cycle, wrapping back to [`UnicodeMode::Linux`].
    pub fn next(self) -> Self {
        match self {
            Self::Linux => Self::MacOs,
            Self::MacOs => Self::Windows,
            Self::Windows => Self::Linux,
        }
    }
}
