use core::future::Future;

use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;

/// Serialize readers and drain an outstanding reply after caller cancellation.
pub(super) struct ReadResponse<T> {
    pending: Mutex<crate::RawMutex, bool>,
    value: Signal<crate::RawMutex, T>,
}

impl<T: Send> ReadResponse<T> {
    pub(super) const fn new() -> Self {
        Self {
            pending: Mutex::new(false),
            value: Signal::new(),
        }
    }

    /// `enqueue` must return Ready in the poll that sends the request;
    /// cancellation while Pending must leave no request queued.
    pub(super) async fn request(&self, enqueue: impl Future<Output = ()>) -> T {
        let mut pending = self.pending.lock().await;
        if *pending {
            self.value.wait().await;
            *pending = false;
        }
        enqueue.await;
        *pending = true;
        let value = self.value.wait().await;
        *pending = false;
        value
    }

    pub(super) fn signal(&self, value: T) {
        self.value.signal(value);
    }
}

#[cfg(test)]
mod tests {
    use core::task::{Context, Poll, Waker};
    use std::boxed::Box;

    use embassy_sync::channel::Channel;

    use super::*;

    #[test]
    fn concurrent_readers_receive_their_own_replies() {
        let response = ReadResponse::new();
        let requests = Channel::<crate::RawMutex, u8, 1>::new();
        let mut cx = Context::from_waker(Waker::noop());
        let mut first = Box::pin(response.request(requests.send(1)));
        let mut second = Box::pin(response.request(requests.send(2)));
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert!(second.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(1));
        assert!(requests.try_receive().is_err());
        response.signal(11);
        assert_eq!(first.as_mut().poll(&mut cx), Poll::Ready(11));
        assert!(second.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(2));
        response.signal(22);
        assert_eq!(second.as_mut().poll(&mut cx), Poll::Ready(22));
    }

    #[test]
    fn cancelled_sent_request_is_drained_before_next_request() {
        let response = ReadResponse::new();
        let requests = Channel::<crate::RawMutex, u8, 1>::new();
        let mut cx = Context::from_waker(Waker::noop());
        let mut first = Box::pin(response.request(requests.send(1)));
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(1));
        drop(first);
        let mut second = Box::pin(response.request(requests.send(2)));
        assert!(second.as_mut().poll(&mut cx).is_pending());
        assert!(requests.try_receive().is_err());
        response.signal(11);
        assert!(second.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(2));
        response.signal(22);
        assert_eq!(second.as_mut().poll(&mut cx), Poll::Ready(22));
    }

    #[test]
    fn cancelled_enqueue_does_not_leave_a_reply_to_drain() {
        let response = ReadResponse::new();
        let requests = Channel::<crate::RawMutex, u8, 1>::new();
        requests.try_send(99).unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        let mut first = Box::pin(response.request(requests.send(1)));
        assert!(first.as_mut().poll(&mut cx).is_pending());
        drop(first);
        assert_eq!(requests.try_receive(), Ok(99));
        let mut second = Box::pin(response.request(requests.send(2)));
        assert!(second.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(2));
        response.signal(22);
        assert_eq!(second.as_mut().poll(&mut cx), Poll::Ready(22));
    }

    #[test]
    fn cancelled_drain_preserves_the_outstanding_reply() {
        let response = ReadResponse::new();
        let requests = Channel::<crate::RawMutex, u8, 1>::new();
        let mut cx = Context::from_waker(Waker::noop());
        let mut first = Box::pin(response.request(requests.send(1)));
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(1));
        drop(first);
        let mut second = Box::pin(response.request(requests.send(2)));
        assert!(second.as_mut().poll(&mut cx).is_pending());
        drop(second);
        let mut third = Box::pin(response.request(requests.send(3)));
        assert!(third.as_mut().poll(&mut cx).is_pending());
        assert!(requests.try_receive().is_err());
        response.signal(11);
        assert!(third.as_mut().poll(&mut cx).is_pending());
        assert_eq!(requests.try_receive(), Ok(3));
        response.signal(33);
        assert_eq!(third.as_mut().poll(&mut cx), Poll::Ready(33));
    }
}
