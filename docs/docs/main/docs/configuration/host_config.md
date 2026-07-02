# Host Configuration

The `[host]` section selects the firmware protocol used by host-side tools.
RMK currently supports two mutually exclusive protocols:

- `rynk_enabled`: RMK's native protocol for RMK-aware host tools.
- `vial_enabled`: the Vial/VIA-compatible protocol for the Vial app.

The `keyboard.toml` values must match the `rmk` Cargo features. If
`host.rynk_enabled = true`, enable the `rynk` Cargo feature. If
`host.vial_enabled = true`, enable the `vial` Cargo feature. Do not enable both.

## Configuration Example

```toml
[host]
# Enable Rynk, RMK's native host protocol.
rynk_enabled = true

# Disable Vial when using Rynk. Rynk and Vial are mutually exclusive.
vial_enabled = false

# Unlock keys for Vial security (optional)
# Keys must be pressed simultaneously to unlock Vial configuration.
# This is used only when Vial is enabled.
unlock_keys = [[0, 0], [0, 1]]  # Keys at (row=0,col=0) and (row=0,col=1)

# Start Vial unlocked, bypassing the unlock-key combo (default: false)
# Used only with Vial and the `vial_lock` Cargo feature.
vial_insecure = false
```

## Common Setups

Use Rynk with the `rmk` default Cargo features:

```toml title="keyboard.toml"
[host]
vial_enabled = false
rynk_enabled = true
```

Use Vial instead:

```toml title="keyboard.toml"
[host]
vial_enabled = true
rynk_enabled = false
unlock_keys = [[0, 0], [0, 1]]
```

```toml title="Cargo.toml"
rmk = { version = "...", default-features = false, features = [
    "defmt",
    "storage",
    "vial",
    "watchdog",
    "rp2040",
] }
```

Disable all host configurator support:

```toml title="keyboard.toml"
[host]
vial_enabled = false
rynk_enabled = false
```

```toml title="Cargo.toml"
rmk = { version = "...", default-features = false, features = [
    "defmt",
    "storage",
    "watchdog",
    "rp2040",
] }
```
