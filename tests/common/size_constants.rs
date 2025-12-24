//! Shared size constants for integration tests.
//!
//! Integration tests are compiled as separate crates (one per top-level file in
//! `tests/`). Placing shared constants under `tests/common/` avoids creating an
//! additional integration test binary while still allowing reuse via:
//!
//! ```rust
//! #[path = "common/size_constants.rs"]
//! mod size_constants;
//! ```

/// Byte count for a single gibibyte.
pub const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;
