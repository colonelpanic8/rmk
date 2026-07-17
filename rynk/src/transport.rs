use embedded_io_async::{Read, Write};

/// A byte link that can transfer ownership of independent I/O halves to the
/// protocol driver.
pub trait Transport {
    type Write: Write;
    type Read: Read;

    /// Return `(host_to_device, device_to_host)`.
    fn split(self) -> (Self::Write, Self::Read);
}
