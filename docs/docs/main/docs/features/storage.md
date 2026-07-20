# Storage

RMK's storage system provides persistent flash memory for storing data like keyboard configurations and BLE bonding information.

## Storage Feature

RMK's storage system is enabled by the `storage` feature, which is part of the default feature set. Enabling BLE automatically pulls in `storage`, since BLE bonding data must be persisted to non-volatile storage. The host configurator protocols (`rynk` and `vial`) rely on `storage` to persist keymap edits across reboots but do not enable it themselves, so keep it enabled when you use them.

## Storage Configuration

By default, RMK saves data to your microcontroller's internal flash memory.

- For users configuring with `keyboard.toml`, the default storage space details are located in the `rmk-config/src/default_config` folder. If your microcontroller's configuration isn't found there, RMK defaults to using the **last two flash sections** of your microcontroller's internal flash memory.

- For Rust API users, you can configure storage via the `RmkConfig.storage_config` field, which accepts a `StorageConfig` struct.

::: warning
Ensure you allocate sufficient storage space for your keymap and bonding information. 32 KiB is generally adequate for most keyboards.
:::

## Sharing nRF flash with application code

On an nRF BLE keyboard, enable the `shared_flash` Cargo feature when application
code needs persistent flash in addition to RMK's storage:

```toml
rmk = { version = "...", features = ["nrf52840_ble", "shared_flash"] }
```

The generated firmware then serializes RMK storage and application operations
through the radio-safe flash driver. Acquire the unique client with a reserved,
non-overlapping, erase-page-aligned half-open address range, then keep that
client in the one application task that owns the partition:

```rust
use rmk::shared_flash::take;

let mut flash = take(0x0F_0000..0x0F_2000).await?;
flash.write(0x0F_0000, &[1, 2, 3, 4]).await?;

let mut value = [0; 4];
flash.read(0x0F_0000, &mut value).await?;
```

Initialization validates the immutable window against flash capacity and erase
alignment. Read/write alignment and every operation boundary are validated
before the driver is called. Given a correct partition, operations are
contained within it; RMK cannot determine whether the partition overlaps the
firmware, bootloader, or RMK's configured storage, so the application must
supply that partition correctly.

For `keyboard.toml` builds, the service is generated only for nRF52 BLE with
storage enabled. Enabling `shared_flash` with `dfu_nrf`, disabled TOML storage,
or a non-nRF chip produces a compile-time diagnostic instead of an API that can
wait forever. `shared_flash` is the feature that exposes the module; enabling
`storage` alone does not.

Pure-Rust initialization does not generate or spawn a service. It must put the
radio-safe driver in `shared_flash::FlashMutex`, measure its capacity with
`flash_capacity`, spawn exactly one `shared_flash::service` task, and pass a
`shared_flash::StorageFlash` adapter to RMK storage before calling `take`.
Those low-level names are hidden from the API index because they are integration
glue, but remain public for explicit initialization. The application service
handles one chunk or erase page per mutex acquisition, which permits RMK
storage to interleave work but does not guarantee scheduling fairness.
