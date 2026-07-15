//! Rynk command handlers.

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

/// The dispatcher calls [`handle_message`](Self::handle_message) for every command.
pub(super) trait Handle<E: Endpoint> {
    /// Fixed-size handlers implement this method.
    /// Bulk handlers override `handle_message` to stream through the session buffer.
    async fn handle(&self, _req: E::Request) -> Result<E::Response, RynkError> {
        Err(RynkError::Unimplemented)
    }

    async fn handle_message(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let req = msg.decode_request::<E::Request>()?;
        let resp = self.handle(req).await?;
        msg.encode_response(&resp)
    }
}
