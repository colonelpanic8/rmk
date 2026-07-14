pub mod common;

mod macro_test {
    use heapless::Vec;
    use rmk::keyboard_macros::{MacroOperation, define_macro_sequences, to_macro_sequence};
    use rmk::sim::{KeymapOverride, SimKeyboard, SimKeyboardSetup};
    use rmk::types::action::{Action, KeyAction};
    use rmk_types::keycode::HidKeyCode;

    use crate::common::{KC_LSHIFT, TEST_KEYMAP};

    const MACRO_KEY_OVERRIDES: [KeymapOverride; 2] = [
        KeymapOverride::new(0, 0, 0, KeyAction::Single(Action::TriggerMacro(0))),
        KeymapOverride::new(0, 0, 1, KeyAction::Single(Action::TriggerMacro(1))),
    ];
    const MACRO_SETUP: SimKeyboardSetup = SimKeyboardSetup::new().keys(&MACRO_KEY_OVERRIDES);

    #[test]
    fn test_macro_key_a_press_release() {
        let macro_sequences = &[Vec::from_slice(&[
            MacroOperation::Press(HidKeyCode::A),
            MacroOperation::Release(HidKeyCode::A),
        ])
        .expect("too many elements")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on(async {
            let mut keyboard = SimKeyboard::builder(TEST_KEYMAP)
                .setup(MACRO_SETUP)
                .macro_sequences(macro_data)
                .build()
                .await;

            keyboard
                .delay(0)
                .press(0, 0) // press Macro0
                .delay(100)
                .release(0, 0) // release Macro0
                .expect_keys([HidKeyCode::A]) // press A
                .expect_all_up() // release A
                .run()
                .await;
        });
    }

    #[test]
    fn test_macro_text() {
        let macro_sequences = &[to_macro_sequence("AbCd123456")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on(async {
            let mut keyboard = SimKeyboard::builder(TEST_KEYMAP)
                .setup(MACRO_SETUP)
                .macro_sequences(macro_data)
                .build()
                .await;

            keyboard
                .delay(0)
                .press(0, 0) // press Macro0
                .delay(100)
                .release(0, 0) // release Macro0
                .expect_only_mods(KC_LSHIFT) // press shift
                .expect_keys_with_mods(KC_LSHIFT, [HidKeyCode::A]) // press A + shift
                .expect_only_mods(KC_LSHIFT) // release A
                .expect_all_up() // release shift
                .expect_keys([HidKeyCode::B]) // press B
                .expect_all_up() // release B
                .expect_only_mods(KC_LSHIFT) // press shift
                .expect_keys_with_mods(KC_LSHIFT, [HidKeyCode::C]) // press C + shift
                .expect_only_mods(KC_LSHIFT) // release C
                .expect_all_up() // release shift
                .expect_keys([HidKeyCode::D]) // press D
                .expect_all_up() // release D
                .expect_keys([HidKeyCode::Kc1]) // press 1
                .expect_all_up() // release 1
                .expect_keys([HidKeyCode::Kc2]) // press 2
                .expect_all_up() // release 2
                .expect_keys([HidKeyCode::Kc3]) // press 3
                .expect_all_up() // release 3
                .expect_keys([HidKeyCode::Kc4]) // press 4
                .expect_all_up() // release 4
                .expect_keys([HidKeyCode::Kc5]) // press 5
                .expect_all_up() // release 5
                .expect_keys([HidKeyCode::Kc6]) // press 6
                .expect_all_up() // release 6
                .run()
                .await;
        });
    }

    #[test]
    fn test_macro_tap_key_a() {
        let macro_sequences = &[Vec::from_slice(&[MacroOperation::Tap(HidKeyCode::A)]).expect("too many elements")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on(async {
            let mut keyboard = SimKeyboard::builder(TEST_KEYMAP)
                .setup(MACRO_SETUP)
                .macro_sequences(macro_data)
                .build()
                .await;

            keyboard
                .delay(0)
                .press(0, 0) // press Macro0
                .delay(100)
                .release(0, 0) // release Macro0
                .expect_keys([HidKeyCode::A]) // press A
                .expect_all_up() // release A
                .run()
                .await;
        });
    }

    #[test]
    fn test_macro_multiple_operations() {
        let macro_sequences = &[Vec::from_slice(&[
            MacroOperation::Press(HidKeyCode::LShift),
            MacroOperation::Tap(HidKeyCode::A),
            MacroOperation::Release(HidKeyCode::LShift),
            MacroOperation::Tap(HidKeyCode::B),
        ])
        .expect("too many elements")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on(async {
            let mut keyboard = SimKeyboard::builder(TEST_KEYMAP)
                .setup(MACRO_SETUP)
                .macro_sequences(macro_data)
                .build()
                .await;

            keyboard
                .delay(0)
                .press(0, 0) // press macro0
                .delay(100)
                .release(0, 0) // release macro0
                .expect_only_mods(KC_LSHIFT) // press shift
                .expect_keys_with_mods(KC_LSHIFT, [HidKeyCode::A]) // press shift + A
                .expect_only_mods(KC_LSHIFT) // release A
                .expect_all_up() // release shift
                .expect_keys([HidKeyCode::B]) // press B
                .expect_all_up() // release B
                .run()
                .await;
        });
    }

    #[test]
    fn test_macro_with_delay() {
        let macro_sequences = &[Vec::from_slice(&[
            MacroOperation::Tap(HidKeyCode::A),
            MacroOperation::Delay(50 << 8), // 50 ms
            MacroOperation::Tap(HidKeyCode::B),
        ])
        .expect("too many elements")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on(async {
            let mut keyboard = SimKeyboard::builder(TEST_KEYMAP)
                .setup(MACRO_SETUP)
                .macro_sequences(macro_data)
                .build()
                .await;

            keyboard
                .delay(0)
                .press(0, 0)
                .delay(100)
                .release(0, 0)
                .expect_keys([HidKeyCode::A]) // press A
                .expect_all_up() // release A
                .expect_keys([HidKeyCode::B]) // press B
                .expect_all_up() // release B
                .run()
                .await;
        });
    }
}
