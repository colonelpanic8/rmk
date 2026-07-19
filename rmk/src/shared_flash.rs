//! Application access to flash shared with RMK storage.
//!
//! On nRF BLE, RMK storage and application-owned persistent data must use the
//! same radio-safe flash driver. The `shared_flash` feature makes generated
//! firmware serialize both consumers through one async mutex.
//!
//! Call [`crate::shared_flash::take`] once with the application's reserved partition. The returned
//! [`crate::shared_flash::SharedFlash`] is the only application client, and its operations require
//! `&mut self`, so the request/reply protocol cannot have multiple in-flight
//! callers. Initialization validates the immutable half-open window against
//! the driver's capacity and alignment before any flash operation is allowed.
//!
//! Given a correct partition that does not overlap firmware, the bootloader,
//! or RMK storage, every application operation is contained within that
//! partition. RMK cannot infer or prove that the supplied partition itself is
//! non-overlapping.

use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embedded_storage_async::nor_flash::{NorFlash as AsyncNorFlash, ReadNorFlash as AsyncReadNorFlash};

use crate::RawMutex;

/// Bytes transferred by each service request.
const CHUNK: usize = 256;

#[derive(Clone, Copy)]
struct FlashWindow {
    start: u32,
    end: u32,
}

enum FlashRequest {
    Initialize(FlashWindow),
    Read { addr: u32, len: u16 },
    Write { addr: u32, len: u16, data: [u8; CHUNK] },
    Erase { from: u32, to: u32 },
}

enum FlashReply {
    Initialized,
    Data { data: [u8; CHUNK], len: u16 },
    Done,
    Error(FlashOpError),
}

static REQUESTS: Channel<RawMutex, FlashRequest, 1> = Channel::new();
static REPLIES: Channel<RawMutex, FlashReply, 1> = Channel::new();
static CLIENT_TAKEN: AtomicBool = AtomicBool::new(false);

/// Failure to acquire, initialize, or operate the shared flash client.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FlashOpError {
    /// Another [`SharedFlash`] client already exists.
    AlreadyTaken,
    /// The configured window is empty or reversed.
    InvalidWindow,
    /// An address range is outside the configured window or flash capacity.
    OutOfBounds,
    /// Address arithmetic overflowed.
    AddressOverflow,
    /// The window or operation does not satisfy the flash alignment.
    InvalidAlignment,
    /// The underlying flash driver returned an error.
    Driver,
    /// The service received an operation before client initialization.
    Uninitialized,
    /// The internal request/reply protocol was violated.
    Protocol,
}

/// The unique application-side shared flash client.
///
/// Obtain this with [`take`]. The configured window is immutable for the
/// lifetime of the client, and every operation borrows the client mutably.
pub struct SharedFlash {
    window: FlashWindow,
}

struct ClientAcquisitionGuard;

impl Drop for ClientAcquisitionGuard {
    fn drop(&mut self) {
        CLIENT_TAKEN.store(false, Ordering::Release);
    }
}

impl Drop for SharedFlash {
    fn drop(&mut self) {
        CLIENT_TAKEN.store(false, Ordering::Release);
    }
}

/// Acquire and initialize the unique application flash client.
///
/// `window` contains absolute flash addresses and must be a non-empty,
/// erase-page-aligned partition within the flash capacity. It must not overlap
/// firmware, the bootloader, or RMK storage. The generated `keyboard.toml`
/// integration starts the service before application tasks run. Pure-Rust
/// initialization must spawn the low-level service explicitly; see the
/// storage guide.
pub async fn take(window: Range<u32>) -> Result<SharedFlash, FlashOpError> {
    if window.start >= window.end {
        return Err(FlashOpError::InvalidWindow);
    }
    CLIENT_TAKEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| FlashOpError::AlreadyTaken)?;
    let guard = ClientAcquisitionGuard;
    let window = FlashWindow {
        start: window.start,
        end: window.end,
    };

    REQUESTS.send(FlashRequest::Initialize(window)).await;
    match REPLIES.receive().await {
        FlashReply::Initialized => {
            core::mem::forget(guard);
            Ok(SharedFlash { window })
        }
        FlashReply::Error(error) => Err(error),
        _ => Err(FlashOpError::Protocol),
    }
}

impl SharedFlash {
    fn checked_range(&self, start: u32, len: usize) -> Result<(), FlashOpError> {
        let len = u32::try_from(len).map_err(|_| FlashOpError::AddressOverflow)?;
        let end = start.checked_add(len).ok_or(FlashOpError::AddressOverflow)?;
        if start < self.window.start || end > self.window.end {
            return Err(FlashOpError::OutOfBounds);
        }
        Ok(())
    }

    async fn exchange(&mut self, request: FlashRequest) -> FlashReply {
        REQUESTS.send(request).await;
        REPLIES.receive().await
    }

    /// Read `buf.len()` bytes from an absolute flash address.
    pub async fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), FlashOpError> {
        self.checked_range(addr, buf.len())?;
        let mut done = 0usize;
        while done < buf.len() {
            let len = (buf.len() - done).min(CHUNK);
            let offset = u32::try_from(done).map_err(|_| FlashOpError::AddressOverflow)?;
            let at = addr.checked_add(offset).ok_or(FlashOpError::AddressOverflow)?;
            match self
                .exchange(FlashRequest::Read {
                    addr: at,
                    len: len as u16,
                })
                .await
            {
                FlashReply::Data { data, len: reply_len } if reply_len as usize == len => {
                    buf[done..done + len].copy_from_slice(&data[..len]);
                    done += len;
                }
                FlashReply::Error(error) => return Err(error),
                _ => return Err(FlashOpError::Protocol),
            }
        }
        Ok(())
    }

    /// Program `data` at an absolute flash address.
    pub async fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), FlashOpError> {
        self.checked_range(addr, data.len())?;
        let mut done = 0usize;
        while done < data.len() {
            let len = (data.len() - done).min(CHUNK);
            let offset = u32::try_from(done).map_err(|_| FlashOpError::AddressOverflow)?;
            let at = addr.checked_add(offset).ok_or(FlashOpError::AddressOverflow)?;
            let mut chunk = [0u8; CHUNK];
            chunk[..len].copy_from_slice(&data[done..done + len]);
            match self
                .exchange(FlashRequest::Write {
                    addr: at,
                    len: len as u16,
                    data: chunk,
                })
                .await
            {
                FlashReply::Done => done += len,
                FlashReply::Error(error) => return Err(error),
                _ => return Err(FlashOpError::Protocol),
            }
        }
        Ok(())
    }

    /// Erase the absolute half-open range `from..to`, one page at a time.
    pub async fn erase(&mut self, from: u32, to: u32) -> Result<(), FlashOpError> {
        let len = to.checked_sub(from).ok_or(FlashOpError::InvalidWindow)?;
        if len == 0 {
            return Err(FlashOpError::InvalidWindow);
        }
        self.checked_range(from, len as usize)?;
        match self.exchange(FlashRequest::Erase { from, to }).await {
            FlashReply::Done => Ok(()),
            FlashReply::Error(error) => Err(error),
            _ => Err(FlashOpError::Protocol),
        }
    }
}

fn range_in_window(window: FlashWindow, start: u32, len: u32) -> Result<(), FlashOpError> {
    let end = start.checked_add(len).ok_or(FlashOpError::AddressOverflow)?;
    if start < window.start || end > window.end {
        return Err(FlashOpError::OutOfBounds);
    }
    Ok(())
}

fn aligned(value: u32, alignment: usize) -> bool {
    u32::try_from(alignment).is_ok_and(|alignment| alignment != 0 && value.is_multiple_of(alignment))
}

async fn process_request<F: AsyncNorFlash>(
    flash: &'static FlashMutex<F>,
    window: &mut Option<FlashWindow>,
    request: FlashRequest,
) -> FlashReply {
    match request {
        FlashRequest::Initialize(candidate) => {
            let capacity = flash.lock().await.capacity();
            let within_capacity = usize::try_from(candidate.end).is_ok_and(|end| end <= capacity);
            if candidate.start >= candidate.end {
                FlashReply::Error(FlashOpError::InvalidWindow)
            } else if !within_capacity {
                FlashReply::Error(FlashOpError::OutOfBounds)
            } else if !aligned(candidate.start, F::ERASE_SIZE) || !aligned(candidate.end, F::ERASE_SIZE) {
                FlashReply::Error(FlashOpError::InvalidAlignment)
            } else {
                *window = Some(candidate);
                FlashReply::Initialized
            }
        }
        FlashRequest::Read { addr, len } => {
            let Some(window) = *window else {
                return FlashReply::Error(FlashOpError::Uninitialized);
            };
            let len = len as usize;
            if len == 0 || len > CHUNK {
                return FlashReply::Error(FlashOpError::Protocol);
            }
            if let Err(error) = range_in_window(window, addr, len as u32) {
                return FlashReply::Error(error);
            }
            if !aligned(addr, F::READ_SIZE) || !len.is_multiple_of(F::READ_SIZE) {
                return FlashReply::Error(FlashOpError::InvalidAlignment);
            }
            let mut data = [0u8; CHUNK];
            match flash.lock().await.read(addr, &mut data[..len]).await {
                Ok(()) => FlashReply::Data { data, len: len as u16 },
                Err(_) => FlashReply::Error(FlashOpError::Driver),
            }
        }
        FlashRequest::Write { addr, len, data } => {
            let Some(window) = *window else {
                return FlashReply::Error(FlashOpError::Uninitialized);
            };
            let len = len as usize;
            if len == 0 || len > CHUNK {
                return FlashReply::Error(FlashOpError::Protocol);
            }
            if let Err(error) = range_in_window(window, addr, len as u32) {
                return FlashReply::Error(error);
            }
            if !aligned(addr, F::WRITE_SIZE) || !len.is_multiple_of(F::WRITE_SIZE) {
                return FlashReply::Error(FlashOpError::InvalidAlignment);
            }
            match flash.lock().await.write(addr, &data[..len]).await {
                Ok(()) => FlashReply::Done,
                Err(_) => FlashReply::Error(FlashOpError::Driver),
            }
        }
        FlashRequest::Erase { from, to } => {
            let Some(window) = *window else {
                return FlashReply::Error(FlashOpError::Uninitialized);
            };
            let Some(len) = to.checked_sub(from) else {
                return FlashReply::Error(FlashOpError::InvalidWindow);
            };
            if len == 0 {
                return FlashReply::Error(FlashOpError::InvalidWindow);
            }
            if let Err(error) = range_in_window(window, from, len) {
                return FlashReply::Error(error);
            }
            if !aligned(from, F::ERASE_SIZE) || !aligned(to, F::ERASE_SIZE) {
                return FlashReply::Error(FlashOpError::InvalidAlignment);
            }

            let page = match u32::try_from(F::ERASE_SIZE) {
                Ok(0) | Err(_) => return FlashReply::Error(FlashOpError::InvalidAlignment),
                Ok(page) => page,
            };
            let mut at = from;
            while at < to {
                let Some(end) = at.checked_add(page) else {
                    return FlashReply::Error(FlashOpError::AddressOverflow);
                };
                if end > to {
                    return FlashReply::Error(FlashOpError::InvalidAlignment);
                }
                if flash.lock().await.erase(at, end).await.is_err() {
                    return FlashReply::Error(FlashOpError::Driver);
                }
                at = end;
            }
            FlashReply::Done
        }
    }
}

/// Mutex type used by generated and pure-Rust integration code.
#[doc(hidden)]
pub type FlashMutex<F> = Mutex<RawMutex, F>;

/// Read capacity before generated code moves the flash driver into its mutex.
#[doc(hidden)]
pub fn flash_capacity<F: AsyncReadNorFlash>(flash: &F) -> usize {
    flash.capacity()
}

/// Serve application requests against a shared flash driver.
///
/// Generated `keyboard.toml` firmware spawns this automatically. Pure-Rust
/// initialization must spawn it exactly once before calling [`take`].
#[doc(hidden)]
pub async fn service<F: AsyncNorFlash>(flash: &'static FlashMutex<F>) -> ! {
    let mut window = None;
    loop {
        let reply = process_request(flash, &mut window, REQUESTS.receive().await).await;
        REPLIES.send(reply).await;
    }
}

/// Adapter passed to RMK storage by generated integration code.
#[doc(hidden)]
pub struct StorageFlash<F: 'static> {
    inner: &'static FlashMutex<F>,
    capacity: usize,
}

impl<F: AsyncReadNorFlash> StorageFlash<F> {
    /// Create the storage adapter with capacity measured before the driver is
    /// moved into its mutex. Zero capacity is a deterministic initialization
    /// failure rather than a silently unusable adapter.
    #[doc(hidden)]
    pub fn new(inner: &'static FlashMutex<F>, capacity: usize) -> Self {
        assert!(capacity > 0, "shared flash driver reported zero capacity");
        Self { inner, capacity }
    }
}

impl<F: embedded_storage::nor_flash::ErrorType> embedded_storage::nor_flash::ErrorType for StorageFlash<F> {
    type Error = F::Error;
}

impl<F: AsyncReadNorFlash> AsyncReadNorFlash for StorageFlash<F> {
    const READ_SIZE: usize = F::READ_SIZE;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.inner.lock().await.read(offset, bytes).await
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<F: AsyncNorFlash> AsyncNorFlash for StorageFlash<F> {
    const WRITE_SIZE: usize = F::WRITE_SIZE;
    const ERASE_SIZE: usize = F::ERASE_SIZE;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.inner.lock().await.erase(from, to).await
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.inner.lock().await.write(offset, bytes).await
    }
}

impl<F: embedded_storage_async::nor_flash::MultiwriteNorFlash> embedded_storage_async::nor_flash::MultiwriteNorFlash
    for StorageFlash<F>
{
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::vec;
    use std::vec::Vec;

    use embassy_futures::select::{Either, select};
    use embedded_storage_async::nor_flash::{NorFlashError, NorFlashErrorKind};

    use super::*;
    use crate::test_support::test_block_on as block_on;

    const CAPACITY: usize = 1024;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Operation {
        Read(u32, usize),
        Write(u32, usize),
        Erase(u32, u32),
    }

    #[derive(Clone, Copy, Debug)]
    struct FakeError;

    impl NorFlashError for FakeError {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }

    struct FakeState {
        bytes: Vec<u8>,
        capacity: usize,
        operations: Vec<Operation>,
        fail: Option<Operation>,
    }

    struct FakeFlash {
        state: Arc<StdMutex<FakeState>>,
    }

    impl embedded_storage::nor_flash::ErrorType for FakeFlash {
        type Error = FakeError;
    }

    impl AsyncReadNorFlash for FakeFlash {
        const READ_SIZE: usize = 1;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let operation = Operation::Read(offset, bytes.len());
            let mut state = self.state.lock().unwrap();
            state.operations.push(operation);
            if state.fail == Some(operation) {
                return Err(FakeError);
            }
            let start = offset as usize;
            bytes.copy_from_slice(&state.bytes[start..start + bytes.len()]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.state.lock().unwrap().capacity
        }
    }

    impl AsyncNorFlash for FakeFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 16;

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let operation = Operation::Write(offset, bytes.len());
            let mut state = self.state.lock().unwrap();
            state.operations.push(operation);
            if state.fail == Some(operation) {
                return Err(FakeError);
            }
            let start = offset as usize;
            state.bytes[start..start + bytes.len()].copy_from_slice(bytes);
            Ok(())
        }

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            let operation = Operation::Erase(from, to);
            let mut state = self.state.lock().unwrap();
            state.operations.push(operation);
            if state.fail == Some(operation) {
                return Err(FakeError);
            }
            state.bytes[from as usize..to as usize].fill(0xff);
            Ok(())
        }
    }

    fn fake_flash(
        capacity: usize,
        fail: Option<Operation>,
    ) -> (&'static FlashMutex<FakeFlash>, Arc<StdMutex<FakeState>>) {
        let state = Arc::new(StdMutex::new(FakeState {
            bytes: vec![0xff; capacity],
            capacity,
            operations: Vec::new(),
            fail,
        }));
        let flash = Box::leak(Box::new(FlashMutex::new(FakeFlash { state: state.clone() })));
        (flash, state)
    }

    fn drive<T>(flash: &'static FlashMutex<FakeFlash>, future: impl Future<Output = T>) -> T {
        block_on(async {
            match select(future, service(flash)).await {
                Either::First(output) => output,
                Either::Second(never) => never,
            }
        })
    }

    #[test]
    fn chunks_reads_and_writes() {
        let (flash, state) = fake_flash(CAPACITY, None);
        let data: Vec<u8> = (0..520).map(|value| value as u8).collect();
        let mut read_back = vec![0; data.len()];

        drive(flash, async {
            let mut client = take(0..CAPACITY as u32).await.unwrap();
            client.write(0, &data).await.unwrap();
            client.read(0, &mut read_back).await.unwrap();
        });

        assert_eq!(read_back, data);
        assert_eq!(
            state.lock().unwrap().operations,
            [
                Operation::Write(0, 256),
                Operation::Write(256, 256),
                Operation::Write(512, 8),
                Operation::Read(0, 256),
                Operation::Read(256, 256),
                Operation::Read(512, 8),
            ]
        );
    }

    #[test]
    fn enforces_window_boundaries_without_touching_flash() {
        let (flash, state) = fake_flash(CAPACITY, None);

        drive(flash, async {
            let mut client = take(16..64).await.unwrap();
            client.write(16, &[1, 2, 3, 4]).await.unwrap();
            client.write(60, &[5, 6, 7, 8]).await.unwrap();
            assert_eq!(client.write(12, &[0; 4]).await, Err(FlashOpError::OutOfBounds));
            assert_eq!(client.write(64, &[0; 4]).await, Err(FlashOpError::OutOfBounds));
        });

        assert_eq!(
            state.lock().unwrap().operations,
            [Operation::Write(16, 4), Operation::Write(60, 4)]
        );
    }

    #[test]
    fn rejects_overflow_without_touching_flash() {
        let (flash, state) = fake_flash(CAPACITY, None);

        drive(flash, async {
            let mut client = take(0..CAPACITY as u32).await.unwrap();
            let mut bytes = [0; 4];
            assert_eq!(
                client.read(u32::MAX - 1, &mut bytes).await,
                Err(FlashOpError::AddressOverflow)
            );
        });

        assert!(state.lock().unwrap().operations.is_empty());
    }

    #[test]
    fn propagates_driver_errors() {
        let failure = Operation::Write(0, 4);
        let (flash, state) = fake_flash(CAPACITY, Some(failure));

        let result = drive(flash, async {
            let mut client = take(0..CAPACITY as u32).await.unwrap();
            client.write(0, &[1, 2, 3, 4]).await
        });

        assert_eq!(result, Err(FlashOpError::Driver));
        assert_eq!(state.lock().unwrap().operations, [failure]);
    }

    #[test]
    fn erases_one_page_per_driver_operation() {
        let (flash, state) = fake_flash(CAPACITY, None);

        drive(flash, async {
            let mut client = take(0..CAPACITY as u32).await.unwrap();
            client.erase(0, 48).await.unwrap();
        });

        assert_eq!(
            state.lock().unwrap().operations,
            [
                Operation::Erase(0, 16),
                Operation::Erase(16, 32),
                Operation::Erase(32, 48)
            ]
        );
    }

    #[test]
    fn invalid_initialization_and_alignment_touch_no_flash() {
        let (flash, state) = fake_flash(CAPACITY, None);

        drive(flash, async {
            assert!(matches!(take(32..32).await, Err(FlashOpError::InvalidWindow)));
            assert!(matches!(take(1..16).await, Err(FlashOpError::InvalidAlignment)));
            let mut client = take(0..CAPACITY as u32).await.unwrap();
            assert_eq!(client.write(1, &[0; 4]).await, Err(FlashOpError::InvalidAlignment));
            assert_eq!(client.erase(0, 15).await, Err(FlashOpError::InvalidAlignment));
        });

        assert!(state.lock().unwrap().operations.is_empty());
    }

    #[test]
    fn rejects_zero_capacity_and_duplicate_clients() {
        let (zero_flash, zero_state) = fake_flash(0, None);
        let zero_result = drive(zero_flash, async { take(0..16).await });
        assert!(matches!(zero_result, Err(FlashOpError::OutOfBounds)));
        assert!(zero_state.lock().unwrap().operations.is_empty());

        let (flash, _) = fake_flash(CAPACITY, None);
        drive(flash, async {
            let client = take(0..CAPACITY as u32).await.unwrap();
            assert!(matches!(
                take(0..CAPACITY as u32).await,
                Err(FlashOpError::AlreadyTaken)
            ));
            drop(client);
        });
    }

    #[test]
    #[should_panic(expected = "shared flash driver reported zero capacity")]
    fn storage_adapter_rejects_zero_capacity() {
        let (flash, _) = fake_flash(0, None);
        let _ = StorageFlash::new(flash, 0);
    }
}
