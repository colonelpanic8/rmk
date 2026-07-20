#![no_std]

//! Minimal custom lighting implementations.
//!
//! The same [`LightingProcessor`] works with a one-bit indicator and an RGB
//! strip. Real keyboard crates implement the small hardware traits below for
//! their selected HAL or device driver.

use rmk::event::LightingCommand;
use rmk::lighting::{
    LightingContext, LightingDriver, LightingEvent, LightingProcessor, LightingRenderResult, LightingRenderer,
};
use rmk::types::action::LightAction;

/// The smallest possible lighting hardware: one on/off indicator.
pub trait IndicatorPin {
    type Error;

    fn set(&mut self, on: bool) -> Result<(), Self::Error>;
}

pub struct IndicatorDriver<P>(pub P);

impl<P: IndicatorPin> LightingDriver<bool> for IndicatorDriver<P> {
    type Error = P::Error;

    async fn init(&mut self) -> Result<(), Self::Error> {
        self.0.set(false)
    }

    async fn write(&mut self, frame: &[bool]) -> Result<(), Self::Error> {
        self.0.set(frame.first().copied().unwrap_or(false))
    }
}

/// Illuminate the indicator when Caps Lock is enabled.
pub struct CapsLockRenderer;

impl LightingRenderer<bool> for CapsLockRenderer {
    fn render(&mut self, context: &LightingContext, frame: &mut [bool]) -> LightingRenderResult {
        let next = context.indicators.caps_lock() && !context.sleeping;
        let changed = frame.first().is_none_or(|current| *current != next);
        if let Some(pixel) = frame.first_mut() {
            *pixel = next;
        }
        changed.into()
    }
}

pub fn caps_lock_indicator<P: IndicatorPin>(
    pin: P,
) -> LightingProcessor<IndicatorDriver<P>, CapsLockRenderer, bool, 1> {
    LightingProcessor::new(IndicatorDriver(pin), CapsLockRenderer, [false])
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb8 {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

/// Hardware-specific RGB transport supplied by a keyboard crate.
pub trait RgbStrip {
    type Error;

    fn init(&mut self) -> impl core::future::Future<Output = Result<(), Self::Error>>;
    fn write(&mut self, pixels: &[Rgb8]) -> impl core::future::Future<Output = Result<(), Self::Error>>;
}

pub struct RgbStripDriver<S>(pub S);

impl<S: RgbStrip> LightingDriver<Rgb8> for RgbStripDriver<S> {
    type Error = S::Error;

    async fn init(&mut self) -> Result<(), Self::Error> {
        self.0.init().await
    }

    async fn write(&mut self, frame: &[Rgb8]) -> Result<(), Self::Error> {
        self.0.write(frame).await
    }
}

/// A deliberately simple renderer; a compositor can replace it later without
/// changing [`RgbStripDriver`] or processor ownership.
pub struct LayerRenderer {
    key_pressed: bool,
    enabled: bool,
}

impl Default for LayerRenderer {
    fn default() -> Self {
        Self {
            key_pressed: false,
            enabled: true,
        }
    }
}

impl LightingRenderer<Rgb8> for LayerRenderer {
    fn on_event(&mut self, event: LightingEvent, _context: &LightingContext) {
        if let LightingEvent::Keyboard(event) = event {
            self.key_pressed = event.pressed;
        }
        if let LightingEvent::Command(event) = event
            && let LightingCommand::Action {
                action: LightAction::RgbTog,
                pressed: true,
            } = event.0
        {
            self.enabled = !self.enabled;
        }
    }

    fn render(&mut self, context: &LightingContext, frame: &mut [Rgb8]) -> LightingRenderResult {
        let next = if context.sleeping || !self.enabled {
            Rgb8::default()
        } else if self.key_pressed {
            Rgb8 {
                red: 0x20,
                green: 0x20,
                blue: 0x20,
            }
        } else {
            Rgb8 {
                red: context.effective_layer.saturating_mul(0x10),
                green: 0,
                blue: 0x10,
            }
        };

        let changed = frame.iter().any(|pixel| *pixel != next);
        frame.fill(next);
        changed.into()
    }
}

pub fn rgb_strip<S: RgbStrip, const N: usize>(
    strip: S,
) -> LightingProcessor<RgbStripDriver<S>, LayerRenderer, Rgb8, N> {
    LightingProcessor::new(RgbStripDriver(strip), LayerRenderer::default(), [Rgb8::default(); N])
}
