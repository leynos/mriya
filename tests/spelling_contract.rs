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
//! The contract has two halves, and they answer different questions.
//!
//! `checks` judges the generated mask list directly: it compiles every
//! `extend-ignore-re` entry with the `regex` crate and removes the matches
//! before looking for the spelling. That is a model of the gate, and a fast
//! one, but a model can drift from the engine.
//!
//! `gate` closes that gap. It loads the same generated configuration into
//! `typos-cli`, the crate the gate's pinned console script is built from, at
//! the version `typos-config-builder` pins, and runs the same `FileChecker` the
//! binary runs over isolated fixture files. Neither half needs a network, a
//! fixed toolchain, or a Typos binary on the path.
//!
//! Between them the two halves check the reach of the exemptions, not the
//! shared dictionary: each probe names a spelling that dictionary reports, so a
//! mask that over-reaches shows up as a probe that should have been exposed and
//! was not.
//!
//! A probe must carry the misspelling it judges, so this module would fail the
//! gate it describes. Each reported spelling is therefore written once, in the
//! three constants below, and the overlay exempts exactly those declaration
//! lines. Every probe and every fixture body interpolates a constant, so the
//! rest of these files — their prose, their code and their attribute fixtures —
//! stays under the gate.

// This file is a crate root, so `mod x;` would look beside it rather than in a
// directory of the same name. Each module names its file explicitly, as the
// repository's other integration tests do.
#[path = "spelling_contract/checks.rs"]
mod checks;
#[path = "spelling_contract/configuration.rs"]
mod configuration;
#[path = "spelling_contract/gate.rs"]
mod gate;
#[path = "spelling_contract/probes.rs"]
mod probes;
#[path = "spelling_contract/property.rs"]
mod property;

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
