//! The generated property: an attribute the base claims to mask is masked whole.
//!
//! The probe table pins the arrangements the shared base documents. This asks
//! the wider question the table cannot: for *any* attribute that grammar claims
//! to accept, with any spacing and any argument order, is the whole attribute
//! swallowed and nothing beyond it? The malformed variant checks the other
//! edge: an attribute missing its closing bracket must not be masked at all.

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

use crate::CARGO_KIND;
use crate::RUNTIME_KEY;
use crate::checks::{compiled_masks, strip_masks};

/// The words a generated line may carry outside the attribute.
const PROSE_WORDS: &[&str] = &[
    "the", "build", "is", "fixed", "and", "an", "appears", "here",
];

/// Cases the generated property checks.
const GENERATED_CASES: u32 = 256;

/// One whitespace run the masks must absorb, as `\s*` does.
fn spacing() -> impl Strategy<Value = String> {
    proptest::sample::select(vec!["", " ", "\t"]).prop_map(str::to_owned)
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

/// Generated runtime attributes are masked whole, their surroundings are not.
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
                "the runtime key was visible: {:?}",
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
