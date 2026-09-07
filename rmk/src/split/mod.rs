use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use postcard::experimental::max_size::MaxSize;
use rmk_types::connection::ConnectionStatus;
use serde::{Deserialize, Serialize};
#[cfg(feature = "dfu_split")]
use serde::{Deserializer, Serializer};

#[cfg(feature = "_ble")]
use crate::event::BatteryStatusEvent;
use crate::event::{KeyboardEvent, PointingEvent};

/// Authoritative, ephemeral layer state sent over the native priority split
/// path. This is deliberately separate from durable/application replication.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Eq, PartialEq, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SplitLayerState {
    pub effective: u8,
    pub default: u8,
    active: [u8; 8],
}

impl SplitLayerState {
    pub const CAPACITY: u8 = 64;

    pub const fn new(effective: u8, default: u8, active: u64) -> Self {
        Self {
            effective,
            default,
            active: active.to_le_bytes(),
        }
    }

    pub const fn active_bits(self) -> u64 {
        u64::from_le_bytes(self.active)
    }

    pub const fn is_active(self, layer: u8) -> bool {
        layer < Self::CAPACITY && self.active_bits() & (1_u64 << layer) != 0
    }
}

impl Default for SplitLayerState {
    fn default() -> Self {
        Self::new(0, 0, 1)
    }
}

static LAYER_STATE: BlockingMutex<crate::RawMutex, Cell<SplitLayerState>> =
    BlockingMutex::new(Cell::new(SplitLayerState::new(0, 0, 1)));

/// Read the latest authoritative layer snapshot without waiting on a queue.
pub fn current_layer_state() -> SplitLayerState {
    LAYER_STATE.lock(Cell::get)
}

pub(crate) fn update_layer_state(state: SplitLayerState) {
    LAYER_STATE.lock(|current| current.set(state));
}

fn update_legacy_effective_layer(layer: u8) {
    LAYER_STATE.lock(|current| {
        let previous = current.get();
        let mut active = previous.active_bits();
        if previous.effective != previous.default && previous.effective != layer {
            active &= !(1_u64 << previous.effective);
        }
        active |= (1_u64 << previous.default) | (1_u64 << layer);
        current.set(SplitLayerState::new(layer, previous.default, active));
    });
}

#[cfg(feature = "_ble")]
pub mod ble;
pub mod central;
/// Common abstraction layer of split driver
pub(crate) mod driver;
pub mod peripheral;
#[cfg(feature = "rp2040")]
pub mod rp;
#[cfg(not(feature = "_ble"))]
pub mod serial;

/// Maximum size of a split message
pub const SPLIT_MESSAGE_MAX_SIZE: usize = SplitMessage::POSTCARD_MAX_SIZE + 4;

/// The rectangular region of the central's keymap covered by one split
/// peripheral: its matrix size, and where it sits.
#[derive(Debug, Clone, Copy)]
pub struct PeripheralMatrixConfig {
    pub rows: u8,
    pub cols: u8,
    pub row_offset: u8,
    pub col_offset: u8,
}

/// Message used from central & peripheral communication
#[repr(u8)]
#[derive(Serialize, Deserialize, Debug, Clone, Copy, MaxSize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum SplitMessage {
    /// Keyboard event, from peripheral to central
    Key(KeyboardEvent),
    /// Pointing device event, from peripheral to central
    Pointing(PointingEvent),
    /// Led state, on/off, from central to peripheral
    LedState(bool),
    /// `ConnectionStatus` snapshot of the central.
    /// Synced central → peripheral on every change.
    ConnectionStatus(ConnectionStatus),
    /// BLE Address, used in syncing address between central and peripheral
    Address([u8; 6]),
    /// Clear the saved peer info
    ClearPeer,
    /// Lock state led indicator from central to peripheral
    KeyboardIndicator(u8),
    /// Layer number from central to peripheral
    Layer(u8),
    /// WPM from central to peripheral
    #[cfg(feature = "display")]
    Wpm(u16),
    /// Modifier state from central to peripheral
    #[cfg(feature = "display")]
    Modifier(u8),
    /// Sleep state from central to peripheral
    #[cfg(feature = "_render_state")]
    SleepState(bool),
    /// Battery status, from peripheral to central
    #[cfg(feature = "_ble")]
    BatteryStatus(BatteryStatusEvent),

    // -----------------------------------------------------------------------
    // dfu_split — firmware update over split link
    // -----------------------------------------------------------------------
    /// Central → Peripheral: query the hash of the ACTIVE slot firmware.
    #[cfg(feature = "dfu_split")]
    FirmwareHashQuery,
    /// Peripheral → Central: respond with the CRC32 of the ACTIVE slot firmware.
    #[cfg(feature = "dfu_split")]
    FirmwareHashResponse(u32),
    /// Central → Peripheral: a chunk of the new firmware at a given offset.
    #[cfg(feature = "dfu_split")]
    FirmwareChunk {
        offset: u32,
        len: u16,
        data: FirmwareChunkData,
    },
    /// Peripheral → Central: acknowledge that `offset` bytes have been written,
    /// together with the CRC-32 of **this single chunk**.
    #[cfg(feature = "dfu_split")]
    FirmwareChunkAck { offset: u32, crc: u32 },
    /// Central → Peripheral: all chunks sent, peripheral should compute DFU CRC.
    #[cfg(feature = "dfu_split")]
    FirmwareUpdateComplete,
    /// Peripheral → Central: CRC-32 of the full DFU partition.
    #[cfg(feature = "dfu_split")]
    FirmwareCrcReport(u32),
    /// Central → Peripheral: end-to-end CRC matches, safe to reset.
    #[cfg(feature = "dfu_split")]
    FirmwareCrcOk,
    /// Central → Peripheral: end-to-end CRC mismatch, do NOT reset.
    #[cfg(feature = "dfu_split")]
    FirmwareCrcFail,
    /// Peripheral → Central: confirm mark_updated succeeded, about to reset.
    #[cfg(feature = "dfu_split")]
    FirmwareUpdateConfirm,
    /// opaque bounded application payload, central →
    /// peripheral (see `crate::split_app`). Kept after the legacy variants so
    /// their postcard discriminants stay stable across revisions.
    Application(crate::split_app::SplitAppData),
    /// Complete central layer state, including lower active layers hidden by
    /// a higher effective layer.
    LayerState(SplitLayerState),
}

// -----------------------------------------------------------------------
// FirmwareChunkData — buffer for dfu_split firmware transfer
// -----------------------------------------------------------------------

/// Fixed-size buffer for firmware chunk transfer over the split link.
///
/// Postcard's COBS encoding stores this as `&[u8]` (varint length prefix
/// + bytes) rather than a fixed `[u8; 256]` — necessary because serde
/// does not implement `Deserialize` for arrays larger than 32 elements.
#[cfg(feature = "dfu_split")]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct FirmwareChunkData(pub [u8; 256]);

#[cfg(feature = "dfu_split")]
impl Serialize for FirmwareChunkData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.as_slice().serialize(serializer)
    }
}

#[cfg(feature = "dfu_split")]
impl<'de> Deserialize<'de> for FirmwareChunkData {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let buf: &[u8] = Deserialize::deserialize(deserializer)?;
        if buf.len() > 256 {
            return Err(D::Error::custom("firmware chunk exceeds 256 bytes"));
        }
        let mut data = [0u8; 256];
        data[..buf.len()].copy_from_slice(buf);
        Ok(FirmwareChunkData(data))
    }
}

#[cfg(feature = "dfu_split")]
impl MaxSize for FirmwareChunkData {
    const POSTCARD_MAX_SIZE: usize = 258;
}

#[cfg(test)]
mod tests {
    use super::{SplitLayerState, SplitMessage};

    #[test]
    fn complete_layer_state_has_compact_layout() {
        let state = SplitLayerState::new(5, 1, 0b10_1010);

        assert_eq!(core::mem::size_of::<SplitLayerState>(), 10);
        assert_eq!(core::mem::align_of::<SplitLayerState>(), 1);
        assert!(state.is_active(5));
        assert!(!state.is_active(4));
    }

    #[test]
    fn legacy_layer_message_keeps_its_wire_discriminant() {
        let mut bytes = [0; 8];
        let encoded = postcard::to_slice(&SplitMessage::Layer(3), &mut bytes).unwrap();

        assert_eq!(encoded, &[7, 3]);
    }

    #[test]
    fn layer_state_snapshot_round_trips_through_the_split_message() {
        let expected = SplitLayerState::new(4, 0, 0b1_0101);
        let message = SplitMessage::LayerState(expected);
        let mut bytes = [0; 32];
        let encoded = postcard::to_slice(&message, &mut bytes).unwrap();
        let decoded = postcard::from_bytes::<SplitMessage>(encoded).unwrap();

        assert!(matches!(decoded, SplitMessage::LayerState(actual) if actual == expected));
    }
}
