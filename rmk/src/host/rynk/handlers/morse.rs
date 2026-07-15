//! Morse handlers.

use rmk_types::constants::BULK_SIZE;
use rmk_types::morse::Morse;
use rmk_types::protocol::rynk::command::{GetMorse, GetMorseBulk, SetMorse, SetMorseBulk};
use rmk_types::protocol::rynk::{GetMorseBulkRequest, RynkError, RynkMessage, SetMorseRequest};

use super::super::RynkService;
use super::Handle;
use super::bulk::{bulk_page, bulk_write_start, take_seq_len, validate_bulk_elements};

impl Handle<GetMorse> for RynkService<'_> {
    async fn handle(&self, idx: u8) -> Result<Morse, RynkError> {
        self.ctx.get_morse(idx).ok_or(RynkError::Invalid)
    }
}

impl Handle<SetMorse> for RynkService<'_> {
    async fn handle(&self, r: SetMorseRequest) -> Result<(), RynkError> {
        if (r.index as usize) >= self.ctx.morses_len() {
            return Err(RynkError::Invalid);
        }
        self.ctx
            .update_morse(r.index, |m| {
                *m = r.config;
            })
            .await;
        Ok(())
    }
}

impl Handle<GetMorseBulk> for RynkService<'_> {
    // Streams the page straight into the response buffer — no `Vec` of `Morse`.
    async fn handle_message(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let req = msg.decode_request::<GetMorseBulkRequest>()?;
        // `bulk_page` keeps every index in range, so `get_morse` is always
        // `Some`; the `unwrap_or_default` fallback is unreachable.
        let page = bulk_page(req.start_index as usize, BULK_SIZE, self.ctx.morses_len());
        let count = page.len();
        msg.encode_bulk_ok(count, page.map(|idx| self.ctx.get_morse(idx as u8).unwrap_or_default()))
    }
}

impl Handle<SetMorseBulk> for RynkService<'_> {
    // Decodes the payload one `Morse` at a time instead of into a `Vec`.
    async fn handle_message(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        // Payload: `start_index` (u8) then a postcard seq of `Morse`.
        let (start_index, rest) = postcard::take_from_bytes::<u8>(msg.payload()).map_err(|_| RynkError::Malformed)?;
        let (count, elements) = take_seq_len(rest)?;

        // Validate the whole run first, so it applies whole or not at all.
        let start = bulk_write_start(start_index as usize, count, self.ctx.morses_len())?;
        validate_bulk_elements::<Morse>(elements, count)?;

        let mut cursor = elements;
        for idx in start..start + count {
            let (config, next) = postcard::take_from_bytes::<Morse>(cursor).map_err(|_| RynkError::Malformed)?;
            cursor = next;
            self.ctx.update_morse(idx as u8, |m| *m = config).await;
        }
        msg.encode_response(&())
    }
}
