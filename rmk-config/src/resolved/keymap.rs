use crate::KeyInfo;

/// Resolved keymap for keymap generation: layer count, per-layer actions,
/// encoder map, plus the matrix-derived per-key info and grid dimensions.
pub struct Keymap {
    pub rows: u8,
    pub cols: u8,
    pub layers: u8,
    pub keymap: Vec<Vec<Vec<String>>>,
    pub encoder_map: Vec<Vec<[String; 2]>>,
    /// Compiled logical occupancy and names, padded to `layers` with vacant slots.
    pub layer_names: Vec<Option<String>>,
    pub key_info: Vec<Vec<KeyInfo>>,
    /// Total number of encoders on the board.
    pub num_encoder: usize,
}

impl crate::KeyboardTomlConfig {
    /// Resolve the keymap configuration from TOML config.
    pub fn keymap(&self) -> Result<Keymap, String> {
        let (keymap_config, key_info) = self.get_keymap_config()?;
        let mut layer_names = self
            .keymap
            .as_ref()
            .map(|keymap| {
                keymap
                    .layer
                    .iter()
                    .enumerate()
                    .map(|(index, layer)| {
                        let name = layer.name.clone().unwrap_or_else(|| format!("Layer {index}"));
                        if name.is_empty() {
                            return Err(format!("keymap layer {index} name must not be empty"));
                        }
                        if name.len() > 32 {
                            return Err(format!(
                                "keymap layer {index} name is {} bytes; the maximum is 32",
                                name.len()
                            ));
                        }
                        Ok(Some(name))
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .transpose()?
            .unwrap_or_default();
        layer_names.resize(keymap_config.layers as usize, None);
        // Encoders may be spread across split halves; only the board-wide total is used here.
        let num_encoder = self.total_encoders();

        // Encoder maps are all-or-none; partial lists would leave encoders dead.
        for (i, encoders) in keymap_config.encoder_map.iter().enumerate() {
            if !encoders.is_empty() && encoders.len() != num_encoder {
                return Err(format!(
                    "keyboard.toml: [[keymap.layer]] #{i} lists {} encoders but the board has \
                     {num_encoder} (configure all {num_encoder} or none)",
                    encoders.len()
                ));
            }
        }

        Ok(Keymap {
            rows: keymap_config.rows,
            cols: keymap_config.cols,
            layers: keymap_config.layers,
            keymap: keymap_config.keymap,
            encoder_map: keymap_config.encoder_map,
            layer_names,
            key_info,
            num_encoder,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::KeyboardTomlConfig;

    fn config(layer: &str) -> KeyboardTomlConfig {
        toml::from_str(&format!(
            "[layout]\nrows = 1\ncols = 1\nmap = \"(0,0)\"\n\
             [keymap]\nlayers = 3\n[[keymap.layer]]\n{layer}\nkeys = \"A\"\n"
        ))
        .unwrap()
    }

    #[test]
    fn layer_names_distinguish_configured_and_reserved_slots() {
        let named = config("name = \"Base\"").keymap().unwrap();
        assert_eq!(named.layer_names, vec![Some("Base".into()), None, None]);

        let unnamed = config("").keymap().unwrap();
        assert_eq!(unnamed.layer_names, vec![Some("Layer 0".into()), None, None]);
    }

    #[test]
    fn layer_names_must_fit_the_wire_payload() {
        let err = config(&format!("name = \"{}\"", "x".repeat(33)))
            .keymap()
            .err()
            .unwrap();
        assert!(err.contains("maximum is 32"));
    }
}
