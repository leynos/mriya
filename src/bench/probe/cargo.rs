//! Counting what a Cargo JSON message stream says about cache reuse.
//!
//! Cargo emits `compiler-artifact` for every unit it produces and marks the
//! reused ones with `"fresh": true`. Counting the two is how an attempt reports
//! whether its cache actually helped, without depending on Cargo's human
//! summary, which changes between versions.

use super::{ProbeError, truncate};

/// What one `cargo` JSON message stream reported about cache reuse.
///
/// The two counts are what makes a cache claim checkable. Cargo emits
/// `compiler-artifact` for every unit it produces, marking the ones it reused
/// with `"fresh": true`; counting the marked and unmarked messages gives the
/// rebuilt and reused totals without reading Cargo's human summary, which
/// changes between versions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BuildMessageCounts {
    /// Artefacts Cargo reused without recompiling.
    pub fresh: u32,
    /// Artefacts Cargo rebuilt.
    pub rebuilt: u32,
}

impl BuildMessageCounts {
    /// Returns whether any artefact was reused at all.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::probe::BuildMessageCounts;
    ///
    /// let counts = BuildMessageCounts {
    ///     fresh: 1,
    ///     rebuilt: 0,
    /// };
    /// assert!(counts.reused_anything());
    /// ```
    #[must_use]
    pub const fn reused_anything(&self) -> bool { self.fresh > 0 }

    /// Returns the total number of artefacts Cargo produced or reused.
    ///
    /// # Examples
    ///
    /// ```
    /// use mriya::bench::probe::BuildMessageCounts;
    ///
    /// assert_eq!(
    ///     BuildMessageCounts {
    ///         fresh: 2,
    ///         rebuilt: 3
    ///     }
    ///     .total(),
    ///     5
    /// );
    /// ```
    #[must_use]
    pub const fn total(&self) -> u32 { self.fresh.saturating_add(self.rebuilt) }
}

/// Counts fresh and rebuilt artefacts in a `cargo` JSON message stream.
///
/// Lines that are not JSON objects, such as the human diagnostics Cargo writes
/// to the same stream, are skipped: the count is about artefacts, and one
/// unparsable diagnostic must not fail a whole build's measurement.
///
/// # Errors
///
/// Returns [`ProbeError::MalformedMessage`] when a line that begins as JSON
/// cannot be read as an object, which would mean the stream is truncated or
/// interleaved rather than merely chatty.
///
/// # Examples
///
/// ```
/// use mriya::bench::probe::count_build_messages;
///
/// let stream = concat!(
///     r#"{"reason":"compiler-artifact","fresh":true}"#,
///     "\n",
///     r#"{"reason":"compiler-artifact"}"#,
///     "\n",
/// );
/// let counts = count_build_messages(stream)?;
/// assert_eq!(counts.fresh, 1);
/// assert_eq!(counts.rebuilt, 1);
/// # Ok::<(), mriya::bench::probe::ProbeError>(())
/// ```
pub fn count_build_messages(stream: &str) -> Result<BuildMessageCounts, ProbeError> {
    let mut counts = BuildMessageCounts::default();
    for line in stream
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some(reason) = compiler_artefact_reason(line)? else {
            continue;
        };
        if reason {
            counts.fresh = counts.fresh.saturating_add(1);
        } else {
            counts.rebuilt = counts.rebuilt.saturating_add(1);
        }
    }
    Ok(counts)
}

/// Returns whether `line` is a `compiler-artifact` message, and whether it is
/// marked fresh, or `None` when the line is not such a message.
///
/// # Errors
///
/// Returns [`ProbeError::MalformedMessage`] when a line starting with `{`
/// cannot be parsed as a JSON object.
fn compiler_artefact_reason(line: &str) -> Result<Option<bool>, ProbeError> {
    if !line.starts_with('{') {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| ProbeError::MalformedMessage {
            found: truncate(line),
        })?;
    if value.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
        return Ok(None);
    }
    Ok(Some(
        value
            .get("fresh")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    ))
}
