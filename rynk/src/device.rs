//! Device discovery handle and transport-to-driver construction.

use alloc::string::String;

use crate::driver::{Client, Driver, RynkHostError};
use crate::transport::Transport;

/// A keyboard recognized as Rynk's but not yet connected.
///
/// Discovery remains transport-specific. Opening a device returns a [`Client`]
/// and its [`Driver`]; callers run the driver concurrently and then call
/// [`Client::handshake`] before using versioned endpoints.
#[allow(async_fn_in_trait)]
pub trait RynkDevice: Sized {
    type Transport: Transport;

    fn label(&self) -> String;

    async fn open(self) -> Result<Self::Transport, RynkHostError>;

    async fn connect(
        self,
    ) -> Result<
        (
            Client<'static>,
            Driver<'static, <Self::Transport as Transport>::Read, <Self::Transport as Transport>::Write>,
        ),
        RynkHostError,
    > {
        Ok(Client::from_transport(self.open().await?))
    }
}
