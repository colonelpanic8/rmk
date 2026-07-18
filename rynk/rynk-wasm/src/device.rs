//! `WebDevice` — the web transport's [`RynkDevice`] implementation.
//!
//! Unlike the native transports, the browser owns discovery: JS runs the
//! WebSerial/WebHID chooser, opens the link, and hands the already-open
//! [`JsByteLink`] to [`WebDevice::new`]. So `WebDevice` implements only the
//! universal half of the lifecycle — [`open`](RynkDevice::open) wraps the link
//! and [`connect`](RynkDevice::connect) (the trait default) handshakes it —
//! with no Rust-side `discover`.

use std::rc::Rc;

use rynk::{RynkDevice, RynkHostError};

use crate::transport::{JsByteLink, LinkShared, WasmReader, WasmWriter};

/// A web Rynk keyboard: an already-open JS byte link, discovered and opened by
/// the browser page, plus the name the page showed in its picker.
pub struct WebDevice {
    link: JsByteLink,
    label: Option<String>,
}

impl WebDevice {
    /// Wrap an already-open JS byte link plus the display name the page showed in
    /// its device picker (`None` if the page supplied none).
    pub fn new(link: JsByteLink, label: Option<String>) -> Self {
        Self { link, label }
    }
}

impl RynkDevice for WebDevice {
    type Read = WasmReader;
    type Write = WasmWriter;

    /// The name the page showed in its chooser (WebHID `productName`, or whatever
    /// it derived for WebSerial), or a default when the page supplied none. The
    /// page owns discovery, so only it knows this name.
    fn label(&self) -> String {
        self.label.clone().unwrap_or_else(|| "Rynk keyboard".into())
    }

    /// Hand out the link's halves. Infallible: JS already opened it.
    async fn open(self) -> Result<(WasmWriter, WasmReader), RynkHostError> {
        let link = Rc::new(LinkShared(self.link));
        Ok((
            WasmWriter { link: link.clone() },
            WasmReader {
                link,
                recv: None,
                pending: Vec::new(),
                pos: 0,
            },
        ))
    }
}
