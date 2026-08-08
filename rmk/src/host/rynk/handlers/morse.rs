//! Morse handlers.

use rmk_types::morse::{Morse, MorseProfile};
use rmk_types::protocol::rynk::command::{
    DeleteMorseProfile, GetMorse, GetMorseBulk, GetMorseProfile, GetMorseProfileBulk, GetMorseProfileCount,
    GetMorseProfileState, SetMorse, SetMorseBulk, SetMorseProfile, SetMorseProfileBulk, SetMorseProfileEntry,
};
use rmk_types::protocol::rynk::{
    GetMorseBulkRequest, GetMorseProfileBulkRequest, GetMorseProfileStateRequest, MorseProfileState, RynkError,
    RynkMessage, SetMorseProfileEntryRequest, SetMorseProfileRequest, SetMorseRequest, bulk_item_capacity,
};

use super::super::RynkService;
use super::bulk::{bulk_page, take_bulk, take_element};
use super::{Handle, HandleBulk};

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

impl HandleBulk<GetMorseBulk> for RynkService<'_> {
    async fn handle_bulk(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let req = msg.decode_request::<GetMorseBulkRequest>()?;
        let cap = bulk_item_capacity(msg.capacity());
        let page = bulk_page(req.start_index as usize, cap, self.ctx.morses_len())?;
        msg.encode_bulk(page.map(|idx| self.ctx.get_morse(idx as u8).unwrap_or_default()))
    }
}

impl HandleBulk<SetMorseBulk> for RynkService<'_> {
    async fn handle_bulk(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let mut cursor = msg.payload();
        let start_index = take_element::<u8>(&mut cursor)? as usize;
        for (idx, config) in take_bulk::<Morse>(&mut cursor, start_index, self.ctx.morses_len())? {
            self.ctx.update_morse(idx as u8, |m| *m = config).await;
        }
        msg.encode_response(&())
    }
}

impl Handle<GetMorseProfileCount> for RynkService<'_> {
    async fn handle(&self, _: ()) -> Result<u8, RynkError> {
        Ok(self.ctx.morse_profiles_capacity() as u8)
    }
}

impl Handle<GetMorseProfile> for RynkService<'_> {
    async fn handle(&self, idx: u8) -> Result<MorseProfile, RynkError> {
        self.ctx.get_morse_profile(idx).ok_or(RynkError::Invalid)
    }
}

impl Handle<SetMorseProfile> for RynkService<'_> {
    async fn handle(&self, r: SetMorseProfileRequest) -> Result<(), RynkError> {
        if self.ctx.set_morse_profile(r.index, r.profile).await {
            Ok(())
        } else {
            Err(RynkError::Invalid)
        }
    }
}

impl HandleBulk<GetMorseProfileBulk> for RynkService<'_> {
    async fn handle_bulk(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let req = msg.decode_request::<GetMorseProfileBulkRequest>()?;
        let cap = bulk_item_capacity(msg.capacity());
        let page = bulk_page(req.start_index as usize, cap, self.ctx.morse_profiles_capacity())?;
        msg.encode_bulk(page.map(|idx| self.ctx.get_morse_profile(idx as u8).unwrap_or_default()))
    }
}

impl HandleBulk<SetMorseProfileBulk> for RynkService<'_> {
    async fn handle_bulk(&self, msg: &mut RynkMessage<'_>) -> Result<(), RynkError> {
        let mut cursor = msg.payload();
        let start_index = take_element::<u8>(&mut cursor)? as usize;
        let total = self.ctx.morse_profiles_capacity();
        for (idx, profile) in take_bulk::<MorseProfile>(&mut cursor, start_index, total)? {
            self.ctx.set_morse_profile(idx as u8, profile).await;
        }
        msg.encode_response(&())
    }
}

impl Handle<GetMorseProfileState> for RynkService<'_> {
    async fn handle(&self, request: GetMorseProfileStateRequest) -> Result<MorseProfileState, RynkError> {
        Ok(self.ctx.morse_profile_state(request.offset))
    }
}

impl Handle<SetMorseProfileEntry> for RynkService<'_> {
    async fn handle(&self, request: SetMorseProfileEntryRequest) -> Result<(), RynkError> {
        if self.ctx.set_morse_profile_entry(request).await {
            Ok(())
        } else {
            Err(RynkError::Invalid)
        }
    }
}

impl Handle<DeleteMorseProfile> for RynkService<'_> {
    async fn handle(&self, index: u8) -> Result<(), RynkError> {
        if self.ctx.delete_morse_profile(index).await {
            Ok(())
        } else {
            Err(RynkError::Invalid)
        }
    }
}
