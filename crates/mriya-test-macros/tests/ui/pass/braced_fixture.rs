//! The attribute must suppress `unused_braces` on the item it decorates.
//!
//! The fixture body is deliberately wrapped in an extra block. Without the
//! attribute that block is `unused_braces`, which this crate denies; the
//! negative control shows the same body without it. So this case fails if the
//! macro's allowance is removed or replaced with an identity pass-through.
//!
//! The body is a plain `fn` rather than an `#[fixture]` on purpose.
//! `#[fixture]` was measured not to trip `unused_braces` on this toolchain
//! (its expansion does not emit the extra braces this lint reports), so a
//! fixture-based case would pass with or without the attribute and prove
//! nothing.
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
