//! Proc-macros for test fixtures that suppress lints triggered by macro expansion.

use proc_macro::TokenStream;
use quote::quote;
use syn::{Item, parse_macro_input};

/// Allows `unused_braces` lint for fixture functions.
///
/// This attribute is used on rstest fixture functions that expand to single-expression
/// bodies. When combined with `fn_single_line = true` in rustfmt.toml, the generated
/// code triggers the `unused_braces` lint. This attribute suppresses that lint
/// specifically for fixture expansions.
#[proc_macro_attribute]
pub fn allow_fixture_expansion_lints(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let parsed_item = parse_macro_input!(item as Item);

    quote! {
        #[allow(
            unused_braces,
            reason = "fixture macro expansion triggers unused-braces on expression bodies"
        )]
        #[cfg_attr(
            clippy,
            expect(
                clippy::allow_attributes,
                reason = "needed to allow unused_braces for fixture macro expansion"
            )
        )]
        #parsed_item
    }
    .into()
}
