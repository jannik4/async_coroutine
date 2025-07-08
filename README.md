# Async Coroutine

[![License](https://img.shields.io/badge/license-MIT%2FApache-blue.svg)](#license)
[![Build Status](https://github.com/jannik4/async_coroutine/workflows/CI/badge.svg)](https://github.com/jannik4/async_coroutine/actions)
[![crates.io](https://img.shields.io/crates/v/async_coroutine.svg)](https://crates.io/crates/async_coroutine)
[![docs.rs](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/async_coroutine)

This crate provides a coroutine implementation based on Rust's async/await syntax.

## Usage

```rust
use async_coroutine::{Coroutine, State};

let mut co = Coroutine::new(|handle, value| async move {
    assert_eq!(value, 1);
    assert_eq!(handle.yield_(true).await, 2);
    assert_eq!(handle.yield_(false).await, 3);
    "Bye"
});

assert_eq!(co.resume_with(1), State::Yield(true));
assert_eq!(co.resume_with(2), State::Yield(false));
assert_eq!(co.resume_with(3), State::Complete("Bye"));
```
