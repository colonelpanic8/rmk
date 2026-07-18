//! Adapts a JS-owned byte link to the `rynk::io::Read`/`Write` halves a
//! [`RynkDevice`](rynk::RynkDevice) opens.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use js_sys::Uint8Array;
use rynk::io::{ErrorKind, ErrorType, Read, Write};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen]
extern "C" {
    /// JS byte link. `recv()` returns bytes, or an empty array only at EOF.
    pub type JsByteLink;

    #[wasm_bindgen(method, catch)]
    async fn send(this: &JsByteLink, frame: Uint8Array) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    async fn recv(this: &JsByteLink) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(method, catch)]
    async fn close(this: &JsByteLink) -> Result<(), JsValue>;
}

/// One in-flight `recv()` call, boxed so it can be parked in the reader.
type RecvFuture = Pin<Box<dyn Future<Output = Result<JsValue, JsValue>>>>;

/// The link shared by the two halves; closed exactly once when the session
/// (both halves) drops.
pub(crate) struct LinkShared(pub(crate) JsByteLink);

impl Drop for LinkShared {
    fn drop(&mut self) {
        let link: JsByteLink = self.0.clone().unchecked_into();
        spawn_local(async move {
            let _ = link.close().await;
        });
    }
}

/// Read half of the JS byte link, buffering `recv()` chunks.
pub struct WasmReader {
    pub(crate) link: Rc<LinkShared>,
    pub(crate) recv: Option<RecvFuture>,
    /// Holds a chunk larger than one `read` buffer across reads.
    pub(crate) pending: Vec<u8>,
    pub(crate) pos: usize,
}

impl ErrorType for WasmReader {
    type Error = ErrorKind;
}

impl Read for WasmReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        // Refill once the current chunk is drained.
        while self.pos >= self.pending.len() {
            if self.recv.is_none() {
                // Clone the handle into the future so it owns all it borrows.
                let link: JsByteLink = self.link.0.clone().unchecked_into();
                self.recv = Some(Box::pin(async move { link.recv().await }));
            }
            // Retain the future so a cancelled read resumes the same JS receive.
            let value = self.recv.as_mut().unwrap().await.map_err(|_| ErrorKind::Other)?;
            self.recv = None;
            // Only an empty byte array is EOF; any other JS value is invalid data.
            let chunk = value.dyn_into::<Uint8Array>().map_err(|_| ErrorKind::InvalidData)?;
            if chunk.length() == 0 {
                return Ok(0); // EOF
            }
            self.pending = chunk.to_vec();
            self.pos = 0;
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Write half of the JS byte link.
pub struct WasmWriter {
    pub(crate) link: Rc<LinkShared>,
}

impl ErrorType for WasmWriter {
    type Error = ErrorKind;
}

impl Write for WasmWriter {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.link
            .0
            .send(Uint8Array::from(buf))
            .await
            .map_err(|_| ErrorKind::Other)?;
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
