# Lighting

RMK's optional `lighting` feature provides an event-driven integration boundary
for custom lighting hardware. It deliberately establishes hardware and state
ownership before prescribing topology, effects, or composition.

Enable it in a Rust-configured keyboard:

```toml
[dependencies]
rmk = { version = "0.8", features = ["lighting"] }
```

## Components

- `LightingContext` contains current event-derived keyboard state.
- `LightingRenderer` turns state and transient events into a logical frame.
- `LightingDriver` initializes and writes board-specific hardware.
- `LightingProcessor` owns all three plus frame storage and is the only hardware
  writer.

The structure follows RMK's display support. A renderer can be a simple layer
color function today and a topology-aware compositor later without changing
the event or driver ownership model.

```rust
use rmk::lighting::{
    LightingContext, LightingDriver, LightingProcessor, LightingRenderer,
};

#[derive(Clone, Copy, Default, Eq, PartialEq)]
struct Rgb8(u8, u8, u8);

struct LayerColor;

impl LightingRenderer<Rgb8> for LayerColor {
    fn render(
        &mut self,
        context: &LightingContext,
        frame: &mut [Rgb8],
    ) -> rmk::lighting::LightingRenderResult {
        let next = Rgb8(context.effective_layer * 16, 0, 16);
        let changed = frame.iter().any(|pixel| *pixel != next);
        frame.fill(next);
        changed.into()
    }
}

// Implement LightingDriver<Rgb8> for a board-specific strip driver, then:
let mut lighting = LightingProcessor::new(driver, LayerColor, [Rgb8::default(); 16]);
run_all!(matrix, lighting, keyboard, host_service).await;
```

The renderer receives key events and `LightAction` commands through `on_event`,
and authoritative state through `LightingContext`. The context currently
exposes the effective layer, host LED indicators, and sleep state. Future state
additions should come from RMK's authoritative state rather than reconstructed
event history.

The generic pixel type lets a single indicator use `bool`, an addressable strip
use an RGB type, and a composite driver route a logical frame to multiple
physical outputs. Drivers retain electrical ordering, encoding, timing, power
sequencing, and safety limits.

See [`examples/use_rust/custom_lighting`](https://github.com/HaoboGu/rmk/tree/main/examples/use_rust/custom_lighting)
for one-bit and RGB driver examples.

## Current scope

This first abstraction does not define:

- LED topology or key association;
- source priorities or composition;
- built-in effects or animation scheduling;
- `keyboard.toml` lighting configuration;
- Rynk, Vial, or persistence integration; or
- split-lighting synchronization.

Those features can be added after the event, renderer, processor, and driver
boundaries settle.
