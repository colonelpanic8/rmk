use embassy_time::{Duration, Timer, with_timeout};
use embedded_io_async::{Read, Write};
use heapless::Deque;

use super::driver::SplitDriverError;
use crate::split::driver::{PeripheralManager, SplitReader, SplitWriter, set_peripheral_connected};
use crate::split::{SPLIT_MESSAGE_MAX_SIZE, SplitMessage};

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
pub(crate) struct SerialSplitDriver<S> {
    serial: S,
    buffer: [u8; SPLIT_MESSAGE_MAX_SIZE],
    n_bytes_part: usize,
}

impl<S> SerialSplitDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self {
            serial,
            buffer: [0_u8; SPLIT_MESSAGE_MAX_SIZE],
            n_bytes_part: 0,
        }
    }
}

impl<S: Read> SplitReader for SerialSplitDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        const SENTINEL: u8 = 0x00;
        // Check the buffer *before* reading: a prior read() call may have
        // pulled in more than one complete message, and the next one is
        // already waiting in `self.buffer[..self.n_bytes_part]`.
        while !self.buffer[..self.n_bytes_part].contains(&SENTINEL) && self.n_bytes_part < self.buffer.len() {
            let n_bytes = self
                .serial
                .read(&mut self.buffer[self.n_bytes_part..])
                .await
                .map_err(|_e| {
                    self.n_bytes_part = 0;
                    SplitDriverError::SerialError
                })?;
            if n_bytes == 0 {
                return Err(SplitDriverError::EmptyMessage);
            }
            debug_assert!(self.n_bytes_part + n_bytes <= self.buffer.len());
            self.n_bytes_part += n_bytes;
        }

        let (result, n_bytes_unused) =
            match postcard::take_from_bytes_cobs::<SplitMessage>(&mut self.buffer[..self.n_bytes_part]) {
                Ok((message, unused_bytes)) => (Ok(message), unused_bytes.len()),
                Err(e) => {
                    error!("Postcard deserialize split message error: {}", e);
                    let n_bytes_unused = self.buffer[..self.n_bytes_part]
                        .iter()
                        .position(|&x| x == SENTINEL)
                        .map_or(0, |index| self.n_bytes_part - index - 1);
                    (Err(SplitDriverError::SerializeError), n_bytes_unused)
                }
            };

        self.buffer
            .copy_within(self.n_bytes_part - n_bytes_unused..self.n_bytes_part, 0);
        self.n_bytes_part = n_bytes_unused;

        result
    }
}

impl<S: Write> SplitWriter for SerialSplitDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        let mut buf = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        let bytes = postcard::to_slice_cobs(message, &mut buf).map_err(|e| {
            error!("Postcard serialize split message error: {}", e);
            SplitDriverError::SerializeError
        })?;
        let mut remaining_bytes = bytes.len();
        while remaining_bytes > 0 {
            let sent_bytes = self
                .serial
                .write(&bytes[bytes.len() - remaining_bytes..])
                .await
                .map_err(|_e| SplitDriverError::SerialError)?;
            remaining_bytes -= sent_bytes;
        }
        Ok(bytes.len())
    }
}

const HALF_DUPLEX_POLL_INTERVAL: Duration = Duration::from_millis(1);
const HALF_DUPLEX_RESPONSE_TIMEOUT: Duration = Duration::from_millis(5);
const HALF_DUPLEX_QUEUE_CAPACITY: usize = 4;

pub(crate) struct HalfDuplexCentralDriver<S> {
    serial: SerialSplitDriver<S>,
}

impl<S> HalfDuplexCentralDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self {
            serial: SerialSplitDriver::new(serial),
        }
    }
}

impl<S: Read + Write> SplitReader for HalfDuplexCentralDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        loop {
            self.serial.write(&SplitMessage::HalfDuplexPoll).await?;
            if let Ok(message) = with_timeout(HALF_DUPLEX_RESPONSE_TIMEOUT, self.serial.read()).await {
                let message = message?;
                if !matches!(message, SplitMessage::HalfDuplexIdle) {
                    return Ok(message);
                }
            }
            Timer::after(HALF_DUPLEX_POLL_INTERVAL).await;
        }
    }
}

impl<S: Write> SplitWriter for HalfDuplexCentralDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        self.serial.write(message).await
    }
}

pub(crate) struct HalfDuplexPeripheralDriver<S> {
    serial: SerialSplitDriver<S>,
    pending: Deque<SplitMessage, HALF_DUPLEX_QUEUE_CAPACITY>,
}

impl<S> HalfDuplexPeripheralDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self {
            serial: SerialSplitDriver::new(serial),
            pending: Deque::new(),
        }
    }
}

impl<S: Read + Write> SplitReader for HalfDuplexPeripheralDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        loop {
            let message = self.serial.read().await?;
            if matches!(message, SplitMessage::HalfDuplexPoll) {
                let response = self.pending.pop_front().unwrap_or(SplitMessage::HalfDuplexIdle);
                self.serial.write(&response).await?;
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
        let mut buf = [0u8; SPLIT_MESSAGE_MAX_SIZE];
        let encoded = postcard::to_slice_cobs(msg, &mut buf).unwrap();
        encoded.to_vec()
    }

    fn decode(bytes: &[u8]) -> SplitMessage {
        let mut bytes = bytes.to_vec();
        postcard::from_bytes_cobs(&mut bytes).unwrap()
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
        let mut driver = HalfDuplexPeripheralDriver::new(fake);

        block_on(driver.write(&SplitMessage::LedState(true))).unwrap();
        assert!(driver.serial.serial.writes.is_empty());

        let message = block_on(driver.read()).unwrap();

        assert!(matches!(message, SplitMessage::ConnectionStatus(_)));
        assert!(matches!(
            decode(&driver.serial.serial.writes[0]),
            SplitMessage::LedState(true)
        ));
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
}
