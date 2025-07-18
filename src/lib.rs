#![deny(rust_2018_idioms)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod executor;
mod yield_now;

use self::executor::Executor;
use self::yield_now::yield_now;
use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc, task::Poll};

/// A generator is a coroutine that does not have a resume value.
pub type Generator<Y, T> = Coroutine<Y, T, ()>;

/// Represents the state of a coroutine, which can either yield a value of type `Y` or complete with
/// a value of type `T`.
#[derive(Debug, PartialEq, Eq)]
pub enum State<Y, T> {
    /// The coroutine yielded a value of type `Y`.
    Yield(Y),
    /// The coroutine completed with a value of type `T`.
    Complete(T),
}

impl<Y, T> State<Y, T> {
    /// Returns `Some` with the yielded value if the state is [`Yield`], otherwise `None`.
    pub fn as_yield(&self) -> Option<&Y> {
        if let Self::Yield(v) = self {
            Some(v)
        } else {
            None
        }
    }

    /// Returns `Some` with the completed value if the state is [`Complete`], otherwise `None`.
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
    /// Returns the value contained in the state, regardless of whether it is a yield or complete state.
    pub fn value(self) -> T {
        match self {
            Self::Yield(value) => value,
            Self::Complete(value) => value,
        }
    }
}

struct ExecutorState<Y, T, R> {
    #[expect(clippy::type_complexity)]
    init: Option<Box<dyn FnOnce(YieldHandle<Y, R>, R) -> Pin<Box<dyn Future<Output = T>>>>>,
    executor: Option<Executor<T>>,
}

impl<Y, T, R> ExecutorState<Y, T, R>
where
    T: 'static,
{
    fn init_or_resume(&mut self, yield_handle: &YieldHandle<Y, R>, resume: R) -> &mut Executor<T> {
        // Can not use match/if-let here because of borrow checker limitations
        if self.executor.is_some() {
            // Put resume into place
            *yield_handle.resume.borrow_mut() = Some(resume);
        } else {
            // Initialize executor
            self.executor = Some(Executor::new(self.init.take().unwrap()(
                yield_handle.clone_(),
                resume,
            )));
        }

        self.executor.as_mut().unwrap()
    }
}

/// A coroutine that can yield values of type `Y`, can be resumed with a value of type `R` and
/// completes with a value of type `T`.
pub struct Coroutine<Y, T, R> {
    executor: ExecutorState<Y, T, R>,
    yield_handle: YieldHandle<Y, R>,
}

impl<Y, T, R> Coroutine<Y, T, R>
where
    T: 'static,
{
    /// Creates a new coroutine from a function that takes the [`YieldHandle`] and the initial
    /// value. The function must return a future that resolves to the final value of type `T`.
    pub fn new<F>(f: impl FnOnce(YieldHandle<Y, R>, R) -> F + 'static) -> Self
    where
        F: Future<Output = T> + 'static,
    {
        Self {
            executor: ExecutorState {
                init: Some(Box::new(move |handle, initial_value| {
                    Box::pin(f(handle, initial_value))
                })),
                executor: None,
            },
            yield_handle: YieldHandle {
                value: Rc::new(RefCell::new(None)),
                resume: Rc::new(RefCell::new(None)),
            },
        }
    }

    /// Resumes the coroutine with a value of type `R`.
    pub fn resume_with(&mut self, resume: R) -> State<Y, T> {
        // Get executor
        let executor = self.executor.init_or_resume(&self.yield_handle, resume);

        // Loop step
        loop {
            let state = match executor.poll() {
                Poll::Ready(res) => Some(State::Complete(res)),
                Poll::Pending => self
                    .yield_handle
                    .value
                    .borrow_mut()
                    .take()
                    .map(State::Yield),
            };
            if let Some(state) = state {
                break state;
            }
        }
    }
}

impl<Y, T> Generator<Y, T>
where
    T: 'static,
{
    /// Resumes the generator.
    pub fn resume(&mut self) -> State<Y, T> {
        self.resume_with(())
    }
}

/// The yield handle can be used from within the coroutine to yield values and receive a resume
/// value when the coroutine is resumed.
pub struct YieldHandle<Y, R = ()> {
    value: Rc<RefCell<Option<Y>>>,
    resume: Rc<RefCell<Option<R>>>,
}

impl<Y, R> YieldHandle<Y, R> {
    /// Yields a value and receives back the resume value when the coroutine is resumed.
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

    // Private so that the user can not clone the handle
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
        let mut generator = Generator::<(), _>::new(|_handle, ()| async {});
        assert_eq!(generator.resume(), State::Complete(()));
    }

    #[test]
    fn test_yield() {
        let mut generator = Generator::new(|handle, ()| async move {
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
        let mut co = Coroutine::new(|handle, _init| async move {
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
        let mut co = Coroutine::new(|handle, state| async move {
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

        async fn nested(handle: &YieldHandle<i32>, value: i32) {
            handle.yield_(0).await;
            handle.yield_(helper(value).await).await;
        }

        let mut generator = Generator::new(|handle, ()| async move {
            handle.yield_(42).await;
            handle.yield_(helper(1).await).await;
            nested(&handle, 13).await;
            nested(&handle, -1).await;
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
        let mut generator = Generator::new(|handle, ()| async move {
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
        let mut generator = Generator::new(|handle, ()| async move {
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
