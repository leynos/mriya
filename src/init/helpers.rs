//! Helper utilities for init configuration and formatting.

const BYTES_PER_GB: u64 = 1024 * 1024 * 1024;
const DISK_BY_ID_PREFIX: &str = "/dev/disk/by-id/scsi-0SCW_BSSD_";

pub(super) fn format_command(volume_id: &str) -> String {
    format!(
        "sudo mkfs.ext4 -F {}{}",
        DISK_BY_ID_PREFIX,
        volume_id.trim()
    )
}

pub(super) fn volume_size_bytes(size_gb: u32) -> Option<u64> {
    u64::from(size_gb).checked_mul(BYTES_PER_GB)
}

pub(super) fn volume_name_for_project(project_name: &str) -> String {
    let slug = slugify(project_name);
    if slug.is_empty() {
        return String::from("mriya-cache");
    }
    format!("mriya-{slug}-cache")
}

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
    use super::*;
    use rstest::rstest;

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

    #[test]
    fn volume_size_bytes_converts_gb() {
        let bytes = volume_size_bytes(2).expect("size bytes");
        assert_eq!(bytes, 2 * BYTES_PER_GB);
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
