use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_time::Timer;

/// No force: follow the debounced cable-detect input.
pub const FORCE_AUTO: u8 = 0;
/// Pin the split transport to the wired link.
pub const FORCE_WIRED: u8 = 1;
/// Pin the split transport to BLE.
pub const FORCE_BLE: u8 = 2;

static AUTO_ENABLED: AtomicBool = AtomicBool::new(false);
static WIRED_SELECTED: AtomicBool = AtomicBool::new(false);
static FORCED: AtomicU8 = AtomicU8::new(FORCE_AUTO);

pub fn initialize(wired: bool) {
    WIRED_SELECTED.store(wired, Ordering::Release);
    AUTO_ENABLED.store(true, Ordering::Release);
}

pub fn update(wired: bool) {
    WIRED_SELECTED.store(wired, Ordering::Release);
}

/// Volatile transport force (one of the `FORCE_*` constants). It masks the
/// detected cable state until the next force or reboot; the detect input
/// keeps updating underneath and [`detected_wired`] keeps reporting it.
pub fn set_forced(mode: u8) {
    FORCED.store(mode.min(FORCE_BLE), Ordering::Release);
}

pub fn forced_mode() -> u8 {
    FORCED.load(Ordering::Acquire)
}

/// The board runs an automatic wired/BLE selection policy.
pub fn auto_enabled() -> bool {
    AUTO_ENABLED.load(Ordering::Acquire)
}

/// The debounced cable-detect input, independent of any force.
pub fn detected_wired() -> bool {
    WIRED_SELECTED.load(Ordering::Acquire)
}

fn effective_wired() -> bool {
    match FORCED.load(Ordering::Acquire) {
        FORCE_WIRED => true,
        FORCE_BLE => false,
        _ => WIRED_SELECTED.load(Ordering::Acquire),
    }
}

pub fn wired_selected() -> bool {
    !AUTO_ENABLED.load(Ordering::Acquire) || effective_wired()
}

pub fn wireless_selected() -> bool {
    !AUTO_ENABLED.load(Ordering::Acquire) || !effective_wired()
}

pub async fn wait_wired_selected() {
    while !AUTO_ENABLED.load(Ordering::Acquire) || !effective_wired() {
        Timer::after_millis(5).await;
    }
}

pub async fn wait_wireless_selected() {
    while !wireless_selected() {
        Timer::after_millis(5).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test body: these assertions share the module's global statics, so
    // parallel test threads would race a second mutating test.
    #[test]
    fn automatic_selection_prefers_the_detected_wired_link() {
        initialize(false);
        assert!(!wired_selected());
        assert!(wireless_selected());

        update(true);
        assert!(wired_selected());
        assert!(!wireless_selected());

        // A force masks the detected state in both directions...
        set_forced(FORCE_BLE);
        assert!(!wired_selected());
        assert!(wireless_selected());
        assert!(detected_wired());

        update(false);
        set_forced(FORCE_WIRED);
        assert!(wired_selected());
        assert!(!detected_wired());

        // ...and releasing it returns control to the detect input.
        set_forced(FORCE_AUTO);
        assert!(!wired_selected());
        assert!(wireless_selected());
    }
}
