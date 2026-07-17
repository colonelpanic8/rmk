# Concurrent host architecture

Status: **Proposed**

This document defines the next host-side Rynk architecture. It separates the
application-facing client from the transport driver so requests and topic
delivery can make progress concurrently.

The proposal changes only the host implementation and APIs. It does not require
a firmware or wire-protocol change.

## Problem

The current `Client<T>` owns a duplex `Read + Write` transport. Both requests
and `next_event()` borrow the client mutably and read frames directly from that
transport.

This creates two related limitations:

- A parked `next_event()` holds `&mut Client`, so the application cannot issue a
  request until a topic arrives.
- A request can collect topics that arrive before its response, but this only
  supports sequential use. It does not provide an independently running topic
  receiver.

Cancellation is coupled to the same ownership model. Cancelling a read or write
future can abandon transport state, so the current client deliberately marks the
connection unusable after a cancelled wire operation.

## Goals

- Allow `request()` and `next_event()` to wait concurrently.
- Keep request and topic methods on one application-facing `Client`.
- Make one `Driver` the exclusive owner of transport I/O and frame parsing.
- Keep the core crate independent of Tokio, Embassy, or any other executor.
- Preserve one request at a time on the wire while allowing callers to enqueue
  requests concurrently.
- Make cancellation of application futures safe for the byte stream.
- Preserve bounded, best-effort topic delivery and observable overflow.
- Give every pending operation a deterministic result when the session ends.

## Non-goals

- Multiple in-flight wire requests.
- Multiple independent topic subscribers.
- A callback-based topic API.
- A built-in timer or request timeout in the runtime-free core.
- Automatic reconnect or retry.
- Changes to command IDs, sequence numbers, frame layout, or topic encoding.

## Public model

The public session consists of two objects:

- `Client` is a cloneable, transport-independent application handle. It owns
  request submission and the single logical topic consumer.
- `Driver<R, W>` owns the read and write halves of the transport. It performs
  all wire I/O and must be run for the session to make progress.

There is no separate public `Topics` or `TopicListener` object.

```rust
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

#[must_use = "Driver must be run for the Rynk session to make progress"]
pub struct Driver<R, W> {
    reader: R,
    writer: W,
    inner: Arc<DriverInner>,
}

pub async fn connect<R, W>(
    reader: R,
    writer: W,
) -> Result<(Client, Driver<R, W>), RynkHostError>
where
    R: Read,
    W: Write;
```

The exact private field layout may change during implementation. The ownership
boundary and public behavior are the design contract.

### Client API

```rust
impl Client {
    pub async fn request<E: Endpoint>(
        &self,
        request: &E::Request,
    ) -> Result<E::Response, RynkHostError>;

    pub async fn next_event(&self) -> Result<IncomingTopic, RynkHostError>;

    pub async fn send_no_reply<E: Endpoint>(
        &self,
        request: &E::Request,
    ) -> Result<(), RynkHostError>;

    pub fn capabilities(&self) -> &DeviceCapabilities;

    pub fn events_dropped(&self) -> u64;

    pub fn is_alive(&self) -> bool;

    pub fn close(&self);
}
```

Requests and topic delivery use `&self`, so separate futures can borrow the same
client concurrently. Clones refer to the same session, request queue, and topic
queue.

`close()` is idempotent. It signals the driver to stop and wakes operations that
are waiting on the session.

### Driver API

```rust
impl<R, W> Driver<R, W>
where
    R: Read,
    W: Write,
{
    pub async fn run(self) -> Result<(), RynkHostError>;
}
```

`run()` consumes the driver, which prevents two driver loops from owning one
session. Dropping the driver, including cancelling the `run()` future, closes the
session and wakes the client.

The core does not spawn `run()`. Each integration decides how to drive it:

```rust
let (client, driver) = device.connect().await?;

tokio::select! {
    result = driver.run() => result?,
    result = application(client) => result?,
}
```

## Full-duplex transport contract

The driver requires independently owned read and write halves. Merely moving a
single `T: Read + Write` behind a mutex does not solve the problem: a read that
holds the mutex while waiting for bytes would still prevent every write.

`RynkDevice` therefore exposes separate associated types:

```rust
pub trait RynkDevice: Sized {
    type Reader: Read;
    type Writer: Write;

    fn label(&self) -> String;

    async fn open(
        self,
    ) -> Result<(Self::Reader, Self::Writer), RynkHostError>;

    async fn connect(
        self,
    ) -> Result<
        (Client, Driver<Self::Reader, Self::Writer>),
        RynkHostError,
    >;
}
```

Transport implementations split at their natural full-duplex boundary:

- Serial and TCP use their runtime's owned read and write halves.
- BLE uses the notification stream as the reader and the output characteristic
  as the writer.
- Web transports keep `recv()` state in the reader and a cloned JavaScript link
  handle in the writer. Both halves share one link-lifetime guard so the link is
  closed exactly once.

Read and write errors may have different concrete types. The driver maps both to
`RynkHostError` at the boundary.

## Connection lifecycle

The connection uses eager initialization:

```text
Opening -> Handshaking -> Connected -> Running -> Closed
                 |                         |
                 +-------------------------+
                         failure/drop
```

`connect()` constructs the client and driver internally, then calls a private
driver initialization path:

1. Send `GetVersion` and validate the protocol major version.
2. Send `GetCapabilities` and cache the result in the client.
3. Resize the driver's transmit scratch to the negotiated frame limit.
4. Queue any topics received between the handshake requests and responses.
5. Return `(Client, Driver)` only after initialization succeeds.

This keeps transport ownership in the driver without exposing a client that is
still waiting to become ready. A failed or cancelled handshake drops both
transport halves and returns no client.

A client request submitted after `connect()` but before `Driver::run()` is
polled may enter the bounded request queue. It completes once the driver starts.
The `must_use` annotation and examples make the required driver lifecycle
explicit. Dropping an unstarted driver closes the queue instead of leaving the
request pending forever.

## Driver structure

The running driver has two logical loops and one supervisor:

```text
                         Client
                 request()     next_event()
                     |              ^
                     v              |
              request queue     topic queue
                     |              ^
                     v              |
                Tx runner       Rx runner
                     |              ^
                     +-- firmware --+
```

- The receive loop continuously reads, reassembles, and classifies frames.
- The transmit loop serializes wire transactions from a bounded request queue.
- The supervisor closes the complete session when either loop exits or the
  client requests shutdown.

The supervisor waits for the first terminal condition rather than joining both
loops. If the receive loop reaches EOF, waiting for the transmit loop to finish
could otherwise deadlock on a response that can no longer arrive.

```rust
select! {
    result = rx_runner.run() => result,
    result = tx_runner.run() => result,
    _ = close_signal.wait() => Ok(()),
}
```

## Request path

The client serializes the typed request before enqueueing a type-erased
envelope:

```rust
struct RequestEnvelope {
    cmd: Cmd,
    payload: Vec<u8>,
    response_mode: ResponseMode,
    completion: OneshotSender<Result<ResponseFrame, RynkHostError>>,
}

enum ResponseMode {
    Required,
    None,
}
```

The envelope contains no borrowed data or endpoint-specific response type. The
client decodes the returned payload after the driver completes the transaction.

The transmit loop processes envelopes in FIFO order:

1. Skip an envelope if its caller was cancelled before transmission.
2. Allocate the next nonzero sequence number.
3. Register the expected sequence number and command in the pending slot.
4. Write the complete request frame.
5. Await the receive loop's matching response.
6. Forward the response to the caller if the caller still exists.
7. Clear the transaction before reading the next envelope.

The driver supports concurrent callers by queueing them, but it deliberately
keeps exactly one request in flight on the wire. Pipelining can be added later
only if it provides a measured benefit and its ordering and cancellation
semantics are specified separately.

### Pending response routing

The driver has at most one pending response:

```rust
struct PendingResponse {
    seq: u8,
    cmd: Cmd,
    completion: OneshotSender<Vec<u8>>,
}
```

The transmit loop installs this slot before starting the first write. A device
may produce a response before an acknowledged write future returns, so
installing it afterward would create a race.

When the receive loop gets a response:

- A matching sequence number and command complete the pending transaction.
- A response with no pending transaction is a session-level protocol error.
- A sequence or command mismatch is a session-level protocol error.

The new driver does not silently discard unexplained responses. With one wire
transaction in flight, such a response means the host and firmware disagree
about session state.

## Cancellation semantics

Application cancellation does not cancel an in-progress wire operation.

| Cancellation point | Driver behavior |
| --- | --- |
| Before enqueue completes | No request is created. |
| Queued but not sent | The transmit loop observes the dropped completion receiver and skips it. |
| After transmission starts | The driver continues the transaction and drains the matching response. |
| While decoding the completed response | The frame is already drained; only caller work is cancelled. |

After a sent request is cancelled, the transmit loop remains responsible for
the response even though forwarding it to the caller will fail. This prevents a
late response from being consumed by the next request.

Cancelling `next_event()` only cancels a queue receive. It never cancels the
transport read owned by the driver.

## Timeouts

The runtime-free core has no clock and does not choose a universal request
deadline. An application may wrap `request()` in its runtime's timeout.

If that timeout expires, the request future is cancelled but the driver keeps
draining the wire transaction. The application has two choices:

- Keep the session open and allow a late response to restore request progress.
- Call `client.close()` when its timeout policy considers the session unusable.

Without an explicit close, a peer that never responds blocks later requests but
does not block topic reception.

## Topic path

Topics remain a single-consumer, best-effort stream exposed directly through
the client.

- The queue exists before the handshake so topics interleaved with handshake
  responses are retained.
- The receive loop never waits for queue capacity.
- The queue capacity remains 64 by default.
- On overflow, the driver removes the oldest topic, adds the new topic, and
  increments `events_dropped`.
- Unknown topic commands become `IncomingTopic::Unknown` and do not close the
  session.
- Critical state remains recoverable through the corresponding `Get*` endpoint.

All client clones share the same queue. At most one `next_event()` call may be
pending at a time. A second concurrent call returns `AlreadyListening` instead
of silently creating competing consumers. An RAII guard releases this right if
the first future is cancelled.

This proposal does not add a public listener or subscription abstraction. If a
future application requires independent subscribers, that feature should define
filtering, replay, lag, and per-subscriber capacity before adding
`subscribe_topics()`.

## Backpressure

Every unbounded producer/consumer edge has an explicit limit:

| Resource | Initial limit | Full behavior |
| --- | ---: | --- |
| Request queue | 8 | `request()` waits for capacity; cancellation before enqueue is safe. |
| Wire transactions | 1 | Later requests remain queued. |
| Pending response | 1 | A second response is a protocol error. |
| Topic queue | 64 | Drop the oldest topic and increment `events_dropped`. |

The exact request capacity may become configuration, but it must remain bounded.
Topic publication must use a nonblocking operation so a slow topic consumer
cannot delay response routing.

## Error scope

Errors are classified by whether frame synchronization remains trustworthy.

### Request-level errors

These fail one request while leaving the session available:

- Local serialization failure.
- An outbound payload larger than the negotiated maximum.
- A firmware `RynkError` response.
- Response payload deserialization failure.
- Trailing bytes inside an otherwise complete response frame.

### Session-level errors

These close the driver and fail every pending operation:

- Read EOF or transport I/O failure.
- Write failure or an incomplete frame write.
- Invalid frame structure that loses byte-stream synchronization.
- A response with no pending request.
- A response with an unexpected sequence number or command.
- Explicit `Client::close()` or dropping the driver.

The shared session stores a cloneable `CloseReason`. `Driver::run()` returns the
detailed originating error, while client operations awakened by shutdown map
the close reason to a stable `RynkHostError`.

## Shutdown guarantees

Every path out of `Driver::run()` executes one idempotent close operation:

1. Transition the session to `Closed` and store the first close reason.
2. Close the request queue so new requests fail immediately.
3. Fail the in-flight and queued request completions.
4. Close the topic queue and wake `next_event()`.
5. Drop both transport halves.

A guard owned by `run()` performs this operation when the future is cancelled.
Dropping a driver that was never run performs the same transition. No client
operation may remain pending after its driver is gone.

## Integration changes

### Serial

Split the opened `SerialStream` into owned read and write halves and adapt each
half to the matching `embedded-io-async` trait. The serial device returns those
halves from `open()`.

### BLE

Move the notification stream and its partial-chunk buffer into the reader. Move
the output characteristic and write-chunk limit into the writer. Keep any handle
required for connection lifetime in shared transport state.

### WebAssembly

The Wasm wrapper starts the driver internally because JavaScript should only see
`RynkClient`:

```rust
let (client, driver) = device.connect().await?;

spawn_local(async move {
    let _ = driver.run().await;
});

Ok(RynkClient { client, label })
```

Exported request methods and `next_event()` borrow `&self`. JavaScript may then
keep one topic pump pending while issuing firmware requests:

```javascript
const topics = (async () => {
  for (;;) console.log(await client.next_event());
})();

const keymap = client.get_keymap_bulk(0, 0, 0);
await Promise.all([topics, keymap]);
```

Dropping the last client closes the request side. The driver then exits and the
Web link lifetime guard closes the underlying JavaScript link.

## Required invariants

The implementation must preserve these invariants:

1. Only the driver touches the transport halves.
2. The receive loop remains active while any request waits for a response.
3. At most one request is in flight on the wire.
4. Every non-topic response belongs to the current pending transaction.
5. Cancelling a caller after transmission cannot leave a response for the next
   request.
6. Topic backpressure cannot block response routing.
7. Driver termination wakes every client waiter.
8. A successful `connect()` returns negotiated capabilities, never a partially
   initialized client.

## Validation plan

Core tests must cover:

- A request completes while `next_event()` is already pending.
- A topic is delivered while a request waits for its response.
- Topic bursts do not delay response completion.
- Concurrent request callers complete in queue order.
- Cancelling before transmission produces no write.
- Cancelling after transmission drains the response and the next request works.
- Cancelling `next_event()` leaves requests and later topic reads usable.
- Topic overflow drops the oldest item and increments the counter.
- EOF wakes both an in-flight request and `next_event()`.
- Unexpected sequence numbers and commands close the session.
- Dropping or cancelling the driver wakes all client operations.
- Topics interleaved with the handshake remain available after `connect()`.
- No-reply commands complete after their frame is written.

Transport integration tests must additionally cover split serial I/O, BLE topic
delivery during a request, and concurrent Wasm method calls.

The existing test that queues a topic during a request remains useful, but it
does not prove the original problem is fixed: the new suite must park
`next_event()` before issuing the request from a separate future.

## Migration plan

1. Add runtime-independent bounded request, topic, completion, and shutdown
   primitives to the core crate.
2. Extract frame decoding and eager handshake logic from the current client into
   the driver.
3. Change typed endpoint methods to use the transport-free `Client` and `&self`.
4. Change `RynkDevice` and every transport to return read and write halves.
5. Update native examples to run the driver with structured concurrency.
6. Update the Wasm wrapper to spawn the driver and permit concurrent methods.
7. Replace cancellation-poison tests with driver-lifecycle and transaction-drain
   tests.
8. Update crate documentation after the implementation matches this proposal.

## Alternatives considered

### Keep transport ownership in Client

This retains the mutable-borrow conflict. A mutex around a single duplex
transport also fails because a pending read can hold the mutex indefinitely.

### Return Client, Topics, and Driver

This makes topic delivery look like a separate subsystem and complicates
ownership for native and Wasm callers. Rynk currently has one logical event
stream, so `Client::next_event()` is sufficient.

### Add a public topic subscription API now

Subscriptions require decisions about replay, filters, subscriber limits, and
lag reporting that the current use cases do not need. The internal driver can be
extended later without committing to that API now.

### Initialize lazily inside Driver::run

This requires client methods to wait for a ready state and exposes an object
that is connected at the transport layer but not negotiated at the protocol
layer. Eager initialization preserves the current meaning of a successful
`connect()` and has fewer externally visible states.

### Let request futures read their own responses

This recreates the ownership conflict and makes post-send cancellation unsafe.
The driver must own transaction completion independently of the caller future.

## Relationship to other host designs

Long-running host stacks commonly separate an exclusive I/O runner from cheap
application handles and continuously demultiplex inbound traffic. That pattern
supports the ownership boundary chosen here.

This proposal does not copy another host stack's object hierarchy, task layout,
notification subscriptions, or initialization model. Its public API and
single-consumer topic semantics follow Rynk's existing requirements.
