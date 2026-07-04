use serde::Deserialize;

use crate::communication::CommunicationProtocol;
use crate::duration::de_opt_millis;

impl crate::KeyboardTomlConfig {
    pub(crate) fn get_display_config(&self) -> Option<DisplayConfig> {
        self.display.clone()
    }
}

/// Display driver type
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayDriver {
    Ssd1306,
    Sh1106,
    Sh1107,
    Sh1108,
    Ssd1309,
}

/// Display configuration
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    pub driver: DisplayDriver,
    pub protocol: CommunicationProtocol,
    pub size: String,
    #[serde(default)]
    pub rotation: u16,
    pub renderer: Option<String>,
    /// Poll interval for periodic redraws (integer ms or a "100ms" string).
    /// When absent, polling is disabled — the display only redraws on events.
    #[serde(default, deserialize_with = "de_opt_millis")]
    pub render_interval: Option<u64>,
    /// Minimum time between event-driven renders (integer ms or a "10ms" string).
    /// Prevents the display from being hammered by rapid events. Default: 10 ms.
    #[serde(default, deserialize_with = "de_opt_millis")]
    pub min_render_interval: Option<u64>,
}
