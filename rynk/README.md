# rynk

Runtime-free host-side client for **Rynk**, RMK's native host-communication
protocol. Use it to read and write a running RMK keyboard's keymap, combos,
forks, morse, macros, and behavior, and to observe live status.

This crate owns the protocol state machine only. Device discovery, connection,
and byte I/O live in separate transport crates such as `rynk-serial` and
`rynk-ble`. The `rynk-kle` crate converts KLE exports / Vial `vial.json` ↔
RMK's `[layout]` and decodes layouts into `rynk::layout` types — natively and,
via its `wasm` feature, on the web; the `rmkit layout` CLI in
[rmkit](https://github.com/haobogu/rmkit) wraps it.

## Concepts

- **[`Client`]** exposes handshake, typed
  methods for the command surface, and pull-based topic delivery via
  `next_event`, which decodes each push into a typed `IncomingTopic` (topics are
  best-effort — re-read a missed value with the matching `Get*` call).
  Requests are serialized through `&mut self` and sent as owned channel messages.
- **[`Driver`] owns [`Reader`] and [`Writer`]** and is run as one long-lived
  executor task. It correlates responses without borrowing the client's buffers.
- **[`Transport`] splits into embedded-io-async `Read` and `Write` halves** — the same traits the
  firmware's session loop reads, re-exported as `rynk::io` so the trait version
  always matches. A third-party transport is its own crate implementing them
  and returns them from `Transport::split`.

## Example

```rust,no_run
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
// Discover marked Rynk keyboards, pick one, and open it. `rynk-ble` mirrors this flow.
use rynk::RynkDevice;
use rynk_serial::SerialDevice;
let device = SerialDevice::discover()
    .await?
    .into_iter()
    .next()
    .ok_or("no Rynk keyboard found")?;
let (mut client, mut driver) = device.connect().await?;
let driver_task = tokio::spawn(async move { driver.run().await });
client.handshake().await?;

let caps = client.get_capabilities().await?;
println!("{}×{}×{} keymap", caps.num_layers, caps.num_rows, caps.num_cols);

let key = client.get_key(0, 0, 0).await?;
println!("L0(0,0) = {key:?}");
drop(client);
driver_task.await??;
# Ok(()) }
```

Each method returns the response value directly; a device rejection surfaces as
`RynkHostError::Rejected`, so `?` propagates both transport and firmware errors.

## `no_std` and `no_alloc`

The default `std` feature provides discovery, owned desktop sessions, layout
decompression, and collection helpers. The protocol core builds without either
`std` or an allocator:

```console
cargo check -p rynk --no-default-features --target thumbv6m-none-eabi
```

In that mode, the application owns a fixed-capacity session and keeps it alive
for the client and driver:

```rust,ignore
let session = rynk::Session::<512, 8>::new();
let (writer, reader) = rynk::Transport::split(transport);
let (mut client, mut driver) = rynk::Driver::new(&session, reader, writer);
let client_task = async move { client.handshake().await };
let (driver_result, handshake_result) =
    embassy_futures::join::join(driver.run(), client_task).await;
driver_result?;
handshake_result?;
```

The first const parameter bounds a complete frame. The second is the queued
topic count; each raw topic retains at most `TOPIC_PAYLOAD_SIZE` bytes. Oversized
frames fail explicitly, while an oversized best-effort topic is counted as
dropped.

## License

MIT OR Apache-2.0
