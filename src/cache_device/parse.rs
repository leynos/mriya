//! Parser for the cache-device scripts' single status line.
//!
//! Exit codes alone cannot distinguish "the serial was missing" from "the root
//! guard refused" from "the mount source was wrong", so the scripts print one
//! machine-readable line and the orchestrator reports the parsed status.

use std::collections::BTreeMap;

use super::script::STATUS_PREFIX;

/// Typed outcome reported by a cache-device script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheDeviceStatus {
    /// The device is mounted and the source was verified.
    Verified,
    /// The device was formatted as ext4.
    Formatted,
    /// The mount point is no longer a mount.
    Unmounted,
    /// No disk carries the requested serial.
    NotFound,
    /// More than one disk carries the requested serial.
    Ambiguous,
    /// The candidate is the root disk or one of its ancestors.
    RootDisk,
    /// The root mount source could not be resolved to a device.
    RootUnknown,
    /// The device already carries a filesystem or partition-table signature.
    SignaturePresent,
    /// A signature probe failed or returned an ambiguous result.
    ProbeFailed,
    /// The device does not carry an ext4 signature.
    NotExt4,
    /// A different source already occupies the mount path.
    MountOccupied,
    /// The mount command failed.
    MountFailed,
    /// The mount source is not the resolved device.
    WrongSource,
    /// The mount point could not be given to the SSH user.
    OwnershipFailed,
    /// The format command failed.
    FormatFailed,
    /// The unmount command failed.
    UnmountFailed,
    /// A status this client does not recognize.
    Unknown(String),
}

impl CacheDeviceStatus {
    /// Maps the script token to the typed status.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::cache_device::CacheDeviceStatus;
    ///
    /// assert_eq!(
    ///     CacheDeviceStatus::from_token("verified"),
    ///     CacheDeviceStatus::Verified
    /// );
    /// ```
    #[must_use]
    pub fn from_token(token: &str) -> Self {
        match token {
            "verified" => Self::Verified,
            "formatted" => Self::Formatted,
            "unmounted" => Self::Unmounted,
            "not-found" => Self::NotFound,
            "ambiguous" => Self::Ambiguous,
            "root-disk" => Self::RootDisk,
            "root-unknown" => Self::RootUnknown,
            "signature-present" => Self::SignaturePresent,
            "probe-failed" => Self::ProbeFailed,
            "not-ext4" => Self::NotExt4,
            "mount-occupied" => Self::MountOccupied,
            "mount-failed" => Self::MountFailed,
            "wrong-source" => Self::WrongSource,
            "ownership-failed" => Self::OwnershipFailed,
            "format-failed" => Self::FormatFailed,
            "unmount-failed" => Self::UnmountFailed,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the script token.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Verified => "verified",
            Self::Formatted => "formatted",
            Self::Unmounted => "unmounted",
            Self::NotFound => "not-found",
            Self::Ambiguous => "ambiguous",
            Self::RootDisk => "root-disk",
            Self::RootUnknown => "root-unknown",
            Self::SignaturePresent => "signature-present",
            Self::ProbeFailed => "probe-failed",
            Self::NotExt4 => "not-ext4",
            Self::MountOccupied => "mount-occupied",
            Self::MountFailed => "mount-failed",
            Self::WrongSource => "wrong-source",
            Self::OwnershipFailed => "ownership-failed",
            Self::FormatFailed => "format-failed",
            Self::UnmountFailed => "unmount-failed",
            Self::Unknown(value) => value.as_str(),
        }
    }

    /// Reports whether the script completed its operation successfully.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Verified | Self::Formatted | Self::Unmounted)
    }
}

/// Parsed status line and its `KEY=value` fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheDeviceReport {
    status: CacheDeviceStatus,
    fields: BTreeMap<String, String>,
}

impl CacheDeviceReport {
    /// Returns the typed status.
    #[must_use]
    pub const fn status(&self) -> &CacheDeviceStatus { &self.status }

    /// Returns the resolved device, when the script reported one.
    #[must_use]
    pub fn device(&self) -> Option<&str> { self.fields.get("DEVICE").map(String::as_str) }

    /// Returns the mount path, when the script reported one.
    #[must_use]
    pub fn path(&self) -> Option<&str> { self.fields.get("PATH").map(String::as_str) }

    /// Returns one field by name.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&str> { self.fields.get(key).map(String::as_str) }

    /// Renders the status and fields for an error message.
    #[must_use]
    pub fn describe(&self) -> String {
        let fields = self
            .fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        if fields.is_empty() {
            String::from(self.status.as_str())
        } else {
            format!("{} {fields}", self.status.as_str())
        }
    }
}

/// Errors raised while parsing a cache-device status line.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CacheDeviceParseError {
    /// The output contained no status line.
    #[error("cache-device script produced no {STATUS_PREFIX} line")]
    MissingStatusLine,
    /// The status line had no status token.
    #[error("cache-device status line is empty after {STATUS_PREFIX}")]
    EmptyStatus,
}

/// Parses the last status line in `stdout`.
///
/// # Errors
///
/// Returns [`CacheDeviceParseError::MissingStatusLine`] when no line starts
/// with the prefix, and [`CacheDeviceParseError::EmptyStatus`] when the line
/// carries no status token.
///
/// # Examples
///
/// ```
/// use mriya::cache_device::{CacheDeviceStatus, parse_report};
///
/// let report = parse_report("noise\nMRIYA_CACHE_STATUS=verified DEVICE=/dev/sdb\n")
///     .expect("status line parses");
/// assert_eq!(report.status(), &CacheDeviceStatus::Verified);
/// assert_eq!(report.device(), Some("/dev/sdb"));
/// ```
pub fn parse_report(stdout: &str) -> Result<CacheDeviceReport, CacheDeviceParseError> {
    let line = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with(STATUS_PREFIX))
        .ok_or(CacheDeviceParseError::MissingStatusLine)?;
    let rest = line
        .trim_start()
        .strip_prefix(STATUS_PREFIX)
        .ok_or(CacheDeviceParseError::MissingStatusLine)?;
    let mut parts = rest.split_whitespace();
    let token = parts.next().ok_or(CacheDeviceParseError::EmptyStatus)?;

    let mut fields = BTreeMap::new();
    for part in parts {
        if let Some((key, value)) = part.split_once('=') {
            fields.insert(key.to_owned(), value.to_owned());
        }
    }

    Ok(CacheDeviceReport {
        status: CacheDeviceStatus::from_token(token),
        fields,
    })
}

#[cfg(test)]
mod tests {
    //! Unit tests for the status-line parser.
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("verified", CacheDeviceStatus::Verified)]
    #[case("formatted", CacheDeviceStatus::Formatted)]
    #[case("root-disk", CacheDeviceStatus::RootDisk)]
    #[case("signature-present", CacheDeviceStatus::SignaturePresent)]
    #[case(
        "future-status",
        CacheDeviceStatus::Unknown(String::from("future-status"))
    )]
    fn token_maps_to_status(#[case] token: &str, #[case] expected: CacheDeviceStatus) {
        assert_eq!(CacheDeviceStatus::from_token(token), expected);
    }

    #[test]
    fn parses_the_last_status_line_and_fields() {
        let report = parse_report(concat!(
            "MRIYA_CACHE_STATUS=not-found\n",
            "MRIYA_CACHE_STATUS=verified DEVICE=/dev/sdb PATH=/mriya\n",
        ))
        .expect("status line parses");
        assert_eq!(report.status(), &CacheDeviceStatus::Verified);
        assert_eq!(report.device(), Some("/dev/sdb"));
        assert_eq!(report.path(), Some("/mriya"));
        assert!(report.status().is_success());
    }

    #[test]
    fn missing_status_line_is_reported() {
        assert_eq!(
            parse_report("mkfs failed\n").expect_err("no status line"),
            CacheDeviceParseError::MissingStatusLine
        );
    }

    #[test]
    fn empty_status_is_reported() {
        assert_eq!(
            parse_report("MRIYA_CACHE_STATUS=\n").expect_err("empty status"),
            CacheDeviceParseError::EmptyStatus
        );
    }

    #[test]
    fn describe_includes_status_and_fields() {
        let report = parse_report("MRIYA_CACHE_STATUS=wrong-source ACTUAL=/dev/sdb")
            .expect("status line parses");
        assert_eq!(report.describe(), "wrong-source ACTUAL=/dev/sdb");
    }
}
