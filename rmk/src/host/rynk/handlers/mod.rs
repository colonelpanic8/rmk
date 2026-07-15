//! Rynk command handlers.
//!
//! Each command row implements [`Handle`] on
//! [`RynkService`](super::RynkService), split across this directory by
//! domain. Most handlers are pure request → response functions; the trait's
//! provided [`handle_message`](Handle::handle_message) carries the shared wire
//! glue, so they never touch the wire view. The variable-length bulk endpoints
//! override `handle_message` to stream over the buffer instead (see [`bulk`]).

use rmk_types::protocol::rynk::endpoint::Endpoint;
use rmk_types::protocol::rynk::{RynkError, RynkMessage};

pub(crate) mod behavior;
mod bulk;
pub(crate) mod combo;
pub(crate) mod connection;
pub(crate) mod fork;
pub(crate) mod keymap;
pub(crate) mod layout;
pub(crate) mod macro_data;
pub(crate) mod morse;
pub(crate) mod status;
pub(crate) mod system;

/// One typed handler per command row (the `Read::read` / `read_exact` naming
/// convention): dispatch always calls [`handle_message`](Self::handle_message).
///
/// Fixed-size endpoints implement the bare [`handle`](Self::handle) primitive and
/// inherit the default `handle_message`. The variable-length bulk endpoints
/// instead override `handle_message` to stream over the buffer — decoding and
/// encoding one element at a time rather than materializing the payload `Vec` —
/// and leave `handle` at its default.
pub(super) trait Handle<E: Endpoint> {
    /// Compute the command's response — pure request → response logic, the wire
    /// never appears here. Bulk endpoints stream in `handle_message` instead, so
    /// this default stands unused for them.
    async fn handle(&self, _req: E::Request) -> Result<E::Response, RynkError> {
        Err(RynkError::Unimplemented)
    }

    /// [`handle`](Self::handle) at the wire level, in place:
    /// decode `E::Request`, await the handler, and encode the reply envelope.
    async fn handle_message(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let req = msg.decode_request::<E::Request>()?;
        let resp = self.handle(req).await?;
        msg.encode_response(&resp)
    }
}
