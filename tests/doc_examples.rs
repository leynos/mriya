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
use mriya::{InitConfig, ScalewayBackendError, volume::MIN_VOLUME_SIZE_GB};

/// The user-facing document whose examples are contracted by this suite.
const USER_GUIDE: &str = "docs/users-guide.md";

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
        let Some(info) = line.trim_start().strip_prefix("```") else {
            pending = pending.filter(|_| line.trim().is_empty());
            continue;
        };
        let id = pending
            .take()
            .ok_or_else(|| format!("line {line_number}: a fence has no tested-example marker"))?;
        fences.push(read_fence(&mut lines, id, info.trim(), line_number)?);
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

/// Reads one fence body after its opening line.
fn read_fence(
    lines: &mut std::iter::Enumerate<std::str::Lines<'_>>,
    id: String,
    language: &str,
    line_number: usize,
) -> Result<Fence, String> {
    let mut body = String::new();
    for (_, line) in lines.by_ref() {
        if line.trim_start().starts_with("```") {
            return Ok(Fence {
                id,
                language: language.to_owned(),
                body,
            });
        }
        body.push_str(line);
        body.push('\n');
    }
    Err(format!("line {line_number}: example {id} is unterminated"))
}

/// Returns the loaded user-guide examples or fails the test.
fn user_guide_examples() -> Vec<Fence> {
    load(Utf8Path::new(USER_GUIDE)).expect("the user guide examples should load")
}

#[test]
fn every_user_guide_fence_is_marked_and_unique() {
    let fences = user_guide_examples();
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

#[test]
fn user_guide_toml_examples_match_the_configuration_types() {
    let mut checked = 0;
    for fence in user_guide_examples()
        .iter()
        .filter(|fence| fence.language == "toml")
    {
        let value: toml::Value = toml::from_str(&fence.body)
            .unwrap_or_else(|error| panic!("example {} is not TOML: {error}", fence.id));
        if let Some(init) = value.get("init").and_then(toml::Value::as_table) {
            let size = init
                .get("volume_size_gb")
                .and_then(toml::Value::as_integer)
                .and_then(|raw| u32::try_from(raw).ok())
                .unwrap_or_else(|| panic!("example {} needs an integer size", fence.id));
            let tier = init
                .get("volume_tier")
                .and_then(toml::Value::as_str)
                .unwrap_or("sbs_5k");
            let config = InitConfig {
                volume_size_gb: size,
                volume_tier: tier.to_owned(),
            };
            config
                .validate()
                .unwrap_or_else(|error| panic!("example {} is invalid: {error}", fence.id));
            assert!(
                u64::from(size) >= MIN_VOLUME_SIZE_GB,
                "example {}",
                fence.id
            );
            checked += 1;
        }
        if let Some(scaleway) = value.get("scaleway").and_then(toml::Value::as_table) {
            // Every documented key must exist in `ScalewayConfig`; a typo in
            // the guide would otherwise configure nothing.
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
                assert!(
                    KNOWN_KEYS.contains(&key.as_str()),
                    "example {} documents unknown key {key}",
                    fence.id
                );
            }
            checked += 1;
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
    for fence in user_guide_examples()
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
    let fences = user_guide_examples();
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
