//! What `hek` is made of, as a library so its tests can reach it.
//!
//! The binary is `src/main.rs`. Everything with a shape worth testing on its own lives
//! here instead, because an integration test in `tests/` can only link the library target.

pub mod grammar;
