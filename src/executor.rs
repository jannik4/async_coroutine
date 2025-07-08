use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

fn dummy_raw_waker() -> RawWaker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        dummy_raw_waker()
    }
    let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
    RawWaker::new(std::ptr::null::<()>(), vtable)
}

fn dummy_waker() -> Waker {
    unsafe { Waker::from_raw(dummy_raw_waker()) }
}

struct Task<R> {
    future: Pin<Box<dyn Future<Output = R>>>,
}

impl<R> Task<R> {
    fn new(future: Pin<Box<dyn Future<Output = R>>>) -> Self {
        Task { future }
    }

    fn poll(&mut self, context: &mut Context<'_>) -> Poll<R> {
        self.future.as_mut().poll(context)
    }
}

/// Single-threaded single-task polling-based executor.
pub struct Executor<T> {
    task: Task<T>,
}

impl<T> Executor<T> {
    pub fn new(future: Pin<Box<dyn Future<Output = T>>>) -> Self {
        Self {
            task: Task::new(future),
        }
    }

    pub fn poll(&mut self) -> Poll<T> {
        let waker = dummy_waker();
        let mut context = Context::from_waker(&waker);
        self.task.poll(&mut context)
    }
}
