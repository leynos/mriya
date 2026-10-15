//! Checks Cargo artefact selection over generated JSON message streams.

use camino::Utf8PathBuf;
use proptest::prelude::*;

use super::{BuildError, executable_from_cargo_messages};

#[derive(Debug)]
struct GeneratedMessage {
    reason: String,
    target_name: String,
    kinds: Vec<String>,
    executable_marker: Option<u8>,
}

/// Generates matching and distractor fields independently for each message.
fn generated_message_strategy() -> impl Strategy<Value = GeneratedMessage> {
    let reason_strategy = prop_oneof![
        3 => Just("compiler-artifact".to_owned()),
        1 => Just("compiler-message".to_owned()),
    ];
    let target_name_strategy = prop_oneof![
        3 => Just("mriya".to_owned()),
        1 => Just("another-target".to_owned()),
    ];
    let kinds_strategy = prop_oneof![
        3 => Just(vec!["bin".to_owned()]),
        1 => Just(vec!["lib".to_owned()]),
        1 => Just(vec!["bin".to_owned(), "example".to_owned()]),
        1 => Just(Vec::new()),
    ];

    (
        reason_strategy,
        target_name_strategy,
        kinds_strategy,
        proptest::option::of(any::<u8>()),
    )
        .prop_map(
            |(reason, target_name, kinds, executable_marker)| GeneratedMessage {
                reason,
                target_name,
                kinds,
                executable_marker,
            },
        )
}

/// Serializes generated message shapes with unique executable paths per line.
///
/// The path index keeps repeated matching artefacts distinguishable, while
/// `None` encodes Cargo messages that carry no executable.
fn cargo_stdout(messages: &[GeneratedMessage]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let executable = message
                .executable_marker
                .map(|marker| format!("/generated-target/{index}-{marker}"));
            serde_json::json!({
                "reason": &message.reason,
                "target": { "name": &message.target_name, "kind": &message.kinds },
                "executable": executable,
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Finds the last executable attached to a matching generated record.
///
/// This record-oriented oracle filters structured input before mapping optional
/// paths, independently expressing the stream contract without parsing JSON.
fn expected_executable(messages: &[GeneratedMessage]) -> Option<Utf8PathBuf> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.reason == "compiler-artifact"
                && message.target_name == "mriya"
                && message.kinds.iter().any(|kind| kind == "bin")
        })
        .filter_map(|(index, message)| {
            message
                .executable_marker
                .map(|marker| Utf8PathBuf::from(format!("/generated-target/{index}-{marker}")))
        })
        .next_back()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn parser_matches_the_last_executable_record_oracle(
        messages in proptest::collection::vec(generated_message_strategy(), 0..32)
    ) {
        let expected_path_option = expected_executable(&messages);
        let parser_result = executable_from_cargo_messages(&cargo_stdout(&messages));

        match (expected_path_option, parser_result) {
            (Some(expected_path), Ok(actual_path)) => prop_assert_eq!(actual_path, expected_path),
            (None, Err(BuildError::MissingExecutable)) => (),
            (wanted, observed) => prop_assert!(
                false,
                "expected executable {wanted:?}; parser returned {observed:?}"
            ),
        }
    }
}
