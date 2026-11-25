//! The attribute must reject a token stream that is not an `Item`.
//!
//! An attribute on a `let` statement reaches the proc macro, which parses it
//! as an `Item` and fails: a `let` statement is not one. Rust separately
//! reports that expanding such an attribute is unstable, so the snapshot below
//! records both. The `syn` failure is the one this case pins: it is the
//! macro's own rejection, and it is what stops a mistaken caller from
//! receiving silently emitted tokens.

use mriya_test_macros::allow_fixture_expansion_lints;

fn main() {
    #[allow_fixture_expansion_lints]
    let not_an_item = 7_u32;
}
