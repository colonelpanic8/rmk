use serde::Deserialize;
use serde_inline_default::serde_inline_default;

impl crate::KeyboardTomlConfig {
    pub(crate) fn get_host_config(&self) -> HostConfig {
        self.host.clone().unwrap_or_default()
    }
}

/// Configuration for host tools
#[serde_inline_default]
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostConfig {
    /// Whether Vial is enabled
    #[serde_inline_default(true)]
    pub vial_enabled: bool,
    /// Whether the RMK-native Rynk protocol is enabled. Mutually exclusive
    /// with `vial_enabled` (the underlying Cargo features conflict).
    #[serde_inline_default(false)]
    pub rynk_enabled: bool,
    /// Unlock keys for Vial (optional)
    pub unlock_keys: Option<Vec<[u8; 2]>>,
    /// Start Vial unlocked, bypassing the unlock-key combo (default: false).
    /// Only has effect with the `vial_lock` feature.
    #[serde_inline_default(false)]
    pub vial_insecure: bool,
}

/// Deserializing an empty table keeps the `serde_inline_default` attributes as
/// the single source of default values.
impl Default for HostConfig {
    fn default() -> Self {
        toml::from_str("").expect("inline defaults fill every HostConfig field")
    }
}
