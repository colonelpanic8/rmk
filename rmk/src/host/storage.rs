use embassy_futures::yield_now;
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
use rmk_types::morse::MorseProfile;
use serde::de::{Error as DeError, SeqAccess, Visitor};
use serde::{Deserializer, Serializer};

use crate::keyboard::combo::Combo;
use crate::storage::{Storage, StorageData, StorageKey, print_storage_error};
use crate::{COMBO_MAX_NUM, FORK_MAX_NUM, MACRO_SPACE_SIZE, MORSE_MAX_NUM, MORSE_PROFILE_MAX_NUM};

pub(crate) mod macro_bytes_serde {
    use super::*;

    pub(crate) fn serialize<S>(value: &[u8; MACRO_SPACE_SIZE], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(value)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<[u8; MACRO_SPACE_SIZE], D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MacroBytesVisitor;

        impl<'de> Visitor<'de> for MacroBytesVisitor {
            type Value = [u8; MACRO_SPACE_SIZE];

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(formatter, "exactly {MACRO_SPACE_SIZE} bytes")
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                if value.len() != MACRO_SPACE_SIZE {
                    return Err(E::invalid_length(value.len(), &self));
                }

                let mut bytes = [0u8; MACRO_SPACE_SIZE];
                bytes.copy_from_slice(value);
                Ok(bytes)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = [0u8; MACRO_SPACE_SIZE];
                for (idx, slot) in bytes.iter_mut().enumerate() {
                    *slot = seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(idx, &self))?;
                }

                if (seq.next_element::<u8>()?).is_some() {
                    return Err(A::Error::invalid_length(MACRO_SPACE_SIZE + 1, &self));
                }

                Ok(bytes)
            }
        }

        deserializer.deserialize_bytes(MacroBytesVisitor)
    }
}

impl<F: AsyncNorFlash, const ROW: usize, const COL: usize, const NUM_LAYER: usize, const NUM_ENCODER: usize>
    Storage<F, ROW, COL, NUM_LAYER, NUM_ENCODER>
{
    pub(crate) async fn read_boot_data(
        &mut self,
        data: &mut crate::keymap::KeymapData<ROW, COL, NUM_LAYER, NUM_ENCODER>,
        behavior: &mut crate::config::BehaviorConfig,
    ) -> Result<(), ()> {
        let mut key_iterator = self
            .flash
            .fetch_all_items(&mut self.buffer)
            .await
            .map_err(|e| print_storage_error::<F>(e))?;

        let mut records_read = 0;
        while let Some((key, item)) = key_iterator
            .next::<StorageData>(&mut self.buffer)
            .await
            .map_err(|e| print_storage_error::<F>(e))?
        {
            match (key, item) {
                (StorageKey::Keymap { layer, row, col }, StorageData::KeyAction(action)) => {
                    let layer = layer as usize;
                    let row = row as usize;
                    let col = col as usize;
                    if layer < NUM_LAYER && row < ROW && col < COL {
                        data.keymap[layer][row][col] = action;
                    }
                }
                (StorageKey::Encoder { layer, idx }, StorageData::EncoderAction(action)) => {
                    let idx = idx as usize;
                    let layer = layer as usize;
                    if layer < NUM_LAYER && idx < NUM_ENCODER {
                        data.encoder_map[layer][idx] = action;
                    }
                }
                (StorageKey::LayoutConfig, StorageData::LayoutConfig(config)) => {
                    // Restore the default (base) layer set via a `PDF` key
                    behavior.default_layer = config.default_layer;
                }
                (StorageKey::BehaviorConfig, StorageData::BehaviorConfig(config)) => {
                    behavior.morse.prior_idle_time = embassy_time::Duration::from_millis(config.prior_idle_time as u64);
                    behavior.morse.default_profile = config.morse_default_profile;
                    behavior.combo.timeout = embassy_time::Duration::from_millis(config.combo_timeout as u64);
                    behavior.one_shot.timeout = embassy_time::Duration::from_millis(config.one_shot_timeout as u64);
                    behavior.tap.tap_interval = config.tap_interval;
                    behavior.tap.tap_capslock_interval = config.tap_capslock_interval;
                }
                (StorageKey::MacroData, StorageData::MacroData(macro_data)) => {
                    behavior.keyboard_macros.macro_sequences.copy_from_slice(&macro_data);
                }
                (StorageKey::Combo(idx), StorageData::Combo(config)) => {
                    let idx = idx as usize;
                    if idx < COMBO_MAX_NUM {
                        debug!("Read combo config: {:?}", config);
                        behavior.combo.combos[idx] = Some(Combo::new(config));
                    }
                }
                (StorageKey::Fork(idx), StorageData::Fork(fork)) => {
                    let idx = idx as usize;
                    if idx < FORK_MAX_NUM
                        && let Some(item) = behavior.fork.forks.get_mut(idx)
                    {
                        *item = fork;
                    }
                }
                (StorageKey::Morse(idx), StorageData::Morse(morse)) => {
                    let idx = idx as usize;
                    if idx < MORSE_MAX_NUM
                        && let Some(item) = behavior.morse.morses.get_mut(idx)
                    {
                        *item = morse;
                    }
                }
                _ => {}
            }

            records_read += 1;
            if records_read % 32 == 0 {
                // Memory-mapped flash can keep every read ready, so let other tasks run.
                yield_now().await;
            }
        }

        Ok(())
    }

    /// Restore profiles written over the host protocol. The table is only as
    /// long as `keyboard.toml` made it, so a stored slot beyond its end grows
    /// it — the same growth the setter does at runtime.
    pub(crate) async fn read_morse_profiles(
        &mut self,
        profiles: &mut heapless::Vec<MorseProfile, MORSE_PROFILE_MAX_NUM>,
    ) -> Result<(), ()> {
        for i in 0..MORSE_PROFILE_MAX_NUM {
            let key = StorageKey::morse_profile(i as u8);
            let read_data = self
                .flash
                .fetch_item(&mut self.buffer, &key)
                .await
                .map_err(|e| print_storage_error::<F>(e))?;

            if let Some(StorageData::MorseProfile(profile)) = read_data {
                if i >= profiles.len() {
                    profiles.resize(i + 1, MorseProfile::default()).ok();
                }
                profiles[i] = profile;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rmk_types::action::Action;
    use rmk_types::keycode::{HidKeyCode, KeyCode};
    use rmk_types::morse::{HOLD, Morse, MorseMode, MorsePattern, MorseProfile, TAP};
    use sequential_storage::map::Value;

    use super::*;

    #[test]
    fn test_morse_serialization_deserialization() {
        let morse = Morse::new_from_vial(
            Action::Key(KeyCode::Hid(HidKeyCode::A)),
            Action::Key(KeyCode::Hid(HidKeyCode::B)),
            Action::Key(KeyCode::Hid(HidKeyCode::C)),
            Action::Key(KeyCode::Hid(HidKeyCode::D)),
            MorseProfile::new(Some(true), Some(MorseMode::PermissiveHold), Some(190u16), Some(180u16)),
        );

        // Serialization
        let mut buffer = [0u8; 64];
        let storage_data = StorageData::Morse(morse.clone());
        let serialized_size = Value::serialize_into(&storage_data, &mut buffer).unwrap();

        // Deserialization
        let deserialized_data = StorageData::deserialize_from(&buffer[..serialized_size]).unwrap();

        // Validation
        match deserialized_data {
            (StorageData::Morse(deserialized_morse), _) => {
                // actions
                assert_eq!(deserialized_morse.actions.len(), morse.actions.len());
                for (original, deserialized) in morse.actions.iter().zip(deserialized_morse.actions.iter()) {
                    assert_eq!(original, deserialized);
                }
                // profile
                assert_eq!(deserialized_morse.profile, morse.profile);
            }
            _ => panic!("Expected MorseData"),
        }
    }

    #[test]
    fn test_morse_with_partial_actions() {
        // Create a Morse with partial actions
        let mut morse = Morse::default();
        _ = morse.put(TAP, Action::Key(KeyCode::Hid(HidKeyCode::A)));
        _ = morse.put(HOLD, Action::Key(KeyCode::Hid(HidKeyCode::B)));

        // Serialization
        let mut buffer = [0u8; 64];
        let storage_data = StorageData::Morse(morse.clone());
        let serialized_size = Value::serialize_into(&storage_data, &mut buffer).unwrap();

        // Deserialization
        let deserialized_data = StorageData::deserialize_from(&buffer[..serialized_size]).unwrap();

        // Validation
        match deserialized_data {
            (StorageData::Morse(deserialized_morse), _) => {
                // actions
                assert_eq!(deserialized_morse.actions.len(), morse.actions.len());
                for (original, deserialized) in morse.actions.iter().zip(deserialized_morse.actions.iter()) {
                    assert_eq!(original, deserialized);
                }
                // profile
                assert_eq!(deserialized_morse.profile, morse.profile);
            }
            _ => panic!("Expected MorseData"),
        }
    }

    #[test]
    fn test_morse_with_morse_serialization_deserialization() {
        let mut morse = Morse {
            profile: MorseProfile::new(
                Some(false),
                Some(MorseMode::HoldOnOtherPress),
                Some(210u16),
                Some(220u16),
            ),
            actions: heapless::LinearMap::default(),
        };
        morse
            .actions
            .insert(MorsePattern::from_u16(0b1_01), Action::Key(KeyCode::Hid(HidKeyCode::A)))
            .ok();
        morse
            .actions
            .insert(
                MorsePattern::from_u16(0b1_1000),
                Action::Key(KeyCode::Hid(HidKeyCode::B)),
            )
            .ok();
        morse
            .actions
            .insert(
                MorsePattern::from_u16(0b1_1010),
                Action::Key(KeyCode::Hid(HidKeyCode::C)),
            )
            .ok();

        // Serialization
        let mut buffer = [0u8; 64];
        let storage_data = StorageData::Morse(morse.clone());
        let serialized_size = Value::serialize_into(&storage_data, &mut buffer).unwrap();

        // Deserialization
        let deserialized_data = StorageData::deserialize_from(&buffer[..serialized_size]).unwrap();

        // Validation
        match deserialized_data {
            (StorageData::Morse(deserialized_morse), _) => {
                // actions
                assert_eq!(deserialized_morse.actions.len(), morse.actions.len());
                for (original, deserialized) in morse.actions.iter().zip(deserialized_morse.actions.iter()) {
                    assert_eq!(original, deserialized);
                }
                // profile
                assert_eq!(deserialized_morse.profile, morse.profile);
            }
            _ => panic!("Expected MorseData"),
        }
    }
}
