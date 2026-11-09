//! The exemptions this repository owns, judged in both directions.

use crate::CARGO_KIND;
use crate::LINKER_NAME;
use crate::RUNTIME_KEY;
use crate::checks::{Probe, accepted, check, reported};

/// The external names this repository exempts, accepted where they belong and
/// exposed as ordinary prose elsewhere.
pub fn local_probes() -> Vec<Probe> {
    vec![
        accepted(
            CARGO_KIND,
            format!("Cargo prints `compiler-{CARGO_KIND}` per unit.\n"),
        ),
        accepted(
            CARGO_KIND,
            format!("The compiler-{CARGO_KIND} kind is stable.\n"),
        ),
        accepted(
            LINKER_NAME,
            format!("We link with `{LINKER_NAME}` on Linux.\n"),
        ),
        accepted(
            LINKER_NAME,
            format!("The {LINKER_NAME} linker ships for Linux alone.\n"),
        ),
        accepted(
            LINKER_NAME,
            format!("Pass install-{LINKER_NAME}: true to setup-rust.\n"),
        ),
        accepted(
            LINKER_NAME,
            format!("Rustc receives -fuse-ld={LINKER_NAME} here.\n"),
        ),
        accepted(
            LINKER_NAME,
            format!("The reader probes value.contains(\"{LINKER_NAME}\") once.\n"),
        ),
        reported(CARGO_KIND, format!("The build {CARGO_KIND} is stale.\n")),
        // Inline code is no longer masked, so this span must reach the gate.
        reported(
            CARGO_KIND,
            format!("A `{CARGO_KIND}` span is still checked.\n"),
        ),
        reported(
            LINKER_NAME,
            format!("The {LINKER_NAME}y bread was thrown out.\n"),
        ),
        reported(LINKER_NAME, format!("The wheel was {LINKER_NAME}.")),
    ]
}

/// The exemptions this repository owns hold in both directions.
#[test]
fn local_exemptions_accept_the_exact_name_and_reject_the_misspelling() -> Result<(), String> {
    check(&local_probes())
}

/// The declaration exemptions this contract file needs for its own constants.
///
/// Each is anchored to one exact declaration line, so the positive probes are
/// those lines and the negative probes are the same spelling on any other
/// line: a different name, a different indent, a different keyword, or the
/// same text in prose. Widening one anchor therefore fails here.
pub fn declaration_probes() -> Vec<Probe> {
    vec![
        accepted(
            CARGO_KIND,
            format!("const CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
        ),
        accepted(
            LINKER_NAME,
            format!("const LINKER_NAME: &str = \"{LINKER_NAME}\";\n"),
        ),
        accepted(
            RUNTIME_KEY,
            format!("const RUNTIME_KEY: &str = \"{RUNTIME_KEY}\";\n"),
        ),
        reported(
            CARGO_KIND,
            format!("    const CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
        ),
        reported(
            CARGO_KIND,
            format!("const CARGO_MSG: &str = \"{CARGO_KIND}\";\n"),
        ),
        reported(
            CARGO_KIND,
            format!("static CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
        ),
        reported(
            LINKER_NAME,
            format!("const OTHER_LINKER: &str = \"{LINKER_NAME}\";\n"),
        ),
        reported(
            RUNTIME_KEY,
            format!("the runtime key is {RUNTIME_KEY} here.\n"),
        ),
    ]
}

/// The declaration exemptions reach their own line and nothing around it.
#[test]
fn declaration_exemptions_are_anchored_to_their_exact_lines() -> Result<(), String> {
    check(&declaration_probes())
}

/// A complete Tokio runtime attribute carrying `body`.
pub fn tokio_attribute(body: &str) -> String {
    format!("#[tokio::{body}]\n")
}

/// The Tokio runtime attributes the shared base masks, declared as data.
///
/// Three runtime arguments is the bound the base's own comment states, so each
/// arrangement of three appears once. The escaped-string form and `main` with a
/// worker pool are here because the shared base's pattern spells them out and
/// nothing else exercises them. Without these probes the generated pattern
/// could lose an arrangement and only a later contributor's build would notice.
pub fn tokio_probes() -> Vec<Probe> {
    let key = RUNTIME_KEY;
    let probe = |body: String| accepted(RUNTIME_KEY, tokio_attribute(&body));
    vec![
        probe(format!("test({key} = \"current_thread\")")),
        probe(format!("test({key} = \"multi_thread\")")),
        probe(format!("main({key} = \"local\")")),
        probe(format!(
            "test({key} = \"multi_thread\", worker_threads = 4)"
        )),
        probe(format!(
            "test(worker_threads = 4, {key} = \"multi_thread\")"
        )),
        probe(format!(
            "test({key} = \"current_thread\", start_paused = false)"
        )),
        probe(format!(
            "test(start_paused = true, {key} = \"multi_thread\")"
        )),
        probe(format!(
            "test({key} = \"multi_thread\", unhandled_panic = \"ignore\")"
        )),
        probe(format!(
            "test(worker_threads = 2, start_paused = true, {key} = \"multi_thread\")"
        )),
        probe(format!(
            "test(start_paused = true, worker_threads = 2, {key} = \"multi_thread\")"
        )),
        probe(format!(
            "test({key} = \"multi_thread\", worker_threads = 2, start_paused = true)"
        )),
        // A `main` runtime may take a worker pool, so the pattern covers it.
        probe(format!(
            "main({key} = \"multi_thread\", worker_threads = 4)"
        )),
        // The escaped form: what the source carries when the attribute is
        // itself written inside a Rust string literal.
        probe(format!("test({key} = \\\"current_thread\\\")")),
        probe(format!("main({key} = \\\"local\\\")")),
    ]
}

/// The generated Tokio masks cover the argument shapes a runtime attribute
/// takes, and stop at the closing bracket.
#[test]
fn tokio_runtime_attributes_are_masked_without_hiding_their_neighbours() -> Result<(), String> {
    check(&tokio_probes())?;
    let attribute_line = tokio_attribute(&format!("test({RUNTIME_KEY} = \"current_thread\")"));
    check(&[
        // The key outside an attribute is the US spelling the dictionary flags.
        reported(
            RUNTIME_KEY,
            format!("The {RUNTIME_KEY} of this build is fixed.\n"),
        ),
        // An attribute's mask must stop at its closing bracket.
        reported(
            CARGO_KIND,
            format!("{} {CARGO_KIND} here\n", attribute_line.trim_end()),
        ),
    ])
}
