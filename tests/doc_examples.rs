//! Loads the marked examples from the user guide and exercises them.
//!
//! The documentation style guide requires every fenced example in a
//! user-facing document to carry a `<!-- tested-example: <id> -->` marker and
//! to be loaded from the published document rather than copied into a fixture.
//! This suite is that loader: it rejects an unmarked, unterminated or
//! duplicated example, then checks the TOML examples against the real
//! configuration types and the shell examples with `bash -n`.

use std::{collections::BTreeSet, process::Command};

use camino::Utf8Path;
use cap_std::{ambient_authority, fs_utf8::Dir};
use mriya::{InitConfig, ScalewayBackendError, ScalewayConfig, volume::MIN_VOLUME_SIZE_GB};
use tempfile::TempDir;

/// The user-facing document whose examples are contracted by this suite.
const USER_GUIDE: &str = "docs/users-guide.md";

/// A fence delimiter: the marker character and its run length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Delimiter {
    /// Marker character, either a backtick or a tilde.
    marker: char,
    /// Number of markers in the run.
    length: usize,
}

/// Returns the delimiter when `line` is a fence line.
fn delimiter(line: &str) -> Option<Delimiter> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = trimmed.chars().take_while(|value| *value == marker).count();
    (length >= 3).then_some(Delimiter { marker, length })
}

/// Reports whether `line` closes a fence opened with `opening`.
fn closes(line: &str, opening: Delimiter) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.chars().all(|value| value == opening.marker)
        && trimmed.chars().count() >= opening.length
}

/// One marked fenced example from a document.
#[derive(Debug)]
struct Fence {
    /// Stable identifier from the preceding marker.
    id: String,
    /// Info string following the opening fence, such as `bash`.
    language: String,
    /// Text between the fences.
    body: String,
}

/// Reads and parses the marked examples in `path`.
fn load(path: &Utf8Path) -> Result<Vec<Fence>, String> {
    let parent = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| format!("{path} has no file name"))?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())
        .map_err(|error| format!("opening {parent}: {error}"))?;
    let text = dir
        .read_to_string(name)
        .map_err(|error| format!("reading {path}: {error}"))?;
    parse(&text)
}

/// Parses marked fences out of `text`, rejecting every marker violation.
fn parse(text: &str) -> Result<Vec<Fence>, String> {
    let mut fences = Vec::new();
    let mut ids = BTreeSet::new();
    let mut pending: Option<String> = None;
    let mut lines = text.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        let line_number = index + 1;
        if let Some(id) = marker_id(line, line_number, &mut ids)? {
            pending = Some(id);
            continue;
        }
        let Some(opening) = delimiter(line) else {
            pending = pending.filter(|_| line.trim().is_empty());
            continue;
        };
        let trimmed = line.trim_start();
        let info: String = trimmed.chars().skip(opening.length).collect();
        let id = pending
            .take()
            .ok_or_else(|| format!("line {line_number}: a fence has no tested-example marker"))?;
        fences.push(read_fence(
            &mut lines,
            id,
            info.trim(),
            Opening {
                delimiter: opening,
                line: line_number,
            },
        )?);
    }
    pending.map_or(Ok(fences), |id| {
        Err(format!("example {id} has a marker but no fence"))
    })
}

/// Returns the identifier on a marker line, recording it as seen.
fn marker_id(
    line: &str,
    line_number: usize,
    ids: &mut BTreeSet<String>,
) -> Result<Option<String>, String> {
    let Some(rest) = line.trim().strip_prefix("<!-- tested-example:") else {
        return Ok(None);
    };
    let id = rest.trim().trim_end_matches("-->").trim().to_owned();
    if id.is_empty() {
        return Err(format!("line {line_number}: the marker has no identifier"));
    }
    if !ids.insert(id.clone()) {
        return Err(format!("line {line_number}: duplicate identifier {id}"));
    }
    Ok(Some(id))
}

/// Where a fence opened.
#[derive(Clone, Copy)]
struct Opening {
    /// Delimiter that must close it.
    delimiter: Delimiter,
    /// Line number of the opening fence.
    line: usize,
}

/// Reads one fence body after its opening line.
fn read_fence(
    lines: &mut std::iter::Enumerate<std::str::Lines<'_>>,
    id: String,
    language: &str,
    opening: Opening,
) -> Result<Fence, String> {
    let mut body = String::new();
    for (_, line) in lines.by_ref() {
        if closes(line, opening.delimiter) {
            return Ok(Fence {
                id,
                language: language.to_owned(),
                body,
            });
        }
        body.push_str(line);
        body.push('\n');
    }
    Err(format!(
        "line {}: example {id} is unterminated",
        opening.line
    ))
}

/// Loads the user-guide examples, reporting the reason they cannot load.
fn user_guide_examples() -> Result<Vec<Fence>, String> { load(Utf8Path::new(USER_GUIDE)) }

#[test]
fn every_user_guide_fence_is_marked_and_unique() {
    let fences = user_guide_examples().expect("the user guide examples should load");
    assert!(
        fences.len() >= 13,
        "the guide should still carry its examples, found {}",
        fences.len()
    );
    for fence in &fences {
        assert!(
            !fence.body.trim().is_empty(),
            "example {} should not be empty",
            fence.id
        );
    }
}

/// Checks one documented `[init]` example against the real config type.
///
/// Returns whether the example contained an `[init]` table.
fn check_init_example(fence: &Fence, init: &toml::value::Table) -> Result<bool, String> {
    let size = init
        .get("volume_size_gb")
        .and_then(toml::Value::as_integer)
        .and_then(|raw| u32::try_from(raw).ok())
        .ok_or_else(|| format!("example {} needs an integer size", fence.id))?;
    let tier = match init.get("volume_tier") {
        Some(value) => value
            .as_str()
            .ok_or_else(|| format!("example {} needs a string volume_tier", fence.id))?,
        None => "sbs_5k",
    };
    let config = InitConfig {
        volume_size_gb: size,
        volume_tier: tier.to_owned(),
    };
    config
        .validate()
        .map_err(|error| format!("example {} is invalid: {error}", fence.id))?;
    if u64::from(size) < MIN_VOLUME_SIZE_GB {
        return Err(format!(
            "example {} is below the {MIN_VOLUME_SIZE_GB} GB minimum",
            fence.id
        ));
    }
    Ok(true)
}

/// Checks one documented `[scaleway]` example against the config type.
///
/// Returns whether the example contained a `[scaleway]` table.
fn check_scaleway_example(fence: &Fence, scaleway: &toml::value::Table) -> Result<bool, String> {
    // Every documented key must exist in `ScalewayConfig`; a typo in the
    // guide would otherwise configure nothing.
    const KNOWN_KEYS: &[&str] = &[
        "access_key",
        "secret_key",
        "default_organization_id",
        "default_project_id",
        "default_zone",
        "default_instance_type",
        "default_image",
        "default_architecture",
        "default_volume_id",
    ];
    for key in scaleway.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            return Err(format!("example {} documents unknown key {key}", fence.id));
        }
    }
    // Deserialize through the production type so a mistyped value fails here.
    // A partial example gets controlled values for the required fields it does
    // not document.
    let mut complete = scaleway.clone();
    complete
        .entry(String::from("secret_key"))
        .or_insert_with(|| toml::Value::String(String::from("documented-placeholder")));
    complete
        .entry(String::from("default_project_id"))
        .or_insert_with(|| {
            toml::Value::String(String::from("00000000-0000-0000-0000-000000000000"))
        });
    let rendered = toml::to_string(&complete)
        .map_err(|error| format!("example {} cannot render: {error}", fence.id))?;
    let config: ScalewayConfig = toml::from_str(&rendered).map_err(|error| {
        format!(
            "example {} is not a valid [scaleway] table: {error}",
            fence.id
        )
    })?;
    config
        .as_request()
        .map_err(|error| format!("example {} is invalid: {error}", fence.id))?;
    Ok(true)
}

#[test]
fn user_guide_toml_examples_match_the_configuration_types() {
    let mut checked = 0;
    let examples = user_guide_examples().expect("the user guide examples should load");
    for fence in examples.iter().filter(|fence| fence.language == "toml") {
        let value: toml::Value = toml::from_str(&fence.body)
            .unwrap_or_else(|error| panic!("example {} is not TOML: {error}", fence.id));
        if let Some(init) = value.get("init").and_then(toml::Value::as_table) {
            let found = check_init_example(fence, init).unwrap_or_else(|error| panic!("{error}"));
            checked += usize::from(found);
        }
        if let Some(scaleway) = value.get("scaleway").and_then(toml::Value::as_table) {
            let found =
                check_scaleway_example(fence, scaleway).unwrap_or_else(|error| panic!("{error}"));
            checked += usize::from(found);
        }
    }
    assert!(
        checked >= 3,
        "expected the guide's TOML examples, found {checked}"
    );
}

#[test]
fn user_guide_shell_examples_are_syntactically_valid() {
    // The recovery example uses `<placeholder>` arguments, which are not
    // valid shell; `the_recovery_example_matches_the_error_message` ties it to
    // the command the implementation prints instead.
    let examples = user_guide_examples().expect("the user guide examples should load");
    for fence in examples
        .iter()
        .filter(|fence| matches!(fence.language.as_str(), "bash" | "sh"))
    {
        let status = Command::new("bash")
            .arg("-n")
            .arg("-c")
            .arg(&fence.body)
            .status()
            .unwrap_or_else(|error| panic!("running bash -n for {}: {error}", fence.id));
        assert!(status.success(), "example {} is not valid shell", fence.id);
    }
}

#[test]
fn the_recovery_example_matches_the_error_message() {
    let fences = user_guide_examples().expect("the user guide examples should load");
    let recovery = fences
        .iter()
        .find(|fence| fence.id == "users-guide-detach-recovery")
        .expect("the recovery example should exist");
    assert!(
        recovery.body.contains("scw instance server detach-volume"),
        "{}",
        recovery.body
    );

    let error = ScalewayBackendError::VolumeDetachRetainsServer {
        volume_id: String::from("vol"),
        instance_id: String::from("srv"),
        zone: String::from("fr-par-1"),
        message: String::from("detach rejected"),
    };
    assert!(
        error
            .to_string()
            .contains("scw instance server detach-volume"),
        "{error}"
    );
}

/// Runs the published `mriya` commands through the real binary.
///
/// The environment has no Scaleway credentials and a missing configuration
/// path, so each command stops at configuration validation. That still proves
/// the documented options and subcommands are accepted by the parser, which a
/// syntax check alone cannot.
#[test]
fn user_guide_mriya_examples_are_accepted_by_the_cli() {
    let binary = env!("CARGO_BIN_EXE_mriya");
    let temp = TempDir::new().expect("a temp dir should be available");
    let root = Utf8Path::from_path(temp.path()).expect("the temp path should be UTF-8");
    // A capability directory cannot link to a target outside its sandbox, so
    // the test creates the link through the host API.
    std::os::unix::fs::symlink(binary, temp.path().join("mriya"))
        .expect("the mriya binary should link into the temp dir");
    let path = format!("{root}:/usr/bin:/bin");
    let config = temp.path().join("missing-mriya.toml");

    let examples = user_guide_examples().expect("the user guide examples should load");
    for fence in examples
        .iter()
        .filter(|fence| fence.language == "bash" && fence.body.trim_start().starts_with("mriya "))
    {
        let output = Command::new("bash")
            .arg("-c")
            .arg(&fence.body)
            .env_clear()
            .env("PATH", &path)
            .env("HOME", temp.path())
            .env("MRIYA_CONFIG_PATH", &config)
            .output()
            .unwrap_or_else(|error| panic!("running example {}: {error}", fence.id));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_ne!(
            output.status.code(),
            Some(2),
            "example {} was rejected by the CLI: {stderr}",
            fence.id
        );
        for marker in [
            "unexpected argument",
            "unrecognized subcommand",
            "invalid value",
        ] {
            assert!(
                !stderr.contains(marker),
                "example {} was rejected by the CLI: {stderr}",
                fence.id
            );
        }
    }
}
