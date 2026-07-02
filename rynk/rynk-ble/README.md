# rynk-ble

Native Bluetooth Low Energy (GATT) transport for the `rynk` host protocol
client, built on the `bluest` crate. Pair it with `rynk` to talk to a wireless
RMK keyboard over BLE.

## How discovery works

A Rynk keyboard exposes a custom GATT service identified by `RYNK_SERVICE_UUID`,
the BLE counterpart to the serial transport's serial marker and independent of
the user-settable BLE name. `BleDevice::discover` lists the *already-connected*
system devices that expose this service — no scan and no pairing prompt, because
Rynk rides the link the OS already bonded. `connect` then attaches, discovers the
Rynk characteristics, subscribes, and runs the handshake.

Bluetooth permission is required. A denied or powered-off adapter waits for the
adapter to become available rather than returning an error.

## Example

```rust
use rynk::RynkDevice;
use rynk_ble::BleDevice;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // List already-connected Rynk keyboards, pick one, and connect.
    let device = BleDevice::discover()
        .await?
        .into_iter()
        .next()
        .ok_or("no Rynk keyboard found")?;
    let mut client = device.connect().await?;

    let battery = client.get_battery_status().await?;
    println!("battery: {battery:?}");
    Ok(())
}
```

## License

MIT OR Apache-2.0
