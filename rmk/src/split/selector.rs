use core::sync::atomic::{AtomicBool, Ordering};

use embassy_time::Timer;

static AUTO_ENABLED: AtomicBool = AtomicBool::new(false);
static WIRED_SELECTED: AtomicBool = AtomicBool::new(false);

pub fn initialize(wired: bool) {
    WIRED_SELECTED.store(wired, Ordering::Release);
    AUTO_ENABLED.store(true, Ordering::Release);
}

pub fn update(wired: bool) {
    WIRED_SELECTED.store(wired, Ordering::Release);
}

pub fn wired_selected() -> bool {
    !AUTO_ENABLED.load(Ordering::Acquire) || WIRED_SELECTED.load(Ordering::Acquire)
}

pub fn wireless_selected() -> bool {
    !AUTO_ENABLED.load(Ordering::Acquire) || !WIRED_SELECTED.load(Ordering::Acquire)
}

pub async fn wait_wired_selected() {
    while !AUTO_ENABLED.load(Ordering::Acquire) || !WIRED_SELECTED.load(Ordering::Acquire) {
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

    #[test]
    fn automatic_selection_prefers_the_detected_wired_link() {
        initialize(false);
        assert!(!wired_selected());
        assert!(wireless_selected());

        update(true);
        assert!(wired_selected());
        assert!(!wireless_selected());
    }
}
