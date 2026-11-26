//! Tests for the guest probe parsers.
//!
//! The inputs here are the shapes the documented tools print, including the
//! awkward ones: a login banner before the value, a diagnostic in the middle of
//! Cargo's JSON stream, and a truncated JSON object.

use rstest::rstest;

use super::super::{
    probe::{
        BuildMessageCounts,
        ProbeError,
        ProbeOutputs,
        allocation_from,
        cache_from,
        concurrency_from,
        count_build_messages,
        parse_architecture,
        parse_mount_status,
        parse_unsigned,
        parse_version_banner,
        split_banner,
    },
    record::{CacheState, GuestArchitecture},
};

#[rstest]
#[case("8\n", 8)]
#[case("  4  \n", 4)]
#[case("\n16\n", 16)]
#[case("0\n", 0)]
fn parse_unsigned_reads_the_first_non_blank_line(#[case] output: &str, #[case] expected: u64) {
    assert_eq!(parse_unsigned("probe", output).expect("a number"), expected);
}

#[rstest]
#[case("", Err(ProbeError::Empty { probe: "probe" }))]
#[case("\n \n", Err(ProbeError::Empty { probe: "probe" }))]
#[case("8\n16\n", Ok(8))]
fn parse_unsigned_reads_or_rejects(
    #[case] output: &str,
    #[case] expected: Result<u64, ProbeError>,
) {
    assert_eq!(parse_unsigned("probe", output), expected);
}

#[test]
fn a_non_numeric_value_is_not_read_as_a_number() {
    let error = parse_unsigned("probe", "eight\n").expect_err("not a number");
    assert!(matches!(
        error,
        ProbeError::NotANumber { probe: "probe", .. }
    ));
}

#[test]
fn an_empty_probe_names_the_probe() {
    let error = parse_unsigned("getconf _NPROCESSORS_ONLN", "").expect_err("no value");
    assert!(
        error.to_string().contains("getconf _NPROCESSORS_ONLN"),
        "{error}"
    );
}

#[test]
fn a_non_numeric_probe_names_what_it_found() {
    let error = parse_unsigned("probe", "eight").expect_err("not a number");
    assert!(matches!(error, ProbeError::NotANumber { .. }), "{error}");
    assert!(error.to_string().contains("eight"), "{error}");
}

#[rstest]
#[case("x86_64\n", GuestArchitecture::X86_64)]
#[case("amd64", GuestArchitecture::X86_64)]
#[case("aarch64\n", GuestArchitecture::Aarch64)]
#[case("arm64\n", GuestArchitecture::Aarch64)]
#[case("riscv64\n", GuestArchitecture::Other)]
fn parse_architecture_maps_the_guest_string(
    #[case] output: &str,
    #[case] expected: GuestArchitecture,
) {
    assert_eq!(
        parse_architecture(output).expect("an architecture"),
        expected
    );
}

#[rstest]
#[case("0\n", true)]
#[case("1\n", false)]
fn parse_mount_status_reads_the_exit_status(#[case] output: &str, #[case] expected: bool) {
    assert_eq!(parse_mount_status(output).expect("a status"), expected);
}

#[test]
fn a_not_mounted_cache_is_absent_even_if_the_size_probe_says_otherwise() {
    let observation = cache_from("1\n", "4096\n").expect("a cache observation");
    assert_eq!(observation.state, CacheState::Absent);
    assert_eq!(observation.size_bytes, 0);
    assert!(!observation.mounted);
}

#[test]
fn a_mounted_empty_cache_is_empty_rather_than_warm() {
    let observation = cache_from("0\n", "0\n").expect("a cache observation");
    assert_eq!(observation.state, CacheState::Empty);
    assert!(observation.mounted);
}

#[test]
fn a_mounted_populated_cache_is_warm() {
    let observation = cache_from("0\n", "4096\n").expect("a cache observation");
    assert_eq!(observation.state, CacheState::Warm);
    assert_eq!(observation.size_bytes, 4096);
}

#[test]
fn an_allocation_is_built_only_from_measured_values() {
    let allocation = allocation_from(&ProbeOutputs {
        architecture: "x86_64\n",
        cpu_count: "4\n",
        memory: "8589934592\n",
        disk: "107374182400\n",
    })
    .expect("an allocation");
    assert_eq!(allocation.cpu_count, 4);
    assert_eq!(allocation.memory_bytes, 8_589_934_592);
    assert_eq!(allocation.disk_bytes, 107_374_182_400);
    assert_eq!(allocation.guest_architecture, GuestArchitecture::X86_64);
}

#[test]
fn an_allocation_with_a_broken_memory_probe_is_an_error_rather_than_a_zero() {
    let error = allocation_from(&ProbeOutputs {
        architecture: "x86_64\n",
        cpu_count: "4\n",
        memory: "MemTotal: 8589934592 kB\n",
        disk: "1\n",
    })
    .expect_err("the awk probe should have printed a number");
    assert!(matches!(error, ProbeError::NotANumber { .. }), "{error}");
}

#[test]
fn concurrency_records_both_the_intent_and_what_the_guest_gave() {
    let concurrency = concurrency_from(8, "2\n").expect("a concurrency");
    assert_eq!(concurrency.intended_workers, 8);
    assert_eq!(concurrency.effective_workers, 2);
}

#[test]
fn cargo_messages_count_fresh_and_rebuilt_artefacts_separately() {
    let stream = concat!(
        r#"{"reason":"compiler-artifact","fresh":true}"#,
        "\n",
        r#"{"reason":"compiler-artifact"}"#,
        "\n",
        r#"{"reason":"compiler-artifact","fresh":false}"#,
        "\r\n",
        r#"{"reason":"build-finished","success":true}"#,
        "\n",
    );
    let counts = count_build_messages(stream).expect("counts");
    assert_eq!(
        counts,
        BuildMessageCounts {
            fresh: 1,
            rebuilt: 2
        }
    );
    assert!(counts.reused_anything());
    assert_eq!(counts.total(), 3);
}

#[test]
fn a_stream_with_no_artefacts_reports_no_reuse() {
    let counts = count_build_messages(r#"{"reason":"build-finished"}"#).expect("counts");
    assert!(!counts.reused_anything());
    assert_eq!(counts.total(), 0);
}

#[test]
fn human_diagnostics_interleaved_with_json_are_skipped() {
    let stream = concat!(
        "   Compiling mriya v0.1.0\n",
        r#"{"reason":"compiler-artifact","fresh":true}"#,
        "\n",
        "warning: unused import\n",
    );
    let counts = count_build_messages(stream).expect("counts");
    assert_eq!(counts.fresh, 1);
    assert_eq!(counts.rebuilt, 0);
}

#[test]
fn a_truncated_json_line_is_reported_rather_than_silently_ignored() {
    let error = count_build_messages(r#"{"reason":"compiler-artifact""#)
        .expect_err("a truncated object is not a diagnostic");
    assert!(
        matches!(error, ProbeError::MalformedMessage { .. }),
        "{error}"
    );
}

#[test]
fn a_version_banner_is_kept_verbatim() {
    let banner =
        parse_version_banner("rustc 1.93.0-nightly (abc 2026-11-01)\n").expect("a version banner");
    assert_eq!(banner, "rustc 1.93.0-nightly (abc 2026-11-01)");
}

#[test]
fn a_version_banner_splits_into_the_tool_and_its_version() {
    let (tool, version) = split_banner("cargo 1.93.0-nightly (abc 2026-11-01)").expect("a banner");
    assert_eq!(tool, "cargo");
    assert_eq!(version, "1.93.0-nightly (abc 2026-11-01)");
}

#[test]
fn an_empty_version_banner_is_an_error() {
    assert!(parse_version_banner("\n \n").is_err());
    assert!(split_banner("   ").is_err());
}
