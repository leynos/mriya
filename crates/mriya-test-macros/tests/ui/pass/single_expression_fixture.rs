//! The attribute must leave a single-expression fixture compilable.
//!
//! The shape matters: an `fn` whose body is one expression written on one
//! line is what triggers `unused_braces` once `fn_single_line = true` is set.
//! This case therefore fails if the attribute stops emitting its allowance.

use mriya_test_macros::allow_fixture_expansion_lints;

fn build() -> u32 {
    7
}

#[allow_fixture_expansion_lints]
fn single_expression_fixture() -> u32 { build() }

fn main() {
    assert_eq!(single_expression_fixture(), 7);
}
