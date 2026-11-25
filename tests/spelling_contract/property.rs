//! The generated property: the masks reach the runtime key and nothing beside it.
//!
//! The probe table pins the arrangements the shared base documents. This asks
//! the wider question the table cannot: for any attribute that grammar claims
//! to accept, with any argument order and spacing drawn from a bounded
//! alphabet, is the runtime key swallowed exactly when the attribute is well
//! formed, and visible when it is not? The malformed variant checks the other
//! edge: the runtime key of an attribute missing its closing bracket must stay
//! visible.
//!
//! It is a check on the key and on the line around it, not on every character
//! of the attribute: an arrangement that masked the key while leaving another
//! part of the attribute visible would pass it. The probe table and the
//! real-engine half of the contract cover the surfaces this leaves open.

use proptest::{
    prelude::*,
    test_runner::{Config, TestRunner},
};

use crate::{
    CARGO_KIND,
    RUNTIME_KEY,
    checks::{compiled_masks, strip_masks},
};

/// The words a generated line may carry outside the attribute.
const PROSE_WORDS: &[&str] = &[
    "the", "build", "is", "fixed", "and", "an", "appears", "here",
];

/// Cases the generated property checks.
const GENERATED_CASES: u32 = 256;

/// One whitespace run the masks must absorb.
///
/// The shared base writes `\s*` everywhere it allows spacing, so the generator
/// draws from a three-character alphabet (space, tab, newline) in runs of up to
/// four characters, rather than from a handful of literals. That covers runs
/// the table never spells out — newlines, mixed tabs and spaces, and empty runs
/// — without pretending to enumerate `\s*` itself.
fn spacing() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(vec![' ', '\t', '\n']), 0..5)
        .prop_map(String::from_iter)
}

/// The quote form a runtime value takes, escaped or plain.
///
/// The base's pattern accepts both, because the attribute reaches the gate
/// either as source or as the inside of a Rust string literal.
fn quote_forms() -> impl Strategy<Value = String> {
    proptest::sample::select(vec!["\\\"".to_owned(), "\"".to_owned()])
}

/// The runtime arguments other than the flavour key that a runtime accepts.
fn generated_argument() -> impl Strategy<Value = String> {
    let panic_policy = (
        proptest::sample::select(vec!["shutdown_runtime", "ignore"]),
        quote_forms(),
    )
        .prop_map(|(policy, quote)| format!("unhandled_panic = {quote}{policy}{quote}"));
    prop_oneof![
        (0_u32..100_000).prop_map(|count| format!("worker_threads = {count}")),
        any::<bool>().prop_map(|is_paused| format!("start_paused = {is_paused}")),
        panic_policy,
    ]
}

/// The runtime kinds an attribute may name, and the flavours each accepts.
///
/// The local runtime is only reachable through `main`, and only without any
/// other argument, so the generator must not pair it with anything else.
fn generated_runtime() -> impl Strategy<Value = (&'static str, &'static str)> {
    proptest::sample::select(vec![
        ("main", "local"),
        ("main", "current_thread"),
        ("main", "multi_thread"),
        ("test", "current_thread"),
        ("test", "multi_thread"),
    ])
}

/// A generated attribute and the context its mask must and must not reach.
#[derive(Debug)]
struct GeneratedCase {
    /// The runtime attribute, or a malformed variant of it.
    attribute: String,
    /// Words placed outside the attribute, which no mask may reach.
    prose: String,
    /// Whether the prose goes before the attribute rather than after it.
    is_prose_first: bool,
    /// Whether the closing bracket was removed from the attribute.
    is_malformed: bool,
}

/// Builds one attribute from the shapes the shared base's pattern spells out.
fn generated_attribute() -> impl Strategy<Value = String> {
    let generated_arguments = proptest::collection::vec(generated_argument(), 0..4);
    (
        generated_runtime(),
        generated_arguments,
        spacing(),
        any::<prop::sample::Index>(),
        spacing(),
        quote_forms(),
    )
        .prop_map(
            |((kind, flavour), mut arguments, lead, position, trail, quote)| {
                if flavour == "local" {
                    arguments.clear();
                }
                let key = format!("{RUNTIME_KEY}{lead}={lead}{quote}{flavour}{quote}");
                arguments.insert(position.index(arguments.len() + 1), key);
                format!("#[{lead}tokio::{kind}({}){trail}]", arguments.join(", "))
            },
        )
}

/// Builds one malformed attribute: the same shape, minus its closing bracket.
fn generated_malformed_attribute() -> impl Strategy<Value = String> {
    generated_attribute().prop_map(|attribute| attribute.trim_end_matches(']').to_owned())
}

/// Builds one generated case: an attribute, prose beside it, and its shape.
fn generated_case() -> impl Strategy<Value = GeneratedCase> {
    let generated_prose = proptest::collection::vec(proptest::sample::select(PROSE_WORDS), 0..4)
        .prop_map(|words| words.join(" "));
    (
        prop_oneof![
            4 => generated_attribute().prop_map(|attribute| (attribute, false)),
            1 => generated_malformed_attribute().prop_map(|attribute| (attribute, true)),
        ],
        generated_prose,
        any::<bool>(),
    )
        .prop_map(
            |((attribute, is_malformed), prose, is_prose_first)| GeneratedCase {
                attribute,
                prose,
                is_prose_first,
                is_malformed,
            },
        )
}

impl GeneratedCase {
    /// The line handed to the masks, with the prose on the chosen side.
    fn text(&self) -> String {
        let prologue = format!("{} {CARGO_KIND}", self.prose);
        if self.is_prose_first {
            format!("{prologue} {}\n", self.attribute)
        } else {
            format!("{} {prologue}\n", self.attribute)
        }
    }
}

/// The generated runtime key is visible exactly when the attribute is
/// malformed, and the spelling beside it survives either way.
#[test]
fn generated_tokio_attributes_are_masked_without_reaching_their_surroundings() -> Result<(), String>
{
    let patterns = compiled_masks()?;
    let mut runner = TestRunner::new(Config::with_cases(GENERATED_CASES));
    runner
        .run(&generated_case(), |case| {
            let text = case.text();
            let masked = strip_masks(&text, &patterns);
            prop_assert_eq!(
                masked.contains(RUNTIME_KEY),
                case.is_malformed,
                "the runtime key visibility did not match well-formedness: {:?}",
                text
            );
            prop_assert!(
                masked.contains(CARGO_KIND),
                "the mask reached past the attribute: {:?}",
                text
            );
            Ok(())
        })
        .map_err(|error| error.to_string())
}
