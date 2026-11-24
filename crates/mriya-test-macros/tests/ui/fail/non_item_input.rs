//! The attribute must not be usable on a statement inside a function body.
//!
//! Attributes apply to items, so a caller that writes this has made a mistake
//! the macro cannot rescue: the item token stream it would receive is not an
//! `Item`. The expected diagnostic is recorded in the adjacent `.stderr`
//! snapshot.

use mriya_test_macros::allow_fixture_expansion_lints;

fn main() {
    #[allow_fixture_expansion_lints]
    let _not_an_item = 7_u32;
}
