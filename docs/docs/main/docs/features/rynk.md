# Rynk

Rynk is RMK's own protocol for configuring a keyboard while it's running — change
your keymap, layers, combos, tap-dance, macros and more without reflashing the
firmware. It plays the same role as [Vial](./vial_support), but it is built
natively for RMK and understands every RMK feature.

::: tip Rynk or Vial — which should I use?
Rynk speaks RMK's full feature set and is enabled by default, but its host tools
are still young: today you use it from a browser demo or a small Rust library —
there is no packaged desktop app yet. [Vial](./vial_support) has a polished
cross-platform app, but only supports the features in the Vial standard.

If you want a click-and-go app today, choose Vial. If you want access to every
RMK feature and don't mind newer tooling, choose Rynk.
:::

## What you can configure

Through Rynk, a host tool can read and change:

- Keymap entries per layer, plus encoders and the default layer
- Combos, forks, tap-dance / morse keys, and macros
- Tap-hold and other behavior settings

It can also watch live status — the current layer, a key tester (matrix state),
typing speed (WPM), the caps-lock/num-lock indicators, battery level, and, on
wireless boards, the connection and BLE profile (including switching or clearing
a profile). Finally it can reboot the keyboard, enter the bootloader, and reset
stored settings.

## Enable Rynk

Rynk and Vial are mutually exclusive — pick one. For `keyboard.toml` projects,
select Rynk in `[host]`:

```toml title="keyboard.toml"
[host]
vial_enabled = false
rynk_enabled = true
```

Then make sure the `rmk` dependency enables the `rynk` feature. It's on by
default, so keeping RMK's default features is enough:

```toml title="Cargo.toml"
rmk = { version = "...", features = ["rp2040"] }
```

If you turn default features off, add the ones you need back explicitly:

```toml title="Cargo.toml"
rmk = { version = "...", default-features = false, features = [
    "defmt",
    "storage",
    "rynk",
    "watchdog",
    "rp2040",
] }
```

::: warning
The `[host]` setting and the Cargo feature must agree. For `keyboard.toml`
projects RMK checks this when it builds: if the `rynk` feature is on,
`rynk_enabled` must be `true` (and `vial_enabled` `false`), otherwise the build
stops with an error. To use Vial instead, flip both settings and swap the `rynk`
feature for `vial`.
:::

::: warning
Keep the `storage` feature enabled (it's on by default) so your changes survive
a reboot. Without it, anything you set over Rynk is lost when the keyboard
restarts. See [Storage](./storage).
:::

## Connecting to your keyboard

Rynk works over the connection you already use:

- **USB** — plug the keyboard in. Host tools find RMK keyboards automatically, so
  you don't have to hunt for the right serial port.
- **Bluetooth** — native tools connect to an already-paired keyboard.
- **Browser** — Chromium browsers (Chrome or Edge) connect over USB or Bluetooth.
  Firefox and Safari are not supported.

Rynk tooling is still young. Today you have two options:

- **A browser demo** — the `rynk-wasm` package ships a reference web page
  (`index.html`) you build and serve locally; follow the steps in its README.
- **Rust libraries** — the `rynk`, `rynk-serial`, and `rynk-ble` crates let you
  build your own tool (see [For tool authors](#for-tool-authors) below).

A ready-made desktop app is not available yet.

## Advanced tuning

Rynk's firmware buffers size themselves automatically and rarely need touching.
If you are tight on RAM or want faster whole-keymap transfers, a few knobs live
in the [`[rmk]`](../configuration/rmk_config#rynk-protocol-configuration) section:
`protocol_max_bulk_size`, `protocol_macro_chunk_size`, and `rynk_buffer_size`.

The optional `bulk` Cargo feature turns on faster bulk transfers at the cost of
extra RAM. Enable it on boards that have room to spare:

```toml title="Cargo.toml"
rmk = { version = "...", features = ["bulk", "rp2040"] }
```

Host tools work with or without `bulk` — the keyboard tells the tool whether it
supports bulk transfers, so the same tool works either way.

## For tool authors

If you're building your own host tool, RMK ships ready-made client crates so you
don't have to implement the protocol yourself:

- `rynk` — the core typed client; talks to a keyboard over any byte link.
- `rynk-serial` — USB discovery and connection.
- `rynk-ble` — native Bluetooth discovery and connection.
- `rynk-wasm` — the browser build, driven from JavaScript over Web Serial / WebHID.

A minimal native USB tool looks like this:

```rust
use rynk::RynkDevice;
use rynk_serial::SerialDevice;

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Find a connected Rynk keyboard and open it (the handshake runs in `connect`).
    let device = SerialDevice::discover()
        .await?
        .into_iter()
        .next()
        .ok_or("no Rynk keyboard found")?;

    let mut client = device.connect().await?;
    let caps = client.capabilities();
    println!("{}x{}x{} keymap", caps.num_layers, caps.num_rows, caps.num_cols);

    let key = client.get_key(0, 0, 0).await?;
    println!("L0(0,0) = {key:?}");
    Ok(())
}
```

Live status arrives as "topics" you poll for:

```rust
while let Ok(topic) = client.next_event().await {
    println!("{topic:?}");
}
```

Topics are best-effort. If you miss one, just read the current value again with
the matching `get_*` method.

Firmware and host share the same message types (via `rmk-types`), so the two ends
can never disagree about a message's format. The client crates handle all the
encoding for you, so you never touch the raw bytes on the wire.
