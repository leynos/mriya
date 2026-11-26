//! Block Storage performance tiers and their provider type names.

use std::fmt;

/// Performance tier of a Scaleway Block Storage volume.
///
/// Each tier maps to a provider `perf_iops` value and a provider type name.
///
/// # Examples
///
/// ```
/// use mriya::volume::VolumeTier;
///
/// assert_eq!(VolumeTier::Sbs5k.perf_iops(), 5_000);
/// assert_eq!(VolumeTier::Sbs15k.as_str(), "sbs_15k");
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VolumeTier {
    /// Block Storage 5K, the default tier.
    #[default]
    Sbs5k,
    /// Block Storage 15K, for higher-throughput workloads.
    Sbs15k,
}

impl VolumeTier {
    /// Returns the provider type name, such as `sbs_5k`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sbs5k => "sbs_5k",
            Self::Sbs15k => "sbs_15k",
        }
    }

    /// Returns the `perf_iops` value the provider expects.
    #[must_use]
    pub const fn perf_iops(self) -> u32 {
        match self {
            Self::Sbs5k => 5_000,
            Self::Sbs15k => 15_000,
        }
    }

    /// Parses a tier from its provider type name.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeTierParseError`] for any value other than `sbs_5k` or
    /// `sbs_15k`, naming the accepted values.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::volume::VolumeTier;
    ///
    /// assert_eq!(
    ///     VolumeTier::parse(" sbs_15k ").expect("known tier"),
    ///     VolumeTier::Sbs15k
    /// );
    /// assert!(VolumeTier::parse("b_ssd").is_err());
    /// ```
    pub fn parse(value: &str) -> Result<Self, VolumeTierParseError> {
        match value.trim() {
            "sbs_5k" => Ok(Self::Sbs5k),
            "sbs_15k" => Ok(Self::Sbs15k),
            other => Err(VolumeTierParseError {
                value: other.to_owned(),
            }),
        }
    }
}

impl fmt::Display for VolumeTier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error returned when a value is not a supported volume tier.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("unsupported volume tier '{value}'; use sbs_5k or sbs_15k")]
pub struct VolumeTierParseError {
    /// Value that was rejected.
    pub value: String,
}
