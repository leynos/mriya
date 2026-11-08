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

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use regex::Regex;

/// The generated configuration, as the gate reads it.
const CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/typos.toml"));
/// The overlay merged into it, which holds the exemptions this repository owns.
const OVERLAY: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/typos.local.toml"));

/// What the masks must do with one probe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// An exemption reaches this text, so nothing is left to report.
    Accepted,
    /// The spelling survives the masks and the gate reports it.
    Reported,
}

/// One line of text and the verdict the exemptions must give it.
#[derive(Clone, Copy, Debug)]
struct Probe {
    /// The spelling the dictionary reports, which the text may or may not mask.
    spelling: &'static str,
    /// The text handed to the masks, newline included.
    text: &'static str,
    /// What the masks must leave behind.
    verdict: Verdict,
}

impl Probe {
    /// A probe whose text an exemption covers.
    const fn accepted(spelling: &'static str, text: &'static str) -> Self {
        Self {
            spelling,
            text,
            verdict: Verdict::Accepted,
        }
    }

    /// A probe whose text carries the spelling unmasked.
    const fn reported(spelling: &'static str, text: &'static str) -> Self {
        Self {
            spelling,
            text,
            verdict: Verdict::Reported,
        }
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
    let disagreements: Vec<String> = probes
        .iter()
        .filter(|probe| {
            strip_masks(probe.text, &patterns).contains(probe.spelling)
                != (probe.verdict == Verdict::Reported)
        })
        .map(|probe| {
            format!(
                "{:?} wanted {:?} for {:?}",
                probe.text, probe.verdict, probe.spelling
            )
        })
        .collect();
    if disagreements.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the exemptions disagreed with {} probe(s): {disagreements:#?}",
        disagreements.len()
    ))
}

/// The external names this repository exempts, accepted where they belong and
/// exposed as ordinary prose elsewhere.
const LOCAL_PROBES: &[Probe] = &[
    Probe::accepted("artifact", "Cargo prints `compiler-artifact` per unit.\n"),
    Probe::accepted("artifact", "The compiler-artifact kind is stable.\n"),
    Probe::accepted("mold", "We link with `mold` on Linux.\n"),
    Probe::accepted("mold", "The mold linker ships for Linux alone.\n"),
    Probe::accepted("mold", "Pass install-mold: true to setup-rust.\n"),
    Probe::accepted("mold", "Rustc receives -fuse-ld=mold here.\n"),
    Probe::accepted("mold", "The reader probes value.contains(\"mold\") once.\n"),
    Probe::reported("artifact", "The build artifact is stale.\n"),
    Probe::reported("artifact", "A `artifact` span is still checked.\n"),
    Probe::reported("mold", "The moldy bread was thrown out.\n"),
    Probe::reported("mold", "The wheel was mold."),
];

/// The exemptions this repository owns hold in both directions.
#[test]
fn local_exemptions_accept_the_exact_name_and_reject_the_misspelling() -> Result<(), String> {
    check(LOCAL_PROBES)
}

/// The Tokio runtime attributes the shared base masks, declared as data.
///
/// Three runtime arguments is the bound the base's own comment states, so each
/// arrangement of three appears once. Without these probes the generated
/// pattern could lose an arrangement and only a later contributor's build would
/// notice.
const TOKIO_PROBES: &[Probe] = &[
    Probe::accepted("flavor", "#[tokio::test(flavor = \"current_thread\")]\n"),
    Probe::accepted("flavor", "#[tokio::test(flavor = \"multi_thread\")]\n"),
    Probe::accepted("flavor", "#[tokio::main(flavor = \"local\")]\n"),
    Probe::accepted(
        "flavor",
        "#[tokio::test(flavor = \"multi_thread\", worker_threads = 4)]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(worker_threads = 4, flavor = \"multi_thread\")]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(flavor = \"current_thread\", start_paused = false)]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(start_paused = true, flavor = \"multi_thread\")]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(flavor = \"multi_thread\", unhandled_panic = \"ignore\")]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(worker_threads = 2, start_paused = true, flavor = \"multi_thread\")]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(start_paused = true, worker_threads = 2, flavor = \"multi_thread\")]\n",
    ),
    Probe::accepted(
        "flavor",
        "#[tokio::test(flavor = \"multi_thread\", worker_threads = 2, start_paused = true)]\n",
    ),
];

/// The generated Tokio masks cover the argument shapes a runtime attribute
/// takes, and stop at the closing bracket.
#[test]
fn tokio_runtime_attributes_are_masked_without_hiding_their_neighbours() -> Result<(), String> {
    check(TOKIO_PROBES)?;
    check(&[
        // The key outside an attribute is the US spelling the dictionary flags.
        Probe::reported("flavor", "The flavor of this build is fixed.\n"),
        // An attribute's mask must stop at its closing bracket.
        Probe::reported(
            "artifact",
            "#[tokio::test(flavor = \"current_thread\")] artifact here\n",
        ),
    ])
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
