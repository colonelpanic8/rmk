use embassy_futures::yield_now;
use embedded_storage_async::nor_flash::{ErrorType, NorFlash, ReadNorFlash};

const OPERATIONS_PER_YIELD: u8 = 32;

/// Bound synchronous flash work during uncached map scans and garbage collection.
pub(crate) struct CooperativeFlash<F> {
    flash: F,
    operations: u8,
}

impl<F> CooperativeFlash<F> {
    pub(crate) fn new(flash: F) -> Self {
        Self { flash, operations: 0 }
    }

    async fn cooperate(&mut self) {
        if self.operations == OPERATIONS_PER_YIELD {
            yield_now().await;
            self.operations = 0;
        }
        self.operations += 1;
    }
}

impl<F: ErrorType> ErrorType for CooperativeFlash<F> {
    type Error = F::Error;
}

impl<F: ReadNorFlash> ReadNorFlash for CooperativeFlash<F> {
    const READ_SIZE: usize = F::READ_SIZE;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.cooperate().await;
        self.flash.read(offset, bytes).await
    }

    fn capacity(&self) -> usize {
        self.flash.capacity()
    }
}

impl<F: NorFlash> NorFlash for CooperativeFlash<F> {
    const WRITE_SIZE: usize = F::WRITE_SIZE;
    const ERASE_SIZE: usize = F::ERASE_SIZE;

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.cooperate().await;
        self.flash.write(offset, bytes).await
    }

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.cooperate().await;
        self.flash.erase(from, to).await
    }
}
