use serde::Deserialize;

impl crate::KeyboardTomlConfig {
    pub(crate) fn get_light_config(&self) -> LightConfig {
        let default = LightConfig::default();
        match self.light.clone() {
            Some(mut light_config) => {
                light_config.capslock = light_config.capslock.or(default.capslock);
                light_config.numslock = light_config.numslock.or(default.numslock);
                light_config.scrolllock = light_config.scrolllock.or(default.scrolllock);
                light_config
            }
            None => default,
        }
    }
}

/// Config for lights
///
/// Field aliases follow the keycode spellings (`CapsLock`, `ScrollLock`,
/// `NumLock`), so users don't have to remember this section's historical names.
#[derive(Clone, Default, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightConfig {
    #[serde(alias = "caps_lock")]
    pub capslock: Option<PinConfig>,
    #[serde(alias = "scroll_lock", alias = "scrollock")]
    pub scrolllock: Option<PinConfig>,
    #[serde(alias = "numlock", alias = "num_lock")]
    pub numslock: Option<PinConfig>,
}

/// Config for a single pin
#[derive(Clone, Default, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinConfig {
    pub pin: String,
    pub low_active: bool,
}
