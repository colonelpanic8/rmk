//! `[input_device]` configuration: encoders, pointing devices and sensors.

use serde::Deserialize;

use crate::communication::{CommunicationProtocol, SpiConfig};
use crate::default_false;
use crate::duration::de_opt_millis;

const fn default_pointing_report_hz() -> u16 {
    125
}

/// Configurations for input devices
///
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputDeviceConfig {
    pub encoder: Option<Vec<EncoderConfig>>,
    pub pointing: Option<Vec<PointingDeviceConfig>>,
    pub joystick: Option<Vec<JoystickConfig>>,
    pub pmw3610: Option<Vec<Pmw3610Config>>,
    pub pmw33xx: Option<Vec<Pmw33xxConfig>>,
    pub iqs5xx: Option<Vec<Iqs5xxConfig>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoystickConfig {
    // Name of the joystick
    pub name: String,
    /// Device id used to match this joystick with its JoystickProcessor.
    /// If omitted, ids are assigned sequentially starting from 0.
    pub id: Option<u8>,
    // Pin a of the joystick
    pub pin_x: String,
    // Pin b of the joystick
    pub pin_y: String,
    // Pin z of the joystick
    pub pin_z: String,
    pub transform: Vec<Vec<i16>>,
    pub bias: Vec<i16>,
    pub resolution: u16,
}

/// PMW3610 optical mouse sensor configuration
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pmw3610Config {
    /// Name of the sensor (used for variable naming)
    pub name: String,
    /// id of the device
    pub id: Option<u8>,
    /// SPI pins
    pub spi: SpiConfig,
    /// Optional motion interrupt pin
    pub motion: Option<String>,
    /// CPI resolution (200-3200, step 200). Optional, uses sensor default if not set.
    pub cpi: Option<u16>,
    /// Invert X axis
    #[serde(default)]
    pub invert_x: bool,
    /// Invert Y axis
    #[serde(default)]
    pub invert_y: bool,
    /// Swap X and Y axes
    #[serde(default)]
    pub swap_xy: bool,
    /// Force awake mode (disable power saving)
    #[serde(default)]
    pub force_awake: bool,
    /// Enable smart mode for better tracking on shiny surfaces
    #[serde(default)]
    pub smart_mode: bool,
    /// Report rate (Hz). Motion will be accumulated and emitted at this rate.
    #[serde(default = "default_pointing_report_hz")]
    pub report_hz: u16,
    #[serde(default)]
    pub proc_invert_x: bool,
    /// Invert Y axis
    #[serde(default)]
    pub proc_invert_y: bool,
    /// Swap X and Y axes
    #[serde(default)]
    pub proc_swap_xy: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Pmw33xxType {
    #[default]
    PMW3360,
    PMW3389,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pmw33xxConfig {
    // Name of the sensor (used for variable naming)
    pub name: String,
    // id of the device
    pub id: Option<u8>,
    // Sensor Type (3360 or 3389)
    pub sensor_type: Pmw33xxType,
    // SPI pins
    pub spi: SpiConfig,
    // Optional motion interrupt pin
    pub motion: Option<String>,
    // CPI resolution (100-12000, step 100).Optional, uses sensor default 1600 if not set.
    pub cpi: Option<u16>,
    // Rotational transform angle (-127 to 127) Optional, uses sensor default 0 if not set.
    pub rot_trans_angle: Option<i8>,
    // liftoff distance. Optional, uses sensor default 0 if not set.
    pub liftoff_dist: Option<u8>,
    // Invert X axis
    #[serde(default)]
    pub proc_invert_x: bool,
    // Invert Y axis
    #[serde(default)]
    pub proc_invert_y: bool,
    // Swap X and Y axes
    #[serde(default)]
    pub proc_swap_xy: bool,
    /// Report rate (Hz). Motion will be accumulated and emitted at this rate.
    #[serde(default = "default_pointing_report_hz")]
    pub report_hz: u16,
}

/// Azoteq IQS5xx trackpad configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Iqs5xxConfig {
    /// Name of the trackpad (used for variable naming).
    pub name: String,
    /// RMK pointing-device id (0-255). Defaults to 0.
    pub id: Option<u8>,
    /// I²C bus the trackpad is connected to. The bus is dedicated to this
    /// device — sharing with other I²C peripherals (e.g. an OLED) is not yet
    /// supported via TOML.
    pub i2c: Iqs5xxI2cConfig,
    /// Optional `RDY` pin. Strongly recommended; without it the driver falls
    /// back to timed polling and may stall the bus through clock-stretching.
    pub rdy: Option<String>,
    /// Invert X in the PointingProcessor.
    #[serde(default)]
    pub proc_invert_x: bool,
    /// Invert Y in the PointingProcessor.
    #[serde(default)]
    pub proc_invert_y: bool,
    /// Swap X and Y in the PointingProcessor.
    #[serde(default)]
    pub proc_swap_xy: bool,
}

/// I²C bus configuration for the IQS5xx. Distinct from the generic `I2cConfig`
/// because the IQS5xx address is fixed (`0x74` by default; can be reprogrammed
/// at the IC, but not at runtime — exposing it would be misleading).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Iqs5xxI2cConfig {
    pub instance: String,
    pub sda: String,
    pub scl: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncoderConfig {
    // Pin a of the encoder
    pub pin_a: String,
    // Pin b of the encoder
    pub pin_b: String,
    // Phase is the working mode of the rotary encoders.
    // Available mode:
    // - default: resolution = 1
    // - e8h7: phase table tuned for E8H7 encoders
    // - resolution: customized resolution, the resolution value and reverse should be specified
    //   A typical [EC11 encoder](https://tech.alpsalpine.com/cms.media/product_catalog_ec_01_ec11e_en_611f078659.pdf)'s resolution is 2
    //   In resolution mode, you can also specify the number of detent and pulses, the resolution will be calculated by `pulse * 4 / detent`
    #[serde(default)]
    pub phase: EncoderPhase,
    // Resolution
    pub resolution: Option<EncoderResolution>,
    // The number of detent
    pub detent: Option<u8>,
    // The number of pulse
    pub pulse: Option<u8>,
    // Whether the direction of the rotary encoder is reversed.
    pub reverse: Option<bool>,
    // Use MCU's internal pull-up resistor or not, defaults to false, the external pull-up resistor is needed
    #[serde(default = "default_false")]
    pub internal_pullup: bool,
    // Debounce interval (integer ms or a "5ms" string). Suppresses spurious events
    // from mechanical contact bounce. Defaults to 0 (disabled) if not specified.
    #[serde(default, deserialize_with = "de_opt_millis")]
    pub debounce_ms: Option<u16>,
}

/// Rotary encoder phase (decoding) mode
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EncoderPhase {
    #[default]
    Default,
    E8h7,
    Resolution,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum EncoderResolution {
    Value(u8),
    Derived { detent: u8, pulse: u8 },
}

impl Default for EncoderResolution {
    fn default() -> Self {
        Self::Value(4)
    }
}

/// Pointing device config
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointingDeviceConfig {
    pub interface: Option<CommunicationProtocol>,
}
