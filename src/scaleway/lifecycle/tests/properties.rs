//! Bounded generated snapshot histories checked against declarative outcomes.
//!
//! Histories have at most 16 observations and run on a paused Tokio clock.
//! This checks ordering and terminal errors; it is sampled evidence, not a proof.

use std::collections::VecDeque;
use std::future::ready;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use proptest::prelude::*;

use super::super::wait::{PollSettings, poll_for_public_ip, poll_until_gone};
use crate::backend::InstanceNetworking;
use crate::scaleway::ScalewayBackendError;

#[derive(Clone, Debug)]
enum Observation {
    Absent,
    Snapshot {
        state: &'static str,
        address: Option<String>,
    },
    Error,
}

#[derive(Debug, Eq, PartialEq)]
enum Outcome {
    Ready(IpAddr, u16),
    Gone,
    MissingIp(String),
    Timeout(String, String),
    Residual(String),
    Provider(String),
    Unexpected(String),
}

fn observations() -> impl Strategy<Value = Vec<Observation>> {
    let address_strategy = prop::option::of(prop_oneof![
        any::<[u8; 4]>().prop_map(|bytes| Ipv4Addr::from(bytes).to_string()),
        any::<[u16; 8]>().prop_map(|segments| Ipv6Addr::from(segments).to_string()),
        Just(String::from("invalid-address")),
    ]);
    let snapshot = (
        prop::sample::select(vec!["starting", "stopped", "running"]),
        address_strategy,
    )
        .prop_map(|(state, address)| Observation::Snapshot { state, address });
    prop::collection::vec(
        prop_oneof![
            Just(Observation::Absent),
            snapshot,
            Just(Observation::Error)
        ],
        0..=16,
    )
}

fn fetch_result(
    observation: Observation,
) -> Result<Option<super::InstanceSnapshot>, ScalewayBackendError> {
    match observation {
        Observation::Absent => Ok(None),
        Observation::Snapshot { state, address } => Ok(Some(super::snapshot(
            "id",
            state,
            Vec::<crate::scaleway::types::Action>::new(),
            address.as_deref(),
        ))),
        Observation::Error => Err(ScalewayBackendError::Provider {
            message: String::from("scripted failure"),
        }),
    }
}

fn readiness_model(script: &[Observation], port: u16) -> Outcome {
    let terminal = script.iter().find_map(|observation| match observation {
        Observation::Error => Some(Outcome::Provider(String::from("scripted failure"))),
        Observation::Snapshot {
            state: "running",
            address: Some(address),
        } => address
            .parse::<IpAddr>()
            .ok()
            .map(|ip| Outcome::Ready(ip, port)),
        _ => None,
    });
    terminal.unwrap_or_else(|| {
        if script.iter().any(|observation| {
            matches!(
                observation,
                Observation::Snapshot {
                    state: "running",
                    ..
                }
            )
        }) {
            Outcome::MissingIp(String::from("id"))
        } else {
            Outcome::Timeout(String::from("wait_for_ready"), String::from("id"))
        }
    })
}

fn teardown_model(script: &[Observation]) -> Outcome {
    script
        .iter()
        .find_map(|observation| match observation {
            Observation::Absent => Some(Outcome::Gone),
            Observation::Error => Some(Outcome::Provider(String::from("scripted failure"))),
            Observation::Snapshot { .. } => None,
        })
        .unwrap_or_else(|| Outcome::Residual(String::from("id")))
}

fn error_outcome(error: ScalewayBackendError) -> Outcome {
    match error {
        ScalewayBackendError::MissingPublicIp { instance_id } => Outcome::MissingIp(instance_id),
        ScalewayBackendError::Timeout {
            action,
            instance_id,
        } => Outcome::Timeout(action, instance_id),
        ScalewayBackendError::ResidualResource { instance_id } => Outcome::Residual(instance_id),
        ScalewayBackendError::Provider { message } => Outcome::Provider(message),
        other => Outcome::Unexpected(other.to_string()),
    }
}

fn ready_outcome(result: Result<InstanceNetworking, ScalewayBackendError>) -> Outcome {
    match result {
        Ok(networking) => Outcome::Ready(networking.public_ip, networking.ssh_port),
        Err(error) => error_outcome(error),
    }
}

fn gone_outcome(result: Result<(), ScalewayBackendError>) -> Outcome {
    match result {
        Ok(()) => Outcome::Gone,
        Err(error) => error_outcome(error),
    }
}

proptest! {
    #[test]
    fn readiness_matches_first_terminal_observation(script in observations(), port in any::<u16>()) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time().start_paused(true).build()
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        let expected = readiness_model(&script, port);
        let mut queue = VecDeque::from(script);
        let actual = runtime.block_on(async {
            let handle = super::handle();
            let settings = PollSettings {
                ssh_port: port,
                poll_interval: Duration::from_millis(1),
                wait_timeout: Duration::from_millis(100),
            };
            poll_for_public_ip(&handle, settings, || {
                ready(fetch_result(queue.pop_front().unwrap_or(Observation::Absent)))
            }).await
        });
        prop_assert_eq!(ready_outcome(actual), expected);
    }

    #[test]
    fn teardown_matches_first_absence_or_error(script in observations()) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time().start_paused(true).build()
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        let expected = teardown_model(&script);
        let mut queue = VecDeque::from(script);
        // Exhausted scripts keep the resource present so only an explicit
        // absence may settle teardown, and empty scripts exercise timeout.
        let actual = runtime.block_on(async {
            let handle = super::handle();
            poll_until_gone(&handle, Duration::from_millis(1), Duration::from_millis(100), || {
                ready(fetch_result(queue.pop_front().unwrap_or(Observation::Snapshot {
                    state: "stopped", address: None,
                })))
            }).await
        });
        prop_assert_eq!(gone_outcome(actual), expected);
    }
}
