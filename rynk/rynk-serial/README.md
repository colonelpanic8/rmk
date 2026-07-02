# rynk-serial

USB CDC-ACM serial transport for the `rynk` host protocol client, built on the
`tokio-serial` crate. Pair it with `rynk` to talk to a wired RMK keyboard over
USB.

## How discovery works

RMK's `rynk` firmware prepends the `RYNK_SERIAL_MAGIC` marker to its USB serial
number, whatever VID/PID or serial string you configure. The OS caches that
serial string at enumeration, so `SerialDevice::discover` finds every Rynk
keyboard on Windows, macOS, and Linux *without opening a port*. That matters:
opening a CDC port toggles DTR and resets some MCUs, so only the device you pick
is opened, exactly once.

The marker plays the role the service UUID plays for `rynk-ble` — it identifies a
Rynk keyboard before any link is opened. `connect` then opens the port and runs
the Rynk handshake, the authoritative confirmation.

## Example

```rust
use rynk::RynkDevice;
use rynk_serial::SerialDevice;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Discover marked keyboards, pick one, and connect (the handshake runs in
    // `connect`).
    let device = SerialDevice::discover()
        .await?
        .into_iter()
        .next()
        .ok_or("no Rynk keyboard found")?;
    let mut client = device.connect().await?;

    let layer = client.get_current_layer().await?;
    println!("active layer: {layer}");
    Ok(())
}
```

Dropping the transport (with its `Client`) ends the Rynk session only; the
keyboard stays connected and usable.

## License

MIT OR Apache-2.0
