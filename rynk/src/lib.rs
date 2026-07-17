#![cfg_attr(not(feature = "std"), no_std)]
//! Runtime-free Rynk protocol client with a no-alloc core.

//!
//! A [`Transport`] splits into independently owned [`Reader`] and [`Writer`]
//! halves. [`Driver`] continuously drives both halves while [`Client`] sends
//! owned requests through bounded channels. This crate does not depend on an
//! async runtime; the caller runs [`Driver::run`] on its executor.
//!
//! ```no_run
//! # use rynk::Client;
//! # async fn run<T: rynk::Transport>(transport: T) -> Result<(), Box<dyn std::error::Error>> {
//! let (mut client, mut driver) = Client::from_transport(transport);
//! // Run these futures concurrently on the application's executor.
//! let driver_task = driver.run();
//! let client_task = async {
//!     client.handshake().await?;
//!     let layer = client.get_current_layer().await?;
//!     println!("active layer: {layer}");
//!     Ok::<_, rynk::RynkHostError>(())
//! };
//! let _ = (driver_task, client_task);
//! # Ok(()) }
//! ```
//!
//! ## Transport contract
//!
//! Implement [`Transport`] using [`io::Read`] + [`io::Write`] halves from the
//! [`io`] re-export so the trait version always matches this crate.
//!
//! - **Writes deliver without flush** — the client never calls
//!   [`flush`](io::Write::flush). A successful `write` MUST commit the returned
//!   bytes; on a lossy medium (e.g. BLE) use acknowledged writes, since a lost
//!   chunk desyncs the firmware's reassembler with no mid-frame resync.
//! - Client request and topic futures are cancellation-safe because transport
//!   reads remain owned by [`Reader`]. [`Driver::run`] itself is long-lived and
//!   should not be cancelled during a partial transport write.
//! - `read` may return arbitrary chunk boundaries; the client reassembles
//!   frames. `Ok(0)` means the link is gone and surfaces as
//!   [`RynkHostError::Disconnected`].
//!
//! ## Multi-version dispatch
//!
//! [`Client::handshake`] rejects only a protocol **major** mismatch. To support
//! several majors at once, link one `rynk` build per major (cargo `package`
//! renames) and probe with the newest first.
//! The probe(`GetVersion`) itself is frozen across all majors by the protocol ICD.

#[cfg(feature = "alloc")]
extern crate alloc;

mod api;
#[cfg(feature = "std")]
mod device;
mod driver;
#[cfg(feature = "alloc")]
pub mod layout;
mod transport;

pub use api::IncomingTopic;
#[cfg(feature = "std")]
pub use device::RynkDevice;
pub use driver::{
    Client, DEFAULT_EVENT_CAPACITY, DEFAULT_FRAME_SIZE, Driver, Reader, RynkHostError, Session, TOPIC_PAYLOAD_SIZE,
    TopicFrame, Writer,
};
pub use embedded_io_async as io;
#[cfg(feature = "alloc")]
pub use layout::LayoutInfo;
pub use rmk_types;
/// The decoded topic union carried by [`IncomingTopic::Topic`]
pub use rmk_types::protocol::rynk::TopicEvent;
pub use transport::Transport;
