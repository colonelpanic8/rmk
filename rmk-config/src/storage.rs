use serde::Deserialize;

use crate::default_true;

impl crate::KeyboardTomlConfig {
    pub(crate) fn get_storage_config(&self) -> StorageConfig {
        self.storage.unwrap_or_default()
    }
}

/// Config for storage
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StorageConfig {
    /// Start address of local storage, MUST BE start of a sector.
    /// If start_addr is set to 0(this is the default value), the last `num_sectors` sectors will be used.
    pub start_addr: Option<usize>,
    // Number of sectors used for storage, >= 2.
    pub num_sectors: Option<u8>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    // Clear on the storage at reboot, set this to true if you want to reset the keymap
    pub clear_storage: Option<bool>,
    // Clear on the layout at reboot, set this to true if you want to reset the layout
    pub clear_layout: Option<bool>,
}
