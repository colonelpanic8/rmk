pub mod common;

mod macro_test {
    use heapless::Vec;
    use rmk::keyboard_macros::{MacroOperation, define_macro_sequences, to_macro_sequence};
    use rmk::sim::{KeymapOverride, SimKeyboard, SimKeyboardSetup};
    use rmk::types::action::{Action, KeyAction};
    use rmk_types::keycode::HidKeyCode;

    use crate::common::{KC_LSHIFT, TEST_KEYMAP};
    use crate::kc_to_u8;

    const MACRO_KEY_OVERRIDES: [KeymapOverride; 2] = [
        KeymapOverride::new(0, 0, 0, KeyAction::Single(Action::TriggerMacro(0))),
        KeymapOverride::new(0, 0, 1, KeyAction::Single(Action::TriggerMacro(1))),
    ];
    const MACRO_SETUP: SimKeyboardSetup<5, 14> = SimKeyboardSetup::new().keys(&MACRO_KEY_OVERRIDES);

    #[test]
    fn test_macro_key_a_press_release() {
        let macro_sequences = &[Vec::from_slice(&[
            MacroOperation::Press(HidKeyCode::A),
            MacroOperation::Release(HidKeyCode::A),
        ])
        .expect("too many elements")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on::test_block_on(async {
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
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(A), 0, 0, 0, 0, 0])) // press A
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release A
                .run()
                .await;
        });
    }

    #[test]
    fn test_macro_text() {
        let macro_sequences = &[to_macro_sequence("AbCd123456")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on::test_block_on(async {
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
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [0, 0, 0, 0, 0, 0])) // press shift
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [kc_to_u8!(A), 0, 0, 0, 0, 0])) // press A + shift
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [0, 0, 0, 0, 0, 0])) // release A
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release shift
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(B), 0, 0, 0, 0, 0])) // press B
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release B
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [0, 0, 0, 0, 0, 0])) // press shift
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [kc_to_u8!(C), 0, 0, 0, 0, 0])) // press C + shift
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [0, 0, 0, 0, 0, 0])) // release C
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release shift
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(D), 0, 0, 0, 0, 0])) // press D
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release D
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(Kc1), 0, 0, 0, 0, 0])) // press 1
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release 1
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(Kc2), 0, 0, 0, 0, 0])) // press 2
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release 2
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(Kc3), 0, 0, 0, 0, 0])) // press 3
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release 3
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(Kc4), 0, 0, 0, 0, 0])) // press 4
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release 4
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(Kc5), 0, 0, 0, 0, 0])) // press 5
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release 5
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(Kc6), 0, 0, 0, 0, 0])) // press 6
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release 6
                .run()
                .await;
        });
    }

    #[test]
    fn test_macro_tap_key_a() {
        let macro_sequences = &[Vec::from_slice(&[MacroOperation::Tap(HidKeyCode::A)]).expect("too many elements")];

        let macro_data = define_macro_sequences(macro_sequences);

        crate::common::test_block_on::test_block_on(async {
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
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(A), 0, 0, 0, 0, 0])) // press A
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release A
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

        crate::common::test_block_on::test_block_on(async {
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
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [0, 0, 0, 0, 0, 0])) // press shift
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [kc_to_u8!(A), 0, 0, 0, 0, 0])) // press shift + A
                .expect_keyboard_report(crate::common::report(KC_LSHIFT, [0, 0, 0, 0, 0, 0])) // release A
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release shift
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(B), 0, 0, 0, 0, 0])) // press B
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release B
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

        crate::common::test_block_on::test_block_on(async {
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
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(A), 0, 0, 0, 0, 0])) // press A
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release A
                .expect_keyboard_report(crate::common::report(0, [kc_to_u8!(B), 0, 0, 0, 0, 0])) // press B
                .expect_keyboard_report(crate::common::report(0, [0, 0, 0, 0, 0, 0])) // release B
                .run()
                .await;
        });
    }
}
