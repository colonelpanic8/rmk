//! Built-in processor module for RMK
//!
//! This module contains built-in processor implementations for output devices.

#[cfg(rmk_ble)]
pub mod battery_led;
#[cfg(rmk_dfu)]
pub mod dfu_led;
pub mod led_indicator;
pub mod wpm;
