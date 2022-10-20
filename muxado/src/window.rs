use std::{
    cmp,
    pin::Pin,
    task::{
        Context,
        Poll,
        Waker,
    },
};

use futures::{
    future::poll_fn,
    lock::BiLock,
    ready,
};

#[derive(Debug)]
pub struct Window {
    max_size: usize,
    size: usize,
    waker: Option<Waker>,
}

impl Window {
    pub(crate) fn new(capacity: usize) -> Self {
        Window {
            max_size: capacity,
            size: capacity,
            waker: None,
        }
    }
    pub fn inc(&mut self, by: usize) {
        if by == 0 {
            return;
        }

        self.size = cmp::min(self.size + by, self.max_size);

        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    pub fn capacity(&self) -> usize {
        self.size
    }

    fn poll_dec(mut self: Pin<&mut Self>, cx: &mut Context<'_>, by: usize) -> Poll<usize> {
        if self.size == 0 {
            self.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        Poll::Ready(self.dec(by))
    }

    pub fn split(self) -> (WindowInc, WindowDec) {
        let (inc, dec) = BiLock::new(self);
        (WindowInc { inner: inc }, WindowDec { inner: dec })
    }

    pub async fn dec_async(&mut self, by: usize) -> usize {
        let mut s = Pin::new(self);
        poll_fn(move |cx| s.as_mut().poll_dec(cx, by)).await
    }

    pub fn dec(&mut self, by: usize) -> usize {
        if self.size == 0 {
            panic!("dec called without capacity")
        }

        let actual = cmp::min(self.size, by);

        self.size -= actual;

        actual
    }
}

pub struct WindowInc {
    inner: BiLock<Window>,
}

impl WindowInc {
    pub async fn inc(&mut self, by: usize) {
        self.inner.lock().await.inc(by);
    }
}

pub struct WindowDec {
    inner: BiLock<Window>,
}

impl WindowDec {
    fn poll_dec(self: Pin<&mut Self>, cx: &mut Context, by: usize) -> Poll<usize> {
        let mut inner = ready!(self.inner.poll_lock(cx));
        inner.as_pin_mut().poll_dec(cx, by)
    }

    pub async fn dec(&mut self, by: usize) -> usize {
        let mut s = Pin::new(self);
        futures::future::poll_fn(|cx| s.as_mut().poll_dec(cx, by)).await
    }
}
