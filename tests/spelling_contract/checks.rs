//! The model half of the contract: judge the mask list with `regex` alone.
//!
//! Typos masks by span, but the model removes matches and looks for what is
//! left. For a whole-line or whole-attribute mask the two agree, and this half
//! stays useful because it names a lost exemption directly, without starting
//! the engine. `gate` is what proves the two rules agree.

use regex::Regex;

use crate::configuration::masks;

/// What the masks must do with one probe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// An exemption reaches this text, so nothing is left to report.
    Accepted,
    /// The spelling survives the masks and the gate reports it.
    Reported,
}

/// One line of text and the verdict the exemptions must give it.
#[derive(Clone, Debug)]
pub struct Probe {
    /// The spelling the dictionary reports, which the text may or may not mask.
    pub spelling: &'static str,
    /// The text handed to the masks, newline included.
    pub text: String,
    /// What the masks must leave behind.
    pub verdict: Verdict,
}

/// A probe whose text an exemption covers.
pub fn accepted(spelling: &'static str, text: impl Into<String>) -> Probe {
    Probe {
        spelling,
        text: text.into(),
        verdict: Verdict::Accepted,
    }
}

/// A probe whose text carries the spelling unmasked.
pub fn reported(spelling: &'static str, text: impl Into<String>) -> Probe {
    Probe {
        spelling,
        text: text.into(),
        verdict: Verdict::Reported,
    }
}

/// Compiles the mask list, reporting the first entry that does not compile.
pub fn compiled_masks() -> Result<Vec<Regex>, String> {
    masks()?
        .iter()
        .map(|pattern| {
            Regex::new(pattern)
                .map_err(|error| format!("exemption {pattern:?} should compile: {error}"))
        })
        .collect()
}

/// Removes every masked span from one probe, as this model judges it.
pub fn strip_masks(text: &str, patterns: &[Regex]) -> String {
    patterns.iter().fold(text.to_owned(), |masked, pattern| {
        pattern.replace_all(&masked, "").into_owned()
    })
}

/// Checks one probe against the masks, naming it if the verdict was wrong.
///
/// A single probe per call is what lets the `rstest` cases fail one at a time:
/// the message names the text that disagreed, not a count over a table.
pub fn check(probe: &Probe) -> Result<(), String> {
    let patterns = compiled_masks()?;
    let survives = strip_masks(&probe.text, &patterns).contains(probe.spelling);
    if survives != (probe.verdict == Verdict::Reported) {
        return Err(format!(
            "{:?} wanted {:?} for {:?}",
            probe.text, probe.verdict, probe.spelling
        ));
    }
    Ok(())
}
