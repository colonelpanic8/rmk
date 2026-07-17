use embedded_io_async::{Read, Write};

pub trait Transport {
    type Write: Write;
    type Read: Read;
    fn split(self) -> (Self::Write, Self::Read);
}
