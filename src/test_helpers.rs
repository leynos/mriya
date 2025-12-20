//! Shared helpers for crate-internal tests.
//!
//! This module re-exports [`crate::test_support::EnvGuard`] so crate tests can
//! serialise environment mutation without duplicating implementation details.

pub use crate::test_support::{ENV_LOCK, EnvGuard};
