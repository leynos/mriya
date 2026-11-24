//! Compile-time contract tests for `allow_fixture_expansion_lints`.
//!
//! The macro body is vendored verbatim from `leynos/weaver` and must not be
//! edited, so this suite tests it from the outside: `trybuild` compiles each
//! case below as its own crate against the real workspace lint configuration.
//! A pass case proves the attribute leaves a single-expression fixture
//! compilable; a fail case proves non-item input is rejected by `syn` rather
//! than silently emitted.

#[test]
fn allow_fixture_expansion_lints_contract() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/single_expression_fixture.rs");
    cases.compile_fail("tests/ui/fail/non_item_input.rs");
}
