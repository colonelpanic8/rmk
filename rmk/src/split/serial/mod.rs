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
    /// Sequenced frames retransmitted by the link layer.
    pub static RETRANSMITS: AtomicU32 = AtomicU32::new(0);
    /// Messages refused at enqueue because their outgoing lane was full.
    /// A refused message is lost above the sequencing layer — nothing
    /// retransmits it — so this is the only trace it leaves.
    pub static LANE_DROPS: AtomicU32 = AtomicU32::new(0);

    pub(super) fn add_rx(n: usize) {
        RX_BYTES.fetch_add(n as u32, Ordering::Relaxed);
    }

    pub fn bump(c: &AtomicU32) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// `(rx_bytes, frames_ok, frames_bad, tx_frames, tx_done, packed)`.
    /// `packed` holds retransmits in the high half and cancelled writes in
    /// the low half.
    pub fn snapshot() -> (u32, u32, u32, u32, u32, u32) {
        (
            RX_BYTES.load(Ordering::Relaxed),
            FRAMES_OK.load(Ordering::Relaxed),
            FRAMES_BAD.load(Ordering::Relaxed),
            TX_FRAMES.load(Ordering::Relaxed),
            TX_DONE.load(Ordering::Relaxed),
            (RETRANSMITS.load(Ordering::Relaxed).min(0xffff) << 16)
                | WRITE_CANCELLED.load(Ordering::Relaxed).min(0xffff),
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
        match embassy_futures::select::select(
            peripheral_manager.transceiver_mut().drain(),
            crate::split::selector::wait_wired_selected(),
        )
        .await
        {
            embassy_futures::select::Either::First(_) | embassy_futures::select::Either::Second(_) => {}
        }
        set_peripheral_connected(id, true);
        peripheral_manager.transceiver_mut().begin_session();
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

// ---------------------------------------------------------------------------
// Link layer
//
// The wired bus gives the layers above none of what a radio stack gives them
// for free: frames vanish whole (CRC drop, turnaround collisions, cancelled
// writes) and nothing pushes back when a receiver's consumer falls behind.
// Every frame therefore carries a 3-byte link header:
//
//   [ctrl][ack][credit] ++ postcard(SplitMessage)
//
// `ctrl` carries a 7-bit sequence number for data frames (bit 7 set); polls
// and burst terminators are unsequenced link maintenance. `ack` is
// cumulative: the sender's next expected in-order sequence. `credit` is how
// many more bulk frames the sender may emit before the receiver's inbox
// drains, so bulk traffic waits instead of overflowing a queue upstream.
// Sequenced frames are retransmitted (go-back-N) until acknowledged, and a
// frame the receiver has no room for is simply not acknowledged.
// ---------------------------------------------------------------------------

const LINK_HEADER: usize = 3;
const SEQ_FLAG: u8 = 0x80;
/// On an unsequenced frame: sequence-state reset. The central raises it on
/// every fresh session (and whenever acknowledgements stop progressing); the
/// peripheral zeroes its endpoint and echoes the flag on its terminator.
/// Without this, one half rebooting mid-session deadlocks the link: each
/// side discards the other's frames as out-of-order forever.
const RESET_FLAG: u8 = 0x40;
const SEQ_MASK: u8 = 0x3f;
/// Outstanding unacknowledged sequenced frames.
const LINK_WINDOW: usize = 4;
/// Exchanges an unacked frame survives before the window retransmits.
const RETRANSMIT_AFTER_EXCHANGES: u8 = 3;
/// In-order frames waiting for the manager to `read()` them.
const INBOX_CAPACITY: usize = 8;
/// Queued outgoing frames per lane.
const LANE_CAPACITY: usize = 8;
/// Sequenced frames sent per exchange, so one exchange stays well under the
/// response timeout even when the whole window retransmits.
const FRAMES_PER_EXCHANGE: usize = 4;

/// `true` if sequence `a` precedes `b` in mod-64 arithmetic.
fn seq_before(a: u8, b: u8) -> bool {
    a != b && (b.wrapping_sub(a) & SEQ_MASK) < 32
}

/// Which outgoing lane a message belongs to. Control messages change link or
/// device state and are few; input events are latency-sensitive; everything
/// else — including application variants this module never names — is bulk
/// and subject to credit.
enum Lane {
    Control,
    Input,
    Bulk,
}

fn lane_of(message: &SplitMessage) -> Lane {
    match message {
        SplitMessage::Key(_) | SplitMessage::Pointing(_) => Lane::Input,
        #[cfg(feature = "_ble")]
        SplitMessage::BatteryStatus(_) => Lane::Input,
        SplitMessage::LedState(_)
        | SplitMessage::ConnectionStatus(_)
        | SplitMessage::Address(_)
        | SplitMessage::ClearPeer
        | SplitMessage::KeyboardIndicator(_)
        | SplitMessage::Layer(_)
        | SplitMessage::TransportOverride(_) => Lane::Control,
        _ => Lane::Bulk,
    }
}

struct LinkEndpoint {
    next_seq: u8,
    unacked: Deque<(u8, SplitMessage), LINK_WINDOW>,
    /// Next in-order sequence this side accepts.
    expected: u8,
    peer_credit: u8,
    /// How many frames at the head of `unacked` are marked for resend.
    resend: usize,
    exchanges_without_ack: u8,
    control: Deque<SplitMessage, LANE_CAPACITY>,
    input: Deque<SplitMessage, LANE_CAPACITY>,
    bulk: Deque<SplitMessage, LANE_CAPACITY>,
    inbox: Deque<SplitMessage, INBOX_CAPACITY>,
}

impl LinkEndpoint {
    fn new() -> Self {
        Self {
            next_seq: 0,
            unacked: Deque::new(),
            expected: 0,
            peer_credit: 0,
            resend: 0,
            exchanges_without_ack: 0,
            control: Deque::new(),
            input: Deque::new(),
            bulk: Deque::new(),
            inbox: Deque::new(),
        }
    }

    fn enqueue(&mut self, message: &SplitMessage) -> Result<(), SplitDriverError> {
        let lane = match lane_of(message) {
            Lane::Control => &mut self.control,
            Lane::Input => &mut self.input,
            Lane::Bulk => &mut self.bulk,
        };
        lane.push_back(*message).map_err(|_| SplitDriverError::SerialError)
    }

    /// Apply the peer's header: pop everything it acknowledges and adopt its
    /// credit.
    fn on_header(&mut self, ack: u8, credit: u8) {
        let mut progressed = false;
        while let Some((seq, _)) = self.unacked.front() {
            if seq_before(*seq, ack) {
                self.unacked.pop_front();
                self.resend = self.resend.saturating_sub(1);
                progressed = true;
            } else {
                break;
            }
        }
        if progressed {
            self.exchanges_without_ack = 0;
        }
        self.peer_credit = credit;
    }

    /// Accept a sequenced frame if it is in order and there is room to hold
    /// it. Out-of-order and unstowable frames are dropped unacknowledged, so
    /// the peer retransmits them.
    fn accept(&mut self, seq: u8, message: SplitMessage) {
        if seq != self.expected {
            return;
        }
        if self.inbox.push_back(message).is_err() {
            return;
        }
        self.expected = (self.expected + 1) & SEQ_MASK;
    }

    fn credit(&self) -> u8 {
        (INBOX_CAPACITY - self.inbox.len()) as u8
    }

    /// Sequenced frames to transmit this exchange: retransmissions first,
    /// then fresh traffic while the window and the peer's credit allow.
    fn fill_burst(&mut self, out: &mut Deque<(u8, SplitMessage), FRAMES_PER_EXCHANGE>) {
        if !self.unacked.is_empty() && self.exchanges_without_ack >= RETRANSMIT_AFTER_EXCHANGES {
            self.resend = self.unacked.len();
            self.exchanges_without_ack = 0;
            counters::bump(&counters::RETRANSMITS);
        }
        let mut resend_left = self.resend;
        for (seq, message) in self.unacked.iter() {
            if resend_left == 0 || out.is_full() {
                break;
            }
            let _ = out.push_back((*seq, *message));
            resend_left -= 1;
        }
        self.resend = resend_left;
        while !out.is_full() && !self.unacked.is_full() {
            let next = if let Some(m) = self.control.pop_front() {
                m
            } else if let Some(m) = self.input.pop_front() {
                m
            } else if self.peer_credit > 0 {
                match self.bulk.pop_front() {
                    Some(m) => {
                        self.peer_credit -= 1;
                        m
                    }
                    None => break,
                }
            } else {
                break;
            };
            let seq = self.next_seq;
            self.next_seq = (self.next_seq + 1) & SEQ_MASK;
            let _ = self.unacked.push_back((seq, next));
            let _ = out.push_back((seq, next));
        }
        if !self.unacked.is_empty() {
            self.exchanges_without_ack = self.exchanges_without_ack.saturating_add(1);
        }
    }

    /// Zero the sequence state for a fresh session. Unacknowledged frames
    /// are dropped rather than replayed: a session boundary means a peer
    /// rebooted or the link deadlocked, and the layers above re-sync their
    /// state on link-up anyway.
    fn reset(&mut self) {
        self.next_seq = 0;
        self.expected = 0;
        self.unacked.clear();
        self.resend = 0;
        self.exchanges_without_ack = 0;
    }

    /// A control-lane message is still queued or awaiting acknowledgement.
    fn control_outstanding(&self) -> bool {
        !self.control.is_empty() || self.unacked.iter().any(|(_, m)| matches!(lane_of(m), Lane::Control))
    }
}

/// Serial framing for BOTH split central and peripheral.
///
/// Wire format: one message per frame,
/// `cobs(link_header ++ postcard(message) ++ crc32_le)` terminated by a zero
/// sentinel. The CRC lets a receiver drop frames that were truncated or
/// corrupted on the bus; the sentinel gives it a resync point.
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
/// trailing CRC-32, then split the link header off the payload.
fn decode_frame(frame: &mut [u8]) -> Result<(u8, u8, u8, SplitMessage), SplitDriverError> {
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
    if payload.len() < LINK_HEADER {
        return Err(SplitDriverError::SerializeError);
    }
    let message = postcard::from_bytes(&payload[LINK_HEADER..]).map_err(|e| {
        error!("Postcard deserialize split message error: {}", e);
        SplitDriverError::SerializeError
    })?;
    Ok((payload[0], payload[1], payload[2], message))
}

impl<S: Read> SerialSplitDriver<S> {
    /// Read one framed message with its link header.
    async fn read_frame(&mut self) -> Result<(u8, u8, u8, SplitMessage), SplitDriverError> {
        // Check the buffer *before* reading: a prior read may have pulled in
        // more than one complete frame, and the next one is already waiting
        // in `self.buffer[..self.n_bytes_part]`.
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

impl<S: Write> SerialSplitDriver<S> {
    /// Write one framed message with a link header.
    async fn write_frame(
        &mut self,
        ctrl: u8,
        ack: u8,
        credit: u8,
        message: &SplitMessage,
    ) -> Result<usize, SplitDriverError> {
        let mut raw = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        let mut frame = [0_u8; SPLIT_MESSAGE_MAX_SIZE];
        raw[0] = ctrl;
        raw[1] = ack;
        raw[2] = credit;
        let payload_len = LINK_HEADER
            + postcard::to_slice(message, &mut raw[LINK_HEADER..])
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

impl<S: Read> SplitReader for SerialSplitDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        let (_, _, _, message) = self.read_frame().await?;
        Ok(message)
    }
}

impl<S: Write> SplitWriter for SerialSplitDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        // The full-duplex transports need no sequencing: nothing shares
        // their wire, so the header is inert.
        self.write_frame(0, 0, u8::MAX, message).await
    }
}

const HALF_DUPLEX_POLL_INTERVAL: Duration = Duration::from_millis(1);
const HALF_DUPLEX_RESPONSE_TIMEOUT: Duration = Duration::from_millis(5);
/// How long the peripheral waits after a poll before answering, so the
/// central's receiver is provably enabled before the reply's first byte.
const HALF_DUPLEX_REPLY_GAP: Duration = Duration::from_millis(2);
/// Exchanges a control-lane write waits for its acknowledgement before
/// reporting the link dead.
const CONTROL_ACK_EXCHANGES: usize = 32;
/// Consecutive exchanges without acknowledgement progress before the central
/// assumes the peer rebooted and renegotiates the sequence session.
const STUCK_ACK_EXCHANGES: u8 = 24;

/// Central side of the polled half-duplex bus.
///
/// The central is the only side that transmits unprompted. All wire I/O
/// happens inside [`Self::exchange`]: one exchange transmits queued frames
/// and a poll, then listens until the peripheral's burst terminator. The
/// manager's `write()` only enqueues (control writes also drive exchanges
/// until acknowledged), so its select cancelling `read()` can neither lose
/// frames nor collide with a reply.
pub(crate) struct HalfDuplexCentralDriver<S> {
    serial: SerialSplitDriver<S>,
    link: LinkEndpoint,
    /// Deadline until which the peripheral may still be answering the last
    /// poll. `None` when the bus is known quiet.
    response_deadline: Option<Instant>,
    /// The sequence handshake is pending: polls carry [`RESET_FLAG`] and
    /// data transfer waits until the peripheral echoes it.
    session_fresh: bool,
}

impl<S> HalfDuplexCentralDriver<S> {
    pub(crate) fn new(serial: S) -> Self {
        Self {
            serial: SerialSplitDriver::new(serial),
            link: LinkEndpoint::new(),
            response_deadline: None,
            session_fresh: true,
        }
    }

    /// Restart the sequence handshake at the next exchange. Called whenever
    /// a wired session (re)starts: the peer may have rebooted since the last
    /// one, and stale sequence state would deadlock the link.
    pub(crate) fn begin_session(&mut self) {
        self.session_fresh = true;
    }
}

impl<S: Read + Write> HalfDuplexCentralDriver<S> {
    /// Listen until the current response window closes: the peripheral's
    /// terminator arrived, a frame proved corrupt, or the deadline passed.
    /// Data frames land in the link inbox. Returns how many frames arrived.
    async fn drain_response_window(&mut self) -> Result<usize, SplitDriverError> {
        let mut frames = 0;
        while let Some(deadline) = self.response_deadline {
            match with_deadline(deadline, self.serial.read_frame()).await {
                Ok(Ok((ctrl, ack, credit, message))) => {
                    frames += 1;
                    if self.session_fresh {
                        // Nothing from the old session can be trusted; only
                        // the peripheral's reset echo matters.
                        if ctrl & SEQ_FLAG == 0 && ctrl & RESET_FLAG != 0 {
                            self.link.reset();
                            self.session_fresh = false;
                            self.response_deadline = None;
                        }
                        continue;
                    }
                    self.link.on_header(ack, credit);
                    if ctrl & SEQ_FLAG != 0 {
                        self.link.accept(ctrl & SEQ_MASK, message);
                    } else if matches!(message, SplitMessage::HalfDuplexIdle) {
                        self.response_deadline = None;
                    }
                }
                Ok(Err(e)) => {
                    self.response_deadline = None;
                    return Err(e);
                }
                Err(_timeout) => self.response_deadline = None,
            }
        }
        Ok(frames)
    }

    /// One full exchange: transmit queued and retransmitted frames plus a
    /// poll, then listen out the response window. Returns `true` if the
    /// peripheral answered at all.
    async fn exchange(&mut self) -> Result<bool, SplitDriverError> {
        self.drain_response_window().await?;
        // A peer whose responses arrive but never acknowledge anything is
        // running a different sequence session (it rebooted); renegotiate.
        if self.link.exchanges_without_ack >= STUCK_ACK_EXCHANGES {
            self.session_fresh = true;
        }
        if self.session_fresh {
            self.response_deadline = Some(Instant::now() + HALF_DUPLEX_RESPONSE_TIMEOUT);
            self.serial
                .write_frame(RESET_FLAG, 0, self.link.credit(), &SplitMessage::HalfDuplexPoll)
                .await?;
            return Ok(self.drain_response_window().await? > 0);
        }
        let mut burst: Deque<(u8, SplitMessage), FRAMES_PER_EXCHANGE> = Deque::new();
        self.link.fill_burst(&mut burst);
        // Armed BEFORE the writes: if this future is dropped mid-send the
        // poll may still reach the wire, and the next transmission must not
        // drive over the reply it provokes.
        self.response_deadline = Some(Instant::now() + HALF_DUPLEX_RESPONSE_TIMEOUT);
        while let Some((seq, message)) = burst.pop_front() {
            self.serial
                .write_frame(SEQ_FLAG | seq, self.link.expected, self.link.credit(), &message)
                .await?;
        }
        self.serial
            .write_frame(0, self.link.expected, self.link.credit(), &SplitMessage::HalfDuplexPoll)
            .await?;
        Ok(self.drain_response_window().await? > 0)
    }
}

impl<S: Read> HalfDuplexCentralDriver<S> {
    /// Consume and discard everything that arrives while this transport is
    /// deselected. An unread UARTE eventually overruns its ring and can wedge
    /// reception; draining also means a resumed session starts from a clean
    /// stream instead of a backlog of stale frames.
    pub(crate) async fn drain(&mut self) {
        loop {
            let _ = self.serial.read_frame().await;
        }
    }
}

impl<S: Read + Write> SplitReader for HalfDuplexCentralDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        loop {
            if let Some(message) = self.link.inbox.pop_front() {
                return Ok(message);
            }
            let active = self.exchange().await?;
            if let Some(message) = self.link.inbox.pop_front() {
                return Ok(message);
            }
            // Only pace out when the peripheral answered nothing: an active
            // link runs at exchange cadence, a quiet one at the interval.
            if !active {
                Timer::after(HALF_DUPLEX_POLL_INTERVAL).await;
            }
        }
    }
}

impl<S: Read + Write> SplitWriter for HalfDuplexCentralDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        let control = matches!(lane_of(message), Lane::Control);
        // A full lane drains only through exchanges, so drive them until the
        // message fits instead of refusing it. The manager treats a write
        // error as droppable, so a refusal here silently severed any
        // application burst larger than one lane: the peripheral staged a
        // snapshot head that never committed, acknowledged nothing, and the
        // central re-sent — and re-lost — the same burst forever.
        while self.link.enqueue(message).is_err() {
            if !self.exchange().await? {
                Timer::after(HALF_DUPLEX_POLL_INTERVAL).await;
            }
        }
        if control {
            // The manager acts on a control send having happened (e.g. it
            // switches transports after a TransportOverride), so drive
            // exchanges here until the peripheral has acknowledged it.
            for _ in 0..CONTROL_ACK_EXCHANGES {
                if !self.link.control_outstanding() {
                    return Ok(SPLIT_MESSAGE_MAX_SIZE);
                }
                if !self.exchange().await? {
                    Timer::after(HALF_DUPLEX_POLL_INTERVAL).await;
                }
            }
            if self.link.control_outstanding() {
                return Err(SplitDriverError::SerialError);
            }
        }
        Ok(SPLIT_MESSAGE_MAX_SIZE)
    }
}

/// Peripheral side of the polled half-duplex bus: transmits only in reply to
/// a poll, so it can never collide with the central.
pub(crate) struct HalfDuplexPeripheralDriver<S> {
    serial: SerialSplitDriver<S>,
    link: LinkEndpoint,
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
            link: LinkEndpoint::new(),
            reply_gap,
        }
    }
}

impl<S: Read> HalfDuplexPeripheralDriver<S> {
    /// See [`HalfDuplexCentralDriver::drain`].
    pub(crate) async fn drain(&mut self) {
        loop {
            let _ = self.serial.read_frame().await;
        }
    }
}

impl<S: Read + Write> SplitReader for HalfDuplexPeripheralDriver<S> {
    async fn read(&mut self) -> Result<SplitMessage, SplitDriverError> {
        loop {
            if let Some(message) = self.link.inbox.pop_front() {
                return Ok(message);
            }
            let (ctrl, ack, credit, message) = self.serial.read_frame().await?;
            if ctrl & SEQ_FLAG == 0 && ctrl & RESET_FLAG != 0 {
                // Fresh session: zero the sequence state and echo the flag so
                // the central knows it may start sending data.
                self.link.reset();
                if self.reply_gap.as_ticks() != 0 {
                    Timer::after(self.reply_gap).await;
                }
                self.serial
                    .write_frame(RESET_FLAG, 0, self.link.credit(), &SplitMessage::HalfDuplexIdle)
                    .await?;
                continue;
            }
            self.link.on_header(ack, credit);
            if ctrl & SEQ_FLAG != 0 {
                self.link.accept(ctrl & SEQ_MASK, message);
                continue;
            }
            if !matches!(message, SplitMessage::HalfDuplexPoll) {
                continue;
            }
            if self.reply_gap.as_ticks() != 0 {
                Timer::after(self.reply_gap).await;
            }
            let mut burst: Deque<(u8, SplitMessage), FRAMES_PER_EXCHANGE> = Deque::new();
            self.link.fill_burst(&mut burst);
            while let Some((seq, reply)) = burst.pop_front() {
                self.serial
                    .write_frame(SEQ_FLAG | seq, self.link.expected, self.link.credit(), &reply)
                    .await?;
            }
            // The terminator that tells the central the bus is free.
            self.serial
                .write_frame(0, self.link.expected, self.link.credit(), &SplitMessage::HalfDuplexIdle)
                .await?;
        }
    }
}

impl<S> SplitWriter for HalfDuplexPeripheralDriver<S> {
    async fn write(&mut self, message: &SplitMessage) -> Result<usize, SplitDriverError> {
        // Unlike the central, this side cannot drive the bus to make room —
        // it transmits only when polled, and its write arm shares a loop
        // with the read arm that answers those polls, so waiting here would
        // deadlock the link. Refusal stays, but counted: a key event refused
        // here vanishes with no other trace.
        if self.link.enqueue(message).is_err() {
            counters::bump(&counters::LANE_DROPS);
            return Err(SplitDriverError::SerialError);
        }
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
    /// than we scripted — that is itself a useful assertion.
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

    fn encode_with(ctrl: u8, ack: u8, credit: u8, msg: &SplitMessage) -> Vec<u8> {
        let mut raw = [0u8; SPLIT_MESSAGE_MAX_SIZE];
        raw[0] = ctrl;
        raw[1] = ack;
        raw[2] = credit;
        let payload_len = LINK_HEADER + postcard::to_slice(msg, &mut raw[LINK_HEADER..]).unwrap().len();
        let crc = crate::crc32::crc32(&raw[..payload_len]);
        raw[payload_len..payload_len + 4].copy_from_slice(&crc.to_le_bytes());
        let mut frame = vec![0u8; SPLIT_MESSAGE_MAX_SIZE];
        let encoded_len = cobs::encode(&raw[..payload_len + 4], &mut frame);
        frame.truncate(encoded_len);
        frame.push(SENTINEL);
        frame
    }

    fn encode(msg: &SplitMessage) -> Vec<u8> {
        encode_with(0, 0, u8::MAX, msg)
    }

    fn seq_frame(seq: u8, ack: u8, msg: &SplitMessage) -> Vec<u8> {
        encode_with(SEQ_FLAG | seq, ack, u8::MAX, msg)
    }

    fn reset_echo() -> Vec<u8> {
        encode_with(RESET_FLAG, 0, 8, &SplitMessage::HalfDuplexIdle)
    }

    fn decode(bytes: &[u8]) -> (u8, u8, u8, SplitMessage) {
        let mut frame = bytes.to_vec();
        assert_eq!(frame.pop(), Some(SENTINEL), "frame must end with the sentinel");
        decode_frame(&mut frame).unwrap()
    }

    fn decoded_writes(writes: &[Vec<u8>]) -> Vec<(u8, u8, u8, SplitMessage)> {
        writes.iter().map(|w| decode(w)).collect()
    }

    #[test]
    fn central_exchange_polls_and_delivers_reply() {
        // Exchange 1 is the session handshake; exchange 2 carries data.
        let mut reply = seq_frame(0, 0, &SplitMessage::LedState(true));
        reply.extend_from_slice(&encode_with(0, 0, 8, &SplitMessage::HalfDuplexIdle));
        let fake = FakeSerial::new([reset_echo(), reply]);
        let mut driver = HalfDuplexCentralDriver::new(fake);

        let message = block_on(driver.read()).unwrap();

        assert!(matches!(message, SplitMessage::LedState(true)));
        let writes = decoded_writes(&driver.serial.serial.writes);
        assert_eq!(writes[0].0, RESET_FLAG, "session opens with a reset poll");
        assert!(matches!(writes[0].3, SplitMessage::HalfDuplexPoll));
        assert!(matches!(writes[1].3, SplitMessage::HalfDuplexPoll));
        assert_eq!(writes[1].1, 0, "first data poll expects seq 0");
    }

    #[test]
    fn central_acks_received_frames_on_next_poll() {
        let mut reply = seq_frame(0, 0, &SplitMessage::LedState(true));
        reply.extend_from_slice(&encode_with(0, 0, 8, &SplitMessage::HalfDuplexIdle));
        let mut second = seq_frame(1, 0, &SplitMessage::LedState(false));
        second.extend_from_slice(&encode_with(0, 0, 8, &SplitMessage::HalfDuplexIdle));
        let fake = FakeSerial::new([reset_echo(), reply, second]);
        let mut driver = HalfDuplexCentralDriver::new(fake);

        let first = block_on(driver.read()).unwrap();
        assert!(matches!(first, SplitMessage::LedState(true)));
        let second = block_on(driver.read()).unwrap();
        assert!(matches!(second, SplitMessage::LedState(false)));

        let writes = decoded_writes(&driver.serial.serial.writes);
        assert_eq!(writes[1].1, 0, "first data poll expects seq 0");
        assert_eq!(writes[2].1, 1, "next poll acknowledges seq 0");
    }

    #[test]
    fn peripheral_replies_only_to_polls_and_sequences_frames() {
        let poll = encode_with(0, 0, 8, &SplitMessage::HalfDuplexPoll);
        let status = rmk_types::connection::ConnectionStatus::default();
        let data = seq_frame(0, 0, &SplitMessage::ConnectionStatus(status));
        let fake = FakeSerial::new([poll, data]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        block_on(driver.write(&SplitMessage::LedState(true))).unwrap();
        assert!(driver.serial.serial.writes.is_empty(), "no unprompted transmissions");

        let message = block_on(driver.read()).unwrap();
        assert!(matches!(message, SplitMessage::ConnectionStatus(_)));

        let writes = decoded_writes(&driver.serial.serial.writes);
        assert_eq!(writes[0].0, SEQ_FLAG, "reply carries seq 0");
        assert!(matches!(writes[0].3, SplitMessage::LedState(true)));
        assert!(matches!(writes[1].3, SplitMessage::HalfDuplexIdle));
    }

    #[test]
    fn unacked_frames_are_retransmitted() {
        // Polls that never acknowledge: after the retransmit threshold the
        // frame is sent again with the same sequence number.
        let poll = |ack: u8| encode_with(0, ack, 8, &SplitMessage::HalfDuplexPoll);
        let stop = seq_frame(
            0,
            0,
            &SplitMessage::ConnectionStatus(rmk_types::connection::ConnectionStatus::default()),
        );
        let fake = FakeSerial::new([poll(0), poll(0), poll(0), poll(0), stop]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        block_on(driver.write(&SplitMessage::LedState(true))).unwrap();
        let _ = block_on(driver.read()).unwrap();

        let writes = decoded_writes(&driver.serial.serial.writes);
        let data_frames: Vec<_> = writes
            .iter()
            .filter(|w| matches!(w.3, SplitMessage::LedState(true)))
            .collect();
        assert!(
            data_frames.len() >= 2,
            "unacked frame must be retransmitted, saw {} transmissions",
            data_frames.len()
        );
        for frame in &data_frames {
            assert_eq!(frame.0, SEQ_FLAG, "retransmissions reuse the same sequence");
        }
    }

    #[test]
    fn acknowledged_frames_are_not_retransmitted() {
        let poll = |ack: u8| encode_with(0, ack, 8, &SplitMessage::HalfDuplexPoll);
        let stop = seq_frame(
            0,
            1,
            &SplitMessage::ConnectionStatus(rmk_types::connection::ConnectionStatus::default()),
        );
        let fake = FakeSerial::new([poll(0), poll(1), poll(1), poll(1), stop]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        block_on(driver.write(&SplitMessage::LedState(true))).unwrap();
        let _ = block_on(driver.read()).unwrap();

        let writes = decoded_writes(&driver.serial.serial.writes);
        let data_frames = writes
            .iter()
            .filter(|w| matches!(w.3, SplitMessage::LedState(true)))
            .count();
        assert_eq!(data_frames, 1, "acked frame must not be retransmitted");
    }

    #[test]
    fn receiver_drops_out_of_order_and_duplicate_frames() {
        let status = rmk_types::connection::ConnectionStatus::default();
        // seq 1 before seq 0: dropped. Then seq 0 delivered, then a
        // duplicate seq 0 dropped, then seq 1 delivered.
        let f1 = seq_frame(1, 0, &SplitMessage::ConnectionStatus(status));
        let f0 = seq_frame(0, 0, &SplitMessage::LedState(true));
        let f0_dup = seq_frame(0, 0, &SplitMessage::LedState(true));
        let tail = seq_frame(1, 0, &SplitMessage::ConnectionStatus(status));
        let fake = FakeSerial::new([f1, f0, f0_dup, tail]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        let m1 = block_on(driver.read()).unwrap();
        assert!(matches!(m1, SplitMessage::LedState(true)));
        let m2 = block_on(driver.read()).unwrap();
        assert!(matches!(m2, SplitMessage::ConnectionStatus(_)));
    }

    #[test]
    fn bulk_traffic_respects_peer_credit() {
        // HalfDuplexIdle classifies to the bulk lane (default arm), which
        // makes it a convenient stand-in for application traffic.
        let poll_no_credit = encode_with(0, 0, 0, &SplitMessage::HalfDuplexPoll);
        let poll_credit = encode_with(0, 0, 4, &SplitMessage::HalfDuplexPoll);
        let stop = seq_frame(
            0,
            1,
            &SplitMessage::ConnectionStatus(rmk_types::connection::ConnectionStatus::default()),
        );
        let fake = FakeSerial::new([poll_no_credit, poll_credit, stop]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        block_on(driver.write(&SplitMessage::HalfDuplexIdle)).unwrap();
        let _ = block_on(driver.read()).unwrap();

        let writes = decoded_writes(&driver.serial.serial.writes);
        // First burst under zero credit: terminator only, nothing sequenced.
        assert_eq!(writes[0].0 & SEQ_FLAG, 0, "no bulk data under zero credit");
        assert!(matches!(writes[0].3, SplitMessage::HalfDuplexIdle));
        // Once credit arrives, the bulk frame goes out sequenced.
        assert!(
            writes.iter().any(|w| w.0 & SEQ_FLAG != 0),
            "bulk frame flows once credit arrives"
        );
    }

    #[test]
    fn central_write_backpressures_bulk_instead_of_dropping() {
        // Overfill the bulk lane: the ninth write must drive exchanges until
        // space frees rather than refuse (and thereby lose) the message.
        // Scripted: the session handshake echo, a credit grant, then a reply
        // acknowledging the first in-flight window.
        let credit_grant = encode_with(0, 0, 8, &SplitMessage::HalfDuplexIdle);
        let window_ack = encode_with(0, 4, 8, &SplitMessage::HalfDuplexIdle);
        let fake = FakeSerial::new([reset_echo(), credit_grant, window_ack]);
        let mut driver = HalfDuplexCentralDriver::new(fake);

        for _ in 0..LANE_CAPACITY {
            block_on(driver.write(&SplitMessage::HalfDuplexIdle)).unwrap();
        }
        block_on(driver.write(&SplitMessage::HalfDuplexIdle)).unwrap();

        let writes = decoded_writes(&driver.serial.serial.writes);
        let sequenced = writes.iter().filter(|w| w.0 & SEQ_FLAG != 0).count();
        assert_eq!(sequenced, 4, "one window of bulk frames reached the wire");
        assert_eq!(
            driver.link.bulk.len(),
            LANE_CAPACITY - 4 + 1,
            "the overflow message waited for lane space instead of vanishing"
        );
    }

    #[test]
    fn peripheral_counts_refused_lane_overflow() {
        use core::sync::atomic::Ordering;

        let fake = FakeSerial::new([]);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));
        let key = SplitMessage::Key(crate::event::KeyboardEvent::key(0, 0, true));
        for _ in 0..LANE_CAPACITY {
            block_on(driver.write(&key)).unwrap();
        }

        let before = counters::LANE_DROPS.load(Ordering::Relaxed);
        block_on(driver.write(&key)).expect_err("a full input lane refuses the write");

        assert_eq!(
            counters::LANE_DROPS.load(Ordering::Relaxed) - before,
            1,
            "the refusal must be counted — it is the only trace of the loss"
        );
    }

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

    #[test]
    fn seq_ordering_wraps() {
        assert!(seq_before(0, 1));
        assert!(seq_before(62, 63));
        assert!(seq_before(63, 0));
        assert!(!seq_before(1, 0));
        assert!(!seq_before(0, 0));
        assert!(!seq_before(0, 40));
    }

    /// A peripheral with stale sequence state (its peer rebooted) zeroes on
    /// a reset poll and the link recovers.
    #[test]
    fn reset_poll_zeroes_peripheral_state() {
        let status = rmk_types::connection::ConnectionStatus::default();
        // Advance the peripheral's expected sequence to 3.
        let advance: Vec<Vec<u8>> = (0..3).map(|i| seq_frame(i, 0, &SplitMessage::LedState(true))).collect();
        let reset = encode_with(RESET_FLAG, 0, 8, &SplitMessage::HalfDuplexPoll);
        // After the reset, seq 0 must be accepted again.
        let fresh = seq_frame(0, 0, &SplitMessage::ConnectionStatus(status));
        let mut chunks = advance;
        chunks.push(reset);
        chunks.push(fresh);
        let fake = FakeSerial::new(chunks);
        let mut driver = HalfDuplexPeripheralDriver::with_reply_gap(fake, Duration::from_ticks(0));

        for _ in 0..3 {
            let m = block_on(driver.read()).unwrap();
            assert!(matches!(m, SplitMessage::LedState(true)));
        }
        let m = block_on(driver.read()).unwrap();
        assert!(
            matches!(m, SplitMessage::ConnectionStatus(_)),
            "post-reset seq 0 accepted"
        );

        let writes = decoded_writes(&driver.serial.serial.writes);
        let echo = writes.iter().find(|w| w.0 & RESET_FLAG != 0).expect("reset echoed");
        assert!(matches!(echo.3, SplitMessage::HalfDuplexIdle));
    }
}
