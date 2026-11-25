//! Compile-time contract tests for `allow_fixture_expansion_lints`.
//!
//! The macro body is vendored verbatim from `leynos/weaver` and must not be
//! edited, so this suite tests it from the outside: `trybuild` compiles each
//! case as its own crate against the real workspace lint configuration.
//!
//! The pass case is load-bearing. It denies `unused_braces` and wraps a
//! fixture body in an extra block, which is exactly the shape the attribute
//! exists to excuse; removing the attribute makes that case fail to compile.
//! The fail case pins the macro's own rejection of a non-item token stream.

#[test]
fn allow_fixture_expansion_lints_contract() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/braced_fixture.rs");
    cases.compile_fail("tests/ui/fail/non_item_input.rs");
}
