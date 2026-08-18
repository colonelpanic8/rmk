/// The trait for runnable input devices and processors.
///
/// For some input devices or processors, they should keep running in a separate task.
/// This trait is used to run them in a separate task.
pub trait Runnable {
    async fn run(&mut self) -> !;
}

impl<T: Runnable> Runnable for Option<T> {
    async fn run(&mut self) -> ! {
        match self {
            Some(runnable) => runnable.run().await,
            None => core::future::pending().await,
        }
    }
}

/// Wraps a future so its poll is a separate, never-inlined function.
///
/// The entry macro joins every board task into one future; letting the
/// compiler flatten all their polls into a single giant state machine has
/// produced layout-sensitive mid-instruction faults on thumbv7em, where
/// growing any inlined component (a match arm, an enum, a codec) could
/// corrupt sibling arms' resume state. One out-of-line poll per arm bounds
/// both the fault blast radius and the frame size.
pub struct NoInline<F>(pub F);

impl<F: core::future::Future> core::future::Future for NoInline<F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> core::task::Poll<F::Output> {
        // SAFETY: structural pinning of the only field.
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}
