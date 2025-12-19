//! Shared constants for integration tests.
//!
//! Integration tests are compiled as separate crates (one per top-level file in
//! `tests/`). Placing shared constants under `tests/common/` avoids creating an
//! additional integration test binary while still allowing reuse via:
//!
//! ```rust
//! #[path = "common/test_constants.rs"]
//! mod test_constants;
//! ```

/// Default Scaleway instance type used by Mriya when no override is provided.
pub const DEFAULT_INSTANCE_TYPE: &str = "DEV1-S";
