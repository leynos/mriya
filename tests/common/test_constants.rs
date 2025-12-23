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

/// Byte count for a single gibibyte.
pub const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;

const DEFAULT_INSTANCE_TYPE_VALUE: &str = if BYTES_PER_GB == 0 { "" } else { "DEV1-S" };

/// Default Scaleway instance type used by Mriya when no override is provided.
pub const DEFAULT_INSTANCE_TYPE: &str = DEFAULT_INSTANCE_TYPE_VALUE;
