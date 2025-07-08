#![deny(rust_2018_idioms)]

mod executor;
mod yield_now;

use self::executor::Executor;
use self::yield_now::yield_now;
use std::{cell::RefCell, future::Future, rc::Rc, task::Poll};

pub type Generator<Y, T> = Coroutine<Y, T, ()>;

#[derive(Debug, PartialEq, Eq)]
pub enum State<Y, T> {
    Yield(Y),
    Complete(T),
}

impl<Y, T> State<Y, T> {
    pub fn as_yield(&self) -> Option<&Y> {
        if let Self::Yield(v) = self {
            Some(v)
        } else {
            None
        }
    }

    pub fn as_complete(&self) -> Option<&T> {
        if let Self::Complete(v) = self {
            Some(v)
        } else {
            None
        }
    }

    /// Returns `true` if the state is [`Yield`].
    pub fn is_yield(&self) -> bool {
        matches!(self, Self::Yield(..))
    }

    /// Returns `true` if the state is [`Complete`].
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete(..))
    }
}

impl<T> State<T, T> {
    pub fn value(self) -> T {
        match self {
            Self::Yield(value) => value,
            Self::Complete(value) => value,
        }
    }
}

pub struct Coroutine<Y, T, R> {
    executor: Executor<T>,
    yield_handle: YieldHandle<Y, R>,
}

impl<Y, T, R> Coroutine<Y, T, R> {
    pub fn new<F>(producer: impl FnOnce(YieldHandle<Y, R>) -> F) -> Self
    where
        F: Future<Output = T> + 'static,
    {
        let yield_handle = YieldHandle {
            value: Rc::new(RefCell::new(None)),
            resume: Rc::new(RefCell::new(None)),
        };
        let future = producer(yield_handle.clone_());

        Self {
            executor: Executor::new(future),
            yield_handle,
        }
    }

    pub fn resume_with(&mut self, resume: R) -> State<Y, T> {
        // Put resume into place
        *self.yield_handle.resume.borrow_mut() = Some(resume);

        // Loop step
        loop {
            if let Some(state) = self.step() {
                break state;
            }
        }
    }

    fn step(&mut self) -> Option<State<Y, T>> {
        match self.executor.poll() {
            Poll::Ready(res) => Some(State::Complete(res)),
            Poll::Pending => self
                .yield_handle
                .value
                .borrow_mut()
                .take()
                .map(State::Yield),
        }
    }
}

impl<Y, T> Generator<Y, T> {
    pub fn resume(&mut self) -> State<Y, T> {
        self.resume_with(())
    }
}

pub struct YieldHandle<Y, R = ()> {
    value: Rc<RefCell<Option<Y>>>,
    resume: Rc<RefCell<Option<R>>>,
}

impl<Y, R> YieldHandle<Y, R> {
    pub async fn yield_(&self, value: Y) -> R {
        // Extra scope necessary because of a false positive of clippy::await_holding_refcell_ref
        {
            // Set current
            let mut current = self.value.borrow_mut();
            match *current {
                Some(_) => panic!("multiple values were yielded without awaiting them"),
                None => *current = Some(value),
            }

            // Drop current ref before yield
            drop(current);
        }

        // Yield one "tick"
        yield_now().await;

        // Get resume value
        self.resume
            .borrow_mut()
            .take()
            .expect("expected resume value")
    }

    /// Take initial resume value. This only works before the first `yield_` call.
    /// If you don't call this the first resume value is just dropped.
    ///
    /// # Panics
    ///
    /// Panics if called multiple times or after any `yield_` call.
    pub fn take_initial_resume(&self) -> R {
        self.resume
            .borrow_mut()
            .take()
            .expect("expected initial resume value")
    }

    fn clone_(&self) -> Self {
        Self {
            value: Rc::clone(&self.value),
            resume: Rc::clone(&self.resume),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let mut generator = Generator::<(), _>::new(|_handle| async {});
        assert_eq!(generator.step(), Some(State::Complete(())));
    }

    #[test]
    fn test_yield() {
        let mut generator = Generator::new(|handle| async move {
            handle.yield_(true).await;
            handle.yield_(false).await;
            "Bye"
        });

        assert_eq!(generator.resume(), State::Yield(true));
        assert_eq!(generator.resume(), State::Yield(false));
        assert_eq!(generator.resume(), State::Complete("Bye"));
    }

    #[test]
    fn test_yield_resume() {
        let mut co = Coroutine::new(|handle| async move {
            let resume = handle.yield_(42).await;
            let resume = handle.yield_(resume * 2).await;
            resume + 1
        });

        assert_eq!(co.resume_with(-1), State::Yield(42));
        assert_eq!(co.resume_with(71), State::Yield(142));
        assert_eq!(co.resume_with(11), State::Complete(12));
    }

    #[test]
    fn test_yield_initial_resume() {
        let mut co = Coroutine::new(|handle| async move {
            let state = handle.take_initial_resume();
            let state = handle.yield_(state + 1).await;
            let state = handle.yield_(state * 2).await;
            state + 7
        });

        let mut state = 4;
        state = *co.resume_with(state).as_yield().unwrap();
        state = *co.resume_with(state).as_yield().unwrap();
        state = *co.resume_with(state).as_complete().unwrap();

        assert_eq!(state, 17);
    }

    #[test]
    fn test_call_async_function() {
        async fn helper(value: i32) -> i32 {
            value * 2
        }

        async fn producer(handle: &YieldHandle<i32>, value: i32) {
            handle.yield_(0).await;
            handle.yield_(helper(value).await).await;
        }

        let mut generator = Generator::new(|handle| async move {
            handle.yield_(42).await;
            handle.yield_(helper(1).await).await;
            producer(&handle, 13).await;
            producer(&handle, -1).await;
            "Bye"
        });

        assert_eq!(generator.resume(), State::Yield(42));
        assert_eq!(generator.resume(), State::Yield(2));
        assert_eq!(generator.resume(), State::Yield(0));
        assert_eq!(generator.resume(), State::Yield(26));
        assert_eq!(generator.resume(), State::Yield(0));
        assert_eq!(generator.resume(), State::Yield(-2));
        assert_eq!(generator.resume(), State::Complete("Bye"));
    }

    #[test]
    fn test_yield_missing_await() {
        let mut generator = Generator::new(|handle| async move {
            #[expect(clippy::let_underscore_future)]
            let _ = handle.yield_(true); // Never gets yield
            handle.yield_(true).await;
            handle.yield_(false).await;
            "Bye"
        });

        assert_eq!(generator.resume(), State::Yield(true));
        assert_eq!(generator.resume(), State::Yield(false));
        assert_eq!(generator.resume(), State::Complete("Bye"));
    }

    #[test]
    #[should_panic(expected = "`async fn` resumed after completion")]
    fn test_resumed_after_completion() {
        let mut generator = Generator::new(|handle| async move {
            handle.yield_(42i32).await;
            handle.yield_(21).await;
            "Ok"
        });

        assert_eq!(generator.resume(), State::Yield(42));
        assert_eq!(generator.resume(), State::Yield(21));
        assert_eq!(generator.resume(), State::Complete("Ok"));
        generator.resume(); // This panics
    }
}
