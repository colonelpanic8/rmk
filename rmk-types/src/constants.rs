include!(concat!(env!("OUT_DIR"), "/constants.rs"));

/// Cells of one keymap row persisted as a single storage item. A row wider than this is
/// stored as several segments, so a host that writes whole rows costs the firmware
/// `num_cols.div_ceil(KEYMAP_SEGMENT_KEYS)` flash items per row.
pub const KEYMAP_SEGMENT_KEYS: usize = 16;
