//! End-to-end checks that run the real spelling engine over isolated fixtures.
//!
//! `checks` judges the mask list with its own regular-expression pass, so it
//! is a model of the gate rather than the gate. Typos masks by *span*: a
//! finding is dropped when its span starts or ends inside a match. The model
//! removes matched text instead. Those two rules agree for a whole-line mask
//! and differ for a spelling that only partly overlaps one, so only the engine
//! can say what a contributor's build actually gets.
//!
//! These tests close that gap without a network, a toolchain pin, or a Typos
//! binary on the path. They load the generated `typos.toml` into `typos-cli` —
//! the crate the gate's pinned console script is built from — and run the same
//! `FileChecker` the binary runs, at the version `typos-config-builder` pins.
//! A fixture written to a temporary directory is judged exactly as a tracked
//! file would be.

use std::sync::Mutex;

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use tempfile::TempDir;
use typos_cli::{
    config::Config,
    file::{FileChecker, Typos},
    policy::{ConfigEngine, ConfigStorage},
    report::{Message, Report},
};

use crate::CONFIG;
use crate::probes::tokio_attribute;
use crate::{CARGO_KIND, LINKER_NAME, RUNTIME_KEY};

/// What the engine must make of one fixture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// No finding survives, because an exemption covers the spelling.
    Clean,
    /// The engine reports the spelling the fixture carries.
    Reported,
}

/// One fixture file, the spelling it carries and the outcome it must get.
#[derive(Debug)]
struct Fixture {
    /// A short name, used as the file name and in failure messages.
    name: &'static str,
    /// The spelling the dictionary reports, which the body may or may not mask.
    spelling: &'static str,
    /// The file body handed to the engine, newline included.
    body: String,
    /// What the engine must make of the body.
    outcome: Outcome,
}

/// A fixture whose text an exemption covers.
fn accepted(name: &'static str, spelling: &'static str, body: impl Into<String>) -> Fixture {
    Fixture {
        name,
        spelling,
        body: body.into(),
        outcome: Outcome::Clean,
    }
}

/// A fixture whose text carries the spelling unmasked.
fn reported(name: &'static str, spelling: &'static str, body: impl Into<String>) -> Fixture {
    Fixture {
        name,
        spelling,
        body: body.into(),
        outcome: Outcome::Reported,
    }
}

/// Collects the spellings the engine reports, so a test can assert on them.
#[derive(Default)]
struct Collected {
    /// The reported spellings, in the order the engine produced them.
    spellings: Mutex<Vec<String>>,
}

impl Report for Collected {
    fn report(&self, message: Message<'_>) -> Result<(), std::io::Error> {
        let Message::Typo(typo) = &message else {
            return Ok(());
        };
        if !typo.corrections.is_valid() {
            self.spellings
                .lock()
                .expect("the collected spellings should not be poisoned")
                .push(typo.typo.to_owned());
        }
        Ok(())
    }
}

impl Collected {
    /// The reported spellings, taken out of the shared cell.
    fn take(&self) -> Vec<String> {
        let mut guard = self
            .spellings
            .lock()
            .expect("the collected spellings should not be poisoned");
        std::mem::take(&mut *guard)
    }
}

/// Writes the generated configuration and each fixture into `directory`.
fn write_fixtures(directory: &Dir, fixtures: &[Fixture]) -> Result<(), String> {
    directory
        .write("typos.toml", CONFIG)
        .map_err(|error| format!("typos.toml should be written: {error}"))?;
    for fixture in fixtures {
        directory
            .write(format!("{}.md", fixture.name), &fixture.body)
            .map_err(|error| format!("{}.md should be written: {error}", fixture.name))?;
    }
    Ok(())
}

/// Runs the engine over every fixture, returning the findings per fixture.
fn judge(fixtures: &[Fixture]) -> Result<Vec<Vec<String>>, String> {
    let temporary = TempDir::new().map_err(|error| format!("a work dir should exist: {error}"))?;
    let root = Utf8Path::from_path(temporary.path())
        .ok_or_else(|| format!("{:?} should be UTF-8", temporary.path()))?;
    let directory = Dir::open_ambient_dir(root, ambient_authority())
        .map_err(|error| format!("{root} should open: {error}"))?;
    write_fixtures(&directory, fixtures)?;

    let storage = ConfigStorage::new();
    let mut engine = ConfigEngine::new(&storage);
    // Isolated mode stops the engine walking up to this worktree's own
    // configuration, so the fixture directory states the whole policy.
    engine.set_isolated(true);
    let overrides = Config::from_file(&root.join("typos.toml").into_std_path_buf())
        .map_err(|error| format!("typos.toml should parse: {error}"))?
        .ok_or_else(|| "typos.toml should carry a Typos configuration".to_owned())?;
    engine.set_overrides(overrides);
    engine
        .init_dir(root.as_std_path())
        .map_err(|error| format!("the work dir should initialize: {error}"))?;

    fixtures
        .iter()
        .map(|fixture| judge_one(root, &engine, fixture))
        .collect()
}

/// Runs the engine over one fixture and returns what it reported.
fn judge_one(
    root: &Utf8Path,
    engine: &ConfigEngine<'_>,
    fixture: &Fixture,
) -> Result<Vec<String>, String> {
    let path = root.join(format!("{}.md", fixture.name));
    let native = path.as_std_path().to_path_buf();
    let policy = engine.policy(&native);
    let reporter = Collected::default();
    // An explicit path is checked in full: no binary sniffing and no skip.
    Typos
        .check_file(&native, true, &policy, &reporter)
        .map_err(|error| format!("{} should be checked: {error}", fixture.name))?;
    Ok(reporter.take())
}

/// Every fixture gets the outcome its body calls for.
fn assert_fixtures(fixtures: &[Fixture]) -> Result<(), String> {
    let judged = judge(fixtures)?;
    let disagreements: Vec<String> = fixtures
        .iter()
        .zip(&judged)
        .filter(|(fixture, found)| disagreed(fixture, found))
        .map(|(fixture, found)| {
            format!(
                "{} wanted {:?} for {:?}, engine found {found:?}",
                fixture.name, fixture.outcome, fixture.spelling
            )
        })
        .collect();
    if disagreements.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the engine disagreed with {} fixture(s): {disagreements:#?}",
        disagreements.len()
    ))
}

/// Whether the engine's findings for one fixture contradict its outcome.
fn disagreed(fixture: &Fixture, found: &[String]) -> bool {
    let is_reported = found.iter().any(|spelling| spelling == fixture.spelling);
    is_reported != (fixture.outcome == Outcome::Reported)
}

/// The three external names pass the real engine, and the same spelling in
/// prose, in a code span, or on any other declaration line is still reported.
#[test]
fn the_real_engine_accepts_the_exempt_names_and_reports_their_misspellings() -> Result<(), String> {
    assert_fixtures(&[
        accepted(
            "cargo-kind-span",
            CARGO_KIND,
            format!("Cargo prints `compiler-{CARGO_KIND}` per unit.\n"),
        ),
        accepted(
            "cargo-kind-prose",
            CARGO_KIND,
            format!("The compiler-{CARGO_KIND} kind is stable.\n"),
        ),
        accepted(
            "linker-span",
            LINKER_NAME,
            format!("We link with `{LINKER_NAME}` on Linux.\n"),
        ),
        accepted(
            "linker-phrase",
            LINKER_NAME,
            format!("The {LINKER_NAME} linker ships for Linux alone.\n"),
        ),
        accepted(
            "linker-input",
            LINKER_NAME,
            format!("Pass install-{LINKER_NAME}: true to setup-rust.\n"),
        ),
        accepted(
            "linker-flag",
            LINKER_NAME,
            format!("Rustc receives -fuse-ld={LINKER_NAME} here.\n"),
        ),
        // The negative direction: the same spelling, unmasked.
        reported(
            "cargo-kind-prose-misspelling",
            CARGO_KIND,
            format!("The build {CARGO_KIND} is stale.\n"),
        ),
        reported(
            "cargo-kind-span-misspelling",
            CARGO_KIND,
            format!("A `{CARGO_KIND}` span is still checked.\n"),
        ),
        // The dictionary knows the en-GB adjective but not this US spelling,
        // so the bare name is what proves the mask stops at a word boundary.
        reported(
            "linker-bare-misspelling",
            LINKER_NAME,
            format!("The wheel was {LINKER_NAME}.\n"),
        ),
        reported(
            "runner-key-prose-misspelling",
            RUNTIME_KEY,
            format!("The {RUNTIME_KEY} of this build is fixed.\n"),
        ),
    ])
}

/// The declaration exemptions are anchored to one exact declaration line each,
/// so a differently named, indented, or spelled declaration still reaches the
/// gate.
#[test]
fn the_declaration_exemptions_do_not_reach_a_different_declaration() -> Result<(), String> {
    assert_fixtures(&[
        accepted(
            "anchored-cargo-kind",
            CARGO_KIND,
            format!("const CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
        ),
        accepted(
            "anchored-linker-name",
            LINKER_NAME,
            format!("const LINKER_NAME: &str = \"{LINKER_NAME}\";\n"),
        ),
        accepted(
            "anchored-runner-key",
            RUNTIME_KEY,
            format!("const RUNTIME_KEY: &str = \"{RUNTIME_KEY}\";\n"),
        ),
        // A checkout may convert line endings without losing the exemption; the
        // anchor has to tolerate the carriage return, and still only here.
        accepted(
            "anchored-crlf-declaration",
            CARGO_KIND,
            format!("const CARGO_KIND: &str = \"{CARGO_KIND}\";\r\n"),
        ),
        reported(
            "crlf-declaration-misspelling",
            CARGO_KIND,
            format!("The build {CARGO_KIND} is stale.\r\n"),
        ),
        reported(
            "indented-declaration",
            CARGO_KIND,
            format!("    const CARGO_KIND: &str = \"{CARGO_KIND}\";\n"),
        ),
        reported(
            "renamed-declaration",
            CARGO_KIND,
            format!("const CARGO_MSG: &str = \"{CARGO_KIND}\";\n"),
        ),
        reported(
            "other-linker-declaration",
            LINKER_NAME,
            format!("const OTHER_LINKER: &str = \"{LINKER_NAME}\";\n"),
        ),
        reported(
            "static-declaration",
            RUNTIME_KEY,
            format!("static RUNTIME_KEY: &str = \"{RUNTIME_KEY}\";\n"),
        ),
    ])
}

/// The Tokio attributes the base masks pass the real engine, including the
/// escaped-string form and `main` with a worker pool, and an attribute whose
/// runtime kind the base does not name still reaches the gate.
#[test]
fn the_real_engine_accepts_the_tokio_attributes_the_base_masks() -> Result<(), String> {
    let attribute = |body: String| tokio_attribute(&body);
    assert_fixtures(&[
        accepted(
            "tokio-current-thread",
            RUNTIME_KEY,
            attribute(format!("test({RUNTIME_KEY} = \"current_thread\")")),
        ),
        accepted(
            "tokio-multi-thread",
            RUNTIME_KEY,
            attribute(format!("test({RUNTIME_KEY} = \"multi_thread\")")),
        ),
        accepted(
            "tokio-local-main",
            RUNTIME_KEY,
            attribute(format!("main({RUNTIME_KEY} = \"local\")")),
        ),
        accepted(
            "tokio-main-multi-thread",
            RUNTIME_KEY,
            attribute(format!(
                "main({RUNTIME_KEY} = \"multi_thread\", worker_threads = 4)"
            )),
        ),
        accepted(
            "tokio-workers-before",
            RUNTIME_KEY,
            attribute(format!(
                "test(worker_threads = 4, {RUNTIME_KEY} = \"multi_thread\")"
            )),
        ),
        // The escaped form: what the source carries when the attribute itself
        // is written inside a Rust string literal.
        accepted(
            "tokio-escaped",
            RUNTIME_KEY,
            attribute(format!("test({RUNTIME_KEY} = \\\"current_thread\\\")")),
        ),
        accepted(
            "tokio-escaped-main",
            RUNTIME_KEY,
            attribute(format!("main({RUNTIME_KEY} = \\\"multi_thread\\\")")),
        ),
        reported(
            "tokio-unmasked-kind",
            RUNTIME_KEY,
            attribute(format!("builder({RUNTIME_KEY} = \"current_thread\")")),
        ),
        reported(
            "tokio-prose",
            RUNTIME_KEY,
            format!("The {RUNTIME_KEY} of this build is fixed.\n"),
        ),
    ])
}
