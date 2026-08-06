//! How a pointing device's motion is interpreted.
//!
//! These live here rather than beside the pointing processor because the
//! Rynk protocol carries them: a host reads and rewrites a pad's mode, and
//! the device stores the result across reboots.

use postcard::experimental::max_size::MaxSize;
use serde::{Deserialize, Serialize};

use crate::keycode::HidKeyCode;

/// Pointing mode determines how raw XY motion is interpreted
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PointingMode {
    /// Default cursor mode - XY maps to mouse XY movement
    Cursor(CursorConfig),
    /// Scroll mode - XY maps to wheel (vertical) and pan (horizontal)
    Scroll(ScrollConfig),
    /// Sniper mode - XY maps to cursor but at reduced sensitivity
    Sniper(SniperConfig),
    /// Caret mode, XY maps to vertical and horizontal caret movement
    Caret(CaretConfig),
    /// Drag mode - XY maps to cursor movement, and a device tap latches a
    /// mouse button down until the next tap
    Drag(DragConfig),
}

impl Default for PointingMode {
    fn default() -> Self {
        Self::Cursor(CursorConfig::default())
    }
}

/// Configuration for cursor mode
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct CursorConfig {
    /// Multiplier for X axis. Higher = more output per unit of motion. 0 disables X.
    pub multiplier_x: u8,
    /// Multiplier for Y axis. Higher = more output per unit of motion. 0 disables Y.
    pub multiplier_y: u8,
    /// Invert X axis movement.
    pub invert_x: bool,
    /// Invert Y axis movement.
    pub invert_y: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            multiplier_x: 1,
            multiplier_y: 1,
            invert_x: false,
            invert_y: false,
        }
    }
}

/// Configuration for caret mode
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct CaretConfig {
    /// Disable X axis in caret mode.
    pub disable_x: bool,
    /// Disable y axis in caret mode.
    pub disable_y: bool,
    /// Invert X axis.
    pub invert_x: bool,
    /// Invert Y axis.
    pub invert_y: bool,
    /// Threshold for accumulated motion. Read this as sensitivity in caret mode.
    /// Higher values mean less sensitivity.
    pub threshold: i16,
    /// Keycode to emit for up rotation. Default: Up arrow
    pub keycode_up: HidKeyCode,
    /// Keycode to emit for down rotation. Default: Down arrow
    pub keycode_down: HidKeyCode,
    /// Keycode to emit for left rotation. Default: Left arrow
    pub keycode_left: HidKeyCode,
    /// Keycode to emit for right rotation. Default: Right arrow
    pub keycode_right: HidKeyCode,
}

impl Default for CaretConfig {
    fn default() -> Self {
        Self {
            disable_x: false,
            disable_y: false,
            invert_x: false,
            invert_y: false,
            threshold: 100,
            keycode_up: HidKeyCode::Up,
            keycode_down: HidKeyCode::Down,
            keycode_left: HidKeyCode::Left,
            keycode_right: HidKeyCode::Right,
        }
    }
}
/// Configuration for scroll mode
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ScrollConfig {
    /// Multiplier for X axis (→ pan). Higher = more output per unit of motion. 0 disables horizontal pan.
    pub multiplier_x: u8,
    /// Divisor for X axis (→ pan). Higher = slower. 0 disables horizontal pan.
    pub divisor_x: u8,
    /// Multiplier for Y axis (→ wheel). Higher = more output per unit of motion. 0 disables vertical scroll.
    pub multiplier_y: u8,
    /// Divisor for Y axis (→ wheel). Higher = slower. 0 disables vertical scroll.
    pub divisor_y: u8,
    /// Invert X axis. In scroll mode X maps to pan, so this reverses pan direction.
    pub invert_x: bool,
    /// Invert Y axis. In scroll mode Y maps to wheel, so this reverses scroll direction.
    pub invert_y: bool,
}

impl Default for ScrollConfig {
    fn default() -> Self {
        Self {
            multiplier_x: 1,
            multiplier_y: 1,
            divisor_x: 8,
            divisor_y: 8,
            invert_x: false,
            invert_y: false,
        }
    }
}
/// Configuration for sniper (precision) mode
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SniperConfig {
    /// Multiplier for both axes. Higher = more output per unit of motion.
    pub multiplier: u8,
    /// Divisor for both axes. Higher = slower, more precise movement.
    pub divisor: u8,
    /// Invert X axis movement.
    pub invert_x: bool,
    /// Invert Y axis movement.
    pub invert_y: bool,
}

impl Default for SniperConfig {
    fn default() -> Self {
        Self {
            multiplier: 1,
            divisor: 4,
            invert_x: false,
            invert_y: false,
        }
    }
}

/// Configuration for drag mode
///
/// A pad with no physical buttons can still drag: the device's own tap
/// gesture latches a button down and holds it, so the next stroke moves
/// whatever the tap grabbed. A second tap drops it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DragConfig {
    /// Motion behaves exactly as it does in cursor mode.
    pub cursor: CursorConfig,
    /// Device button whose press toggles the latch, as a bit mask in the
    /// HID mouse report's button order (bit 0 is the primary button).
    pub toggled_by: u8,
    /// Button held while the latch is engaged, in the same bit order.
    pub latches: u8,
}

impl Default for DragConfig {
    fn default() -> Self {
        Self {
            cursor: CursorConfig::default(),
            toggled_by: 1,
            latches: 1,
        }
    }
}
