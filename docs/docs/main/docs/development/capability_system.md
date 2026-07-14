# The capability system

This page is for RMK developers. It explains how `keyboard.toml`, Cargo features and the `rmk_*`
cfgs fit together, and what to touch when you add a new capability or a new dependency-gating
feature.

## Why it exists

RMK has two configuration surfaces: `keyboard.toml` (what `#[rmk_keyboard]` users edit) and the
`rmk` crate's Cargo features (what pure Rust API users edit). Historically both could activate the
same functionality, so they could silently disagree — the macro generated code for one world while
the library compiled another, and users got inscrutable type errors.

The capability system makes one place authoritative:

```
(keyboard.toml, active cargo features)
        │
        ▼
rmk_config::resolved::Capabilities::resolve()      ← the single authority
        │
        ├── rmk/build.rs        → emits rmk_* rustc cfgs + validates features (fix-it errors)
        ├── rmk-types/build.rs  → emits the same cfgs (forwarded resolution) + sizes constants
        ├── rmk-macro           → Capabilities::from_toml() drives codegen decisions
        └── KeyboardTomlConfig::firmware_features() → feature lists for project generators
```

Resolution semantics:

- **With a keyboard toml** (it has a `[keyboard]` section): the toml is authoritative. Features are
  validated against it — a capability that needs a dependency-gating feature which is missing, or a
  feature asserting something the toml disables, fails the build with an error that names the fix.
- **Without one** (pure Rust API, tests, docs.rs): features activate capabilities directly, exactly
  as cargo resolved them.
- A few toml-less inputs stay purely additive (`bulk`, `host_lock`, `_usb_high_speed`), and
  `Option` fields like `[keyboard].steno` only become authoritative when set.

Because the macro reads the same resolution (`Capabilities::from_toml`) that the build script
emitted as cfgs, generated code and library gates cannot disagree by construction. The macro does
not re-validate features: `rmk`'s build script runs before the user's crate compiles, so by
expansion time the gate has already passed.

## cfg vs feature: the one rule

**Capability code gates on `rmk_*` cfgs. Only dependency selection gates on features.**

- `#[cfg(rmk_split)]`, `#[cfg(rmk_ble)]`, `#[cfg(rmk_storage)]`, … — anything a keyboard _has_.
  These are emitted by the build scripts from the resolution, so both activation paths (toml or
  feature) flow through one gate. Never write `#[cfg(feature = "split")]` for these; the feature
  still exists, but only as a resolution input.
- `#[cfg(feature = "_nrf_ble")]`, `#[cfg(feature = "ssd1306")]`, `#[cfg(feature = "defmt")]`, … —
  anything that selects _dependencies_ (HAL family, driver crate, logging backend). Cargo resolves
  the dependency graph before any build script runs, so these must stay features. The resolution
  guarantees a `rmk_*` cfg is only enabled when its dependency feature is present.

The full cfg list lives in `Capabilities::flags()` (`rmk-config/src/resolved/capabilities.rs`).
Both build scripts declare every cfg on every build (`CfgSet::set` emits `rustc-check-cfg`), so a
typo in a cfg name is a compiler warning, never a silently-false gate.

## Adding a new capability

Say you're adding a `foo` capability that keyboards can have or not:

1. **Toml field.** Add it to the right section in `rmk-config/src/lib.rs` (every struct is
   `deny_unknown_fields` — the field must be declared). Global compile-level switches live in
   `[keyboard]`; domain-specific ones live in their section. Use `Option<bool>` if a feature should
   still be able to activate it when the toml is silent.
2. **Resolution.** In `rmk-config/src/resolved/capabilities.rs`:
   - add the field to `Capabilities` and to `flags()` (this defines the `rmk_foo` cfg name);
   - read the toml value in `toml_caps()`;
   - map the feature in `from_features()`;
   - decide the merge rule in `resolve_with_toml()`: authoritative (add to the contradiction
     table so `feature on + toml off` errors) or additive (union). Capabilities the **macro**
     shapes must be authoritative — the macro only sees the toml, so a feature-only activation
     would desynchronize generated code from the library.
   - if it needs a dependency-gating feature, add the requires-check with an error message that
     names the exact feature to add.
3. **Alias feature.** Add `foo = []` to `rmk/Cargo.toml` `[features]` so pure Rust API users can
   activate it. If `rmk-types` also needs the cfg, keep the `rmk-types/foo` forward.
4. **Gate the code** with `#[cfg(rmk_foo)]`.
5. **Macro codegen** (if any) branches on `caps.foo` — plain `if`, no `#[cfg]` in the macro and no
   `#[cfg]` emitted into user code.
6. **Event subscribers.** If the capability's tasks subscribe to events, add an entry to
   `rmk-config/src/default_config/subscriber_default.toml` (matched against capability names, not
   feature names).
7. **Tests.** Unit tests in `capabilities.rs` for the semantics (activation from toml, from the
   feature, and every new error path). The `use_config_example_features_are_consistent` test in
   `rmk-config/tests/keyboard_toml_validation.rs` guards the bundled examples.
8. **Docs.** `configuration/appendix.md` for the field, plus the feature page if there is one.

## Adding a dependency-gating feature

For a new HAL family, driver crate or protocol stack: add the feature with its `dep:`/forward
edges as usual, keep the code it gates on `#[cfg(feature = ...)]`, and teach the resolution to
_require_ it when the toml asks for the functionality (a requires-check in `resolve_with_toml()`
plus a `firmware_features()` entry so generators emit it). See how the display driver families
(`ssd1306`/`oled_async`) and the chip BLE aliases are handled.

## Odds and ends

- **`resolve_forwarded()`** — `rmk-types/build.rs` uses this variant because it only sees the
  features `rmk` forwards to it; running the full missing-feature validation there would misfire.
  Full validation belongs to `rmk/build.rs` alone.
- **rust-analyzer** — both build scripts detect `RUSTC_WRAPPER` containing `rust-analyzer` and
  degrade resolution errors to `cargo:warning`s with a features-only fallback, so the IDE keeps
  working while `keyboard.toml` is mid-edit.
- **Chip facts** — has-USB comes from `usb_interrupt_map.rs`, high-speed USB from
  `chip::usb_high_speed()`, chip defaults from `rmk-config/src/default_config/<chip>.toml`. New
  chips need all three (see the nRF54 entries for a chip whose `#[rmk_keyboard]` codegen is still
  pending — the resolution layer can lead the macro).
- **Testing feature combinations** — CI's feature matrix (`.github/ci/_lib.sh`) exercises the
  featureland path; the bundled `use_config` examples exercise the toml path end to end.
