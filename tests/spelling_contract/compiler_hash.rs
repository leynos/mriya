//! The compiler-hash exemption, judged by the real spelling engine.
//!
//! The pinned nightly's hash contains a letter pair the dictionary reports as
//! a misspelling. The overlay exempts that one opaque identifier, so a
//! different hash carrying the same pair must still reach the gate. This
//! module lives apart from `gate` to keep each module within the size budget.

use crate::COMPILER_HASH;
use crate::gate::{accepted, assert_fixtures, reported};

/// The exact hash is accepted; a different hash with the same pair is not.
#[test]
fn the_compiler_hash_exemption_covers_one_identifier() -> Result<(), String> {
    assert_fixtures(&[
        accepted(
            "compiler-hash",
            COMPILER_HASH,
            format!(
                "The pinned compiler is `rustc 1.102.0-nightly (ac8310{COMPILER_HASH}6 2026-11-09)`.\n"
            ),
        ),
        reported(
            "other-hash",
            COMPILER_HASH,
            format!("A different hash is ac8310{COMPILER_HASH}7.\n"),
        ),
    ])
}
