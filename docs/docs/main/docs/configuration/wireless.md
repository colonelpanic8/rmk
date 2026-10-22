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

### Host advertising backoff

Host BLE connections advertise quickly for a short window, then reduce their
advertising rate while waiting for the computer. Configure the compiled firmware
in `keyboard.toml`:

```toml
[ble]
enabled = true
advertising_fast_interval_ms = 30
advertising_slow_interval_ms = 200
advertising_fast_timeout_secs = 5
```

These are the defaults. Both intervals must be between 20 and 10240 milliseconds,
and the fast interval must not exceed the slow interval. The controller represents
intervals in 0.625 ms units, so whole-millisecond values may be rounded down.
Set `advertising_fast_timeout_secs = 0` to advertise at the slow interval immediately.
Equal intervals also skip the phase transition.

Each new host advertising attempt starts a fresh fast window, including after boot,
disconnect, profile selection, and waking from advertising-timeout sleep. The
existing total advertising timeout includes both phases; backoff does not extend
it. Split and dongle advertisements keep their existing timing. This affects
connection discovery, not the connection interval or key-report rate once connected.

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

Battery configuration controls voltage measurement, charging-state detection, and an optional indicator LED. These features also work with BLE disabled.

Choose the table for the board you are configuring:

| Board            | Table                                                                |
| ---------------- | -------------------------------------------------------------------- |
| Unibody keyboard | `[battery]`                                                          |
| Split central    | `[split.central.battery]`                                            |
| Split peripheral | `[split.peripheral.battery]`, below its `[[split.peripheral]]` entry |

Configure each split board separately. Use only the split battery tables for a split keyboard; the top-level `[battery]` table is not accepted.

### Battery fields

All three tables accept the same fields. Omit inputs and outputs that your board does not have.

| Field                      | Description                                                                                                                                                             | Default                                                  |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| `battery_adc_pin`          | ADC pin connected to the battery voltage divider, or `"vddh"` for the internal VDDH input on nRF52840/nRF52833.                                                         | No voltage measurement                                   |
| `adc_divider_measured`     | Resistance between the ADC input and ground. Requires `battery_adc_pin`.                                                                                                | `1`                                                      |
| `adc_divider_total`        | Total resistance of the voltage divider. Requires `battery_adc_pin`.                                                                                                    | `1`                                                      |
| `charge_state`             | Charging-status input as `{ pin, low_active }`. Set `low_active = true` if the charger drives the pin low while charging.                                               | No charging-status input                                 |
| `charge_led`               | Indicator output as `{ pin, low_active }`. Set `low_active = true` if driving the pin low turns the LED on. Requires an ADC or charging-status input on the same board. | No indicator LED                                         |
| `battery_user_description` | Battery name exposed over BLE. A peripheral requires `battery_adc_pin` to expose its battery service.                                                                   | `"Central"` or `"Peripheral N"`, where `N` starts at `0` |

For unibody keyboards, omitted fields use the board preset; an empty `[battery]` table keeps those defaults.

For split keyboards, an omitted side table uses the board preset. An explicit table replaces it entirely; an empty table disables that side's battery inputs.

Set both divider values in the same units. For example, a divider with 806 kΩ between the battery and ADC input and 2 MΩ between the input and ground uses `2000` and `2806`. Both values must be greater than zero. With `battery_adc_pin = "vddh"`, RMK uses the internal 1:5 divider and ignores these two fields.

On nRF52 chips with an ADC, RMK sets up voltage measurement automatically. The examples below use nRF52840 pins; replace them and the divider values to match your board.

With only `charge_state` configured, RMK reports charging state without a battery percentage. The indicator LED stays on while charging, blinks below 10% when not charging, and stays off otherwise.

### Unibody keyboard

Add `[battery]` to `keyboard.toml`. This example measures voltage through an external divider and reads a charging-status pin:

```toml
[battery]
battery_adc_pin = "P0_05"
adc_divider_measured = 2000
adc_divider_total = 2806
charge_state = { pin = "P0_20", low_active = true }
# Optional indicator LED
charge_led = { pin = "P0_21", low_active = false }
battery_user_description = "Main"
```

### Split battery ADC configuration

Add a battery table to override the preset for that side. Place each `[split.peripheral.battery]` table after the corresponding `[[split.peripheral]]` entry and before the next peripheral entry. Keep each board's existing matrix and connection settings.

```toml
[split.central.battery]
battery_adc_pin = "P0_01"
adc_divider_measured = 2000
adc_divider_total = 2806
battery_user_description = "Left"

[[split.peripheral]]
# Keep this peripheral's existing board settings here.

[split.peripheral.battery]
battery_adc_pin = "P0_02"
adc_divider_measured = 2000
adc_divider_total = 2806
battery_user_description = "Right"
```

For an existing configuration with battery fields under `[ble]` or directly under a split board, see [Migrate battery configuration](../migration/v09_v10#battery-configuration-tables).

### RP2040 battery input

Set `battery_adc_pin` to an ADC-capable pin, such as `PIN_26`, and set the divider values to match your board. RP2040 voltage measurement assumes a 3.3 V ADC reference.

For a unibody board with a 1:2 divider:

```toml
[battery]
battery_adc_pin = "PIN_26"
adc_divider_measured = 1
adc_divider_total = 2
```

For a split board, put these fields in its central or peripheral battery table. RMK waits 30 seconds before each sample, including the first. If a read fails, it keeps the previous reading and retries at the next sampling interval. Boards with a different ADC reference need a custom Rust reader that publishes ADC input millivolts.

### ESP32 battery input

Set `battery_adc_pin` to an ADC1-capable GPIO for your chip and set the divider values to match your board. RMK uses calibrated voltage readings and waits 30 seconds before each sample, including the first.

For an ESP32-C3 unibody board with a 1:2 divider:

```toml
[battery]
battery_adc_pin = "GPIO0"
adc_divider_measured = 1
adc_divider_total = 2
```

For a split board, put these fields in its central or peripheral battery table. GPIO numbering and ADC support vary by chip; the pin in this example is for ESP32-C3.

Battery measurement reserves ADC1. This configuration requires RMK's ESP32 BLE initialization, which enables the radio before using the hardware random-number generator.

### Peripheral battery reporting over BLE GATT

When peripherals are configured to sample their batteries (see above), their levels are forwarded to the central over the split BLE links and re-exposed to the host through standard Battery Service instances (UUID `0x180F`) on the central's GATT server. The host sees one Battery Service instance for:

- the central's own battery level, and
- each peripheral with `battery_adc_pin` set by its own table or the board preset.

Set `battery_user_description` in each board's battery table to give its battery a name such as `"Left"` or `"Right"`. The host determines whether these names and separate battery levels appear in its interface.

The split feature uses trouble-host's default client ATT table size. To reserve more space for client-specific attributes such as CCCDs, set `TROUBLE_HOST_CLIENT_ATT_TABLE_SIZE` in the project environment, for example in `.cargo/config.toml`:

```toml
[env]
TROUBLE_HOST_CLIENT_ATT_TABLE_SIZE = "128"
```

This project-wide override takes precedence over trouble-host Cargo feature settings and can be set to the size required by the enabled services.
