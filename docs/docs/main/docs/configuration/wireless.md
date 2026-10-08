# Wireless/Bluetooth

### `[ble]`

To enable BLE, add `enabled = true` under the `[ble]` section.

```toml
# Ble configuration
# To use the default configuration, ignore this section completely
[ble]
# Whether to enable BLE feature
enabled = true
# Set the BLE tx power; higher means better signal but more power consumption. For nRF52840 the maximum tx power is 8.
# nRF52 only, ignored on other chips
default_tx_power = 0
# Host connection PHY: true = 2M (default), false = 1M.
use_2m_phy = true
# Enable or disable passkey entry, defaults to false
passkey_entry = false
# Timeout in seconds for passkey entry, defaults to 120
passkey_entry_timeout = 120
```

### Host connection PHY

`[ble].use_2m_phy` requests 2M (`true`, default) or 1M (`false`) for host connections on all supported BLE chips.

To request 1M:

```toml
[ble]
enabled = true
use_2m_phy = false
```

The `use_1m_phy` Cargo feature overrides this setting to request 1M. Dongle and split links always request 2M.

### Passkey entry

RMK supports typing a BLE passkey directly on the keyboard during pairing. This is disabled by default, and requires the `passkey_entry` Cargo feature of the `rmk` crate in addition to the configuration below.

```toml
[ble]
# Enable or disable passkey entry (default: false)
# When disabled, passkey pairing requests from the host are automatically rejected.
passkey_entry = true
# Timeout in seconds for passkey entry (default: 120, minimum: 30)
# If the user does not finish entering the passkey within this time, pairing is cancelled.
# Setting this below 30 will cause a build error.
passkey_entry_timeout = 120
```

During passkey mode, the keyboard intercepts all keypresses. Only the following keys are recognized:

| Key                         | Action                     |
| --------------------------- | -------------------------- |
| `0`–`9` (top row or numpad) | Enter a digit              |
| `Enter` / `Numpad Enter`    | Submit the 6-digit passkey |
| `Escape`                    | Cancel pairing             |
| `Backspace`                 | Delete the last digit      |

All other keys are silently discarded while passkey mode is active.

## Battery configuration

Configure voltage measurement, charging detection, and an optional indicator LED in these tables. Battery monitoring also works with BLE disabled; ESP32 ADC setup has an additional requirement below.

| Board            | Table                                                                |
| ---------------- | -------------------------------------------------------------------- |
| Unibody          | `[battery]`                                                          |
| Split central    | `[split.central.battery]`                                            |
| Split peripheral | `[split.peripheral.battery]`, after its `[[split.peripheral]]` entry |

The board selected by `[keyboard].board` supplies any battery preset, such as nice!nano's ADC wiring:

- **Unibody:** Omitted fields use the preset; an empty table keeps it.
- **Split:** An omitted side table uses the preset. An explicit table replaces it entirely; an empty table disables that side's battery inputs. Top-level `[battery]` is not accepted.

### Battery fields

| Field                      | Description                                                                    | Default without a preset        |
| -------------------------- | ------------------------------------------------------------------------------ | ------------------------------- |
| `battery_adc_pin`          | ADC pin, or `"vddh"` on nRF52840/nRF52833.                                     | No voltage measurement          |
| `adc_divider_measured`     | Divider resistance from ADC input to ground.                                   | `1`                             |
| `adc_divider_total`        | Total divider resistance.                                                      | `1`                             |
| `charge_state`             | Charging input: `{ pin, low_active }`.                                         | None                            |
| `charge_led`               | Indicator output: `{ pin, low_active }`. Requires a voltage or charging input. | None                            |
| `battery_user_description` | Battery name exposed over BLE; peripherals require an ADC input.               | `"Central"` or `"Peripheral N"` |

Divider fields require `battery_adc_pin` and positive values in the same units: 2 MΩ to ground plus 806 kΩ to the battery gives `2000` / `2806`. `"vddh"` uses a fixed 1:5 divider and ignores these fields.

Set `low_active = true` for signals active at low voltage. With only `charge_state`, RMK reports charging without a percentage. The LED stays on while charging, blinks below 10%, and stays off otherwise.

### Unibody keyboard

Example for nRF52840; adjust pins and divider values for your board:

```toml
[battery]
battery_adc_pin = "P0_05"
adc_divider_measured = 2000
adc_divider_total = 2806
charge_state = { pin = "P0_20", low_active = true }
charge_led = { pin = "P0_21", low_active = false }
battery_user_description = "Main"
```

### Split battery ADC configuration

Omit side tables to use the preset, or use the fields above to override each side independently. Place a peripheral's table after its existing `[[split.peripheral]]` entry and before the next one. For an nRF52840 peripheral:

```toml
[split.peripheral.battery]
battery_adc_pin = "vddh"
battery_user_description = "Right"
```

For old `[ble]` or flat split battery fields, see [Migrate battery configuration](../migration/v09_v10#battery-configuration-tables).

### RP2040 battery input

Use an ADC pin such as `PIN_26`. Conversion assumes a 3.3 V reference; other references require a custom Rust reader that publishes ADC input millivolts.

Sampling starts after 30 seconds and repeats every 30 seconds. Failed reads keep the previous measurement until a later sample succeeds.

### ESP32 battery input

Use an ADC1-capable GPIO, such as `GPIO0` on ESP32-C3. Readings are calibrated. Sampling starts after 30 seconds and repeats every 30 seconds.

This reserves ADC1 and requires RMK's ESP32 BLE initialization. Choose pins and divider values for your chip and board.

### Peripheral battery reporting over BLE GATT

Over split BLE, the central exposes a standard Battery Service (`0x180F`) for itself and each peripheral with an ADC input, including inputs from presets. `battery_user_description` sets each battery's name; the host decides whether to display separate names and levels.

RMK uses trouble-host's default client ATT table size. To increase it, set this project-wide override in `.cargo/config.toml`; it takes precedence over trouble-host Cargo features:

```toml
[env]
TROUBLE_HOST_CLIENT_ATT_TABLE_SIZE = "128"
```
