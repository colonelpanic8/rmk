use darling::FromMeta;

/// List of functions that can be overwritten
#[derive(Debug, Clone, Copy, Eq, PartialEq, FromMeta)]
pub enum Overwritten {
    Usb,
    ChipConfig,
    ChipInit,
    HostService,
    Entry,
}
