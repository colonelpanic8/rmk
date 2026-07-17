//! Allocation-free protocol driver for Rynk.
//!
//! [`Client`] exchanges fixed-capacity owned frames with independently driven
//! [`Reader`] and [`Writer`] halves. A [`Session`] owns their channels and
//! lifecycle state.

#[cfg(feature = "std")]
use alloc::format;
#[cfg(feature = "alloc")]
use alloc::string::String;
#[cfg(feature = "std")]
use alloc::sync::Arc;
use core::cell::Cell;
#[cfg(feature = "std")]
use core::marker::PhantomData;

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, TrySendError};
use embassy_sync::signal::Signal;
#[cfg(not(feature = "std"))]
use embedded_io_async::ErrorKind;
use embedded_io_async::{Read, Write};
use rmk_types::protocol::rynk::endpoint::Endpoint;
use rmk_types::protocol::rynk::{
    Cmd, DeviceCapabilities, ProtocolVersion, RYNK_HEADER_SIZE, RynkError, RynkHeader, RynkMessage,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

#[cfg(feature = "std")]
use crate::transport::Transport;

/// Default maximum complete frame size, including the five-byte header.
pub const DEFAULT_FRAME_SIZE: usize = 4096;
/// Default number of queued topic frames.
pub const DEFAULT_EVENT_CAPACITY: usize = 64;
/// Maximum raw payload retained for one queued topic.
pub const TOPIC_PAYLOAD_SIZE: usize = 256;

/// Fixed-capacity storage and lifecycle state for one client/driver session.
///
/// A no-alloc application owns this value and passes a reference to
/// [`Driver::new`].
/// `FRAME` bounds every complete wire frame; `EVENTS` controls the best-effort
/// topic queue. A session is single-use; create a new one for each transport
/// connection.
pub struct Session<const FRAME: usize = DEFAULT_FRAME_SIZE, const EVENTS: usize = DEFAULT_EVENT_CAPACITY> {
    requests: Channel<CriticalSectionRawMutex, ([u8; FRAME], bool), 1>,
    responses: Channel<CriticalSectionRawMutex, Result<[u8; FRAME], RynkHostError>, 1>,
    topics: Channel<CriticalSectionRawMutex, TopicFrame, EVENTS>,
    events_dropped: Mutex<CriticalSectionRawMutex, Cell<u64>>,
    dead: Mutex<CriticalSectionRawMutex, Cell<bool>>,
    disconnected: Signal<CriticalSectionRawMutex, ()>,
    shutdown: Signal<CriticalSectionRawMutex, ()>,
}

impl<const FRAME: usize, const EVENTS: usize> Session<FRAME, EVENTS> {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        assert!(FRAME >= RYNK_HEADER_SIZE, "Rynk frame capacity must fit the header");
        assert!(EVENTS > 0, "Rynk topic capacity must be non-zero");
        Self {
            requests: Channel::new(),
            responses: Channel::new(),
            topics: Channel::new(),
            events_dropped: Mutex::new(Cell::new(0)),
            dead: Mutex::new(Cell::new(false)),
            disconnected: Signal::new(),
            shutdown: Signal::new(),
        }
    }

    fn is_alive(&self) -> bool {
        self.dead.lock(|dead| !dead.get())
    }

    fn events_dropped(&self) -> u64 {
        self.events_dropped.lock(Cell::get)
    }

    fn queue_topic(&self, topic: TopicFrame) {
        if let Err(TrySendError::Full(topic)) = self.topics.try_send(topic) {
            let _ = self.topics.try_receive();
            let _ = self.topics.try_send(topic);
            self.record_dropped_topic();
        }
    }

    fn record_dropped_topic(&self) {
        self.events_dropped.lock(|events_dropped| {
            events_dropped.set(events_dropped.get().saturating_add(1));
        });
    }

    fn disconnect(&self, error: Option<RynkHostError>) {
        if let Some(error) = error {
            let _ = self.responses.try_send(Err(error));
        }
        self.dead.lock(|dead| dead.set(true));
        self.disconnected.signal(());
    }
}

/// Errors from the Rynk core and optional host layer.
#[derive(Debug, Error)]
pub enum RynkHostError {
    #[error("transport disconnected")]
    Disconnected,
    #[cfg(feature = "std")]
    #[error("io error: {0}")]
    Io(String),
    #[cfg(not(feature = "std"))]
    #[error("io error: {0:?}")]
    Io(ErrorKind),
    #[cfg(feature = "std")]
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error(
        "protocol major version mismatch — firmware speaks v{firmware_major}.{firmware_minor}, this tool speaks \
         v{host_major}.x (currently v{host_major}.{host_max_minor})"
    )]
    VersionMismatch {
        firmware_major: u8,
        firmware_minor: u8,
        host_major: u8,
        host_max_minor: u8,
    },
    #[error("device rejected {0:?}")]
    Rejected(RynkError),
    #[error("request {0:?} does not fit the configured frame buffer")]
    Encode(Cmd),
    #[error("response decode failed for {cmd:?}: {source}")]
    Deserialize { cmd: Cmd, source: postcard::Error },
    #[cfg(feature = "alloc")]
    #[error("layout blob decode failed: {0}")]
    Layout(String),
    #[error("frame of {len} bytes exceeds configured capacity {max}")]
    FrameTooLarge { len: usize, max: usize },
    #[error("response for {cmd:?} had trailing bytes")]
    TrailingBytes { cmd: Cmd },
    #[error("response cmd mismatch: sent {sent:?}, got {got:?}")]
    CmdMismatch { sent: Cmd, got: Cmd },
    #[error("{0:?} is a topic, not a request")]
    TopicCmd(Cmd),
    #[error("device does not support {0:?}: {1}")]
    Unsupported(Cmd, &'static str),
}

impl RynkHostError {
    fn io(error: &impl embedded_io_async::Error) -> Self {
        #[cfg(feature = "std")]
        {
            Self::Io(format!("{error:?}"))
        }
        #[cfg(not(feature = "std"))]
        {
            Self::Io(error.kind())
        }
    }
}

#[cfg(feature = "wasm")]
impl From<RynkHostError> for wasm_bindgen::JsValue {
    fn from(error: RynkHostError) -> Self {
        let kind = match &error {
            RynkHostError::Disconnected => "Disconnected",
            RynkHostError::Io(_) => "TransportError",
            RynkHostError::DeviceNotFound(_) => "DeviceNotFound",
            RynkHostError::Rejected(_) => "Rejected",
            RynkHostError::Unsupported(..) => "Unsupported",
            RynkHostError::VersionMismatch { .. } => "VersionMismatch",
            RynkHostError::Encode(_) => "RequestEncodeError",
            RynkHostError::Deserialize { .. } => "ResponseDecodeError",
            RynkHostError::Layout(_) => "LayoutDecodeError",
            RynkHostError::FrameTooLarge { .. } => "FrameTooLarge",
            RynkHostError::TrailingBytes { .. } => "ResponseTrailingBytes",
            RynkHostError::CmdMismatch { .. } => "ResponseCommandMismatch",
            RynkHostError::TopicCmd(_) => "InvalidRequestCommand",
        };
        let js_error = js_sys::Error::new(&error.to_string());
        js_error.set_name(kind);
        js_error.into()
    }
}

/// Raw server-to-host topic frame with fixed payload capacity.
#[derive(Debug, Clone)]
pub struct TopicFrame {
    pub cmd: Cmd,
    payload: [u8; TOPIC_PAYLOAD_SIZE],
    payload_len: usize,
}

impl TopicFrame {
    fn new(cmd: Cmd, payload: &[u8]) -> Result<Self, RynkHostError> {
        if payload.len() > TOPIC_PAYLOAD_SIZE {
            return Err(RynkHostError::FrameTooLarge {
                len: payload.len(),
                max: TOPIC_PAYLOAD_SIZE,
            });
        }
        let mut bytes = [0; TOPIC_PAYLOAD_SIZE];
        bytes[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            cmd,
            payload: bytes,
            payload_len: payload.len(),
        })
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len]
    }
}

/// Protocol-facing handle. It never owns transport halves or frame buffers.
pub struct Client<'a, const FRAME: usize = DEFAULT_FRAME_SIZE, const EVENTS: usize = DEFAULT_EVENT_CAPACITY> {
    #[cfg(feature = "std")]
    session: Arc<Session<FRAME, EVENTS>>,
    #[cfg(not(feature = "std"))]
    session: &'a Session<FRAME, EVENTS>,
    #[cfg(feature = "std")]
    _lifetime: PhantomData<&'a ()>,
    next_seq: u8,
    capabilities: Option<DeviceCapabilities>,
}

impl<const FRAME: usize, const EVENTS: usize> Drop for Client<'_, FRAME, EVENTS> {
    fn drop(&mut self) {
        self.session.shutdown.signal(());
    }
}

impl<'a, const FRAME: usize, const EVENTS: usize> Client<'a, FRAME, EVENTS> {
    pub async fn handshake(&mut self) -> Result<(), RynkHostError> {
        let version: ProtocolVersion = self.request_raw(Cmd::GetVersion, &()).await?;
        let supported = ProtocolVersion::CURRENT;
        if version.major != supported.major {
            return Err(RynkHostError::VersionMismatch {
                firmware_major: version.major,
                firmware_minor: version.minor,
                host_major: supported.major,
                host_max_minor: supported.minor,
            });
        }
        if version.minor > supported.minor {
            log::info!(
                "rynk: firmware protocol v{}.{} is newer than this client's v{}.{}",
                version.major,
                version.minor,
                supported.major,
                supported.minor
            );
        }
        let capabilities = self.request_raw(Cmd::GetCapabilities, &()).await?;
        self.capabilities = Some(capabilities);
        Ok(())
    }

    pub fn cached_capabilities(&self) -> Option<DeviceCapabilities> {
        self.capabilities
    }

    pub(crate) fn capabilities(&self) -> DeviceCapabilities {
        self.cached_capabilities().unwrap_or_default()
    }

    pub fn is_alive(&self) -> bool {
        self.session.is_alive()
    }

    pub fn events_dropped(&self) -> u64 {
        self.session.events_dropped()
    }

    pub async fn next_topic_frame(&mut self) -> Result<TopicFrame, RynkHostError> {
        let session = &self.session;
        loop {
            if !session.is_alive() {
                return Err(RynkHostError::Disconnected);
            }
            match select3(
                session.topics.receive(),
                session.responses.receive(),
                session.disconnected.wait(),
            )
            .await
            {
                Either3::First(topic) => return Ok(topic),
                Either3::Second(Ok(_)) => {}
                Either3::Second(Err(error)) => return Err(error),
                Either3::Third(()) => return Err(RynkHostError::Disconnected),
            }
        }
    }

    pub async fn request<E: Endpoint>(&mut self, request: &E::Request) -> Result<E::Response, RynkHostError> {
        self.request_raw(E::CMD, request).await
    }

    pub async fn request_raw<Req: Serialize, Resp: DeserializeOwned>(
        &mut self,
        cmd: Cmd,
        request: &Req,
    ) -> Result<Resp, RynkHostError> {
        let frame = self.exchange(cmd, request, true).await?;
        let header = RynkHeader::parse(frame.first_chunk().unwrap());
        let (envelope, rest) =
            postcard::take_from_bytes::<Result<Resp, RynkError>>(&frame[RYNK_HEADER_SIZE..header.frame_len()])
                .map_err(|source| RynkHostError::Deserialize { cmd, source })?;
        if !rest.is_empty() {
            return Err(RynkHostError::TrailingBytes { cmd });
        }
        envelope.map_err(RynkHostError::Rejected)
    }

    pub async fn send_no_reply<E: Endpoint>(&mut self, request: &E::Request) -> Result<(), RynkHostError> {
        self.exchange(E::CMD, request, false).await?;
        Ok(())
    }

    async fn exchange<Req: Serialize>(
        &mut self,
        cmd: Cmd,
        request: &Req,
        expects_response: bool,
    ) -> Result<[u8; FRAME], RynkHostError> {
        if cmd.is_topic() {
            return Err(RynkHostError::TopicCmd(cmd));
        }

        let seq = self.next_seq;
        self.next_seq = self.next_seq.checked_add(1).unwrap_or(1);
        let mut frame = [0; FRAME];
        let capacity = FRAME.min(RYNK_HEADER_SIZE + u16::MAX as usize);
        let header = RynkMessage::build(&mut frame[..capacity], cmd, seq, request)
            .map_err(|_| RynkHostError::Encode(cmd))?
            .header();
        if self
            .capabilities
            .is_some_and(|capabilities| header.payload_len > capabilities.max_payload_size)
        {
            return Err(RynkHostError::Encode(cmd));
        }

        let session = &self.session;
        if !session.is_alive() {
            return Err(RynkHostError::Disconnected);
        }
        match select(
            session.requests.send((frame, expects_response)),
            session.disconnected.wait(),
        )
        .await
        {
            Either::First(()) => {}
            Either::Second(()) => return Err(RynkHostError::Disconnected),
        }

        loop {
            match select(session.responses.receive(), session.disconnected.wait()).await {
                Either::First(Ok(frame)) => {
                    let header = RynkHeader::parse(frame.first_chunk().unwrap());
                    if header.seq != seq {
                        continue;
                    }
                    if header.cmd != cmd {
                        return Err(RynkHostError::CmdMismatch {
                            sent: cmd,
                            got: header.cmd,
                        });
                    }
                    return Ok(frame);
                }
                Either::First(Err(error)) => return Err(error),
                Either::Second(()) => return Err(RynkHostError::Disconnected),
            }
        }
    }
}

#[cfg(feature = "std")]
impl Client<'static, DEFAULT_FRAME_SIZE, DEFAULT_EVENT_CAPACITY> {
    /// Construct a desktop-host pair with an owned default-capacity session.
    pub fn from_transport<T: Transport>(
        transport: T,
    ) -> (
        Self,
        Driver<'static, T::Read, T::Write, DEFAULT_FRAME_SIZE, DEFAULT_EVENT_CAPACITY>,
    ) {
        let (writer, reader) = transport.split();
        Driver::new(Session::new(), reader, writer)
    }
}

/// Owns and concurrently drives a [`Reader`] and [`Writer`].
pub struct Driver<
    'a,
    R: Read,
    W: Write,
    const FRAME: usize = DEFAULT_FRAME_SIZE,
    const EVENTS: usize = DEFAULT_EVENT_CAPACITY,
> {
    pub reader: Reader<'a, R, FRAME, EVENTS>,
    pub writer: Writer<'a, W, FRAME, EVENTS>,
    #[cfg(feature = "std")]
    session: Arc<Session<FRAME, EVENTS>>,
    #[cfg(not(feature = "std"))]
    session: &'a Session<FRAME, EVENTS>,
}

#[cfg(feature = "std")]
impl<R: Read, W: Write, const FRAME: usize, const EVENTS: usize> Driver<'static, R, W, FRAME, EVENTS> {
    pub fn new(session: Session<FRAME, EVENTS>, reader: R, writer: W) -> (Client<'static, FRAME, EVENTS>, Self) {
        let session = Arc::new(session);
        let client = Client {
            session: session.clone(),
            _lifetime: PhantomData,
            next_seq: 1,
            capabilities: None,
        };
        let reader = Reader {
            buf: [0; FRAME],
            used: 0,
            reader,
            session: session.clone(),
            _lifetime: PhantomData,
        };
        let writer = Writer {
            writer,
            session: session.clone(),
            _lifetime: PhantomData,
        };
        (
            client,
            Self {
                reader,
                writer,
                session,
            },
        )
    }
}

#[cfg(not(feature = "std"))]
impl<'a, R: Read, W: Write, const FRAME: usize, const EVENTS: usize> Driver<'a, R, W, FRAME, EVENTS> {
    pub fn new(session: &'a Session<FRAME, EVENTS>, reader: R, writer: W) -> (Client<'a, FRAME, EVENTS>, Self) {
        let client = Client {
            session,
            next_seq: 1,
            capabilities: None,
        };
        let reader = Reader {
            buf: [0; FRAME],
            used: 0,
            reader,
            session,
        };
        let writer = Writer { writer, session };
        (
            client,
            Self {
                reader,
                writer,
                session,
            },
        )
    }
}

impl<R: Read, W: Write, const FRAME: usize, const EVENTS: usize> Driver<'_, R, W, FRAME, EVENTS> {
    pub async fn run(&mut self) -> Result<(), RynkHostError> {
        let result = match select3(self.reader.run(), self.writer.run(), self.session.shutdown.wait()).await {
            Either3::First(result) | Either3::Second(result) => result,
            Either3::Third(()) => Ok(()),
        };
        self.session.disconnect(None);
        result
    }
}

impl<R: Read, W: Write, const FRAME: usize, const EVENTS: usize> Drop for Driver<'_, R, W, FRAME, EVENTS> {
    fn drop(&mut self) {
        self.session.disconnect(None);
    }
}

/// Device-to-host frame reader and response/topic dispatcher.
pub struct Reader<'a, R: Read, const FRAME: usize = DEFAULT_FRAME_SIZE, const EVENTS: usize = DEFAULT_EVENT_CAPACITY> {
    buf: [u8; FRAME],
    used: usize,
    reader: R,
    #[cfg(feature = "std")]
    session: Arc<Session<FRAME, EVENTS>>,
    #[cfg(not(feature = "std"))]
    session: &'a Session<FRAME, EVENTS>,
    #[cfg(feature = "std")]
    _lifetime: PhantomData<&'a ()>,
}

impl<'a, R: Read, const FRAME: usize, const EVENTS: usize> Reader<'a, R, FRAME, EVENTS> {
    pub async fn run(&mut self) -> Result<(), RynkHostError> {
        loop {
            while let Some(header) = self.buf[..self.used]
                .first_chunk::<RYNK_HEADER_SIZE>()
                .map(RynkHeader::parse)
            {
                let frame_len = header.frame_len();
                if frame_len > FRAME {
                    let error = RynkHostError::FrameTooLarge {
                        len: frame_len,
                        max: FRAME,
                    };
                    self.session.disconnect(Some(error));
                    return Err(RynkHostError::FrameTooLarge {
                        len: frame_len,
                        max: FRAME,
                    });
                }
                if self.used < frame_len {
                    break;
                }

                let payload = &self.buf[RYNK_HEADER_SIZE..frame_len];
                if header.cmd.is_topic() {
                    match TopicFrame::new(header.cmd, payload) {
                        Ok(topic) => self.session.queue_topic(topic),
                        Err(_) => self.session.record_dropped_topic(),
                    }
                } else {
                    let mut frame = [0; FRAME];
                    frame[..frame_len].copy_from_slice(&self.buf[..frame_len]);
                    self.session.responses.send(Ok(frame)).await;
                }

                self.buf.copy_within(frame_len..self.used, 0);
                self.used -= frame_len;
            }

            if self.used == FRAME {
                let error = RynkHostError::FrameTooLarge {
                    len: self.used + 1,
                    max: FRAME,
                };
                self.session.disconnect(Some(error));
                return Err(RynkHostError::FrameTooLarge {
                    len: self.used + 1,
                    max: FRAME,
                });
            }

            let read = match self.reader.read(&mut self.buf[self.used..]).await {
                Ok(0) => {
                    self.session.disconnect(Some(RynkHostError::Disconnected));
                    return Err(RynkHostError::Disconnected);
                }
                Ok(read) => read,
                Err(error) => {
                    let client_error = RynkHostError::io(&error);
                    self.session.disconnect(Some(client_error));
                    return Err(RynkHostError::io(&error));
                }
            };
            self.used += read;
        }
    }
}

/// Host-to-device request framer and writer.
pub struct Writer<'a, W: Write, const FRAME: usize = DEFAULT_FRAME_SIZE, const EVENTS: usize = DEFAULT_EVENT_CAPACITY> {
    writer: W,
    #[cfg(feature = "std")]
    session: Arc<Session<FRAME, EVENTS>>,
    #[cfg(not(feature = "std"))]
    session: &'a Session<FRAME, EVENTS>,
    #[cfg(feature = "std")]
    _lifetime: PhantomData<&'a ()>,
}

impl<'a, W: Write, const FRAME: usize, const EVENTS: usize> Writer<'a, W, FRAME, EVENTS> {
    pub async fn run(&mut self) -> Result<(), RynkHostError> {
        loop {
            let (frame, expects_response) = self.session.requests.receive().await;
            if !self.session.is_alive() {
                return Err(RynkHostError::Disconnected);
            }

            let frame_len = RynkHeader::parse(frame.first_chunk().unwrap()).frame_len();
            if let Err(error) = self.writer.write_all(&frame[..frame_len]).await {
                let client_error = RynkHostError::io(&error);
                self.session.disconnect(Some(client_error));
                return Err(RynkHostError::io(&error));
            }

            if !expects_response {
                self.session.responses.send(Ok(frame)).await;
            }
        }
    }
}

#[cfg(all(test, feature = "std", not(target_arch = "wasm32")))]
mod tests {
    use std::future::pending;
    use std::time::Duration;
    use std::vec::Vec;

    use embassy_futures::join::join;
    use embassy_futures::select::{Either, select};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embassy_sync::pipe::Pipe;
    use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
    use rmk_types::protocol::rynk::{RynkMessage, TopicEvent};
    use tokio::time::{sleep, timeout};

    use super::*;
    use crate::IncomingTopic;

    type TestPipe = Pipe<NoopRawMutex, 2048>;

    fn frame<T: Serialize>(cmd: Cmd, seq: u8, value: &T) -> Vec<u8> {
        let mut bytes = vec![0; 512];
        RynkMessage::build(&mut bytes, cmd, seq, value)
            .unwrap()
            .frame()
            .to_vec()
    }

    fn reply<T: Serialize>(cmd: Cmd, seq: u8, value: T) -> Vec<u8> {
        frame(cmd, seq, &Ok::<T, RynkError>(value))
    }

    fn capabilities() -> DeviceCapabilities {
        DeviceCapabilities {
            num_layers: 4,
            num_rows: 6,
            num_cols: 14,
            max_payload_size: 123,
            ..Default::default()
        }
    }

    async fn read_request(pipe: &TestPipe) -> RynkHeader {
        let mut reader = pipe;
        let mut bytes = [0; RYNK_HEADER_SIZE];
        reader.read_exact(&mut bytes).await.unwrap();
        let header = RynkHeader::parse(&bytes);
        let mut payload = vec![0; header.payload_len as usize];
        reader.read_exact(&mut payload).await.unwrap();
        header
    }

    async fn write_bytes(pipe: &TestPipe, bytes: &[u8]) {
        let mut writer = pipe;
        Write::write_all(&mut writer, bytes).await.unwrap();
    }

    async fn handshake_peer(requests: &TestPipe, responses: &TestPipe) {
        let version = read_request(requests).await;
        assert_eq!(version.cmd, Cmd::GetVersion);
        write_bytes(responses, &reply(version.cmd, version.seq, ProtocolVersion::CURRENT)).await;

        let capabilities_request = read_request(requests).await;
        assert_eq!(capabilities_request.cmd, Cmd::GetCapabilities);
        write_bytes(
            responses,
            &reply(capabilities_request.cmd, capabilities_request.seq, capabilities()),
        )
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_reassembles_and_routes() {
        let requests = TestPipe::new();
        let responses = TestPipe::new();
        let (mut client, mut driver) = Driver::new(Session::<128, 4>::new(), &responses, &requests);

        let peer = async {
            let version = read_request(&requests).await;
            let mut bytes = frame(Cmd::LayerChange, 0, &3u8);
            bytes.extend_from_slice(&reply(version.cmd, version.seq, ProtocolVersion::CURRENT));
            write_bytes(&responses, &bytes[..2]).await;
            write_bytes(&responses, &bytes[2..]).await;

            let capabilities_request = read_request(&requests).await;
            write_bytes(
                &responses,
                &reply(capabilities_request.cmd, capabilities_request.seq, capabilities()),
            )
            .await;

            let wpm = read_request(&requests).await;
            write_bytes(&responses, &reply(wpm.cmd, 0xee, 99u16)).await;
            write_bytes(&responses, &reply(wpm.cmd, wpm.seq, 42u16)).await;
        };
        let client_task = async {
            client.handshake().await.unwrap();
            assert_eq!(client.cached_capabilities().unwrap().num_cols, 14);
            assert!(matches!(
                client.next_event().await.unwrap(),
                IncomingTopic::Topic(TopicEvent::LayerChange(3))
            ));
            assert_eq!(client.get_wpm().await.unwrap(), 42);
        };

        match select(driver.run(), join(peer, client_task)).await {
            Either::First(result) => panic!("driver stopped early: {result:?}"),
            Either::Second(((), ())) => {}
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_request_does_not_cancel_reader() {
        let requests = TestPipe::new();
        let responses = TestPipe::new();
        let (mut client, mut driver) = Driver::new(Session::<128, 4>::new(), &responses, &requests);

        let peer = async {
            handshake_peer(&requests, &responses).await;
            let first = read_request(&requests).await;
            assert_eq!(first.cmd, Cmd::GetWpm);
            sleep(Duration::from_millis(20)).await;
            write_bytes(&responses, &reply(first.cmd, first.seq, 12u16)).await;

            let second = read_request(&requests).await;
            assert_eq!(second.cmd, Cmd::GetSleepState);
            write_bytes(&responses, &reply(second.cmd, second.seq, true)).await;
        };
        let client_task = async {
            client.handshake().await.unwrap();
            assert!(timeout(Duration::from_millis(5), client.get_wpm()).await.is_err());
            assert!(client.get_sleep_state().await.unwrap());
            assert!(client.is_alive());
        };

        match select(driver.run(), join(peer, client_task)).await {
            Either::First(result) => panic!("driver stopped early: {result:?}"),
            Either::Second(((), ())) => {}
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn full_topic_queue_drops_oldest_without_blocking_reply() {
        let requests = TestPipe::new();
        let responses = TestPipe::new();
        let (mut client, mut driver) = Driver::new(Session::<128, 2>::new(), &responses, &requests);

        let peer = async {
            handshake_peer(&requests, &responses).await;
            let request = read_request(&requests).await;
            let mut bytes = Vec::new();
            for layer in 0..3u8 {
                bytes.extend_from_slice(&frame(Cmd::LayerChange, 0, &layer));
            }
            bytes.extend_from_slice(&reply(request.cmd, request.seq, 7u16));
            write_bytes(&responses, &bytes).await;
        };
        let client_task = async {
            client.handshake().await.unwrap();
            assert_eq!(client.get_wpm().await.unwrap(), 7);
            assert_eq!(client.events_dropped(), 1);
            assert!(matches!(
                client.next_event().await.unwrap(),
                IncomingTopic::Topic(TopicEvent::LayerChange(1))
            ));
        };

        match select(driver.run(), join(peer, client_task)).await {
            Either::First(result) => panic!("driver stopped early: {result:?}"),
            Either::Second(((), ())) => {}
        }
    }

    struct HangingReader;

    impl ErrorType for HangingReader {
        type Error = ErrorKind;
    }

    impl Read for HangingReader {
        async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
            pending().await
        }
    }

    struct FailingWriter;

    impl ErrorType for FailingWriter {
        type Error = ErrorKind;
    }

    impl Write for FailingWriter {
        async fn write(&mut self, _buf: &[u8]) -> Result<usize, Self::Error> {
            Err(ErrorKind::Other)
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn writer_failure_reaches_client() {
        let (mut client, mut driver) = Driver::new(Session::<64, 2>::new(), HangingReader, FailingWriter);
        client.capabilities = Some(DeviceCapabilities {
            max_payload_size: 59,
            ..Default::default()
        });

        let (driver_result, ()) = join(driver.run(), async {
            assert!(matches!(client.get_wpm().await, Err(RynkHostError::Io(_))));
            assert!(!client.is_alive());
        })
        .await;
        assert!(matches!(driver_result, Err(RynkHostError::Io(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn protocol_response_errors_do_not_desynchronize_driver() {
        let requests = TestPipe::new();
        let responses = TestPipe::new();
        let (mut client, mut driver) = Driver::new(Session::<128, 2>::new(), &responses, &requests);

        let peer = async {
            handshake_peer(&requests, &responses).await;

            let rejected = read_request(&requests).await;
            write_bytes(
                &responses,
                &frame(rejected.cmd, rejected.seq, &Err::<(), RynkError>(RynkError::Invalid)),
            )
            .await;

            let mismatch = read_request(&requests).await;
            write_bytes(&responses, &reply(Cmd::GetSleepState, mismatch.seq, true)).await;

            let trailing = read_request(&requests).await;
            let mut bytes = reply(trailing.cmd, trailing.seq, 9u16);
            let payload_len = u16::from_le_bytes([bytes[3], bytes[4]]) + 1;
            bytes[3..5].copy_from_slice(&payload_len.to_le_bytes());
            bytes.push(0xaa);
            write_bytes(&responses, &bytes).await;
        };
        let client_task = async {
            client.handshake().await.unwrap();
            assert!(matches!(
                client.set_default_layer(9).await,
                Err(RynkHostError::Rejected(RynkError::Invalid))
            ));
            assert!(matches!(
                client.get_wpm().await,
                Err(RynkHostError::CmdMismatch {
                    sent: Cmd::GetWpm,
                    got: Cmd::GetSleepState,
                })
            ));
            assert!(matches!(
                client.get_wpm().await,
                Err(RynkHostError::TrailingBytes { cmd: Cmd::GetWpm })
            ));
            assert!(client.is_alive());
        };

        match select(driver.run(), join(peer, client_task)).await {
            Either::First(result) => panic!("driver stopped early: {result:?}"),
            Either::Second(((), ())) => {}
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_reply_completes_after_writer_accepts_frame() {
        let requests = TestPipe::new();
        let responses = TestPipe::new();
        let (mut client, mut driver) = Driver::new(Session::<64, 2>::new(), &responses, &requests);
        client.capabilities = Some(DeviceCapabilities {
            max_payload_size: 59,
            ..Default::default()
        });

        let peer = async {
            let request = read_request(&requests).await;
            assert_eq!(request.cmd, Cmd::Reboot);
        };
        let client_task = async {
            client.reboot().await.unwrap();
        };

        match select(driver.run(), join(peer, client_task)).await {
            Either::First(result) => panic!("driver stopped early: {result:?}"),
            Either::Second(((), ())) => {}
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_while_enqueueing_leaves_channel_unchanged() {
        let (mut client, _driver) = Driver::new(Session::<64, 2>::new(), HangingReader, FailingWriter);
        client.capabilities = Some(DeviceCapabilities {
            max_payload_size: 59,
            ..Default::default()
        });
        let mut occupied = [0; 64];
        RynkMessage::build(&mut occupied, Cmd::GetVersion, 1, &()).unwrap();
        assert!(client.session.requests.try_send((occupied, true)).is_ok());

        assert!(timeout(Duration::from_millis(1), client.get_wpm()).await.is_err());
        let (queued, expects_response) = client.session.requests.try_receive().unwrap();
        assert_eq!(RynkHeader::parse(queued.first_chunk().unwrap()).cmd, Cmd::GetVersion);
        assert!(expects_response);
        assert!(client.session.requests.try_receive().is_err());
    }
}
