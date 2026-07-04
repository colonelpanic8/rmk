use serde::Deserialize;

use crate::KeyboardTomlConfig;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChipSeries {
    Stm32,
    Nrf52,
    #[default]
    Rp2040,
    Esp32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChipModel {
    pub series: ChipSeries,
    pub chip: String,
    pub board: Option<String>,
}

/// A supported dev board: accepted names → chip + default-config layer.
struct BoardDef {
    names: &'static [&'static str],
    chip: &'static str,
    series: ChipSeries,
    default_config: &'static str,
}

/// Single source of board knowledge for both chip-model resolution and
/// default-config lookup — adding a board means adding one entry here.
const SUPPORTED_BOARDS: &[BoardDef] = &[
    BoardDef {
        names: &["nice!nano", "nice!nano_v1", "nicenano"],
        chip: "nrf52840",
        series: ChipSeries::Nrf52,
        default_config: include_str!("default_config/nice_nano.toml"),
    },
    BoardDef {
        names: &["nice!nano_v2", "nice!nano v2"],
        chip: "nrf52840",
        series: ChipSeries::Nrf52,
        default_config: include_str!("default_config/nice_nano_v2.toml"),
    },
    BoardDef {
        names: &["XIAO BLE", "nrfmicro", "bluemicro840", "puchi_ble"],
        chip: "nrf52840",
        series: ChipSeries::Nrf52,
        default_config: include_str!("default_config/nrf52840.toml"),
    },
    BoardDef {
        names: &["Pi Pico W", "Pico W", "pi_pico_w", "pico_w"],
        chip: "rp2040",
        series: ChipSeries::Rp2040,
        default_config: include_str!("default_config/pi_pico_w.toml"),
    },
];

fn find_board(name: &str) -> Option<&'static BoardDef> {
    SUPPORTED_BOARDS.iter().find(|def| def.names.contains(&name))
}

impl ChipModel {
    pub fn get_default_config_str(&self) -> Result<&'static str, String> {
        if let Some(board) = &self.board {
            if let Some(def) = find_board(board) {
                return Ok(def.default_config);
            }
            // ChipModel is only built via get_chip_model, which rejects unknown
            // boards — this fallback is a safety net, not a real path.
            eprintln!("Fallback to use chip config for board: {}", board);
        }
        self.get_default_config_str_from_chip(&self.chip)
    }

    fn get_default_config_str_from_chip(&self, chip: &str) -> Result<&'static str, String> {
        match chip {
            "nrf52840" => Ok(include_str!("default_config/nrf52840.toml")),
            "nrf52833" => Ok(include_str!("default_config/nrf52833.toml")),
            "nrf52832" => Ok(include_str!("default_config/nrf52832.toml")),
            "nrf52810" | "nrf52811" => Ok(include_str!("default_config/nrf52810.toml")),
            "rp2040" => Ok(include_str!("default_config/rp2040.toml")),
            s if s.starts_with("stm32") => Ok(include_str!("default_config/stm32.toml")),
            s if s.starts_with("esp32") => {
                if s == "esp32s3" {
                    Ok(include_str!("default_config/esp32s3.toml"))
                } else {
                    Ok(include_str!("default_config/esp32.toml"))
                }
            }
            _ => Err(format!(
                "No default chip config for {}, please report at https://github.com/HaoboGu/rmk/issues",
                self.chip
            )),
        }
    }
}

impl KeyboardTomlConfig {
    pub(crate) fn get_chip_model(&self) -> Result<ChipModel, String> {
        let keyboard = self
            .keyboard
            .as_ref()
            .ok_or_else(|| {
                "[keyboard] section is required — add `[keyboard]` with `name`, `vendor_id`, `product_id` and `chip` (or `board`)"
                    .to_string()
            })?;
        if keyboard.board.is_none() == keyboard.chip.is_none() {
            return Err("Either \"board\" or \"chip\" should be set in keyboard.toml, but not both".to_string());
        }

        // Check board type
        if let Some(board) = keyboard.board.clone() {
            match find_board(&board) {
                Some(def) => Ok(ChipModel {
                    series: def.series.clone(),
                    chip: def.chip.to_string(),
                    board: Some(board),
                }),
                None => {
                    let supported = SUPPORTED_BOARDS
                        .iter()
                        .flat_map(|def| def.names.iter().copied())
                        .collect::<Vec<_>>()
                        .join(", ");
                    Err(format!(
                        "Unsupported board \"{board}\" — supported boards are {supported}; for other hardware set `chip` instead"
                    ))
                }
            }
        } else if let Some(chip) = keyboard.chip.clone() {
            if chip.to_lowercase().starts_with("stm32") {
                Ok(ChipModel {
                    series: ChipSeries::Stm32,
                    chip,
                    board: None,
                })
            } else if chip.to_lowercase().starts_with("nrf52") {
                Ok(ChipModel {
                    series: ChipSeries::Nrf52,
                    chip,
                    board: None,
                })
            } else if chip.to_lowercase().starts_with("rp2040") {
                Ok(ChipModel {
                    series: ChipSeries::Rp2040,
                    chip,
                    board: None,
                })
            } else if chip.to_lowercase().starts_with("esp32") {
                Ok(ChipModel {
                    series: ChipSeries::Esp32,
                    chip,
                    board: None,
                })
            } else {
                Err(format!(
                    "Unsupported chip \"{chip}\" — supported chip families are stm32*, nrf52*, rp2040 and esp32*"
                ))
            }
        } else {
            Err("Neither board nor chip is specified".to_string())
        }
    }

    pub(crate) fn get_chip_config(&self) -> ChipConfig {
        // An unresolvable chip is reported by the resolution entry points; here it just
        // means there is no chip-specific config to look up.
        let Ok(chip_model) = self.get_chip_model() else {
            return ChipConfig::default();
        };
        self.chip
            .as_ref()
            .and_then(|chip_configs| chip_configs.get(&chip_model.chip))
            .cloned()
            .unwrap_or_default()
    }
}

/// Configurations for keyboard info
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KeyboardInfo {
    /// Keyboard name
    pub name: String,
    /// Vender id
    pub vendor_id: u16,
    /// Product id
    pub product_id: u16,
    /// Manufacturer
    pub manufacturer: Option<String>,
    /// Product name, if not set, it will use `name` as default
    pub product_name: Option<String>,
    /// Serial number
    pub serial_number: Option<String>,
    /// Board name(if a supported board is used)
    pub board: Option<String>,
    /// Chip model
    pub chip: Option<String>,
    /// enable usb
    pub usb_enable: Option<bool>,
}

/// nRF52840 DCDC REG0 output voltage
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub enum DcdcReg0Voltage {
    #[serde(rename = "3V3")]
    V3_3,
    #[serde(rename = "1V8")]
    V1_8,
}

/// Config for chip-specific settings
#[derive(Clone, Default, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChipConfig {
    /// DCDC regulator 0 enabled (for nrf52840)
    pub dcdc_reg0: Option<bool>,
    /// DCDC regulator 1 enabled (for nrf52840, nrf52833)
    pub dcdc_reg1: Option<bool>,
    /// DCDC regulator 0 voltage (for nrf52840)
    pub dcdc_reg0_voltage: Option<DcdcReg0Voltage>,
}
