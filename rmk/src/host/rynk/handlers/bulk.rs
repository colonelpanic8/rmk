//! Shared logic for the bulk transfer handlers: flat-index arithmetic plus the
//! streaming decode helpers that let a `Set*Bulk` handler walk its payload
//! element by element instead of materializing the whole `Vec`.
//!
//! Per-resource addressing (combo/morse slot index, keymap `layer/row/col`) is
//! translated to a flat index by each handler before calling the arithmetic.

use rmk_types::protocol::rynk::RynkError;
use serde::de::DeserializeOwned;

/// Clamp a bulk read to one page: at most `cap` items from flat index `start`,
/// never past `total`. An out-of-range `start` yields an empty range — the empty
/// final page that tells the host it has read everything.
pub(super) fn bulk_page(start: usize, cap: usize, total: usize) -> core::ops::Range<usize> {
    start..(start + cap).min(total)
}

/// Validate an all-or-nothing bulk write of `len` items at flat index `start`
/// into a resource of `total` items; returns the flat start offset.
pub(super) fn bulk_write_start(start: usize, len: usize, total: usize) -> Result<usize, RynkError> {
    if len == 0 || start + len > total {
        return Err(RynkError::Invalid);
    }
    Ok(start)
}

/// Decode the postcard varint a bulk `Vec` payload begins with — the element
/// count — returning it and the remaining element bytes. The count never exceeds
/// the reported bulk budget (`u8::MAX`), so a `u16` decode covers every valid
/// frame and rejects an overlong varint as malformed.
pub(super) fn take_seq_len(bytes: &[u8]) -> Result<(usize, &[u8]), RynkError> {
    let (len, rest) = postcard::take_from_bytes::<u16>(bytes).map_err(|_| RynkError::Malformed)?;
    Ok((len as usize, rest))
}

/// Pass one of the two-pass bulk write: confirm all `count` elements decode
/// cleanly, so a malformed tail aborts the whole write before any element lands
/// (all-or-nothing). Pass two re-decodes the same bytes and applies each.
pub(super) fn validate_bulk_elements<T: DeserializeOwned>(mut bytes: &[u8], count: usize) -> Result<(), RynkError> {
    for _ in 0..count {
        let (_, rest) = postcard::take_from_bytes::<T>(bytes).map_err(|_| RynkError::Malformed)?;
        bytes = rest;
    }
    Ok(())
}
