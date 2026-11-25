//! The attribute must suppress `unused_braces` on a fixture body.
//!
//! The body below is deliberately wrapped in an extra block. Without the
//! attribute that block is `unused_braces`, which this crate denies, so the
//! case fails to compile if the attribute stops emitting its allowance.
#![deny(unused_braces)]

use mriya_test_macros::allow_fixture_expansion_lints;

fn build() -> u32 {
    7
}

#[allow_fixture_expansion_lints]
fn braced_fixture() -> u32 { { build() } }

fn main() {
    assert_eq!(braced_fixture(), 7);
}
