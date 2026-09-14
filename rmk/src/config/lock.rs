/// Rynk maintenance and legacy physical-lock configuration, emitted by the
/// macro from `[host]` in keyboard.toml.
#[derive(Clone, Copy, Debug)]
pub struct LockConfig {
    /// Physical `(row, col)` keys held simultaneously to unlock. Empty ⇒ no
    /// unlock possible.
    pub unlock_keys: &'static [(u8, u8)],
    /// Start (and stay) unlocked — development escape hatch.
    pub insecure: bool,
    /// Legacy physical-lock write policy, retained for configuration
    /// compatibility. It does not authorize Rynk commands.
    pub write_requires_unlock: bool,
    /// Initial state of the maintenance-operation gate. A key action may
    /// change the live state until reboot.
    pub maintenance_mode_default: bool,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            unlock_keys: &[],
            insecure: false,
            write_requires_unlock: false,
            maintenance_mode_default: true,
        }
    }
}
