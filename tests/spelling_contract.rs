//! Contract tests for the spelling exemptions this repository keeps.
//!
//! `make spelling` regenerates `typos.toml` from the live shared dictionary and
//! merges `typos.local.toml` into it, so neither file alone states what the gate
//! accepts. That makes the exemptions easy to widen by accident: a blanket word
//! or a mask over every code span keeps the gate green while hiding real
//! misspellings everywhere. These tests pin the effective configuration in both
//! directions. Each exempted name must still be accepted in the context it was
//! added for, and the same spelling in ordinary prose must still be exposed.
//!
//! The tests apply the masks the way the gate does — every `extend-ignore-re`
//! entry is removed from the text before the spelling is judged — using the same
//! regular-expression engine the Typos binary uses. They therefore need no
//! network, no fixed toolchain, and no Typos binary on the path. They check the
//! reach of the exemptions, not the shared dictionary: each probe names a
//! spelling that dictionary reports, so a mask that over-reaches shows up as a
//! probe that should have been exposed and was not.
//!
//! A probe must carry the misspelling it judges, so this file would fail the
//! gate it describes. Each reported spelling is therefore written once, in the
//! three constants below, and the overlay exempts exactly those declaration
//! lines. Every probe interpolates a constant, so the rest of this file — its
//! prose, its code and its attribute fixtures — stays under the gate.

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use regex::Regex;

/// The generated configuration, as the gate reads it.
const CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/typos.toml"));
/// The overlay merged into it, which holds the exemptions this repository owns.
const OVERLAY: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/typos.local.toml"));

/// The spelling the dictionary reports for Cargo's `compiler-artifact` kind.
const CARGO_KIND: &str = "artifact";
/// The spelling the dictionary reports for `mould`, which is the linker's name.
const LINKER_NAME: &str = "mold";
/// The spelling the dictionary reports for `flavour`, which is Tokio's key name.
const RUNTIME_KEY: &str = "flavor";

/// What the masks must do with one probe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// An exemption reaches this text, so nothing is left to report.
    Accepted,
    /// The spelling survives the masks and the gate reports it.
    Reported,
}

/// One line of text and the verdict the exemptions must give it.
#[derive(Clone, Debug)]
struct Probe {
    /// The spelling the dictionary reports, which the text may or may not mask.
    spelling: &'static str,
    /// The text handed to the masks, newline included.
    text: String,
    /// What the masks must leave behind.
    verdict: Verdict,
}

/// A probe whose text an exemption covers.
fn accepted(spelling: &'static str, text: impl Into<String>) -> Probe {
    Probe {
        spelling,
        text: text.into(),
        verdict: Verdict::Accepted,
    }
}

/// A probe whose text carries the spelling unmasked.
fn reported(spelling: &'static str, text: impl Into<String>) -> Probe {
    Probe {
        spelling,
        text: text.into(),
        verdict: Verdict::Reported,
    }
}

/// The effective mask list, read out of the generated configuration.
///
/// Both files are TOML and the shared base writes multi-line patterns, so the
/// list is read as data rather than pattern-matched out of the file. A file that
/// does not parse, or that carries no list, is an error: an unreadable
/// configuration must never read as "there are no exemptions".
fn masks() -> Result<Vec<String>, String> {
    let parsed: toml::Table = CONFIG
        .parse()
        .map_err(|error| format!("typos.toml should parse: {error}"))?;
    let entries = parsed
        .get("default")
        .and_then(|default| default.get("extend-ignore-re"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "typos.toml should carry default.extend-ignore-re".to_owned())?;
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("an ignore-re entry should be a string: {entry}"))
        })
        .collect()
}

/// Removes every masked span from one probe, as the gate does before judging it.
fn strip_masks(text: &str, patterns: &[Regex]) -> String {
    patterns.iter().fold(text.to_owned(), |masked, pattern| {
        pattern.replace_all(&masked, "").into_owned()
    })
}

/// Compiles the mask list, reporting the first entry that does not compile.
fn compiled_masks() -> Result<Vec<Regex>, String> {
    masks()?
        .iter()
        .map(|pattern| {
            Regex::new(pattern)
                .map_err(|error| format!("exemption {pattern:?} should compile: {error}"))
        })
        .collect()
}

/// Checks every probe against the masks and names the ones that disagreed.
fn check(probes: &[Probe]) -> Result<(), String> {
    let patterns = compiled_masks()?;
    let found = disagreements(&patterns, probes);
    if found.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the exemptions disagreed with {} probe(s): {found:#?}",
        found.len()
    ))
}

/// The probes whose text did not survive the masks with the wanted verdict.
fn disagreements(patterns: &[Regex], probes: &[Probe]) -> Vec<String> {
    probes
        .iter()
        .filter(|probe| {
            strip_masks(&probe.text, patterns).contains(probe.spelling)
                != (probe.verdict == Verdict::Reported)
        })
        .map(|probe| {
            format!(
                "{:?} wanted {:?} for {:?}",
                probe.text, probe.verdict, probe.spelling
            )
        })
        .collect()
}

/// The external names this repository exempts, accepted where they belong and
/// exposed as ordinary prose elsewhere.
fn local_probes() -> Vec<Probe> {
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

/// A complete Tokio runtime attribute carrying `body`.
fn tokio_attribute(body: &str) -> String {
    format!("#[tokio::{body}]\n")
}

/// The Tokio runtime attributes the shared base masks, declared as data.
///
/// Three runtime arguments is the bound the base's own comment states, so each
/// arrangement of three appears once. Without these probes the generated
/// pattern could lose an arrangement and only a later contributor's build would
/// notice.
fn tokio_probes() -> Vec<Probe> {
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

/// The words a generated line may carry outside the attribute.
const PROSE_WORDS: &[&str] = &[
    "the", "build", "is", "fixed", "and", "an", "appears", "here",
];

/// One whitespace run the masks must absorb, as `\s*` does.
fn spacing() -> impl Strategy<Value = String> {
    proptest::sample::select(vec!["", " ", "\t"]).prop_map(str::to_owned)
}

/// The runtime arguments other than the flavour key that a runtime accepts.
fn generated_argument() -> impl Strategy<Value = String> {
    let panic_policy = (
        proptest::sample::select(vec!["shutdown_runtime", "ignore"]),
        any::<bool>(),
    )
        .prop_map(|(policy, is_escaped)| {
            let quote = if is_escaped { "\\\"" } else { "\"" };
            format!("unhandled_panic = {quote}{policy}{quote}")
        });
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
    )
        .prop_map(|((kind, flavour), mut arguments, lead, position, trail)| {
            if flavour == "local" {
                arguments.clear();
            }
            let key = format!("{RUNTIME_KEY}{lead}={lead}\"{flavour}\"");
            arguments.insert(position.index(arguments.len() + 1), key);
            format!("#[{lead}tokio::{kind}({}){trail}]", arguments.join(", "))
        })
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

/// Cases the generated property checks.
const GENERATED_CASES: u32 = 256;

/// Generated runtime attributes are masked whole, their surroundings are not.
///
/// The table above pins the arrangements the shared base documents. This
/// property asks the wider question the table cannot: for any attribute that
/// grammar claims to accept, with any spacing, any argument order and either
/// quote form, is the whole attribute swallowed and nothing beyond it? The
/// malformed variant checks the other edge: an attribute whose closing bracket
/// is missing must not be masked at all.
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

/// `typos.toml` is generated, so a stale copy would leave these tests judging a
/// configuration the gate never uses. Every overlay entry must reach the output.
#[test]
fn the_overlay_reaches_the_generated_configuration() -> Result<(), String> {
    let generated = masks()?;
    let missing: Vec<&str> = overlay_entries()
        .into_iter()
        .filter(|entry| !generated.iter().any(|mask| mask == entry))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "typos.toml lacks overlay entries {missing:?}; run make spelling"
    ))
}

/// The single-quoted entries of the overlay, which are the patterns it adds.
fn overlay_entries() -> Vec<&'static str> {
    OVERLAY
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('\''))
        .filter_map(|line| line.strip_prefix('\''))
        .filter_map(|line| line.strip_suffix('\'').or_else(|| line.strip_suffix("',")))
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// The configuration files, paired with the copies compiled into this test.
const CONFIGURATIONS: [(&str, &str); 2] = [("typos.toml", CONFIG), ("typos.local.toml", OVERLAY)];

/// Reads one configuration file through a capability handle rooted at the
/// manifest directory.
fn read_configuration(name: &str) -> Result<String, String> {
    let manifest = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = Dir::open_ambient_dir(manifest, ambient_authority())
        .map_err(|error| format!("{manifest} should open: {error}"))?;
    dir.read_to_string(name)
        .map_err(|error| format!("{manifest}/{name} should read: {error}"))
}

/// The configuration files on disk must be the ones compiled into this test, so
/// a failure above always names the file a contributor should regenerate.
#[test]
fn the_configuration_files_are_the_ones_the_gate_reads() -> Result<(), String> {
    let mut drifted: Vec<&str> = Vec::new();
    for (name, built_in) in CONFIGURATIONS {
        if read_configuration(name)? != built_in {
            drifted.push(name);
        }
    }
    if drifted.is_empty() {
        return Ok(());
    }
    Err(format!(
        "these files differ from the tested copies: {drifted:?}"
    ))
}
