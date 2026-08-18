use embassy_time::{Duration, Instant, Timer, with_deadline};
use embedded_io_async::{Read, Write};
use heapless::Deque;

use super::driver::SplitDriverError;
use crate::split::driver::{PeripheralManager, SplitReader, SplitWriter, set_peripheral_connected};
use crate::split::{SPLIT_MESSAGE_MAX_SIZE, SplitMessage};

/// Frame delimiter: COBS guarantees no zero bytes inside an encoded frame.
const SENTINEL: u8 = 0x00;

/// Wired-link traffic counters, so a half that goes quiet can be told apart
/// from one that never saw the bus at all. Read them over the host protocol
/// on the central and through the split debug relay on the peripheral.
pub mod counters {
    use core::sync::atomic::{AtomicU32, Ordering};

    /// Bytes delivered by the serial transport.
    pub static RX_BYTES: AtomicU32 = AtomicU32::new(0);
    /// Frames that decoded and passed CRC.
    pub static FRAMES_OK: AtomicU32 = AtomicU32::new(0);
    /// Frames dropped by COBS, CRC, or postcard.
    pub static FRAMES_BAD: AtomicU32 = AtomicU32::new(0);
    /// Frames handed to the transport for transmission.
    pub static TX_FRAMES: AtomicU32 = AtomicU32::new(0);
    /// Frames whose bytes all reached the transport. A large gap below
    /// `TX_FRAMES` means writes are being cancelled mid-frame.
    pub static TX_DONE: AtomicU32 = AtomicU32::new(0);
    /// Writes dropped before the bus was released normally.
    pub static WRITE_CANCELLED: AtomicU32 = AtomicU32::new(0);
    /// Reads that returned a transport error (overrun, framing, break).
    /// Distinguishes a receiver that is failing from a quiet wire.
    pub static RX_ERRORS: AtomicU32 = AtomicU32::new(0);

    pub(super) fn add_rx(n: usize) {
        RX_BYTES.fetch_add(n as u32, Ordering::Relaxed);
    }

    pub fn bump(c: &AtomicU32) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// `(rx_bytes, frames_ok, frames_bad, tx_frames, tx_done, rx_errors_and_cancels)`.
    /// The last word packs read errors in the high half and cancelled writes
    /// in the low half.
    pub fn snapshot() -> (u32, u32, u32, u32, u32, u32) {
        (
            RX_BYTES.load(Ordering::Relaxed),
            FRAMES_OK.load(Ordering::Relaxed),
            FRAMES_BAD.load(Ordering::Relaxed),
            TX_FRAMES.load(Ordering::Relaxed),
            TX_DONE.load(Ordering::Relaxed),
            (RX_ERRORS.load(Ordering::Relaxed).min(0xffff) << 16) | WRITE_CANCELLED.load(Ordering::Relaxed).min(0xffff),
        )
    }
}

/// Receive split message from peripheral via serial and process it
///
/// Generic parameters:
/// - `const ROW`: row number of the peripheral's matrix
/// - `const COL`: column number of the peripheral's matrix
/// - `const ROW_OFFSET`: row offset of the peripheral's matrix in the whole matrix
/// - `const COL_OFFSET`: column offset of the peripheral's matrix in the whole matrix
/// - `S`: a serial port that implements `Read` and `Write` trait in embedded-io-async
pub(crate) async fn run_serial_peripheral_manager<S: Read + Write>(
    id: usize,
    receiver: S,
    matrix_config: crate::split::PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: crate::split::driver::UpdatePolicy,
) {
    let split_serial_driver: SerialSplitDriver<S> = SerialSplitDriver::new(receiver);
    let mut peripheral_manager = PeripheralManager::new(
        split_serial_driver,
        id,
        matrix_config,
        #[cfg(feature = "dfu_split")]
        policy,
    );
    info!("Running peripheral manager {}", id);

    // A wired peripheral is connected for as long as its manager runs.
    set_peripheral_connected(id, true);
    peripheral_manager.run().await;
}

/// Run one central-side manager over a polled half-duplex serial bus.
pub(crate) async fn run_half_duplex_peripheral_manager<S: Read + Write>(
    id: usize,
    serial: S,
    matrix_config: crate::split::PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: crate::split::driver::UpdatePolicy,
) {
    let driver = HalfDuplexCentralDriver::new(serial);
    let mut peripheral_manager = PeripheralManager::new(
        driver,
        id,
        matrix_config,
        #[cfg(feature = "dfu_split")]
        policy,
    );
    info!("Running half-duplex peripheral manager {}", id);
    set_peripheral_connected(id, true);
    peripheral_manager.run().await;
}

/// Run one central-side manager only while automatic selection prefers wired.
pub(crate) async fn run_auto_half_duplex_peripheral_manager<S: Read + Write>(
    id: usize,
    serial: S,
    matrix_config: crate::split::PeripheralMatrixConfig,
    #[cfg(feature = "dfu_split")] policy: crate::split::driver::UpdatePolicy,
) {
    let driver = HalfDuplexCentralDriver::new(serial);
    let mut peripheral_manager = PeripheralManager::new(
        driver,
        id,
        matrix_config,
        #[cfg(feature = "dfu_split")]
        policy,
    );

    loop {
        crate::split::selector::wait_wired_selected().await;
        set_peripheral_connected(id, true);
        match embassy_futures::select::select(
            peripheral_manager.run(),
            crate::split::selector::wait_wireless_selected(),
        )
        .await
        {
            embassy_futures::select::Either::First(_) => {}
            embassy_futures::select::Either::Second(_) => {
                set_peripheral_connected(id, false);
            }
        }
    }
}

/// Serial driver for BOTH split central and peripheral
///
/// Wire format: one message per frame, `cobs(postcard(message) ++ crc32_le)`
/// terminated by a zero sentinel. The CRC lets a receiver drop frames that
/// were truncated or corrupted on the bus; the sentinel gives it a resync
/// point in the following bytes.
pub(crate) struct SerialSplitDriver<S> {
    serial: S,
    buffer: [u8; SPLIT_MESSAGE_MAX_SIZE * 2],
    n_bytes_part: usize,
}

impl<S> SerialSplitDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self {
            serial,
            buffer: [0_u8; SPLIT_MESSAGE_MAX_SIZE * 2],
            n_bytes_part: 0,
        }
    }
}

/// Decode one sentinel-delimited frame: COBS-decode in place, verify the
/// trailing CRC-32, then deserialize the payload.
fn decode_frame(frame: &mut [u8]) -> Result<SplitMessage, SplitDriverError> {
    let decoded_len = cobs::decode_in_place(frame).map_err(|_| SplitDriverError::SerializeError)?;
    let Some(payload_len) = decoded_len.checked_sub(4) else {
        return Err(SplitDriverError::SerializeError);
    };
    let (payload, crc_bytes) = frame[..decoded_len].split_at(payload_len);
    let expected = u32::from_le_bytes(crc_bytes.try_into().unwrap());
    if crate::crc32::crc32(payload) != expected {
        error!("Split frame CRC mismatch, dropping frame");
        return Err(SplitDriverError::SerializeError);
    }
    postcard::from_bytes(payload).map_err(|e| {
        error!("Postcard deserialize split message error: {}", e);
        SplitDriverError::SerializeError
    })
}

impl<S: Read> SplitReader for SerialSplitDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        // Check the buffer *before* reading: a prior read() call may have
        // pulled in more than one complete frame, and the next one is
        // already waiting in `self.buffer[..self.n_bytes_part]`.
        let sentinel_index = loop {
            if let Some(index) = self.buffer[..self.n_bytes_part].iter().position(|&b| b == SENTINEL) {
                break index;
            }
            if self.n_bytes_part >= self.buffer.len() {
                // Sentinel-free garbage filled the buffer; drop it all.
                self.n_bytes_part = 0;
                return Err(SplitDriverError::SerializeError);
            }
            let n_bytes = self
                .serial
                .read(&mut self.buffer[self.n_bytes_part..])
                .await
                .map_err(|_e| {
                    counters::bump(&counters::RX_ERRORS);
                    self.n_bytes_part = 0;
                    SplitDriverError::SerialError
                })?;
            if n_bytes == 0 {
                return Err(SplitDriverError::EmptyMessage);
            }
            counters::add_rx(n_bytes);
            self.n_bytes_part += n_bytes;
        };

        let result = decode_frame(&mut self.buffer[..sentinel_index]);
        counters::bump(if result.is_ok() {
            &counters::FRAMES_OK
        } else {
            &counters::FRAMES_BAD
        });
        // Consume the frame and its sentinel, keeping any following bytes.
        self.buffer.copy_within(sentinel_index + 1..self.n_bytes_part, 0);
        self.n_bytes_part -= sentinel_index + 1;

        result
    }
}

impl<S: Write> SplitWriter for SerialSplitDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        let mut raw = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        let mut frame = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        let payload_len = postcard::to_slice(message, &mut raw)
            .map_err(|e| {
                error!("Postcard serialize split message error: {}", e);
                SplitDriverError::SerializeError
            })?
            .len();
        let crc = crate::crc32::crc32(&raw[..payload_len]);
        raw[payload_len..payload_len + 4].copy_from_slice(&crc.to_le_bytes());
        let encoded_len = cobs::encode(&raw[..payload_len + 4], &mut frame);
        frame[encoded_len] = SENTINEL;
        let bytes = &frame[..encoded_len + 1];
        counters::bump(&counters::TX_FRAMES);

        let mut remaining_bytes = bytes.len();
        while remaining_bytes > 0 {
            let sent_bytes = self
                .serial
                .write(&bytes[bytes.len() - remaining_bytes..])
                .await
                .map_err(|_e| SplitDriverError::SerialError)?;
            remaining_bytes -= sent_bytes;
        }
        counters::bump(&counters::TX_DONE);
        Ok(bytes.len())
    }
}

const HALF_DUPLEX_POLL_INTERVAL: Duration = Duration::from_millis(1);
const HALF_DUPLEX_RESPONSE_TIMEOUT: Duration = Duration::from_millis(5);
const HALF_DUPLEX_QUEUE_CAPACITY: usize = 8;
/// How long the peripheral waits after decoding a poll before answering.
///
/// The central only enables its receiver once the last byte of the poll has
/// left the wire and its turnaround has elapsed. A reply that starts inside
/// that window is transmitted while the central is still driving, so it is
/// lost whole rather than corrupted. The gap has to clear the central's
/// turnaround plus the tick granularity of the timer that measures it.
const HALF_DUPLEX_REPLY_GAP: Duration = Duration::from_millis(2);

/// Central side of the polled half-duplex bus.
///
/// The central is the only side that transmits unprompted, and it must never
/// transmit while the peripheral may still be answering the last poll — on a
/// half-duplex bus the two frames would collide. `response_deadline` tracks
/// that window across cancelled `read()` futures, and both `read()` and
/// `write()` wait it out before touching the wire.
pub(crate) struct HalfDuplexCentralDriver<S> {
    serial: SerialSplitDriver<S>,
    /// Deadline until which the peripheral may still be answering the last
    /// poll. `None` when the bus is known quiet.
    response_deadline: Option<Instant>,
    /// Response that arrived while `write()` was clearing the bus; returned
    /// by the next `read()`.
    stashed: Option<SplitMessage>,
}

impl<S> HalfDuplexCentralDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self {
            serial: SerialSplitDriver::new(serial),
            response_deadline: None,
            stashed: None,
        }
    }
}

impl<S: Read + Write> HalfDuplexCentralDriver<S> {
    /// Wait until the pending response window is over: an answer arrived,
    /// the frame proved corrupt, or the deadline passed.
    async fn clear_response_window(&mut self) -> Result<Option<SplitMessage>, SplitDriverError> {
        let Some(deadline) = self.response_deadline else {
            return Ok(None);
        };
        let result = with_deadline(deadline, self.serial.read()).await;
        self.response_deadline = None;
        match result {
            Ok(Ok(SplitMessage::HalfDuplexIdle)) => Ok(None),
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(e)) => Err(e),
            Err(_timeout) => Ok(None),
        }
    }
}

impl<S: Read + Write> SplitReader for HalfDuplexCentralDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        loop {
            if let Some(message) = self.stashed.take() {
                return Ok(message);
            }
            if let Some(message) = self.clear_response_window().await? {
                return Ok(message);
            }
            // Armed BEFORE the write: if this future is dropped mid-send the
            // poll may still have reached the wire, and the next transmission
            // must not be allowed to drive over the reply it provokes.
            self.response_deadline = Some(Instant::now() + HALF_DUPLEX_RESPONSE_TIMEOUT);
            self.serial.write(&SplitMessage::HalfDuplexPoll).await?;
            if let Some(message) = self.clear_response_window().await? {
                return Ok(message);
            }
            Timer::after(HALF_DUPLEX_POLL_INTERVAL).await;
        }
    }
}

impl<S: Read + Write> SplitWriter for HalfDuplexCentralDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        // The manager awaits write() outside its select, so this send cannot
        // be cancelled mid-frame. It only has to stay out of the response
        // window of a poll whose read() future was cancelled.
        match self.clear_response_window().await {
            Ok(Some(response)) => self.stashed = Some(response),
            Ok(None) => {}
            // A corrupt response still means the window is over.
            Err(_) => {}
        }
        self.serial.write(message).await
    }
}

/// Peripheral side of the polled half-duplex bus: transmits exactly one
/// frame per received poll, so it can never collide with the central.
pub(crate) struct HalfDuplexPeripheralDriver<S> {
    serial: SerialSplitDriver<S>,
    pending: Deque<SplitMessage, HALF_DUPLEX_QUEUE_CAPACITY>,
    reply_gap: Duration,
}

impl<S> HalfDuplexPeripheralDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self::with_reply_gap(serial, HALF_DUPLEX_REPLY_GAP)
    }

    /// Tests drive this with a zero gap: they run on a clock only the test
    /// advances, so a real gap would never elapse.
    pub(crate) fn with_reply_gap(serial: S, reply_gap: Duration) -> Self {
        Self {
            serial: SerialSplitDriver::new(serial),
            pending: Deque::new(),
            reply_gap,
        }
    }
}

impl<S: Read + Write> SplitReader for HalfDuplexPeripheralDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        loop {
            let message = self.serial.read().await?;
            if matches!(message, SplitMessage::HalfDuplexPoll) {
                let response = self.pending.front().copied().unwrap_or(SplitMessage::HalfDuplexIdle);
                if self.reply_gap.as_ticks() != 0 {
                    Timer::after(self.reply_gap).await;
                }
                self.serial.write(&response).await?;
                // Pop only after the reply is on the wire: a read() future
                // dropped mid-send retransmits instead of losing the message.
                if !matches!(response, SplitMessage::HalfDuplexIdle) {
                    self.pending.pop_front();
                }
            } else {
                return Ok(message);
            }
        }
    }
}

impl<S> SplitWriter for HalfDuplexPeripheralDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        self.pending
            .push_back(*message)
            .map_err(|_| SplitDriverError::SerialError)?;
        Ok(SPLIT_MESSAGE_MAX_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::convert::Infallible;

    use embassy_futures::block_on;
    use embedded_io_async::ErrorType;

    use super::*;

    /// Fake `embedded_io_async::Read`: each `serial.read()` call returns the
    /// next scripted chunk. Panics if the driver calls `read()` more times
    /// than we scripted — that is itself a useful assertion, since #801 is
    /// about the driver making a `read()` call it should not have made.
    struct FakeSerial {
        chunks: VecDeque<Vec<u8>>,
        read_calls: usize,
        writes: Vec<Vec<u8>>,
    }

    impl FakeSerial {
        fn new<I: IntoIterator<Item = Vec<u8>>>(chunks: I) -> Self {
            Self {
                chunks: chunks.into_iter().collect(),
                read_calls: 0,
                writes: Vec::new(),
            }
        }
    }

    impl ErrorType for FakeSerial {
        type Error = Infallible;
    }

    impl Read for FakeSerial {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
            self.read_calls += 1;
            let chunk = self
                .chunks
                .pop_front()
                .expect("SerialSplitDriver made an unexpected underlying read() call");
            assert!(
                chunk.len() <= buf.len(),
                "scripted chunk larger than driver's read slice"
            );
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    impl Write for FakeSerial {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
            self.writes.push(buf.to_vec());
            Ok(buf.len())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn encode(msg: &SplitMessage) -> Vec<u8> {
        let mut raw = [0u8; SPLIT_MESSAGE_MAX_SIZE];
        let payload_len = postcard::to_slice(msg, &mut raw).unwrap().len();
        let crc = crate::crc32::crc32(&raw[..payload_len]);
        raw[payload_len..payload_len + 4].copy_from_slice(&crc.to_le_bytes());
        let mut frame = vec![0u8; SPLIT_MESSAGE_MAX_SIZE];
        let encoded_len = cobs::encode(&raw[..payload_len + 4], &mut frame);
        frame.truncate(encoded_len);
        frame.push(SENTINEL);
        frame
    }

    fn decode(bytes: &[u8]) -> SplitMessage {
        let mut frame = bytes.to_vec();
        assert_eq!(frame.pop(), Some(SENTINEL), "frame must end with the sentinel");
        let decoded_len = cobs::decode_in_place(&mut frame).unwrap();
        let payload_len = decoded_len - 4;
        let expected = u32::from_le_bytes(frame[payload_len..decoded_len].try_into().unwrap());
        assert_eq!(crate::crc32::crc32(&frame[..payload_len]), expected, "CRC mismatch");
        postcard::from_bytes(&frame[..payload_len]).unwrap()
    }

    #[test]
    fn half_duplex_central_polls_before_reading() {
        let fake = FakeSerial::new([encode(&SplitMessage::LedState(true))]);
        let mut driver = HalfDuplexCentralDriver::new(fake);

        let message = block_on(driver.read()).unwrap();

        assert!(matches!(message, SplitMessage::LedState(true)));
        assert!(matches!(
            decode(&driver.serial.serial.writes[0]),
            SplitMessage::HalfDuplexPoll
        ));
    }

    #[test]
    fn half_duplex_peripheral_only_transmits_in_response_to_poll() {
        let status = rmk_types::connection::ConnectionStatus::default();
        let fake = FakeSerial::new([
            encode(&SplitMessage::HalfDuplexPoll),
            encode(&SplitMessage::ConnectionStatus(status)),
        ]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        block_on(driver.write(&SplitMessage::LedState(true))).unwrap();
        assert!(driver.serial.serial.writes.is_empty());

        let message = block_on(driver.read()).unwrap();

        assert!(matches!(message, SplitMessage::ConnectionStatus(_)));
        assert!(matches!(
            decode(&driver.serial.serial.writes[0]),
            SplitMessage::LedState(true)
        ));
    }

    /// A queued message is sent once per poll, not duplicated: the second
    /// poll gets an Idle reply because the queue is empty again.
    #[test]
    fn half_duplex_peripheral_sends_each_message_once() {
        let status = rmk_types::connection::ConnectionStatus::default();
        let fake = FakeSerial::new([
            encode(&SplitMessage::HalfDuplexPoll),
            encode(&SplitMessage::HalfDuplexPoll),
            encode(&SplitMessage::ConnectionStatus(status)),
        ]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        block_on(driver.write(&SplitMessage::LedState(true))).unwrap();
        let message = block_on(driver.read()).unwrap();

        assert!(matches!(message, SplitMessage::ConnectionStatus(_)));
        assert!(matches!(
            decode(&driver.serial.serial.writes[0]),
            SplitMessage::LedState(true)
        ));
        assert!(matches!(
            decode(&driver.serial.serial.writes[1]),
            SplitMessage::HalfDuplexIdle
        ));
    }

    /// `write()` must not transmit into an open response window: it listens
    /// first, stashes whatever response arrives, and only then sends. The
    /// stashed message comes out of the next `read()` without touching the
    /// wire.
    #[test]
    fn half_duplex_central_write_clears_response_window_first() {
        let fake = FakeSerial::new([encode(&SplitMessage::LedState(true))]);
        let mut driver = HalfDuplexCentralDriver::new(fake);
        driver.response_deadline = Some(Instant::now() + HALF_DUPLEX_RESPONSE_TIMEOUT);

        block_on(driver.write(&SplitMessage::LedState(false))).unwrap();

        assert!(matches!(driver.stashed, Some(SplitMessage::LedState(true))));
        assert!(matches!(
            decode(&driver.serial.serial.writes[0]),
            SplitMessage::LedState(false)
        ));

        // No chunks are scripted anymore: read() must return the stash
        // without polling.
        let message = block_on(driver.read()).unwrap();
        assert!(matches!(message, SplitMessage::LedState(true)));
        assert_eq!(driver.serial.serial.writes.len(), 1);
    }

    #[test]
    fn read_single_message_in_one_chunk() {
        let fake = FakeSerial::new([encode(&SplitMessage::LedState(true))]);
        let mut drv = SerialSplitDriver::new(fake);

        let msg = block_on(drv.read()).expect("read should succeed");
        assert!(matches!(msg, SplitMessage::LedState(true)));
        assert_eq!(drv.serial.read_calls, 1);
    }

    #[test]
    fn read_message_split_across_chunks() {
        let bytes = encode(&SplitMessage::LedState(true));
        let (a, b) = bytes.split_at(bytes.len() / 2);
        let fake = FakeSerial::new([a.to_vec(), b.to_vec()]);
        let mut drv = SerialSplitDriver::new(fake);

        let msg = block_on(drv.read()).expect("read should succeed");
        assert!(matches!(msg, SplitMessage::LedState(true)));
        assert_eq!(drv.serial.read_calls, 2);
    }

    /// Regression test for https://github.com/rmk-rs/rmk/issues/801: when
    /// two complete messages arrive in a single underlying read, the driver
    /// must deliver both without issuing a second underlying read.
    #[test]
    fn two_bundled_messages_do_not_trigger_extra_read() {
        let status = rmk_types::connection::ConnectionStatus::new();
        let mut bundled = encode(&SplitMessage::LedState(true));
        bundled.extend_from_slice(&encode(&SplitMessage::ConnectionStatus(status)));

        let fake = FakeSerial::new([bundled]);
        let mut drv = SerialSplitDriver::new(fake);

        let m1 = block_on(drv.read()).expect("first read should succeed");
        assert!(matches!(m1, SplitMessage::LedState(true)));

        let m2 = block_on(drv.read()).expect("second read should not touch serial");
        match m2 {
            SplitMessage::ConnectionStatus(s) => assert_eq!(s, status),
            other => panic!("expected ConnectionStatus, got {:?}", other),
        }

        assert_eq!(drv.serial.read_calls, 1);
    }

    /// Complete message followed by the prefix of a second in one chunk; the
    /// suffix of the second arrives later. The driver should deliver the
    /// first immediately, then assemble the second from the carried-over
    /// prefix plus the next chunk.
    #[test]
    fn trailing_partial_message_is_carried_over() {
        let status = rmk_types::connection::ConnectionStatus::new();
        let full1 = encode(&SplitMessage::LedState(true));
        let full2 = encode(&SplitMessage::ConnectionStatus(status));
        let (prefix, suffix) = full2.split_at(full2.len() / 2);

        let mut first_chunk = full1;
        first_chunk.extend_from_slice(prefix);

        let fake = FakeSerial::new([first_chunk, suffix.to_vec()]);
        let mut drv = SerialSplitDriver::new(fake);

        let m1 = block_on(drv.read()).expect("first read should succeed");
        assert!(matches!(m1, SplitMessage::LedState(true)));

        let m2 = block_on(drv.read()).expect("second read should succeed");
        match m2 {
            SplitMessage::ConnectionStatus(s) => assert_eq!(s, status),
            other => panic!("expected ConnectionStatus, got {:?}", other),
        }

        assert_eq!(drv.serial.read_calls, 2);
    }

    /// A frame corrupted on the wire fails its CRC and is dropped, and the
    /// stream resyncs on the sentinel: the following healthy frame decodes.
    #[test]
    fn corrupt_frame_is_dropped_and_stream_resyncs() {
        let mut bad = encode(&SplitMessage::LedState(false));
        let idx = bad.len() / 2;
        // Corrupt one mid-frame byte without creating a spurious sentinel.
        bad[idx] = if bad[idx] == 0xFF { 0xFE } else { 0xFF };
        let mut stream = bad;
        stream.extend_from_slice(&encode(&SplitMessage::LedState(true)));

        let fake = FakeSerial::new([stream]);
        let mut drv = SerialSplitDriver::new(fake);

        let err = block_on(drv.read()).expect_err("corrupt frame must not decode");
        assert!(matches!(err, SplitDriverError::SerializeError));

        let msg = block_on(drv.read()).expect("healthy frame after resync");
        assert!(matches!(msg, SplitMessage::LedState(true)));
        assert_eq!(drv.serial.read_calls, 1);
    }

    /// A truncated frame (sender died mid-transmission, then a healthy frame
    /// follows) is rejected by CRC rather than misparsed.
    #[test]
    fn truncated_frame_is_rejected() {
        let full = encode(&SplitMessage::LedState(false));
        let mut stream = full[..full.len() - 3].to_vec();
        stream.push(SENTINEL);
        stream.extend_from_slice(&encode(&SplitMessage::LedState(true)));

        let fake = FakeSerial::new([stream]);
        let mut drv = SerialSplitDriver::new(fake);

        let err = block_on(drv.read()).expect_err("truncated frame must not decode");
        assert!(matches!(err, SplitDriverError::SerializeError));

        let msg = block_on(drv.read()).expect("healthy frame after resync");
        assert!(matches!(msg, SplitMessage::LedState(true)));
    }

    /// Round-trip through the production writer: what `SerialSplitDriver`
    /// writes, `SerialSplitDriver` reads back.
    #[test]
    fn writer_reader_round_trip() {
        let status = rmk_types::connection::ConnectionStatus::new();
        let fake = FakeSerial::new([]);
        let mut writer = SerialSplitDriver::new(fake);
        block_on(writer.write(&SplitMessage::ConnectionStatus(status))).unwrap();

        let frames = writer.serial.writes.concat();
        let fake = FakeSerial::new([frames]);
        let mut reader = SerialSplitDriver::new(fake);
        let msg = block_on(reader.read()).unwrap();
        match msg {
            SplitMessage::ConnectionStatus(s) => assert_eq!(s, status),
            other => panic!("expected ConnectionStatus, got {:?}", other),
        }
    }
}
