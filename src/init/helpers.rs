//! Helper utilities for init configuration and formatting.

/// Bytes in one gigabyte as used for volume sizes (binary, 1024^3).
const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;
/// Device path prefix under which Scaleway exposes an attached Block Storage
/// volume; the volume ID completes it.
const DISK_BY_ID_PREFIX: &str = "/dev/disk/by-id/scsi-0SCW_BSSD_";

/// Builds the remote command that formats the volume as ext4.
///
/// The `-F` flag forces formatting without prompting. For example, `abc`
/// yields `sudo mkfs.ext4 -F /dev/disk/by-id/scsi-0SCW_BSSD_abc`.
pub(super) fn format_command(volume_id: &str) -> String {
    format!(
        "sudo mkfs.ext4 -F {}{}",
        DISK_BY_ID_PREFIX,
        volume_id.trim()
    )
}

/// Converts a size in gigabytes to bytes, or `None` on overflow.
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
    fn volume_size_bytes_converts_gb() {
        let bytes = volume_size_bytes(2).expect("size bytes");
        // Assert the literal value: comparing against `2 * BYTES_PER_GB`
        // mutates together with the constant under test, making the
        // assertion tautological. Kills the survivor tracked in #56.
        assert_eq!(bytes, 2_147_483_648);
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
