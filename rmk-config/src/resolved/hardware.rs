//! Resolved hardware types for the public API of `rmk-config`.
//!
//! This module is the deliberate boundary between the TOML schema and codegen:
//! leaf types whose TOML shape is already what codegen needs (pins, sensor
//! configs, split boards) are re-exported 1:1, while types that require real
//! transformation (`Storage`, `Hardware`, chip/communication resolution) are
//! defined here. Consumers import everything through this module, so schema
//! types can move between rmk-config's internal modules without touching them.

// Re-export leaf types from the TOML schema modules
pub use crate::DependencyConfig;
pub use crate::board::{
    BoardConfig, DebouncerType, MatrixConfig, MatrixType, OutputConfig, SerialConfig, SplitBoardConfig, SplitConfig,
    SplitConnection, UniBodyConfig,
};
pub use crate::chip::{ChipConfig, ChipModel, ChipSeries, DcdcReg0Voltage};
pub use crate::communication::{BleConfig, CommunicationConfig, CommunicationProtocol, I2cConfig, SpiConfig, UsbInfo};
pub use crate::display::{DisplayConfig, DisplayDriver};
pub use crate::input_device::{
    EncoderConfig, EncoderPhase, EncoderResolution, InputDeviceConfig, Iqs5xxConfig, Iqs5xxI2cConfig, JoystickConfig,
    Pmw33xxConfig, Pmw33xxType, Pmw3610Config, PointingDeviceConfig,
};
pub use crate::keymap::KeyInfo;
pub use crate::light::{LightConfig, PinConfig};

/// Resolved storage hardware config
pub struct Storage {
    pub start_addr: usize,
    pub num_sectors: u8,
    pub clear_storage: bool,
    pub clear_layout: bool,
}

/// Complete hardware configuration for init code generation.
pub struct Hardware {
    pub chip: ChipModel,
    pub chip_config: ChipConfig,
    pub communication: CommunicationConfig,
    pub board: BoardConfig,
    pub storage: Option<Storage>,
    pub light: LightConfig,
    pub display: Option<DisplayConfig>,
    pub output: Vec<OutputConfig>,
    pub dependency: DependencyConfig,
}

impl crate::KeyboardTomlConfig {
    /// Resolve hardware configuration from TOML config.
    pub fn hardware(&self) -> Result<Hardware, String> {
        let chip = self.get_chip_model()?;
        let chip_config = self.get_chip_config();
        let communication = self.get_communication_config()?;
        let board = self.get_board_config()?;
        let storage_toml = self.get_storage_config();
        let storage = if storage_toml.enabled {
            Some(Storage {
                start_addr: storage_toml.start_addr.unwrap_or(0),
                num_sectors: storage_toml.num_sectors.unwrap_or(2),
                clear_storage: storage_toml.clear_storage.unwrap_or(false),
                clear_layout: storage_toml.clear_layout.unwrap_or(false),
            })
        } else {
            None
        };
        let light = self.get_light_config();
        let display = self.get_display_config();
        let output = self.get_output_config()?;
        let dependency = self.get_dependency_config();
        Ok(Hardware {
            chip,
            chip_config,
            communication,
            board,
            storage,
            light,
            display,
            output,
            dependency,
        })
    }
}
