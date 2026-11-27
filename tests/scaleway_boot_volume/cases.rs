//! Case parsing and opt-in detection for the live boot-volume check.
//!
//! The suite is enabled by `MRIYA_RUN_SCALEWAY_BOOT_TESTS` and reads its cases
//! from `MRIYA_TEST_BOOT_CASES` as `instance_type:volume_type:size_gb` triples
//! separated by semicolons. When enabled, an absent or empty list is a
//! failure, not a skip: an operator who asked for the check must not be told
//! it passed when it never ran.

use mockable::{DefaultEnv, Env};
use mriya::BootVolumeType;

/// Variable that opts the live suite in.
const ENABLE_VARIABLE: &str = "MRIYA_RUN_SCALEWAY_BOOT_TESTS";

/// Variable listing the cases to execute.
const CASES_VARIABLE: &str = "MRIYA_TEST_BOOT_CASES";

/// One live case to execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Case {
    /// Commercial type to provision.
    pub(super) instance_type: String,
    /// Boot storage type to request.
    pub(super) volume_type: BootVolumeType,
    /// Capacity to request, in decimal gigabytes.
    pub(super) size_gb: u32,
}

/// Reports whether the suite was opted into.
pub(super) fn enabled() -> bool {
    DefaultEnv.raw(ENABLE_VARIABLE).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

/// Reads the configured cases, refusing an empty list.
///
/// # Errors
///
/// Returns a description naming the variable when the list is absent, empty,
/// or contains an unparsable case.
pub(super) fn cases_from_env() -> Result<Vec<Case>, String> {
    let raw = DefaultEnv
        .raw(CASES_VARIABLE)
        .map_err(|_| missing_cases())?;
    let cases = parse_cases(&raw)?;
    if cases.is_empty() {
        return Err(missing_cases());
    }
    Ok(cases)
}

/// The failure reported when no case was listed.
fn missing_cases() -> String {
    format!("{CASES_VARIABLE} must list at least one instance_type:volume_type:size_gb case")
}

/// Parses the semicolon-separated case list.
///
/// For example, `DEV1-S:l_ssd:20;DEV1-S:sbs_volume:20` yields two cases.
///
/// # Errors
///
/// Returns the offending entry when a field is missing or unparsable.
fn parse_cases(raw: &str) -> Result<Vec<Case>, String> {
    raw.split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(parse_case)
        .collect()
}

/// Parses one `instance_type:volume_type:size_gb` triple.
fn parse_case(entry: &str) -> Result<Case, String> {
    let mut fields = entry.split(':');
    let instance_type = fields
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("case '{entry}' is missing its instance type"))?;
    let volume_type = fields
        .next()
        .ok_or_else(|| format!("case '{entry}' is missing its volume type"))?
        .trim()
        .parse::<BootVolumeType>()
        .map_err(|error| format!("case '{entry}': {error}"))?;
    let size_gb = fields
        .next()
        .ok_or_else(|| format!("case '{entry}' is missing its size in GB"))?
        .trim()
        .parse::<u32>()
        .map_err(|error| format!("case '{entry}': invalid size: {error}"))?;
    if fields.next().is_some() {
        return Err(format!("case '{entry}' has more than three fields"));
    }
    Ok(Case {
        instance_type: instance_type.to_owned(),
        volume_type,
        size_gb,
    })
}
