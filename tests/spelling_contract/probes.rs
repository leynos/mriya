//! The exemptions this repository owns, judged in both directions.
//!
//! Each probe is one `rstest` case rather than a row in a table, so a
//! disagreement names the single arrangement that broke and that arrangement
//! can be run on its own. The probe still states its input and its expected
//! verdict together, which is what makes the file readable as a specification.

use rstest::rstest;

use crate::CARGO_KIND;
use crate::COMPILER_HASH;
use crate::LINKER_NAME;
use crate::RUNTIME_KEY;
use crate::checks::{Probe, accepted, check, reported};

/// The external names this repository exempts, accepted where they belong and
/// exposed as ordinary prose elsewhere.
#[rstest]
#[case::cargo_kind_code_span(accepted(
    CARGO_KIND,
    format!("Cargo prints `compiler-{CARGO_KIND}` per unit.\n"),
))]
#[case::cargo_kind_prose(accepted(
    CARGO_KIND,
    format!("The compiler-{CARGO_KIND} kind is stable.\n"),
))]
#[case::linker_code_span(accepted(
    LINKER_NAME,
    format!("We link with `{LINKER_NAME}` on Linux.\n"),
))]
#[case::linker_phrase(accepted(
    LINKER_NAME,
    format!("The {LINKER_NAME} linker ships for Linux alone.\n"),
))]
#[case::linker_setup_input(accepted(
    LINKER_NAME,
    format!("Pass install-{LINKER_NAME}: true to setup-rust.\n"),
))]
#[case::linker_flag(accepted(
    LINKER_NAME,
    format!("Rustc receives -fuse-ld={LINKER_NAME} here.\n"),
))]
#[case::linker_contains(accepted(
    LINKER_NAME,
    format!("The reader probes value.contains(\"{LINKER_NAME}\") once.\n"),
))]
#[case::cargo_kind_misspelling_in_prose(reported(
    CARGO_KIND,
    format!("The build {CARGO_KIND} is stale.\n"),
))]
// Inline code is no longer masked, so this span must reach the gate.
#[case::cargo_kind_misspelling_in_code_span(reported(
    CARGO_KIND,
    format!("A `{CARGO_KIND}` span is still checked.\n"),
))]
#[case::linker_misspelling_suffix(reported(
    LINKER_NAME,
    format!("The {LINKER_NAME}y bread was thrown out.\n"),
))]
#[case::linker_misspelling_bare(reported(
    LINKER_NAME,
    format!("The wheel was {LINKER_NAME}."),
))]
// The compiler-hash exemption covers one opaque identifier, so a different
// hash that carries the same letter pair must still reach the gate.
#[case::compiler_hash_code_span(accepted(
    COMPILER_HASH,
    format!(
        "The pinned compiler is `rustc 1.102.0-nightly (ac8310{COMPILER_HASH}6 2026-11-09)`.\n"
    ),
))]
#[case::other_hash_reported(reported(
    COMPILER_HASH,
    format!("A different hash is ac8310{COMPILER_HASH}7.\n"),
))]
fn local_exemptions_accept_the_exact_name_and_reject_the_misspelling(
    #[case] probe: Probe,
) -> Result<(), String> {
    check(&probe)
}

/// The declaration exemptions this contract file needs for its own constants.
///
/// Each is anchored to one exact declaration line, so the positive probes are
/// those lines and the negative probes are the same spelling on any other
/// line: a different name, a different indent, a different keyword, or the
/// same text in prose. Widening one anchor therefore fails here.
#[rstest]
#[case::anchored_cargo_kind(accepted(
    CARGO_KIND,
    format!("const CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
))]
#[case::anchored_linker_name(accepted(
    LINKER_NAME,
    format!("const LINKER_NAME: &str = \"{LINKER_NAME}\";\n"),
))]
#[case::anchored_runner_key(accepted(
    RUNTIME_KEY,
    format!("const RUNTIME_KEY: &str = \"{RUNTIME_KEY}\";\n"),
))]
// A checkout that converts line endings must not lose the exemption, because
// the anchor then has to tolerate the carriage return.
#[case::anchored_cargo_kind_crlf(accepted(
    CARGO_KIND,
    format!("const CARGO_KIND: &str = \"{CARGO_KIND}\";\r\n"),
))]
#[case::indented_declaration(reported(
    CARGO_KIND,
    format!("    const CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
))]
#[case::indented_declaration_crlf(reported(
    CARGO_KIND,
    format!("    const CARGO_KIND: &str = \"{CARGO_KIND}\";\r\n"),
))]
#[case::renamed_declaration(reported(
    CARGO_KIND,
    format!("const CARGO_MSG: &str = \"{CARGO_KIND}\";\n"),
))]
#[case::static_declaration(reported(
    CARGO_KIND,
    format!("static CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
))]
#[case::other_linker_declaration(reported(
    LINKER_NAME,
    format!("const OTHER_LINKER: &str = \"{LINKER_NAME}\";\n"),
))]
#[case::declaration_spelling_in_prose(reported(
    RUNTIME_KEY,
    format!("the runtime key is {RUNTIME_KEY} here.\n"),
))]
fn declaration_exemptions_are_anchored_to_their_exact_lines(
    #[case] probe: Probe,
) -> Result<(), String> {
    check(&probe)
}

/// A complete Tokio runtime attribute carrying `body`.
pub fn tokio_attribute(body: &str) -> String {
    format!("#[tokio::{body}]\n")
}

/// The Tokio runtime attributes the shared base masks.
///
/// Three runtime arguments is the bound the base's own comment states, so each
/// arrangement of three appears once. The escaped-string form and `main` with a
/// worker pool are here because the shared base's pattern spells them out and
/// nothing else exercises them. Without these probes the generated pattern
/// could lose an arrangement and only a later contributor's build would notice.
#[rstest]
#[case::test_current_thread(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("test({RUNTIME_KEY} = \"current_thread\")")),
))]
#[case::test_multi_thread(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("test({RUNTIME_KEY} = \"multi_thread\")")),
))]
#[case::main_local(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("main({RUNTIME_KEY} = \"local\")")),
))]
#[case::test_workers_after(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("test({RUNTIME_KEY} = \"multi_thread\", worker_threads = 4)")),
))]
#[case::test_workers_before(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("test(worker_threads = 4, {RUNTIME_KEY} = \"multi_thread\")")),
))]
#[case::test_pause_after(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!(
        "test({RUNTIME_KEY} = \"current_thread\", start_paused = false)"
    )),
))]
#[case::test_pause_before(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!(
        "test(start_paused = true, {RUNTIME_KEY} = \"multi_thread\")"
    )),
))]
#[case::test_panic_policy(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!(
        "test({RUNTIME_KEY} = \"multi_thread\", unhandled_panic = \"ignore\")"
    )),
))]
#[case::test_three_arguments_key_first(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!(
        "test(worker_threads = 2, start_paused = true, {RUNTIME_KEY} = \"multi_thread\")"
    )),
))]
#[case::test_three_arguments_key_middle(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!(
        "test(start_paused = true, worker_threads = 2, {RUNTIME_KEY} = \"multi_thread\")"
    )),
))]
#[case::test_three_arguments_key_last(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!(
        "test({RUNTIME_KEY} = \"multi_thread\", worker_threads = 2, start_paused = true)"
    )),
))]
// A `main` runtime may take a worker pool, so the pattern covers it.
#[case::main_with_worker_pool(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("main({RUNTIME_KEY} = \"multi_thread\", worker_threads = 4)")),
))]
// The escaped form: what the source carries when the attribute is itself
// written inside a Rust string literal.
#[case::test_escaped_quote(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("test({RUNTIME_KEY} = \\\"current_thread\\\")")),
))]
#[case::main_escaped_quote(accepted(
    RUNTIME_KEY,
    tokio_attribute(&format!("main({RUNTIME_KEY} = \\\"local\\\")")),
))]
// The key outside an attribute is the US spelling the dictionary flags.
#[case::key_outside_an_attribute(reported(
    RUNTIME_KEY,
    format!("The {RUNTIME_KEY} of this build is fixed.\n"),
))]
// An attribute's mask must stop at its closing bracket.
#[case::spelling_after_an_attribute(reported(
    CARGO_KIND,
    format!("{} {CARGO_KIND} here\n", tokio_attribute(&format!("test({RUNTIME_KEY} = \"current_thread\")")).trim_end()),
))]
fn tokio_runtime_attributes_are_masked_without_hiding_their_neighbours(
    #[case] probe: Probe,
) -> Result<(), String> {
    check(&probe)
}
