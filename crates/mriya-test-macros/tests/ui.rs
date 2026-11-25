//! Compile-time contract tests for `allow_fixture_expansion_lints`.
//!
//! The macro body is vendored verbatim from `leynos/weaver` and must not be
//! edited, so this suite tests it from the outside with `trybuild`.
//!
//! Three cases together pin the behaviour:
//!
//! - `pass/braced_fixture.rs` denies `unused_braces` and wraps a body in an
//!   extra block, the shape the attribute exists to excuse.
//! - `fail/braced_without_attribute.rs` is the same body without the
//!   attribute, expected to fail on that lint. It is the control that makes
//!   the pass case meaningful.
//! - `fail/non_item_input.rs` pins the macro's own rejection of a token
//!   stream that is not an `Item`.

#[test]
fn allow_fixture_expansion_lints_contract() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/braced_fixture.rs");
    cases.compile_fail("tests/ui/fail/braced_without_attribute.rs");
    cases.compile_fail("tests/ui/fail/non_item_input.rs");
}
