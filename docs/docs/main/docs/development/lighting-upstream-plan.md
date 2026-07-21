# Lighting upstream plan

The implementation branch is intentionally broader than any one review. Land
it as independently useful slices, keeping board-specific data downstream.

## 1. Whole-frame service and controller

- `LightingEngine<Snapshot>` selects an associated whole `Frame` type.
- `LightingOutput<Frame>` owns initialization, presentation, suspend/resume,
  retry policy, and hardware errors.
- `LightingService` is the sole mutable owner and uses relative render
  deadlines, commit-after-success history, and authoritative snapshots.
- `LightingProcessor` consumes a dedicated lossless `LightAction` channel,
  treats state events as invalidations, and serializes host commands through
  a bounded request-correlated mailbox.
- Demonstrate a heterogeneous frame so the API cannot regress to a bare pixel
  slice assumption.

This is the smallest first PR that settles the maintainer's controller and
event-monitoring questions without prematurely fixing a keyboard shape.

## 2. Topology and compositor

- Add stable `LedId`, local `LedSlot`, optional key/position/zones, and separate
  physical routing.
- Add validated logical frames, sparse/dense source adapters, deterministic
  priority/transparency, visible-only deadlines, and output transforms.
- Add the built-in active-layer stack, static/blink/breathe effects, and TTL
  overlay.
- Keep all storage caller-owned and allocation-free.

## 3. Shared layout and configuration

- Make RMK's existing `[layout].map`/KLE model the sole authority for key
  centers, shape, and rotation; lighting, display, and Rynk readback consume
  fixed-point data derived from it.
- Add optional lighting emitters, named zones, output declarations, and routes
  to `keyboard.toml` plus equivalent Rust constructors.
- Resolve stable-ID, key, and zone selectors to local slots at build/load time.
- Reject duplicate IDs, matrix holes, bad zone spans, output collisions,
  unintended holes, incompatible capabilities, and inconsistent split maps.

## 4. Protocol adapters

- Rynk exposes capabilities, topology revision, paginated topology/routing
  readback, revision-checked typed commands, and stable-ID transient overlay
  operations through the service mailbox. Multi-packet overlay replacement is
  bounded and atomic.
- Land the wire schema independently on `main`; while the Rynk runtime remains
  on its feature branch, add firmware handlers and client methods as a stacked
  Rynk change instead of inventing a second dispatcher on `main`.
- VIA/Vial remains a compatibility adapter. Its brightness, hue, speed, and
  supported mode fields control a designated background source; they do not
  replace layer, overlay, or status sources.
- Neither protocol owns live lighting state. Persistence stores validated
  service configuration, not a second compositor model.

## 5. Drivers, split, and board migration

- Add reusable output adapters only where multiple boards share electrical
  behavior. Board repositories keep pins, power sequencing, channel order,
  current limits, and concrete routes.
- Split halves render dense local frames using board-wide stable IDs and
  coordinates. Synchronize authoritative configuration and animation epoch;
  do not encode left/right arithmetic in core.
- Migrate Glove80 chain-index configuration through an explicit compatibility
  map, then remove the legacy compositor after frame equivalence and hardware
  qualification.

## Verification gates

Each slice must retain host tests, `clippy -D warnings`, `no_std` checks, a
Cortex-M target check, static-output/no-write behavior, exact deadline tests,
driver-failure retry, topology stress fixtures, and size reporting for the
processor future and board frame.
