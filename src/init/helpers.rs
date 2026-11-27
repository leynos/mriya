//! Helper utilities for init configuration.

use crate::volume::BYTES_PER_GB;

/// Converts a size in decimal gigabytes to bytes, or `None` on overflow.
pub(super) fn volume_size_bytes(size_gb: u32) -> Option<u64> {
    u64::from(size_gb).checked_mul(BYTES_PER_GB)
}

/// Derives the cache volume name `mriya-<slug>-cache` from the project name,
/// falling back to `mriya-cache` when the slug is empty.
pub(super) fn volume_name_for_project(project_name: &str) -> String {
    let slug = slugify(project_name);
    if slug.is_empty() {
        return String::from("mriya-cache");
    }
    format!("mriya-{slug}-cache")
}

/// Lowercases ASCII alphanumerics and collapses every other run of
/// characters into a single dash, trimming dashes from both ends.
///
/// Non-ASCII characters are dropped to dashes, so `Fancy Project!` becomes
/// `fancy-project`.
pub(super) fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    //! Unit tests for init helper functions.
    use rstest::rstest;

    use super::*;

    #[test]
    fn volume_name_for_project_defaults_when_empty() {
        let name = volume_name_for_project("");
        assert_eq!(name, "mriya-cache");
    }

    #[test]
    fn volume_name_for_project_slugifies() {
        let name = volume_name_for_project("Fancy Project!");
        assert_eq!(name, "mriya-fancy-project-cache");
    }

    /// Guards the exact byte conversion so the expected value cannot mutate
    /// together with the conversion constant.
    #[test]
    fn volume_size_bytes_converts_decimal_gb() {
        let bytes = volume_size_bytes(2).expect("size bytes");
        // Assert the literal value: comparing against `2 * BYTES_PER_GB`
        // mutates together with the constant under test, making the
        // assertion tautological.
        assert_eq!(bytes, 2_000_000_000);
    }

    #[rstest]
    #[case("Fancy Project!", "fancy-project")]
    #[case("Café 🚀", "caf")]
    #[case("Hello---World!!", "hello-world")]
    #[case("12345", "12345")]
    fn slugify_handles_varied_input(#[case] input: &str, #[case] expected: &str) {
        let slug = slugify(input);
        assert_eq!(slug, expected);
    }
}
