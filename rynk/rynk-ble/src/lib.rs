//! BLE GATT transport using `bluest`.
//!
//! A Rynk keyboard is identified by its service UUID (`RYNK_SERVICE_UUID`), not its
//! user-settable BLE name — the counterpart to the serial transport's serial marker.
//! [`BleDevice::discover`] lists already-connected devices exposing that service (no
//! scan, no attach); [`RynkDevice::connect`] then attaches and handshakes.

use std::time::Duration;

use async_stream::stream;
use bluest::{Adapter, Characteristic, Device, DeviceId, Uuid};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use rynk::io::{ErrorKind, Read, Write};
use rynk::rmk_types::protocol::rynk::RYNK_BLE_CHUNK_SIZE;
use rynk::{RynkDevice, RynkHostError};

const RYNK_SERVICE_UUID: Uuid = Uuid::from_u128(rynk::rmk_types::protocol::rynk::RYNK_SERVICE_UUID);
const RYNK_INPUT_CHAR_UUID: Uuid = Uuid::from_u128(rynk::rmk_types::protocol::rynk::RYNK_INPUT_CHAR_UUID);
const RYNK_OUTPUT_CHAR_UUID: Uuid = Uuid::from_u128(rynk::rmk_types::protocol::rynk::RYNK_OUTPUT_CHAR_UUID);

/// Bounds each GATT step (connect, discovery, subscribe); they carry no inherent
/// timeout, so a radio-silent device would otherwise pend forever.
const GATT_TIMEOUT: Duration = Duration::from_secs(5);

/// ATT-minimum MTU payload.
const BLE_SAFE_WRITE: usize = 20;

/// Read half of the attached Rynk GATT link: an async generator that owns the
/// input characteristic and yields each notification chunk. The `notify()`
/// borrow stays inside this one pinned state machine, so there is no
/// self-referential struct, no leak, and no task; dropping it unsubscribes
/// (bluest's guard runs) and frees the characteristic.
pub struct BleReader {
    input: BoxStream<'static, Vec<u8>>,
    /// Holds a notification chunk larger than one `read` buffer across reads.
    pending: Vec<u8>,
    pos: usize,
    /// Held so the GATT connection (owned by the central) outlives the session.
    _adapter: Adapter,
}

impl rynk::io::ErrorType for BleReader {
    type Error = std::io::Error;
}

impl Read for BleReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        while self.pos >= self.pending.len() {
            match self.input.next().await {
                Some(chunk) => {
                    self.pending = chunk;
                    self.pos = 0;
                }
                // Generator ended (notify error or unsubscribe) → EOF → Disconnected.
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Write half of the attached Rynk GATT link: acknowledged GATT writes, capped
/// to the characteristic's capacity.
pub struct BleWriter {
    output: Characteristic,
    write_chunk: usize,
}

impl rynk::io::ErrorType for BleWriter {
    type Error = std::io::Error;
}

impl Write for BleWriter {
    /// One GATT write per call, capped to the characteristic; `write_all` loops the
    /// rest. Acknowledged — a dropped chunk would desync the firmware's reassembler.
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let n = buf.len().min(self.write_chunk);
        self.output.write(&buf[..n]).await.map_err(|e| {
            // The driver reports only the ErrorKind; the detail lives here.
            log::warn!("rynk-ble: gatt write: {e}");
            std::io::Error::other("gatt write")
        })?;
        Ok(n)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A discovered Rynk keyboard, for a device picker. Holds cheap `bluest` handles,
/// not a live session; `connect` performs the first attach.
pub struct BleDevice {
    /// The keyboard's BLE name, if it advertised one.
    pub name: Option<String>,
    adapter: Adapter,
    device: Device,
}

impl BleDevice {
    /// Stable picker key — unlike the BLE name, which may be absent or shared.
    pub fn id(&self) -> DeviceId {
        self.device.id()
    }

    /// List already-connected Rynk keyboards (those exposing the service) — no scan,
    /// no attach. Requires Bluetooth permission; a denied/off adapter hangs in
    /// `wait_available` rather than erroring. Discovery is transport-specific, so
    /// it's an inherent call, not part of [`RynkDevice`].
    pub async fn discover() -> Result<Vec<Self>, RynkHostError> {
        let adapter = Adapter::default()
            .await
            .ok_or_else(|| RynkHostError::DeviceNotFound("no BLE adapter".into()))?;
        adapter
            .wait_available()
            .await
            .map_err(|e| RynkHostError::Transport("wait_available", e.to_string()))?;

        let connected = adapter
            .connected_devices_with_services(&[RYNK_SERVICE_UUID])
            .await
            .map_err(|e| RynkHostError::Transport("connected_devices_with_services", e.to_string()))?;
        Ok(connected
            .into_iter()
            .map(|device| BleDevice {
                name: device.name().ok(),
                adapter: adapter.clone(),
                device,
            })
            .collect())
    }

    // Discover the Rynk service and its input/output characteristics.
    async fn discover_characteristic(&self) -> Result<(Characteristic, Characteristic), RynkHostError> {
        let service = self
            .device
            .discover_services_with_uuid(RYNK_SERVICE_UUID)
            .await
            .map_err(|e| RynkHostError::Transport("discover_services", e.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| RynkHostError::DeviceNotFound("Rynk GATT service not found".into()))?;
        let mut input_char = None;
        let mut output_char = None;
        for c in service
            .discover_characteristics()
            .await
            .map_err(|e| RynkHostError::Transport("discover_characteristics", e.to_string()))?
        {
            match c.uuid() {
                u if u == RYNK_INPUT_CHAR_UUID => input_char = Some(c),
                u if u == RYNK_OUTPUT_CHAR_UUID => output_char = Some(c),
                _ => {}
            }
        }
        let input = input_char.ok_or_else(|| RynkHostError::DeviceNotFound("input characteristic missing".into()))?;
        let output =
            output_char.ok_or_else(|| RynkHostError::DeviceNotFound("output characteristic missing".into()))?;
        Ok((input, output))
    }

    /// Subscribe and build the transport. bluest's notify stream borrows the
    /// characteristic, so a generator owns `input` and `notify()`s it — keeping the
    /// borrow inside one pinned state machine (no self-reference, no leak, no task).
    /// Its synthetic empty first chunk acks that the subscription is live; consuming
    /// it here means `attach` returns only once subscribed, the order the firmware
    /// needs before the client's first write (bounded; a silent device never acks).
    async fn attach(
        &self,
        input: Characteristic,
        output: Characteristic,
    ) -> Result<(BleWriter, BleReader), RynkHostError> {
        // Cap writes to the characteristic's capacity.
        let write_chunk = output
            .max_write_len_async()
            .await
            .unwrap_or(BLE_SAFE_WRITE)
            .clamp(BLE_SAFE_WRITE, RYNK_BLE_CHUNK_SIZE);

        let mut input = stream! {
            // `notify().await` returns only once the subscription is live; `input`
            // is moved into and owned by this state machine.
            let Ok(updates) = input.notify().await else {
                return; // subscribe failed → stream ends → caller sees `None`
            };
            yield Vec::new(); // readiness ack: subscription is now live
            futures_util::pin_mut!(updates);
            // A notify error (disconnect) ends the stream → read sees EOF.
            while let Some(Ok(chunk)) = updates.next().await {
                yield chunk;
            }
        }
        .boxed();

        // Block on the readiness ack (bounded) so we return only once live.
        match tokio::time::timeout(GATT_TIMEOUT, input.next()).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err(RynkHostError::Disconnected),
            Err(_) => return Err(RynkHostError::Io(ErrorKind::TimedOut)),
        }

        Ok((
            BleWriter { output, write_chunk },
            BleReader {
                input,
                pending: Vec::new(),
                pos: 0,
                _adapter: self.adapter.clone(),
            },
        ))
    }
}

impl RynkDevice for BleDevice {
    type Read = BleReader;
    type Write = BleWriter;

    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("{:?}", self.id()))
    }

    /// Connect, discover characteristics, and subscribe — once, no retry. A failure
    /// means the device is gone or isn't a Rynk keyboard.
    async fn open(self) -> Result<(BleWriter, BleReader), RynkHostError> {
        // Bound connect + discovery; `attach` bounds its own subscribe step.
        let (input, output) = tokio::time::timeout(GATT_TIMEOUT, async {
            self.adapter
                .connect_device(&self.device)
                .await
                .map_err(|e| RynkHostError::Transport("connect_device", e.to_string()))?;
            self.discover_characteristic().await
        })
        .await
        .map_err(|_| RynkHostError::Io(ErrorKind::TimedOut))??;

        self.attach(input, output).await
    }
}
