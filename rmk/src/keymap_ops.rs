//! A bounded operation channel for reading and writing the live keymap.
//!
//! Operations are consumed by the Vial service so external configuration
//! interfaces use the same conversion, persistence path, and task ownership
//! as Vial. Exactly one operation may be in flight: a single client sends an
//! operation on [`KEYMAP_OPS`] and awaits the corresponding value from
//! [`KEYMAP_OP_RESULTS`].

use embassy_sync::channel::Channel;

use crate::RawMutex;

/// One keymap operation using VIA/Vial's 16-bit keycode encoding.
pub enum KeymapOp {
    /// Read the action at `(layer, row, col)` as a VIA keycode.
    Get { layer: u8, row: u8, col: u8 },
    /// Decode and persist a VIA keycode at `(layer, row, col)`.
    Set { layer: u8, row: u8, col: u8, keycode: u16 },
}

/// Operations submitted to the Vial service task.
pub static KEYMAP_OPS: Channel<RawMutex, KeymapOp, 1> = Channel::new();
/// The canonical VIA keycode stored after each operation.
pub static KEYMAP_OP_RESULTS: Channel<RawMutex, u16, 1> = Channel::new();
