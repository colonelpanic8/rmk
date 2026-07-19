//! Shared access to the (radio-safe) flash driver so the application can run
//! its own runtime-config partition alongside RMK's storage task.
//!
//! Motivation: on nRF+BLE the only safe flash driver is `nrf_mpsl::Flash` —
//! a singleton that schedules erases/writes into multiprotocol timeslots —
//! and RMK's generated `main` hands it wholesale to the storage task. A
//! firmware that needs a second flash consumer (for example a reserved
//! runtime-config partition holding persistent application settings) can no
//! longer take the driver exclusively, so the generated code wraps the
//! singleton in an async mutex and shares it:
//!
//! - RMK storage receives a [`SharedFlash`] wrapper (locks per NorFlash op).
//! - A tiny service task ([`service`]) executes bounded application requests
//!   ([`REQUESTS`]/[`REPLIES`]) against the same mutex, one chunk or one
//!   erase page per lock acquisition, so neither consumer can starve the
//!   other and no flash work ever blocks key scanning (everything stays
//!   async on the shared executor).
//!
//! Safety rail: the service refuses to touch flash outside the window the
//! application registers with [`set_window`] (e.g. its reserved partition),
//! so a bug cannot erase the running image or RMK's storage.
//!
//! Single client by design: exactly one application task may use the
//! [`read`]/[`write`]/[`erase`] helpers (they pair one request with one
//! reply) — typically the task that owns the runtime-config partition.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embedded_storage_async::nor_flash::NorFlash as AsyncNorFlash;
use embedded_storage_async::nor_flash::ReadNorFlash as AsyncReadNorFlash;

use crate::RawMutex;

/// Bytes moved per request. Small enough to keep the channels cheap, large
/// enough that an 8 KiB blob moves in ~32 requests.
pub const CHUNK: usize = 256;

/// One bounded flash request from the application.
pub enum FlashRequest {
    /// Read `len <= CHUNK` bytes at `addr`.
    Read { addr: u32, len: u16 },
    /// Program `len <= CHUNK` bytes at `addr` (respect the flash's write
    /// alignment: on nRF52, 4-byte aligned address and length).
    Write { addr: u32, len: u16, data: [u8; CHUNK] },
    /// Erase `from..to` (must be erase-page aligned). Executed one page per
    /// mutex lock so storage traffic interleaves.
    Erase { from: u32, to: u32 },
}

/// The service's answer to one [`FlashRequest`].
pub enum FlashReply {
    Data { data: [u8; CHUNK], len: u16 },
    Done,
    /// Driver error or window violation. The service logs the detail.
    Error,
}

/// Application → service. Capacity 1: one request in flight (the client
/// helpers await the reply before sending the next request).
pub static REQUESTS: Channel<RawMutex, FlashRequest, 1> = Channel::new();
/// Service → application.
pub static REPLIES: Channel<RawMutex, FlashReply, 1> = Channel::new();

/// Address window (absolute flash addresses) the service may operate in.
/// Nothing is allowed until [`set_window`] runs.
static WINDOW_START: AtomicU32 = AtomicU32::new(0);
static WINDOW_END: AtomicU32 = AtomicU32::new(0);

/// Register the only address range [`service`] will touch.
pub fn set_window(start: u32, end: u32) {
    WINDOW_START.store(start, Ordering::Relaxed);
    WINDOW_END.store(end, Ordering::Relaxed);
}

fn in_window(start: u32, len: u32) -> bool {
    let lo = WINDOW_START.load(Ordering::Relaxed);
    let hi = WINDOW_END.load(Ordering::Relaxed);
    let Some(end) = start.checked_add(len) else {
        return false;
    };
    lo < hi && start >= lo && end <= hi
}

/// Serve application flash requests against the shared driver forever.
/// Spawned by the generated `main` alongside the other firmware tasks.
pub async fn service<F: AsyncNorFlash>(flash: &'static Mutex<RawMutex, F>) -> ! {
    loop {
        let reply = match REQUESTS.receive().await {
            FlashRequest::Read { addr, len } => {
                let len = (len as usize).min(CHUNK);
                if !in_window(addr, len as u32) {
                    error!("shared_flash: read outside window: 0x{:x}+{}", addr, len);
                    FlashReply::Error
                } else {
                    let mut data = [0u8; CHUNK];
                    let res = { flash.lock().await.read(addr, &mut data[..len]).await };
                    match res {
                        Ok(()) => FlashReply::Data { data, len: len as u16 },
                        Err(_) => {
                            error!("shared_flash: read failed at 0x{:x}", addr);
                            FlashReply::Error
                        }
                    }
                }
            }
            FlashRequest::Write { addr, len, data } => {
                let len = (len as usize).min(CHUNK);
                if !in_window(addr, len as u32) {
                    error!("shared_flash: write outside window: 0x{:x}+{}", addr, len);
                    FlashReply::Error
                } else {
                    let res = { flash.lock().await.write(addr, &data[..len]).await };
                    match res {
                        Ok(()) => FlashReply::Done,
                        Err(_) => {
                            error!("shared_flash: write failed at 0x{:x}", addr);
                            FlashReply::Error
                        }
                    }
                }
            }
            FlashRequest::Erase { from, to } => {
                if to < from || !in_window(from, to - from) {
                    error!("shared_flash: erase outside window: 0x{:x}..0x{:x}", from, to);
                    FlashReply::Error
                } else {
                    // One erase page per lock acquisition; the storage task
                    // can interleave between pages.
                    let page = F::ERASE_SIZE as u32;
                    let mut at = from;
                    let mut ok = true;
                    while at < to {
                        let end = (at + page).min(to);
                        let res = { flash.lock().await.erase(at, end).await };
                        if res.is_err() {
                            error!("shared_flash: erase failed at 0x{:x}", at);
                            ok = false;
                            break;
                        }
                        at = end;
                    }
                    if ok { FlashReply::Done } else { FlashReply::Error }
                }
            }
        };
        REPLIES.send(reply).await;
    }
}

/// Application-side error: the service reported a failure (details in its
/// log) or a protocol hiccup.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct FlashOpError;

/// Read `buf.len()` bytes at `addr` (chunked; single client only).
pub async fn read(addr: u32, buf: &mut [u8]) -> Result<(), FlashOpError> {
    let mut done = 0usize;
    while done < buf.len() {
        let want = (buf.len() - done).min(CHUNK);
        REQUESTS
            .send(FlashRequest::Read { addr: addr + done as u32, len: want as u16 })
            .await;
        match REPLIES.receive().await {
            FlashReply::Data { data, len } if len as usize == want => {
                buf[done..done + want].copy_from_slice(&data[..want]);
                done += want;
            }
            _ => return Err(FlashOpError),
        }
    }
    Ok(())
}

/// Program `data` at `addr` (chunked; caller supplies aligned addr/len).
pub async fn write(addr: u32, data: &[u8]) -> Result<(), FlashOpError> {
    let mut done = 0usize;
    while done < data.len() {
        let want = (data.len() - done).min(CHUNK);
        let mut chunk = [0u8; CHUNK];
        chunk[..want].copy_from_slice(&data[done..done + want]);
        REQUESTS
            .send(FlashRequest::Write { addr: addr + done as u32, len: want as u16, data: chunk })
            .await;
        match REPLIES.receive().await {
            FlashReply::Done => done += want,
            _ => return Err(FlashOpError),
        }
    }
    Ok(())
}

/// Erase `from..to` (erase-page aligned).
pub async fn erase(from: u32, to: u32) -> Result<(), FlashOpError> {
    REQUESTS.send(FlashRequest::Erase { from, to }).await;
    match REPLIES.receive().await {
        FlashReply::Done => Ok(()),
        _ => Err(FlashOpError),
    }
}

/// [`AsyncNorFlash`] adapter over a shared `&'static Mutex<_, F>`: locks the
/// mutex around every driver operation. Handed to RMK's storage task in
/// place of the exclusive driver.
pub struct SharedFlash<F: 'static> {
    inner: &'static Mutex<RawMutex, F>,
    /// Cached because [`AsyncReadNorFlash::capacity`] is synchronous.
    capacity: usize,
}

impl<F: AsyncReadNorFlash> SharedFlash<F> {
    /// Wrap a freshly created shared driver. Must be called while nothing
    /// else holds the mutex (i.e. right where the driver is constructed).
    pub fn new(inner: &'static Mutex<RawMutex, F>) -> Self {
        let capacity = inner.try_lock().map(|f| f.capacity()).unwrap_or(0);
        debug_assert!(capacity > 0, "SharedFlash::new must run before the mutex is contended");
        Self { inner, capacity }
    }
}

impl<F: embedded_storage::nor_flash::ErrorType> embedded_storage::nor_flash::ErrorType
    for SharedFlash<F>
{
    type Error = F::Error;
}

impl<F: AsyncReadNorFlash> AsyncReadNorFlash for SharedFlash<F> {
    const READ_SIZE: usize = F::READ_SIZE;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.inner.lock().await.read(offset, bytes).await
    }

    fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<F: AsyncNorFlash> AsyncNorFlash for SharedFlash<F> {
    const WRITE_SIZE: usize = F::WRITE_SIZE;
    const ERASE_SIZE: usize = F::ERASE_SIZE;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.inner.lock().await.erase(from, to).await
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.inner.lock().await.write(offset, bytes).await
    }
}

impl<F: embedded_storage_async::nor_flash::MultiwriteNorFlash>
    embedded_storage_async::nor_flash::MultiwriteNorFlash for SharedFlash<F>
{
}
