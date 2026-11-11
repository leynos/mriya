//! The routing half of the build-tools contract: which Make routes reach the preflight, in what
//! order, and which stay clear of it. It reads what `make -n -B <target> CARGO=probe-cargo` prints,
//! so a Cargo command that hard-codes `cargo` instead of `$(CARGO)` shows up, and a prerequisite
//! that went missing shows up as a preflight that is absent or comes after the first Cargo command.

use super::{
    config::Problems,
    make::{Host, MakeRunner, Target},
    shell::{names_program, shell_commands, without_leading_keywords},
};

/// The executable the dry run substitutes for `$(CARGO)`.
const PROBE: &str = "probe-cargo";
/// The command the preflight prerequisite runs.
const PREFLIGHT: &str = "scripts/check-build-tools.sh";

/// Routes that compile under the standard's linker and so must pass the preflight before Cargo
/// runs. The list is this repository's own: a route that stops being defined fails the contract.
pub const PREFLIGHT_ROUTES: &[&str] = &[
    "build",
    "test",
    "lint",
    "typecheck",
    "scaleway-test",
    "scaleway-janitor",
];

/// Routes that stay off the standard and so must not demand its tools.
pub const EXCLUDED_ROUTES: &[&str] = &["release"];

/// Appends the word being built, when it holds anything, and leaves it empty.
fn take_word(found: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        found.push(std::mem::take(current));
    }
}

/// Splits a command into words at whitespace outside quotes, keeping each quoted span in one word.
fn words(command: impl AsRef<str>) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in command.as_ref().chars() {
        if let Some(open) = quote {
            current.push(c);
            quote = (c != open).then_some(open);
            continue;
        }
        if matches!(c, '\'' | '"') {
            current.push(c);
            quote = Some(c);
            continue;
        }
        if c.is_whitespace() {
            take_word(&mut found, &mut current);
            continue;
        }
        current.push(c);
    }
    take_word(&mut found, &mut current);
    found
}

/// Returns whether a word is a `NAME=value` environment assignment.
fn is_assignment(word: impl AsRef<str>) -> bool {
    let text = word.as_ref();
    text.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Returns the executable a command runs: its first word that is not an environment assignment.
///
/// ```text
/// executable("RUSTFLAGS=\"-D warnings\" probe-cargo test") == Some("probe-cargo")
/// executable("scripts/check-build-tools.sh")              == Some("scripts/check-build-tools.sh")
/// ```
fn executable(command: impl AsRef<str>) -> Option<String> {
    words(command).into_iter().find(|word| !is_assignment(word))
}

/// Returns every command `make -n` printed, one per entry, in the order Make printed them.
fn commands(stdout: impl AsRef<str>) -> Vec<String> {
    stdout
        .as_ref()
        .replace("\\\n", " ")
        .lines()
        .flat_map(shell_commands)
        .map(|command| without_leading_keywords(&command).trim().to_owned())
        .filter(|command| !command.is_empty())
        .collect()
}

/// Returns whether a command is the preflight itself.
fn is_preflight(command: impl AsRef<str>) -> bool {
    executable(command).as_deref() == Some(PREFLIGHT)
}

/// Returns whether a command runs the substituted Cargo.
fn runs_probe(command: impl AsRef<str>) -> bool { executable(command).as_deref() == Some(PROBE) }

/// Returns whether any word of the output names a real `cargo` once the probe is taken out, which
/// means some invocation ignored `$(CARGO)`, including one inside a quoted `trap` or `-c` string.
fn names_unsubstituted_cargo(stdout: impl AsRef<str>) -> bool {
    stdout
        .as_ref()
        .replace(PROBE, "")
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '\\')))
        .any(|word| names_program(word, "cargo"))
}

/// Returns the complaints about one route, judged from the commands `make -n` printed for it.
///
/// A route that must pass the preflight needs the preflight to be printed, and before the first
/// Cargo command. Every route needs at least one Cargo command, so the check cannot read nothing
/// and pass, and none may hard-code `cargo`.
pub fn route_problems(
    target: impl AsRef<str>,
    stdout: impl AsRef<str>,
    needs_preflight: bool,
) -> Problems {
    let route = target.as_ref();
    let printed = commands(stdout.as_ref());
    let preflight_at = printed.iter().position(is_preflight);
    let first_cargo_at = printed.iter().position(runs_probe);
    let mut problems = Problems::new();
    if first_cargo_at.is_none() {
        problems.push(format!(
            "`make {route}` prints no {PROBE} command, so the route reads nothing"
        ));
    }
    if names_unsubstituted_cargo(stdout) {
        problems.push(format!(
            "`make {route}` runs a `cargo` that ignores $(CARGO)"
        ));
    }
    match (needs_preflight, preflight_at, first_cargo_at) {
        (true, None, _) => problems.push(format!(
            "`make {route}` never reaches the {PREFLIGHT} preflight"
        )),
        (true, Some(preflight), Some(cargo)) if cargo < preflight => problems.push(format!(
            "`make {route}` runs {PROBE} (command {cargo}) before the preflight (command \
             {preflight})"
        )),
        (false, Some(_), _) => problems.push(format!(
            "`make {route}` reaches the {PREFLIGHT} preflight, which this route must not require"
        )),
        _ => {}
    }
    problems
}

/// Returns the complaints about every preflight route and every excluded route, on one host, and
/// how many routes it read. Each route's own text is fetched through `runner`.
///
/// # Errors
///
/// Returns the reason when a listed route is not defined or `make` cannot run.
pub fn routing_problems(runner: MakeRunner, host: Host) -> Result<(Problems, usize), String> {
    let mut problems = Problems::new();
    let mut read = 0;
    for (routes, needs_preflight) in [(PREFLIGHT_ROUTES, true), (EXCLUDED_ROUTES, false)] {
        for name in routes {
            let stdout = runner(Target(name), host)?;
            problems.extend(route_problems(name, stdout, needs_preflight));
            read += 1;
        }
    }
    Ok((problems, read))
}
