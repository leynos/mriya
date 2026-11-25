//! Control for the pass case: the same braced body without the attribute.
//!
//! Expected to fail on `unused_braces`. This is what proves the pass case is
//! load-bearing rather than vacuously green.
#![deny(unused_braces)]

fn build() -> u32 {
    7
}

fn braced_fixture() -> u32 { { build() } }

fn main() {
    assert_eq!(braced_fixture(), 7);
}
