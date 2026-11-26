//! Contract for the mutation exclusions this repository owns.
//!
//! `.cargo/mutants.toml` decides which mutants are never tested, so an entry
//! that is too wide hides survivors that should have been caught and an entry
//! that is too narrow leaves the hang it existed to remove. These tests pin the
//! narrow entry directly: they compile every `exclude_re` with the `regex`
//! crate, locate the end-of-file exclusion inside `src/sync/types.rs`'s
//! `forward_stream`, and probe it with the observed mutant name and with names
//! that must not match.
//!
//! The probes are a model of cargo-mutants' selection, not the tool itself. A
//! regex that satisfies this contract still has to be shown live, by comparing
//! `cargo mutants --list --json` before and after the entry; that is the check
//! that proves cargo-mutants selects exactly the mutant this contract says it
//! does, and it is recorded in the pull request that added the entry. Nothing
//! here can replace it, because the selection is cargo-mutants' own behaviour.

use regex::Regex;

/// The configuration this contract reads.
const CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.cargo/mutants.toml"));

/// The function whose end-of-file mutant is excluded.
const FORWARD_STREAM: &str = "forward_stream";

/// The file that defines it, as cargo-mutants names it in a mutant name.
const TYPES_FILE: &str = "src/sync/types.rs";

/// The operator replacement the exclusion exists for.
const EOF_REPLACEMENT: &str = "replace == with != in forward_stream";

/// A target name that differs from the observed one only in position.
///
/// cargo-mutants regenerates names from the current source, so line and column
/// move whenever anything above the check changes. The exclusion must survive
/// that, or the next unrelated edit silently restores the hang.
const SAME_MUTANT_ELSEWHERE: &str =
    "src/sync/types.rs:412:33: replace == with != in forward_stream";

/// The single exclusion that must match `SAME_MUTANT_ELSEWHERE`, found by its
/// effect rather than by its text so a rewrite cannot satisfy it by name alone.
fn eof_exclusion() -> Result<Regex, String> {
    let patterns = exclusion_patterns()?;
    let mut matching = patterns.iter().filter_map(|pattern| {
        Regex::new(pattern)
            .ok()
            .filter(|regex| regex.is_match(SAME_MUTANT_ELSEWHERE))
    });

    let exclusion = matching.next().ok_or_else(|| {
        format!(
            "no exclude_re entry matches `{SAME_MUTANT_ELSEWHERE}`; the forward_stream \
             end-of-file exclusion is missing or no longer tolerates a moved check"
        )
    })?;
    if matching.next().is_some() {
        return Err(format!(
            "more than one exclude_re entry matches `{SAME_MUTANT_ELSEWHERE}`, so the \
             forward_stream end-of-file exclusion is ambiguous"
        ));
    }
    Ok(exclusion)
}

/// Every `exclude_re` entry, read as TOML rather than pattern-matched out of
/// the file.
///
/// A file that does not parse, or that carries no list, is an error: an
/// unreadable configuration must never read as "nothing is excluded".
fn exclusion_patterns() -> Result<Vec<String>, String> {
    let document = CONFIG
        .parse::<toml::Table>()
        .map_err(|error| format!(".cargo/mutants.toml should parse: {error}"))?;
    document
        .get("exclude_re")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| ".cargo/mutants.toml should carry exclude_re".to_owned())?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("an exclude_re entry should be a string: {entry}"))
        })
        .collect()
}

#[test]
fn every_exclusion_compiles() -> Result<(), String> {
    let invalid: Vec<String> = exclusion_patterns()?
        .into_iter()
        .filter_map(|pattern| match Regex::new(&pattern) {
            Ok(_) => None,
            Err(error) => Some(format!("`{pattern}`: {error}")),
        })
        .collect();

    if invalid.is_empty() {
        Ok(())
    } else {
        Err(format!(
            ".cargo/mutants.toml carries patterns cargo-mutants cannot compile: {invalid:#?}"
        ))
    }
}

/// The entry exists, tolerates a moved check, and matches the mutant exactly.
#[rstest::rstest]
#[case::observed_name(format!("{TYPES_FILE}:125:21: {EOF_REPLACEMENT}"))]
#[case::another_line(format!("{TYPES_FILE}:60:21: {EOF_REPLACEMENT}"))]
#[case::another_column(format!("{TYPES_FILE}:125:9: {EOF_REPLACEMENT}"))]
#[case::same_mutant_elsewhere(SAME_MUTANT_ELSEWHERE.to_owned())]
fn the_exclusion_matches_the_end_of_file_mutant(#[case] name: String) -> Result<(), String> {
    let exclusion = eof_exclusion()?;
    if exclusion.is_match(&name) {
        Ok(())
    } else {
        Err(format!(
            "the forward_stream end-of-file exclusion does not match `{name}`"
        ))
    }
}

/// The exclusion reaches nothing else.
///
/// Each probe is a real neighbour from the baseline `cargo mutants --list`
/// output, apart from the operator probe: 27.1.0 generates no other operator
/// mutant in `forward_stream`, so that probe borrows the operator-mutant name
/// shape this repository does produce. A pattern anchored only on the operator,
/// only on the function, or only on the file would match one of these and
/// silently stop reporting a survivor.
#[rstest::rstest]
#[case::other_operator_in_forward_stream(format!(
    "{TYPES_FILE}:125:21: replace < with <= in {FORWARD_STREAM}"
))]
#[case::function_body_replacement(format!(
    "{TYPES_FILE}:106:5: replace {FORWARD_STREAM} -> thread::JoinHandle<Result<String, SyncError>> with JoinHandle::new()"
))]
#[case::same_operator_in_another_function(String::from(
    "src/scaleway/mod.rs:69:38: replace == with != in ScalewayBackend::is_instance_type_error"
))]
#[case::same_text_in_another_file(format!("src/sync/other.rs:125:21: {EOF_REPLACEMENT}"))]
#[case::same_text_with_a_longer_function_name(format!(
    "{TYPES_FILE}:125:21: {EOF_REPLACEMENT}_extra"
))]
#[case::same_text_with_a_prefix(format!(
    "prefix {TYPES_FILE}:125:21: {EOF_REPLACEMENT}"
))]
fn the_exclusion_reaches_nothing_else(#[case] name: String) -> Result<(), String> {
    let exclusion = eof_exclusion()?;
    if exclusion.is_match(&name) {
        Err(format!(
            "the forward_stream end-of-file exclusion also matches `{name}`"
        ))
    } else {
        Ok(())
    }
}
