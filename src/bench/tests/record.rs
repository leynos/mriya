//! Tests for the persisted record shapes.

use super::super::record::{
    CacheObservation,
    CacheState,
    GuestArchitecture,
    PhaseKind,
    PhaseRecord,
    PhaseStatus,
    Record,
    RecordKind,
    ResidualRecord,
    SCHEMA_VERSION,
    parse_line,
    to_line,
};

#[test]
fn every_phase_name_is_distinct_and_in_protocol_order() {
    let names = PhaseKind::ALL.map(PhaseKind::as_str);
    let unique: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "phase names must be distinct");
    assert_eq!(names[0], "allocate");
    assert_eq!(names[names.len() - 1], "teardown");
}

#[test]
fn a_phase_record_names_its_phase_with_the_protocol_spelling() {
    let record = Record::Phase(Box::new(PhaseRecord {
        schema: SCHEMA_VERSION,
        kind: RecordKind::Phase,
        campaign_id: String::from("c"),
        workload_id: String::from("cold-build"),
        repetition: 1,
        phase: PhaseKind::CacheRestore,
        segment: 1,
        duration_nanos: 42,
        status: PhaseStatus::Unverified,
    }));
    let line = to_line(&record).expect("the phase should serialise");
    assert!(line.contains(r#""phase":"cache-restore""#), "{line}");
    assert!(line.contains(r#""status":"unverified""#), "{line}");
    assert_eq!(parse_line(&line).expect("round trip"), record);
}

#[test]
fn a_residual_record_names_every_identifier_it_carries() {
    let record = Record::Residual(Box::new(ResidualRecord {
        schema: SCHEMA_VERSION,
        kind: RecordKind::ResidualResource,
        campaign_id: String::from("c"),
        provider: String::from("scaleway"),
        resource_ids: vec![String::from("a"), String::from("b")],
        reason: String::from("listing unreadable"),
    }));
    let line = to_line(&record).expect("the residual should serialise");
    assert!(line.contains(r#""resource_ids":["a","b"]"#), "{line}");
    assert_eq!(record.kind(), RecordKind::ResidualResource);
}

#[test]
fn a_record_carries_no_field_a_credential_could_hide_in() {
    let line = to_line(&Record::Residual(Box::new(ResidualRecord {
        schema: SCHEMA_VERSION,
        kind: RecordKind::ResidualResource,
        campaign_id: String::from("c"),
        provider: String::from("scaleway"),
        resource_ids: vec![String::from("a")],
        reason: String::from("listing unreadable"),
    })))
    .expect("serialises");
    for forbidden in ["secret", "token", "password", "key", "credential"] {
        assert!(
            !line.to_lowercase().contains(forbidden),
            "a persisted record must not name {forbidden}: {line}"
        );
    }
}

#[test]
fn a_cache_observation_round_trips_every_state() {
    for state in [
        CacheState::Empty,
        CacheState::Warm,
        CacheState::Seed,
        CacheState::Absent,
    ] {
        let observation = CacheObservation {
            state,
            size_bytes: 7,
            mounted: state != CacheState::Absent,
            saved_bytes: 3,
        };
        let json = serde_json::to_string(&observation).expect("observation serialises");
        let back: CacheObservation = serde_json::from_str(&json).expect("observation parses");
        assert_eq!(back, observation);
    }
}

#[test]
fn guest_architectures_map_from_the_strings_a_guest_reports() {
    assert_eq!(
        GuestArchitecture::from_report(" x86_64 "),
        GuestArchitecture::X86_64
    );
    assert_eq!(
        GuestArchitecture::from_report("arm64"),
        GuestArchitecture::Aarch64
    );
    assert_eq!(
        GuestArchitecture::from_report("s390x"),
        GuestArchitecture::Other
    );
}
