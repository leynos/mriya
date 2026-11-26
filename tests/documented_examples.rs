//! Contract tests for the executable examples in the user guide.
//!
//! The guide marks each runnable fence with `<!-- tested-example: <id> -->`.
//! These tests load the published Markdown, so a documented command that stops
//! existing, an unmarked fence, a duplicate identifier, or an unterminated
//! fence fails here rather than drifting silently from the shipped text.

use std::collections::BTreeSet;

use camino::Utf8Path;

/// The document whose examples are contracts.
const GUIDE: &str = "docs/users-guide.md";
/// Prefix of a marker comment that names the following fence.
const MARKER_PREFIX: &str = "<!-- tested-example: ";
/// Suffix that closes a marker comment.
const MARKER_SUFFIX: &str = " -->";
/// Fence prefixes Markdown accepts for a code block.
///
/// A tilde fence is as much a contract as a backtick fence; recognizing only
/// one form would let an unmarked `~~~bash` example bypass the test.
const FENCE_PREFIXES: [&str; 2] = ["```", "~~~"];

/// One fenced example with its parsed lines.
#[derive(Debug, Eq, PartialEq)]
struct Example {
    /// Identifier from the marker comment.
    identifier: String,
    /// Fence language, such as `bash`.
    language: String,
    /// Lines between the fences.
    lines: Vec<String>,
}

/// An example whose closing fence has not been seen yet.
#[derive(Debug)]
struct OpenExample {
    /// Identifier from the marker comment.
    identifier: String,
    /// Delimiter that opened the block.
    fence: String,
    /// Fence language, such as `bash`.
    language: String,
    /// Lines read so far.
    lines: Vec<String>,
}

impl OpenExample {
    /// Completes the example now that its closing fence has arrived.
    fn finish(self) -> Example {
        Example {
            identifier: self.identifier,
            language: self.language,
            lines: self.lines,
        }
    }
}

/// Collects marked examples from a Markdown document.
#[derive(Debug, Default)]
struct ExampleParser {
    /// Examples whose fences have both been seen.
    examples: Vec<Example>,
    /// Identifier of a marker waiting for its opening fence.
    pending: Option<String>,
    /// Example between its opening and closing fences.
    current: Option<OpenExample>,
    /// Identifiers already claimed, so duplicates cannot alias one another.
    seen: BTreeSet<String>,
}

impl ExampleParser {
    /// Consumes one Markdown line.
    fn push_line(&mut self, line: &str) -> Result<(), String> {
        let trimmed = line.trim();
        if trimmed.starts_with(MARKER_PREFIX) {
            return self.open_marker(line, trimmed);
        }
        if let Some((fence, rest)) = split_fence(trimmed) {
            return self.toggle_fence(line, fence, rest.trim());
        }
        if let Some(current) = self.current.as_mut() {
            current.lines.push(line.to_owned());
        }
        Ok(())
    }

    /// Records the identifier a marker names for the next fence.
    fn open_marker(&mut self, line: &str, trimmed: &str) -> Result<(), String> {
        let identifier = trimmed
            .strip_prefix(MARKER_PREFIX)
            .and_then(|rest| rest.strip_suffix(MARKER_SUFFIX))
            .ok_or_else(|| format!("unterminated example marker: {line}"))?
            .trim()
            .to_owned();
        if identifier.is_empty() {
            return Err(format!("example marker without an identifier: {line}"));
        }
        if !self.seen.insert(identifier.clone()) {
            return Err(format!("duplicate example identifier: {identifier}"));
        }
        if self.pending.replace(identifier).is_some() {
            return Err(String::from("two markers before one fence"));
        }
        Ok(())
    }

    /// Opens an example or completes the one currently open.
    fn toggle_fence(&mut self, line: &str, fence: &str, language: &str) -> Result<(), String> {
        // A fence ends its example; a marker belongs to exactly one fence, so
        // an unmarked fence is a contract gap, not prose.
        match self.current.take() {
            None => {
                let identifier = self
                    .pending
                    .take()
                    .ok_or_else(|| format!("unmarked fenced example at: {line}"))?;
                self.current = Some(OpenExample {
                    identifier,
                    fence: fence.to_owned(),
                    language: language.to_owned(),
                    lines: Vec::new(),
                });
            }
            Some(open) => {
                // A closing fence must use the delimiter that opened it; the
                // other form is ordinary content inside the block.
                if open.fence != fence {
                    return Err(format!(
                        "closing fence {fence} does not match opening fence {}: {line}",
                        open.fence
                    ));
                }
                self.examples.push(open.finish());
            }
        }
        Ok(())
    }

    /// Requires every marker and fence to pair up, then returns the examples.
    fn finish(self) -> Result<Vec<Example>, String> {
        if let Some(open) = self.current {
            return Err(format!(
                "unterminated fence for example {}",
                open.identifier
            ));
        }
        if let Some(identifier) = self.pending {
            return Err(format!("example {identifier} has no fence"));
        }
        Ok(self.examples)
    }
}

/// Splits a fence line into its delimiter and remainder.
fn split_fence(line: &str) -> Option<(&'static str, &str)> {
    FENCE_PREFIXES
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix).map(|rest| (*prefix, rest)))
}

/// Extracts every marked example from `document`, rejecting a bad structure.
fn examples(document: &str) -> Result<Vec<Example>, String> {
    let mut parser = ExampleParser::default();
    for line in document.lines() {
        parser.push_line(line)?;
    }
    parser.finish()
}

/// Reads the published guide and extracts its examples.
fn load_examples() -> Result<Vec<Example>, String> {
    let dir = cap_std::fs_utf8::Dir::open_ambient_dir(".", cap_std::ambient_authority())
        .map_err(|error| format!("the repository root must be readable: {error}"))?;
    let document = dir
        .read_to_string(Utf8Path::new(GUIDE))
        .map_err(|error| format!("the user guide must be readable: {error}"))?;
    examples(&document)
}

/// Returns an error when `condition` does not hold.
fn ensure(condition: bool, message: impl Into<String>) -> Result<(), String> {
    if condition {
        return Ok(());
    }
    Err(message.into())
}

/// Returns the example with `identifier`.
fn example<'a>(examples: &'a [Example], identifier: &str) -> Result<&'a Example, String> {
    examples
        .iter()
        .find(|example| example.identifier == identifier)
        .ok_or_else(|| format!("missing documented example: {identifier}"))
}

#[test]
fn the_guide_marks_every_fence_uniquely() -> Result<(), String> {
    let parsed = load_examples()?;
    ensure(
        !parsed.is_empty(),
        "the guide must carry at least one marked example",
    )
}

#[test]
fn the_recover_example_is_the_documented_command() -> Result<(), String> {
    let parsed = load_examples()?;
    let recover = example(&parsed, "recover-command")?;
    ensure(
        recover.language == "bash",
        format!("the recover example must be bash, not {}", recover.language),
    )?;
    ensure(
        recover.lines == [String::from("mriya recover")],
        format!(
            "the recover example must be `mriya recover`, not {:?}",
            recover.lines
        ),
    )?;
    // The documented command must be one the shipped binary accepts. An
    // isolated state directory keeps this from touching real journals.
    let state = tempfile::TempDir::new()
        .map_err(|error| format!("the isolated state directory must be created: {error}"))?;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mriya"))
        .arg("recover")
        .env("MRIYA_STATE_DIR", state.path())
        .env("SCW_SECRET_KEY", "test-secret")
        .env("SCW_DEFAULT_PROJECT_ID", "test-project")
        .output()
        .map_err(|error| format!("`mriya recover` must start: {error}"))?;
    ensure(
        output.status.success(),
        format!(
            "`mriya recover` must accept the documented command: {stderr}",
            stderr = String::from_utf8_lossy(&output.stderr)
        ),
    )
}

#[test]
fn the_cache_setting_example_matches_the_configuration_key() -> Result<(), String> {
    let parsed = load_examples()?;
    let setting = example(&parsed, "cache-volume-setting")?;
    ensure(
        setting.language == "toml",
        format!("the cache setting must be TOML, not {}", setting.language),
    )?;
    let rendered = setting.lines.join("\n");
    let parsed_setting: toml::Value =
        toml::from_str(&rendered).map_err(|error| format!("invalid TOML: {error}"))?;
    let volume_id = parsed_setting
        .get("scaleway")
        .and_then(|section| section.get("default_volume_id"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| String::from("the example must set scaleway.default_volume_id"))?;
    // The key the store writes and the guide documents must be the same one.
    ensure(
        !volume_id.trim().is_empty(),
        "the documented volume ID must be a real value",
    )?;
    // The application's own loader must accept the documented shape. The
    // snippet is written to be added to an existing configuration, so the
    // required credentials are supplied here to reach the loader with the
    // documented key intact.
    let mut merged: toml::Table =
        toml::from_str(&rendered).map_err(|error| format!("invalid TOML: {error}"))?;
    let scaleway = merged
        .get_mut("scaleway")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| String::from("the example must set [scaleway]"))?;
    scaleway.insert(
        String::from("secret_key"),
        toml::Value::String(String::from("test-secret")),
    );
    scaleway.insert(
        String::from("default_project_id"),
        toml::Value::String(String::from("test-project")),
    );
    // The loader requires the same fields a real configuration file carries.
    for (key, value) in [
        ("default_zone", "fr-par-1"),
        ("default_instance_type", "DEV1-S"),
        ("default_image", "ubuntu-22-04"),
        ("default_architecture", "x86_64"),
    ] {
        scaleway.insert(String::from(key), toml::Value::String(String::from(value)));
    }
    // The application's loader reads this section's fields, so the section is
    // extracted and handed to the same `ScalewayConfig` deserializer the
    // configuration file uses.
    let section = merged
        .remove("scaleway")
        .ok_or_else(|| String::from("the example must set [scaleway]"))?;
    let config: mriya::ScalewayConfig = section
        .try_into()
        .map_err(|error| format!("the documented setting must load: {error}"))?;
    ensure(
        config.default_volume_id.as_deref() == Some(volume_id),
        "the loader must read the documented volume ID",
    )
}
