//! Combo handlers.

use rmk_types::combo::Combo as ComboConfig;
use rmk_types::constants::BULK_SIZE;
use rmk_types::protocol::rynk::command::{GetCombo, GetComboBulk, SetCombo, SetComboBulk};
use rmk_types::protocol::rynk::{GetComboBulkRequest, RynkError, RynkMessage, SetComboRequest};

use super::super::RynkService;
use super::Handle;
use super::bulk::{bulk_page, bulk_write_start, take_seq_len, validate_bulk_elements};

impl Handle<GetCombo> for RynkService<'_> {
    async fn handle(&self, idx: u8) -> Result<ComboConfig, RynkError> {
        // Empty in-range slots return the empty config; OOR is an error.
        self.ctx.with_combos(|combos| {
            if (idx as usize) >= combos.len() {
                return Err(RynkError::Invalid);
            }
            Ok(combos[idx as usize]
                .as_ref()
                .map(|c| c.config.clone())
                .unwrap_or_else(ComboConfig::empty))
        })
    }
}

impl Handle<SetCombo> for RynkService<'_> {
    async fn handle(&self, r: SetComboRequest) -> Result<(), RynkError> {
        if self.ctx.set_combo(r.index, r.config).await {
            Ok(())
        } else {
            Err(RynkError::Invalid)
        }
    }
}

impl Handle<GetComboBulk> for RynkService<'_> {
    // Streams the page straight into the response buffer — no `Vec` of `Combo`.
    async fn handle_message(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let req = msg.decode_request::<GetComboBulkRequest>()?;
        // Empty slots read back as the empty config, same as the single Get; an
        // out-of-range `start_index` yields an empty page.
        self.ctx.with_combos(|combos| {
            let page = bulk_page(req.start_index as usize, BULK_SIZE, combos.len());
            let count = page.len();
            msg.encode_bulk_ok(
                count,
                page.map(|i| {
                    combos[i]
                        .as_ref()
                        .map(|c| c.config.clone())
                        .unwrap_or_else(ComboConfig::empty)
                }),
            )
        })
    }
}

impl Handle<SetComboBulk> for RynkService<'_> {
    // Decodes the payload one `Combo` at a time instead of into a `Vec`.
    async fn handle_message(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        // Payload: `start_index` (u8) then a postcard seq of `Combo`.
        let (start_index, rest) = postcard::take_from_bytes::<u8>(msg.payload()).map_err(|_| RynkError::Malformed)?;
        let (count, elements) = take_seq_len(rest)?;

        // Validate the whole run first, so it applies whole or not at all: the
        // range fits, and every element decodes (pass one).
        let num_combos = self.ctx.with_combos(|combos| combos.len());
        let start = bulk_write_start(start_index as usize, count, num_combos)?;
        validate_bulk_elements::<ComboConfig>(elements, count)?;

        // Pass two: re-decode and apply. Range and decode are already checked,
        // so `set_combo` succeeds for every slot.
        let mut cursor = elements;
        for idx in start..start + count {
            let (config, next) = postcard::take_from_bytes::<ComboConfig>(cursor).map_err(|_| RynkError::Malformed)?;
            cursor = next;
            self.ctx.set_combo(idx as u8, config).await;
        }
        msg.encode_response(&())
    }
}
