use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use embassy_futures::join::join;
use rmk_types::action::KeyAction;
use rmk_types::battery::BatteryStatus;
use rmk_types::connection::{ConnectionStatus, ConnectionType};
use rmk_types::protocol::rynk::{
    GetComboBulkResponse, GetKeymapBulkResponse, GetMorseBulkResponse, PeripheralStatus, SetComboBulkRequest,
    SetKeymapBulkRequest, SetMorseBulkRequest,
};
use tokio::time::timeout;

use super::*;
use crate::device::RynkDevice;

/// The scripted link wrapped as a device, so tests connect through the one
/// entry point: [`RynkDevice::connect`].
struct MockDevice(Vec<Step>);

impl RynkDevice for MockDevice {
    type Read = MockRead;
    type Write = MockWrite;

    fn label(&self) -> String {
        "mock".into()
    }

    async fn open(self) -> Result<(MockWrite, MockRead), RynkHostError> {
        Ok(mock_halves(self.0, false))
    }
}

enum Step {
    /// Deliver these bytes to the reader (across one or more reads).
    Chunk(Vec<u8>),
    /// Park the reader until the writer has completed this many frame
    /// writes — paces scripted replies like real firmware. The count
    /// equals the reply SEQ, since both count requests from session start.
    AwaitWrites(usize),
    /// Park the reader forever (the script drives the session to its end).
    Hang,
}

/// Written-frame counter shared by the mock halves, with a waker so the
/// reader can await it without spinning (spinning would starve tokio's
/// paused-clock auto-advance).
struct Writes {
    count: AtomicUsize,
    notify: tokio::sync::Notify,
}

/// A scripted link's two halves, sharing the written-frame counter.
fn mock_halves(steps: Vec<Step>, fail_send: bool) -> (MockWrite, MockRead) {
    let writes = Arc::new(Writes {
        count: AtomicUsize::new(0),
        notify: tokio::sync::Notify::new(),
    });
    (
        MockWrite {
            fail_send,
            writes: writes.clone(),
        },
        MockRead {
            steps: steps.into(),
            pending: Vec::new(),
            pos: 0,
            writes,
        },
    )
}

struct MockRead {
    steps: VecDeque<Step>,
    pending: Vec<u8>,
    pos: usize,
    writes: Arc<Writes>,
}

impl embedded_io_async::ErrorType for MockRead {
    type Error = ErrorKind;
}

impl Read for MockRead {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ErrorKind> {
        while self.pos >= self.pending.len() {
            // Pop only once a step completes: reads are cancelled at select
            // exit, and a blocking step must survive into the next `run`.
            match self.steps.front() {
                Some(Step::Chunk(_)) => {
                    let Some(Step::Chunk(c)) = self.steps.pop_front() else {
                        unreachable!()
                    };
                    self.pending = c;
                    self.pos = 0;
                }
                Some(&Step::AwaitWrites(n)) => {
                    loop {
                        let notified = self.writes.notify.notified();
                        if self.writes.count.load(Ordering::Acquire) >= n {
                            break;
                        }
                        notified.await;
                    }
                    self.steps.pop_front();
                }
                Some(Step::Hang) => std::future::pending().await,
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct MockWrite {
    fail_send: bool,
    writes: Arc<Writes>,
}

impl embedded_io_async::ErrorType for MockWrite {
    type Error = ErrorKind;
}

impl Write for MockWrite {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, ErrorKind> {
        if self.fail_send {
            return Err(ErrorKind::Other);
        }
        self.writes.count.fetch_add(1, Ordering::Release);
        self.writes.notify.notify_waiters();
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), ErrorKind> {
        Ok(())
    }
}

/// An unhandshaked session with a generous send budget, for tests that
/// exercise the wire without `connect`.
fn raw_session(steps: Vec<Step>) -> (Client, Driver<MockRead, MockWrite>) {
    let mut client = Client::new();
    client.capabilities.max_payload_size = 4096;
    let (writer, reader) = mock_halves(steps, false);
    (client, Driver::new(reader, writer))
}

/// Run `fut` with the driver pumping, expecting the link to outlive it.
async fn drive<F: Future>(driver: &mut Driver<MockRead, MockWrite>, client: &Client, fut: F) -> F::Output {
    match select(driver.run(client), fut).await {
        Either::First(err) => panic!("driver died during test: {err}"),
        Either::Second(v) => v,
    }
}

/// A bare frame: header + postcard(value). Used for raw headers and topics.
fn frame<T: Serialize>(cmd: Cmd, seq: u8, value: &T) -> Vec<u8> {
    let mut buf = vec![0u8; 4096];
    let msg = RynkMessage::build(&mut buf, cmd, seq, value).unwrap();
    msg.frame().to_vec()
}

/// An `Ok` response frame, enveloped as the firmware sends it.
fn reply<T: Serialize>(cmd: Cmd, seq: u8, value: T) -> Vec<u8> {
    frame(cmd, seq, &Ok::<T, RynkError>(value))
}

/// A topic push frame (bare payload, SEQ 0).
fn topic<T: Serialize>(cmd: Cmd, value: T) -> Vec<u8> {
    frame(cmd, 0, &value)
}

fn header(cmd_raw: u16, seq: u8, len: u16) -> Vec<u8> {
    let c = cmd_raw.to_le_bytes();
    let l = len.to_le_bytes();
    vec![c[0], c[1], seq, l[0], l[1]]
}

// Tests clone this and flip only the capability under test.
fn caps() -> DeviceCapabilities {
    DeviceCapabilities {
        num_layers: 4,
        num_rows: 6,
        num_cols: 14,
        max_combos: 8,
        max_combo_keys: 4,
        macro_space_size: 1024,
        max_morse: 4,
        max_patterns_per_key: 4,
        max_forks: 4,
        storage_enabled: true,
        max_payload_size: 256,
        macro_chunk_size: 64,
        ..Default::default()
    }
}

/// The handshake reply pair every `connect` consumes first.
fn handshake_steps(capabilities: DeviceCapabilities) -> Vec<Step> {
    vec![
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::GetVersion, 1, ProtocolVersion::CURRENT)),
        Step::AwaitWrites(2),
        Step::Chunk(reply(Cmd::GetCapabilities, 2, capabilities)),
    ]
}

#[tokio::test]
async fn reply_round_trip() {
    let (client, mut driver) = raw_session(vec![
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::GetWpm, 1, 42u16)),
        Step::Hang,
    ]);
    let got = drive(&mut driver, &client, client.get_wpm()).await.unwrap();
    assert_eq!(got, 42);
}

#[tokio::test]
async fn rejected_response_flattens() {
    let (client, mut driver) = raw_session(vec![
        Step::AwaitWrites(1),
        Step::Chunk(frame(
            Cmd::SetDefaultLayer,
            1,
            &Err::<(), RynkError>(RynkError::Invalid),
        )),
        Step::Hang,
    ]);
    let r = drive(&mut driver, &client, client.set_default_layer(9)).await;
    assert!(matches!(r, Err(RynkHostError::Rejected(RynkError::Invalid))));
}

#[tokio::test]
async fn trailing_bytes_rejected() {
    // Reply declares extra bytes beyond the decoded u16.
    let mut chunk = reply(Cmd::GetWpm, 1, 42u16);
    chunk[3] += 2; // bump the declared LEN
    chunk.extend_from_slice(&[0xAA, 0xBB]);
    let (client, mut driver) = raw_session(vec![Step::AwaitWrites(1), Step::Chunk(chunk), Step::Hang]);
    let r = drive(&mut driver, &client, client.get_wpm()).await;
    assert!(matches!(r, Err(RynkHostError::TrailingBytes { cmd: Cmd::GetWpm })));
}

#[tokio::test]
async fn topic_cmd_to_request_rejected() {
    // The reject happens before any wire traffic, so no driver is needed.
    let (client, _driver) = raw_session(vec![]);
    let r = client.request_raw::<(), u8>(Cmd::LayerChange, &()).await;
    assert!(matches!(r, Err(RynkHostError::TopicCmd(Cmd::LayerChange))));
}

#[tokio::test]
async fn unknown_cmd_drained_by_len() {
    let mut chunk = header(0x7fff, 0xEE, 5);
    chunk.extend_from_slice(&[1, 2, 3, 4, 5]);
    chunk.extend_from_slice(&reply(Cmd::GetWpm, 1, 42u16));
    let (client, mut driver) = raw_session(vec![Step::AwaitWrites(1), Step::Chunk(chunk), Step::Hang]);
    let got = drive(&mut driver, &client, client.get_wpm()).await.unwrap();
    assert_eq!(got, 42);
}

#[tokio::test]
async fn unknown_topic_skipped_by_next_event() {
    let mut chunk = header(0x80ff, 0, 3);
    chunk.extend_from_slice(&[1, 2, 3]);
    chunk.extend_from_slice(&topic(Cmd::LayerChange, 3u8));
    let (client, mut driver) = raw_session(vec![Step::Chunk(chunk), Step::Hang]);
    let ev = drive(&mut driver, &client, client.next_event()).await;
    assert!(matches!(ev, TopicEvent::LayerChange(3)));
}

#[tokio::test]
async fn unknown_response_cmd_mismatch_detected() {
    let (client, mut driver) = raw_session(vec![
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::from_raw(0x7fff), 1, 42u16)),
        Step::Hang,
    ]);
    let r = drive(&mut driver, &client, client.get_wpm()).await;
    assert!(matches!(
        r,
        Err(RynkHostError::CmdMismatch {
            sent: Cmd::GetWpm,
            got,
        }) if got == Cmd::from_raw(0x7fff)
    ));
}

#[tokio::test]
async fn oversized_inbound_topic_is_not_fatal() {
    // 300 bytes overflows the no-alloc topic slot; the link must stay healthy.
    let mut big = header(0x80ff, 0, 300);
    big.extend_from_slice(&vec![0xAB; 300]);
    let mut steps = vec![Step::Chunk(big)];
    steps.extend([
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::GetWpm, 1, 7u16)),
        Step::Hang,
    ]);
    let (client, mut driver) = raw_session(steps);
    let got = drive(&mut driver, &client, client.get_wpm()).await.unwrap();
    assert_eq!(got, 7);
}

#[tokio::test]
async fn eof_ends_driver() {
    let (client, mut driver) = raw_session(vec![]);
    match select(driver.run(&client), client.get_wpm()).await {
        Either::First(err) => assert!(matches!(err, RynkHostError::Disconnected)),
        Either::Second(r) => panic!("request should not resolve on a dead link: {r:?}"),
    }
}

#[tokio::test]
async fn send_failure_ends_driver() {
    let mut client = Client::new();
    client.capabilities.max_payload_size = 4096;
    let (writer, reader) = mock_halves(vec![Step::Hang], true);
    let mut driver = Driver::new(reader, writer);
    match select(driver.run(&client), client.get_wpm()).await {
        Either::First(err) => assert!(matches!(err, RynkHostError::Io(_))),
        Either::Second(r) => panic!("request should not resolve on a dead link: {r:?}"),
    }
}

#[tokio::test]
async fn topic_during_request_is_queued() {
    let mut chunk = topic(Cmd::LayerChange, 3u8);
    chunk.extend_from_slice(&reply(Cmd::GetWpm, 1, 42u16));
    let (client, mut driver) = raw_session(vec![Step::AwaitWrites(1), Step::Chunk(chunk), Step::Hang]);
    let got = drive(&mut driver, &client, client.get_wpm()).await.unwrap();
    assert_eq!(got, 42);
    let ev = drive(&mut driver, &client, client.next_event()).await;
    assert!(matches!(ev, TopicEvent::LayerChange(3)));
}

#[tokio::test]
async fn request_and_next_event_run_full_duplex() {
    // One branch waits for a reply while the other receives a topic — the
    // two consume different channels and never block each other.
    let mut chunk = topic(Cmd::LayerChange, 7u8);
    chunk.extend_from_slice(&reply(Cmd::GetWpm, 1, 42u16));
    let (client, mut driver) = raw_session(vec![Step::AwaitWrites(1), Step::Chunk(chunk), Step::Hang]);
    let (wpm, ev) = drive(&mut driver, &client, join(client.get_wpm(), client.next_event())).await;
    assert_eq!(wpm.unwrap(), 42);
    assert!(matches!(ev, TopicEvent::LayerChange(7)));
}

#[tokio::test]
async fn stale_seq_reply_dropped() {
    let mut chunk = reply(Cmd::GetWpm, 0xEE, 99u16);
    chunk.extend_from_slice(&reply(Cmd::GetWpm, 1, 42u16));
    let (client, mut driver) = raw_session(vec![Step::AwaitWrites(1), Step::Chunk(chunk), Step::Hang]);
    let got = drive(&mut driver, &client, client.get_wpm()).await.unwrap();
    assert_eq!(got, 42);
}

#[tokio::test]
async fn cmd_mismatch_detected() {
    let (client, mut driver) = raw_session(vec![
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::GetSleepState, 1, true)),
        Step::Hang,
    ]);
    let r = drive(&mut driver, &client, client.get_wpm()).await;
    assert!(matches!(
        r,
        Err(RynkHostError::CmdMismatch {
            sent: Cmd::GetWpm,
            got: Cmd::GetSleepState,
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn cancelled_request_does_not_poison_the_link() {
    // Request 1 (seq 1) gets no reply and is cancelled by its timeout;
    // request 2 (seq 2) succeeds over the same session.
    let (client, mut driver) = raw_session(vec![
        Step::AwaitWrites(2),
        Step::Chunk(reply(Cmd::GetWpm, 2, 42u16)),
        Step::Hang,
    ]);
    let got = drive(&mut driver, &client, async {
        let cancelled = timeout(Duration::from_millis(10), client.get_wpm()).await;
        assert!(cancelled.is_err(), "request 1 should time out");
        client.get_wpm().await
    })
    .await
    .unwrap();
    assert_eq!(got, 42);
}

#[tokio::test(start_paused = true)]
async fn stale_reply_of_cancelled_request_is_dropped() {
    // Request 1's reply arrives only after its caller gave up; request 2
    // must skip it by SEQ and still get its own answer.
    let (client, mut driver) = raw_session(vec![
        Step::AwaitWrites(2),
        Step::Chunk(reply(Cmd::GetWpm, 1, 99u16)),
        Step::Chunk(reply(Cmd::GetWpm, 2, 42u16)),
        Step::Hang,
    ]);
    let got = drive(&mut driver, &client, async {
        let cancelled = timeout(Duration::from_millis(10), client.get_wpm()).await;
        assert!(cancelled.is_err(), "request 1 should time out");
        client.get_wpm().await
    })
    .await
    .unwrap();
    assert_eq!(got, 42);
}

#[tokio::test]
async fn topic_queue_overflow_drops_oldest() {
    // 65 topics into a 64-slot queue: the first is evicted, the rest arrive.
    let mut chunk = Vec::new();
    for i in 0u8..=64 {
        chunk.extend_from_slice(&topic(Cmd::LayerChange, i));
    }
    let (client, mut driver) = raw_session(vec![Step::Chunk(chunk), Step::Hang]);
    let first = drive(&mut driver, &client, client.next_event()).await;
    assert!(matches!(first, TopicEvent::LayerChange(1)), "oldest topic evicted");
}

#[tokio::test]
async fn next_event_decodes_typed_payload() {
    let status = ConnectionStatus {
        preferred: ConnectionType::Ble,
        ..Default::default()
    };
    let (client, mut driver) = raw_session(vec![Step::Chunk(topic(Cmd::ConnectionChange, status)), Step::Hang]);
    let ev = drive(&mut driver, &client, client.next_event()).await;
    match ev {
        TopicEvent::ConnectionChange(s) => assert_eq!(s.preferred, ConnectionType::Ble),
        other => panic!("expected ConnectionChange, got {other:?}"),
    }
}

async fn connect_session(mut steps: Vec<Step>, trailing: Vec<Step>) -> (Client, Driver<MockRead, MockWrite>) {
    steps.extend(trailing);
    MockDevice(steps).connect().await.expect("connect should succeed")
}

#[tokio::test]
async fn connect_handshake_loopback() {
    let (client, mut driver) = connect_session(
        handshake_steps(caps()),
        vec![
            Step::AwaitWrites(3),
            Step::Chunk(reply(Cmd::GetWpm, 3, 37u16)),
            Step::Hang,
        ],
    )
    .await;
    assert_eq!(client.capabilities().num_cols, 14);
    let got = drive(&mut driver, &client, client.get_wpm()).await.unwrap();
    assert_eq!(got, 37);
}

#[tokio::test]
async fn capability_gate_rejects_without_wire_send() {
    let (client, _driver) = connect_session(handshake_steps(caps()), vec![Step::Hang]).await;
    assert!(!client.capabilities().ble_enabled);
    // The gate rejects locally; no driver polling is needed.
    let r = client.get_battery_status().await;
    assert!(matches!(r, Err(RynkHostError::Unsupported(Cmd::GetBatteryStatus, _))));
}

#[tokio::test]
async fn wired_split_peripheral_status_is_supported() {
    let status = PeripheralStatus {
        connected: true,
        battery: BatteryStatus::Unavailable,
    };
    let mut capabilities = caps();
    capabilities.is_split = true;
    capabilities.ble_enabled = false;
    let (client, mut driver) = connect_session(
        handshake_steps(capabilities),
        vec![
            Step::AwaitWrites(3),
            Step::Chunk(reply(Cmd::GetPeripheralStatus, 3, status)),
            Step::Hang,
        ],
    )
    .await;
    assert_eq!(
        drive(&mut driver, &client, client.get_peripheral_status(0))
            .await
            .unwrap(),
        status
    );
}

#[tokio::test]
async fn oversized_request_rejected_locally() {
    let mut tiny = caps();
    tiny.max_payload_size = 4;
    let (client, _driver) = connect_session(handshake_steps(tiny), vec![Step::Hang]).await;
    let r = client.set_key(0, 0, 0, KeyAction::Morse(3)).await;
    assert!(matches!(r, Err(RynkHostError::Encode(Cmd::SetKeyAction))));
}

#[tokio::test]
async fn bulk_methods_gate_without_wire_send() {
    let (client, _driver) = connect_session(handshake_steps(caps()), vec![Step::Hang]).await;
    assert!(!client.capabilities().bulk_transfer_supported);

    let keymap_req = SetKeymapBulkRequest {
        layer: 0,
        start_row: 0,
        start_col: 0,
        actions: Default::default(),
    };
    let combo_req = SetComboBulkRequest {
        start_index: 0,
        configs: Default::default(),
    };
    let morse_req = SetMorseBulkRequest {
        start_index: 0,
        configs: Default::default(),
    };

    assert!(matches!(
        client.get_keymap_bulk(0, 0, 0).await,
        Err(RynkHostError::Unsupported(Cmd::GetKeymapBulk, _))
    ));
    assert!(matches!(
        client.set_keymap_bulk(keymap_req).await,
        Err(RynkHostError::Unsupported(Cmd::SetKeymapBulk, _))
    ));
    assert!(matches!(
        client.get_combo_bulk(0).await,
        Err(RynkHostError::Unsupported(Cmd::GetComboBulk, _))
    ));
    assert!(matches!(
        client.set_combo_bulk(combo_req).await,
        Err(RynkHostError::Unsupported(Cmd::SetComboBulk, _))
    ));
    assert!(matches!(
        client.get_morse_bulk(0).await,
        Err(RynkHostError::Unsupported(Cmd::GetMorseBulk, _))
    ));
    assert!(matches!(
        client.set_morse_bulk(morse_req).await,
        Err(RynkHostError::Unsupported(Cmd::SetMorseBulk, _))
    ));
}

#[tokio::test]
async fn bulk_methods_round_trip_when_supported() {
    let mut supported = caps();
    supported.bulk_transfer_supported = true;
    supported.max_bulk_keys = 8;

    let keymap_resp = GetKeymapBulkResponse {
        actions: Default::default(),
    };
    let combo_resp = GetComboBulkResponse {
        configs: Default::default(),
    };
    let morse_resp = GetMorseBulkResponse {
        configs: Default::default(),
    };
    let (client, mut driver) = connect_session(
        handshake_steps(supported),
        vec![
            Step::AwaitWrites(3),
            Step::Chunk(reply(Cmd::SetKeymapBulk, 3, ())),
            Step::AwaitWrites(4),
            Step::Chunk(reply(Cmd::GetKeymapBulk, 4, keymap_resp.clone())),
            Step::AwaitWrites(5),
            Step::Chunk(reply(Cmd::SetComboBulk, 5, ())),
            Step::AwaitWrites(6),
            Step::Chunk(reply(Cmd::GetComboBulk, 6, combo_resp.clone())),
            Step::AwaitWrites(7),
            Step::Chunk(reply(Cmd::SetMorseBulk, 7, ())),
            Step::AwaitWrites(8),
            Step::Chunk(reply(Cmd::GetMorseBulk, 8, morse_resp.clone())),
            Step::Hang,
        ],
    )
    .await;

    drive(&mut driver, &client, async {
        client
            .set_keymap_bulk(SetKeymapBulkRequest {
                layer: 0,
                start_row: 0,
                start_col: 0,
                actions: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(client.get_keymap_bulk(0, 0, 0).await.unwrap(), keymap_resp);

        client
            .set_combo_bulk(SetComboBulkRequest {
                start_index: 0,
                configs: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(client.get_combo_bulk(0).await.unwrap(), combo_resp);

        client
            .set_morse_bulk(SetMorseBulkRequest {
                start_index: 0,
                configs: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(client.get_morse_bulk(0).await.unwrap(), morse_resp);
    })
    .await;
}

#[tokio::test]
async fn read_all_keymap_concatenates_pages() {
    // Ten keys at four per page yields 4, 4, then 2.
    let mut supported = caps();
    supported.bulk_transfer_supported = true;
    supported.max_bulk_keys = 4;
    supported.num_layers = 1;
    supported.num_rows = 1;
    supported.num_cols = 10;

    let page = |base: u8, n: u8| GetKeymapBulkResponse {
        actions: (0..n).map(|i| KeyAction::Morse(base + i)).collect(),
    };
    let expected: Vec<KeyAction> = (0u8..10).map(KeyAction::Morse).collect();

    let (client, mut driver) = connect_session(
        handshake_steps(supported),
        vec![
            Step::AwaitWrites(3),
            Step::Chunk(reply(Cmd::GetKeymapBulk, 3, page(0, 4))),
            Step::AwaitWrites(4),
            Step::Chunk(reply(Cmd::GetKeymapBulk, 4, page(4, 4))),
            Step::AwaitWrites(5),
            Step::Chunk(reply(Cmd::GetKeymapBulk, 5, page(8, 2))),
            Step::AwaitWrites(6),
            Step::Chunk(reply(Cmd::GetWpm, 6, 42u16)),
            Step::Hang,
        ],
    )
    .await;
    drive(&mut driver, &client, async {
        assert_eq!(client.read_all_keymap().await.unwrap(), expected);
        // Trailing reply proves the pager stopped after three fetches.
        assert_eq!(client.get_wpm().await.unwrap(), 42);
    })
    .await;
}

#[tokio::test]
async fn read_all_stops_on_clamped_empty_page() {
    let mut supported = caps();
    supported.bulk_transfer_supported = true;
    supported.max_bulk_keys = 4;
    supported.num_layers = 1;
    supported.num_rows = 1;
    supported.num_cols = 10;

    let full = GetKeymapBulkResponse {
        actions: (0u8..4).map(KeyAction::Morse).collect(),
    };
    let empty = GetKeymapBulkResponse { actions: vec![] };
    let (client, mut driver) = connect_session(
        handshake_steps(supported),
        vec![
            Step::AwaitWrites(3),
            Step::Chunk(reply(Cmd::GetKeymapBulk, 3, full)),
            Step::AwaitWrites(4),
            Step::Chunk(reply(Cmd::GetKeymapBulk, 4, empty)),
            Step::AwaitWrites(5),
            Step::Chunk(reply(Cmd::GetWpm, 5, 7u16)),
            Step::Hang,
        ],
    )
    .await;
    drive(&mut driver, &client, async {
        assert_eq!(client.read_all_keymap().await.unwrap().len(), 4);
        // Trailing reply proves the empty page halted the loop.
        assert_eq!(client.get_wpm().await.unwrap(), 7);
    })
    .await;
}

#[tokio::test]
async fn write_all_keymap_chunks_by_page_size() {
    // Five actions at two per page should send 2, 2, then 1.
    let mut supported = caps();
    supported.bulk_transfer_supported = true;
    supported.max_bulk_keys = 2;

    let (client, mut driver) = connect_session(
        handshake_steps(supported),
        vec![
            Step::AwaitWrites(3),
            Step::Chunk(reply(Cmd::SetKeymapBulk, 3, ())),
            Step::AwaitWrites(4),
            Step::Chunk(reply(Cmd::SetKeymapBulk, 4, ())),
            Step::AwaitWrites(5),
            Step::Chunk(reply(Cmd::SetKeymapBulk, 5, ())),
            Step::AwaitWrites(6),
            Step::Chunk(reply(Cmd::GetWpm, 6, 99u16)),
            Step::Hang,
        ],
    )
    .await;
    drive(&mut driver, &client, async {
        let actions: Vec<KeyAction> = (0u8..5).map(KeyAction::Morse).collect();
        client.write_all_keymap(&actions).await.unwrap();
        assert_eq!(client.get_wpm().await.unwrap(), 99);
    })
    .await;
}

#[tokio::test]
async fn connect_rejects_newer_major() {
    let newer = ProtocolVersion {
        major: ProtocolVersion::CURRENT.major + 1,
        minor: 0,
    };
    let err = MockDevice(vec![
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::GetVersion, 1, newer)),
        Step::Hang,
    ])
    .connect()
    .await
    .err()
    .expect("connect must fail");
    assert!(matches!(err, RynkHostError::VersionMismatch { .. }));
}

#[tokio::test]
async fn connect_accepts_newer_minor() {
    // Minor version is informational within the same major.
    let newer = ProtocolVersion {
        major: ProtocolVersion::CURRENT.major,
        minor: ProtocolVersion::CURRENT.minor + 1,
    };
    MockDevice(vec![
        Step::AwaitWrites(1),
        Step::Chunk(reply(Cmd::GetVersion, 1, newer)),
        Step::AwaitWrites(2),
        Step::Chunk(reply(Cmd::GetCapabilities, 2, caps())),
        Step::Hang,
    ])
    .connect()
    .await
    .expect("same-major newer-minor must connect");
}

#[tokio::test(start_paused = true)]
async fn caller_can_timeout_silent_connect() {
    let err = timeout(Duration::from_millis(10), MockDevice(vec![Step::Hang]).connect()).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn rynk_device_trait_drives_lifecycle() {
    // Generic `RynkDevice` consumers should not name the transport type.
    async fn run_first<D: RynkDevice>(d: D) -> u16 {
        assert_eq!(d.label(), "mock");
        let (client, mut driver) = d.connect().await.unwrap();
        match select(driver.run(&client), client.get_wpm()).await {
            Either::First(err) => panic!("driver died: {err}"),
            Either::Second(wpm) => wpm.unwrap(),
        }
    }

    let mut steps = handshake_steps(caps());
    steps.extend([
        Step::AwaitWrites(3),
        Step::Chunk(reply(Cmd::GetWpm, 3, 7u16)),
        Step::Hang,
    ]);
    assert_eq!(run_first(MockDevice(steps)).await, 7);
}
