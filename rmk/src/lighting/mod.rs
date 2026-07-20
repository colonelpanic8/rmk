//! Event-driven lighting support.
//!
//! This module provides the integration boundary between RMK state and
//! board-specific lighting hardware. [`LightingProcessor`] owns the current
//! [`LightingContext`], a replaceable [`LightingRenderer`], a caller-sized
//! frame, and a [`LightingDriver`]. It is the only component that writes the
//! driver.
//!
//! Topology, source composition, effect catalogs, and animation scheduling are
//! intentionally outside this initial abstraction. They can be implemented by
//! a renderer without changing event or hardware ownership.

use rmk_macro::processor;
use rmk_types::led_indicator::LedIndicator;

use crate::core_traits::Runnable;
use crate::event::{KeyboardEvent, LayerChangeEvent, LedIndicatorEvent, LightingCommandEvent, SleepStateEvent};
use crate::processor::Processor;

/// Current RMK state made available to lighting renderers.
///
/// This context contains state with an authoritative value. Transient input is
/// delivered separately through [`LightingRenderer::on_event`]. A richer layer
/// snapshot can extend this type once RMK exposes the default and complete
/// active-layer state to processors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct LightingContext {
    /// Current effective (topmost) layer.
    pub effective_layer: u8,
    /// Host keyboard LED indicators such as Caps Lock and Num Lock.
    pub indicators: LedIndicator,
    /// Whether RMK has entered its sleep state.
    pub sleeping: bool,
}

impl Default for LightingContext {
    fn default() -> Self {
        Self {
            effective_layer: 0,
            indicators: LedIndicator::new(),
            sleeping: false,
        }
    }
}

/// An RMK event delivered to a lighting renderer.
///
/// State fields in [`LightingContext`] are updated before this event is
/// delivered. The enum is non-exhaustive so RMK can expose additional relevant
/// events without forcing renderers to handle them immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LightingEvent {
    /// A key or encoder input event.
    Keyboard(KeyboardEvent),
    /// The effective layer changed.
    LayerChanged(LayerChangeEvent),
    /// Host keyboard LED indicators changed.
    IndicatorsChanged(LedIndicatorEvent),
    /// RMK entered or left sleep.
    SleepChanged(SleepStateEvent),
    /// A keymap or host control command.
    Command(LightingCommandEvent),
}

/// Outcome of one renderer pass.
///
/// This type intentionally hides its representation so future versions can
/// add a next-render deadline without changing [`LightingRenderer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct LightingRenderResult {
    changed: bool,
}

impl LightingRenderResult {
    /// The rendered frame may differ from the previously completed frame.
    pub const CHANGED: Self = Self { changed: true };

    /// The rendered frame is known to be unchanged.
    pub const UNCHANGED: Self = Self { changed: false };

    /// Whether the rendered frame may have changed.
    pub const fn may_have_changed(self) -> bool {
        self.changed
    }
}

impl From<bool> for LightingRenderResult {
    fn from(changed: bool) -> Self {
        Self { changed }
    }
}

/// Board- or application-provided lighting behavior.
///
/// A renderer writes logical pixels into the provided frame and reports
/// whether the completed frame may have changed. Reporting a possible change
/// conservatively is always correct; reporting no change allows the processor
/// to skip a hardware write.
///
/// Renderers may later contain topology, compositors, or effect engines. They
/// do not initialize hardware or write a driver directly.
pub trait LightingRenderer<P> {
    /// Observe an event before the resulting frame is rendered.
    ///
    /// The default implementation is suitable for renderers that depend only
    /// on current state in [`LightingContext`].
    fn on_event(&mut self, _event: LightingEvent, _context: &LightingContext) {}

    /// Update `frame` from the current context.
    ///
    /// Report whether the frame may have changed since the previous call.
    fn render(&mut self, context: &LightingContext, frame: &mut [P]) -> LightingRenderResult;
}

/// Async output boundary for board-specific lighting hardware.
///
/// `P` is deliberately generic: a single GPIO-controlled LED may use `bool`,
/// while an addressable strip may use a board- or ecosystem-specific RGB type.
/// A driver can route one logical frame to multiple physical outputs.
pub trait LightingDriver<P> {
    /// Hardware initialization or write error.
    type Error;

    /// Initialize the lighting hardware.
    async fn init(&mut self) -> Result<(), Self::Error>;

    /// Apply a completed logical frame to the hardware.
    async fn write(&mut self, frame: &[P]) -> Result<(), Self::Error>;
}

/// Owns event-derived state, rendering, frame storage, and hardware writes.
///
/// The frame capacity is selected by the board at compile time. The processor
/// performs an initial write when it starts, then writes only when the renderer
/// reports a possible change or a previous driver write remains pending.
#[processor(subscribe = [KeyboardEvent, LayerChangeEvent, LedIndicatorEvent, SleepStateEvent, LightingCommandEvent])]
#[::rmk::macros::runnable_generated]
pub struct LightingProcessor<D, R, P, const N: usize>
where
    D: LightingDriver<P>,
    R: LightingRenderer<P>,
    P: Copy,
{
    driver: D,
    renderer: R,
    context: LightingContext,
    frame: [P; N],
    initialized: bool,
    pending_write: bool,
}

impl<D, R, P, const N: usize> LightingProcessor<D, R, P, N>
where
    D: LightingDriver<P>,
    R: LightingRenderer<P>,
    P: Copy,
{
    /// Create a processor with caller-provided initial frame storage.
    ///
    /// Passing the frame as an array lets Rust infer the processor's capacity
    /// while keeping storage explicit at the board construction site.
    pub fn new(driver: D, renderer: R, frame: [P; N]) -> Self {
        Self {
            driver,
            renderer,
            context: LightingContext::default(),
            frame,
            initialized: false,
            // Hardware state is unknown until the first successful write.
            pending_write: true,
        }
    }

    /// Current event-derived lighting state.
    pub fn context(&self) -> &LightingContext {
        &self.context
    }

    /// Last logical frame produced by the renderer.
    pub fn frame(&self) -> &[P; N] {
        &self.frame
    }

    /// Mutably access application rendering state.
    ///
    /// Call [`refresh`](Self::refresh) after making a change that should become
    /// visible. The processor retains sole ownership of hardware writes.
    pub fn renderer_mut(&mut self) -> &mut R {
        &mut self.renderer
    }

    /// Render current state and flush pending output.
    ///
    /// Returns `Ok(true)` when the driver was written. Initialization and write
    /// failures remain pending and are retried on the next refresh or event.
    pub async fn refresh(&mut self) -> Result<bool, D::Error> {
        if !self.initialized {
            self.driver.init().await?;
            self.initialized = true;
        }

        if self.renderer.render(&self.context, &mut self.frame).may_have_changed() {
            self.pending_write = true;
        }

        if !self.pending_write {
            return Ok(false);
        }

        self.driver.write(&self.frame).await?;
        self.pending_write = false;
        Ok(true)
    }

    async fn handle_event(&mut self, event: LightingEvent) {
        match event {
            LightingEvent::Keyboard(_) => {}
            LightingEvent::LayerChanged(event) => {
                self.context.effective_layer = event.0;
            }
            LightingEvent::IndicatorsChanged(event) => {
                self.context.indicators = event.0;
            }
            LightingEvent::SleepChanged(event) => {
                self.context.sleeping = event.0;
            }
            LightingEvent::Command(_) => {}
        }

        self.renderer.on_event(event, &self.context);
        // A failed write remains pending. The next event or explicit refresh
        // retries it without requiring a polling loop.
        if self.refresh().await.is_err() {
            error!("Lighting driver update failed");
        }
    }

    async fn on_keyboard_event(&mut self, event: KeyboardEvent) {
        self.handle_event(LightingEvent::Keyboard(event)).await;
    }

    async fn on_layer_change_event(&mut self, event: LayerChangeEvent) {
        self.handle_event(LightingEvent::LayerChanged(event)).await;
    }

    async fn on_led_indicator_event(&mut self, event: LedIndicatorEvent) {
        self.handle_event(LightingEvent::IndicatorsChanged(event)).await;
    }

    async fn on_sleep_state_event(&mut self, event: SleepStateEvent) {
        self.handle_event(LightingEvent::SleepChanged(event)).await;
    }

    async fn on_lighting_command_event(&mut self, event: LightingCommandEvent) {
        self.handle_event(LightingEvent::Command(event)).await;
    }
}

impl<D, R, P, const N: usize> Runnable for LightingProcessor<D, R, P, N>
where
    D: LightingDriver<P>,
    R: LightingRenderer<P>,
    P: Copy,
{
    async fn run(&mut self) -> ! {
        if self.refresh().await.is_err() {
            error!("Initial lighting driver update failed");
        }
        self.process_loop().await
    }
}

#[cfg(test)]
mod tests {
    use rmk_types::action::LightAction;

    use super::*;
    use crate::event::{KeyPos, KeyboardEvent, LightingCommand, LightingCommandEvent};
    use crate::test_support::test_block_on as block_on;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FakeError {
        Init,
        Write,
    }

    struct FakeDriver<P: Copy, const N: usize> {
        init_calls: usize,
        write_calls: usize,
        fail_init_once: bool,
        fail_write_once: bool,
        last_frame: [P; N],
    }

    impl<P: Copy, const N: usize> FakeDriver<P, N> {
        fn new(initial: P) -> Self {
            Self {
                init_calls: 0,
                write_calls: 0,
                fail_init_once: false,
                fail_write_once: false,
                last_frame: [initial; N],
            }
        }
    }

    impl<P: Copy, const N: usize> LightingDriver<P> for FakeDriver<P, N> {
        type Error = FakeError;

        async fn init(&mut self) -> Result<(), Self::Error> {
            self.init_calls += 1;
            if core::mem::take(&mut self.fail_init_once) {
                Err(FakeError::Init)
            } else {
                Ok(())
            }
        }

        async fn write(&mut self, frame: &[P]) -> Result<(), Self::Error> {
            self.write_calls += 1;
            if core::mem::take(&mut self.fail_write_once) {
                return Err(FakeError::Write);
            }
            self.last_frame.copy_from_slice(frame);
            Ok(())
        }
    }

    #[derive(Default)]
    struct StateRenderer {
        events: usize,
        last_event: Option<LightingEvent>,
    }

    impl LightingRenderer<u8> for StateRenderer {
        fn on_event(&mut self, event: LightingEvent, _context: &LightingContext) {
            self.events += 1;
            self.last_event = Some(event);
        }

        fn render(&mut self, context: &LightingContext, frame: &mut [u8]) -> LightingRenderResult {
            let next = if context.sleeping {
                0
            } else if context.indicators.caps_lock() {
                0x80
            } else {
                context.effective_layer
            };
            let changed = frame.iter().any(|pixel| *pixel != next);
            frame.fill(next);
            changed.into()
        }
    }

    #[test]
    fn initializes_and_writes_the_first_frame_once() {
        let driver = FakeDriver::<u8, 3>::new(0xff);
        let mut processor = LightingProcessor::new(driver, StateRenderer::default(), [0; 3]);

        assert_eq!(block_on(processor.refresh()), Ok(true));
        assert_eq!(processor.driver.init_calls, 1);
        assert_eq!(processor.driver.write_calls, 1);
        assert_eq!(processor.driver.last_frame, [0; 3]);

        assert_eq!(block_on(processor.refresh()), Ok(false));
        assert_eq!(processor.driver.init_calls, 1);
        assert_eq!(processor.driver.write_calls, 1);
    }

    #[test]
    fn updates_context_before_notifying_and_rendering() {
        let driver = FakeDriver::<u8, 2>::new(0);
        let mut processor = LightingProcessor::new(driver, StateRenderer::default(), [0; 2]);
        block_on(processor.refresh()).unwrap();

        block_on(processor.on_layer_change_event(LayerChangeEvent::new(3)));
        assert_eq!(processor.context.effective_layer, 3);
        assert_eq!(processor.frame, [3; 2]);

        block_on(processor.on_led_indicator_event(LedIndicatorEvent::new(LedIndicator::CAPS_LOCK)));
        assert_eq!(processor.context.indicators, LedIndicator::CAPS_LOCK);
        assert_eq!(processor.frame, [0x80; 2]);

        block_on(processor.on_sleep_state_event(SleepStateEvent::new(true)));
        assert!(processor.context.sleeping);
        assert_eq!(processor.frame, [0; 2]);
        assert_eq!(processor.renderer.events, 3);
        assert_eq!(processor.driver.write_calls, 4);
    }

    #[test]
    fn delivers_transient_keyboard_events_without_polling() {
        struct ReactiveRenderer {
            pressed: bool,
        }

        impl LightingRenderer<bool> for ReactiveRenderer {
            fn on_event(&mut self, event: LightingEvent, _context: &LightingContext) {
                if let LightingEvent::Keyboard(event) = event {
                    self.pressed = event.pressed;
                }
            }

            fn render(&mut self, _context: &LightingContext, frame: &mut [bool]) -> LightingRenderResult {
                let changed = frame[0] != self.pressed;
                frame[0] = self.pressed;
                changed.into()
            }
        }

        let driver = FakeDriver::<bool, 1>::new(false);
        let renderer = ReactiveRenderer { pressed: false };
        let mut processor = LightingProcessor::new(driver, renderer, [false; 1]);
        block_on(processor.refresh()).unwrap();

        block_on(processor.on_keyboard_event(KeyboardEvent::key(1, 2, true)));
        assert_eq!(processor.frame, [true]);
        block_on(processor.on_keyboard_event(KeyboardEvent::key(1, 2, false)));
        assert_eq!(processor.frame, [false]);
        assert_eq!(processor.driver.write_calls, 3);
    }

    #[test]
    fn retries_initialization_and_failed_writes() {
        let mut driver = FakeDriver::<u8, 1>::new(0);
        driver.fail_init_once = true;
        let mut processor = LightingProcessor::new(driver, StateRenderer::default(), [0; 1]);

        assert_eq!(block_on(processor.refresh()), Err(FakeError::Init));
        assert_eq!(block_on(processor.refresh()), Ok(true));
        assert_eq!(processor.driver.init_calls, 2);

        processor.driver.fail_write_once = true;
        block_on(processor.on_layer_change_event(LayerChangeEvent::new(2)));
        assert_eq!(processor.driver.write_calls, 2);
        assert_eq!(processor.driver.last_frame, [0]);

        assert_eq!(block_on(processor.refresh()), Ok(true));
        assert_eq!(processor.driver.write_calls, 3);
        assert_eq!(processor.driver.last_frame, [2]);
    }

    #[test]
    fn event_type_preserves_keyboard_position() {
        let event = KeyboardEvent::key(4, 5, true);
        let wrapped = LightingEvent::Keyboard(event);
        assert_eq!(wrapped, LightingEvent::Keyboard(event));
        assert_eq!(
            event.pos,
            crate::event::KeyboardEventPos::Key(KeyPos { row: 4, col: 5 })
        );
    }

    #[test]
    fn delivers_light_actions_through_the_command_boundary() {
        let driver = FakeDriver::<u8, 1>::new(0);
        let mut processor = LightingProcessor::new(driver, StateRenderer::default(), [0; 1]);
        block_on(processor.refresh()).unwrap();

        let event = LightingCommandEvent::from_action(LightAction::RgbTog, true);
        block_on(processor.on_lighting_command_event(event));

        assert_eq!(
            processor.renderer.last_event,
            Some(LightingEvent::Command(LightingCommandEvent(LightingCommand::Action {
                action: LightAction::RgbTog,
                pressed: true,
            })))
        );
    }
}
