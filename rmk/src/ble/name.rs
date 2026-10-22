use core::cell::RefCell;
use core::fmt::Write;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use heapless::String;
use rmk_types::ble::{BLE_NAME_MAX_LEN, BleName};

const SLOT_TOKEN: &str = "{slot}";

static BLE_NAME: Mutex<crate::RawMutex, RefCell<Option<BleName>>> = Mutex::new(RefCell::new(None));

/// Wakes an active advertiser after the runtime template changes.
pub(crate) static BLE_NAME_CHANGED: Signal<crate::RawMutex, ()> = Signal::new();

fn from_default(template: &str) -> BleName {
    let mut value = String::new();
    for ch in template.chars() {
        if value.push(ch).is_err() {
            break;
        }
    }
    BleName { template: value }
}

fn replace(value: BleName, notify: bool) {
    let changed = BLE_NAME.lock(|current| {
        let mut current = current.borrow_mut();
        if current.as_ref() == Some(&value) {
            false
        } else {
            *current = Some(value);
            true
        }
    });
    if changed && notify {
        BLE_NAME_CHANGED.signal(());
    }
}

/// Seed the runtime value from the compiled configuration.
pub(crate) fn initialize(template: &str) {
    BLE_NAME.lock(|current| {
        let mut current = current.borrow_mut();
        if current.is_none() {
            *current = Some(from_default(template));
        }
    });
}

/// Restore a validated value from persistent storage without waking advertising.
pub(crate) fn restore(value: BleName) {
    if !value.template.is_empty() {
        replace(value, false);
    }
}

pub(crate) fn current() -> BleName {
    BLE_NAME.lock(|current| current.borrow().clone().unwrap_or_else(|| from_default("RMK")))
}

/// Replace the live template and wake advertising. Empty names are rejected.
pub(crate) fn set(value: BleName) -> Result<(), ()> {
    if value.template.is_empty() {
        return Err(());
    }
    replace(value, true);
    Ok(())
}

/// Render the current template for a zero-based BLE profile.
pub(crate) fn render(profile: u8) -> String<BLE_NAME_MAX_LEN> {
    let template = current();
    let mut rendered = String::new();
    let mut rest = template.template.as_str();
    while let Some(index) = rest.find(SLOT_TOKEN) {
        rendered.push_str(&rest[..index]).unwrap();
        write!(&mut rendered, "{}", u16::from(profile) + 1).unwrap();
        rest = &rest[index + SLOT_TOKEN.len()..];
    }
    rendered.push_str(rest).unwrap();
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn renders_one_based_slot_tokens() {
        let _guard = TEST_LOCK.lock().unwrap();
        replace(
            BleName {
                template: String::try_from("G{slot}-{slot}").unwrap(),
            },
            false,
        );
        assert_eq!(render(0).as_str(), "G1-1");
        assert_eq!(render(3).as_str(), "G4-4");
    }

    #[test]
    fn leaves_fixed_names_unchanged() {
        let _guard = TEST_LOCK.lock().unwrap();
        replace(
            BleName {
                template: String::try_from("Glove80").unwrap(),
            },
            false,
        );
        assert_eq!(render(2).as_str(), "Glove80");
    }

    #[test]
    fn rejects_empty_runtime_name() {
        let _guard = TEST_LOCK.lock().unwrap();
        assert!(
            set(BleName {
                template: String::new()
            })
            .is_err()
        );
    }
}
