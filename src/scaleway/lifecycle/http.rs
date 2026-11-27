//! HTTP boundary for instance reads, snapshot polling, deletion, and power-on
//! actions.
//!
//! Keeping the endpoint private lets tests substitute a loopback API without
//! altering global environment variables or calling the live provider. The
//! instance read, list, delete, power-on, create, and volume paths all format
//! their URL from `instance_api_base` for the same reason: the SDK's own
//! helpers hard-code the production host, which leaves the caller unreachable
//! from a test. Image lookup is the one exception; it still delegates to
//! `ScalewayListInstanceImagesBuilder` on the SDK client and is covered by the
//! pure selection cases instead.

use reqwest::{RequestBuilder, Response};
use scaleway_rs::{ScalewayApiError, ScalewayError, ScalewayInstance};
use serde::Deserialize;

use crate::{
    backend::InstanceHandle,
    scaleway::{ScalewayBackend, ScalewayBackendError, types::Zone},
};

/// Response page from the list-servers call.
///
/// # Examples
///
/// ```ignore
/// // Schematic: page 1 carries one server and page 2 is empty, which ends
/// // pagination. A real `ScalewayInstance` has about thirty required fields
/// // with no serde defaults, so abbreviated JSON is not decodable; the tests
/// // use `src/scaleway/lifecycle/tests/fixtures/detach-instance.json` as a
/// // complete server.
/// {"servers": [{ ... }]}   // page 1: one server
/// {"servers": []}          // page 2: empty, so polling stops
/// ```
#[derive(Deserialize)]
struct ListInstancesResponse {
    /// Instances on this page; an empty list marks the end of pagination.
    servers: Vec<ScalewayInstance>,
}

#[derive(Deserialize)]
struct GetInstanceResponse {
    server: ScalewayInstance,
}

/// Adds request context when a transport or response-body operation fails.
///
/// For example, a connection timeout becomes a provider error prefixed
/// `Request:`.
pub(super) fn request_error(error: &reqwest::Error) -> ScalewayBackendError {
    ScalewayBackendError::Provider {
        message: format!("Request: {error}"),
    }
}

/// Sends a request with the shared client and the per-request budget.
///
/// The timeout is applied here rather than only on the client because the
/// shared client falls back to an unconfigured one when its builder fails.
pub(super) async fn send_raw(request: RequestBuilder) -> Result<Response, ScalewayBackendError> {
    request
        .timeout(super::HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|error| request_error(&error))
}

/// Converts a client-error response into the SDK-shaped provider error.
///
/// For example, a 403 `{"type":"denied","message":"no access"}` response
/// becomes a provider error with the message `Api: denied - no access`.
pub(super) async fn api_error(response: Response) -> ScalewayBackendError {
    match response.json::<ScalewayApiError>().await {
        Ok(error) => ScalewayError::Api(error).into(),
        Err(error) => request_error(&error),
    }
}

/// Preserves the SDK's structured client-error messages and HTTP status checks.
///
/// For example, a 403 `{"type":"denied","message":"no access"}` response
/// becomes a provider error with the message `Api: denied - no access`.
pub(super) async fn checked_response(
    request: RequestBuilder,
) -> Result<Response, ScalewayBackendError> {
    let response = send_raw(request).await?;
    if response.status().is_client_error() {
        return Err(api_error(response).await);
    }
    response
        .error_for_status()
        .map_err(|error| request_error(&error))
}

impl ScalewayBackend {
    /// Reads one instance through the substitutable HTTP boundary.
    ///
    /// For example, a `{"server": {...}}` response yields that server, while a
    /// provider denial becomes a `ScalewayError::Api` provider error.
    pub(in crate::scaleway) async fn get_instance(
        &self,
        handle: &InstanceHandle,
    ) -> Result<ScalewayInstance, ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/servers/{}",
            self.instance_api_base, handle.zone, handle.id,
        );
        let response = checked_response(
            super::HTTP_CLIENT
                .get(&url)
                .header("X-Auth-Token", &self.config.secret_key),
        )
        .await?;
        let result = response
            .json::<GetInstanceResponse>()
            .await
            .map_err(|error| request_error(&error))?;
        Ok(result.server)
    }

    /// Lists the selected instance using the SDK's pagination contract.
    ///
    /// For example, one matching server on page 1 followed by an empty page 2
    /// yields that server; an empty first page yields `None`.
    pub(in crate::scaleway) async fn list_instance(
        &self,
        handle: &InstanceHandle,
    ) -> Result<Option<ScalewayInstance>, ScalewayBackendError> {
        let url = format!("{}/zones/{}/servers", self.instance_api_base, handle.zone);
        let mut page = 1_u64;
        let mut last_server = None;
        loop {
            let response = checked_response(
                super::HTTP_CLIENT
                    .get(&url)
                    .header("X-Auth-Token", &self.config.secret_key)
                    .query(&[("servers", handle.id.as_str()), ("per_page", "1")])
                    .query(&[("page", page)]),
            )
            .await?;
            let mut result = response
                .json::<ListInstancesResponse>()
                .await
                .map_err(|error| request_error(&error))?;
            if result.servers.is_empty() {
                return Ok(last_server);
            }
            last_server = result.servers.pop();
            page += 1;
        }
    }

    /// Lists servers whose name matches `name` exactly.
    ///
    /// Scaleway filters by `name` and `project` server-side. The caller uses
    /// this only to reconcile a create whose outcome is unknown, so every
    /// returned server is one the provider says carries the generated name.
    pub(in crate::scaleway) async fn find_instances_named(
        &self,
        zone: &str,
        project_id: &str,
        name: &str,
    ) -> Result<Vec<ScalewayInstance>, ScalewayBackendError> {
        let url = format!("{}/zones/{zone}/servers", self.instance_api_base);
        let mut found = Vec::new();
        let mut page = 1_u64;
        loop {
            let response = checked_response(
                super::HTTP_CLIENT
                    .get(&url)
                    .header("X-Auth-Token", &self.config.secret_key)
                    .query(&[("name", name), ("project", project_id), ("per_page", "50")])
                    .query(&[("page", page)]),
            )
            .await?;
            let result = response
                .json::<ListInstancesResponse>()
                .await
                .map_err(|error| request_error(&error))?;
            if result.servers.is_empty() {
                return Ok(found);
            }
            // Scaleway's name filter is a partial match (`server1` also matches
            // `server100`), and these servers are deleted, so only an exact
            // name is a candidate.
            found.extend(
                result
                    .servers
                    .into_iter()
                    .filter(|server| server.name == name),
            );
            page += 1;
        }
    }

    /// Deletes an instance before the caller polls for its disappearance.
    ///
    /// For example, a successful 204 response returns `Ok(())`; a provider
    /// client error is returned before teardown polling starts.
    pub(in crate::scaleway) async fn delete_instance(
        &self,
        handle: &InstanceHandle,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/servers/{}",
            self.instance_api_base, handle.zone, handle.id,
        );
        checked_response(
            super::HTTP_CLIENT
                .delete(&url)
                .header("X-Auth-Token", &self.config.secret_key),
        )
        .await?;
        Ok(())
    }

    /// Issues a power-on action through the same substitutable HTTP boundary.
    ///
    /// For example, a successful action response returns `Ok(())`, while an
    /// API error retains its provider message.
    pub(in crate::scaleway) async fn power_on_instance(
        &self,
        zone: &Zone,
        instance_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        self.send_server_action(zone, instance_id, "poweron").await
    }

    /// Issues a power-off action through the same substitutable HTTP boundary.
    ///
    /// Detaching a Block Storage volume requires a stopped server, so teardown
    /// powers the instance off before it detaches the cache.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the action is rejected.
    pub(in crate::scaleway) async fn power_off_instance(
        &self,
        zone: &Zone,
        instance_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        self.send_server_action(zone, instance_id, "poweroff").await
    }

    /// Sends one server action and checks the response.
    async fn send_server_action(
        &self,
        zone: &Zone,
        instance_id: &str,
        action: &str,
    ) -> Result<(), ScalewayBackendError> {
        let url = format!(
            "{}/zones/{}/servers/{instance_id}/action",
            self.instance_api_base,
            zone.as_str(),
        );
        checked_response(
            super::HTTP_CLIENT
                .post(&url)
                .header("X-Auth-Token", &self.config.secret_key)
                .json(&serde_json::json!({ "action": action })),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Exercises HTTP request budgets against a local server that withholds responses.

    use std::time::Duration;

    use tokio::net::TcpListener;

    use super::{super::HTTP_TIMEOUT, checked_response};
    use crate::scaleway::ScalewayBackendError;

    /// Ensures an accepted request returns at its deadline if no response
    /// arrives.
    #[tokio::test]
    async fn checked_response_applies_deadline_without_response() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind to a loopback port");
        let address = listener
            .local_addr()
            .expect("test listener should expose its local address");
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client should build without a total timeout");
        let mut request_task = tokio::spawn(async move {
            checked_response(client.get(format!("http://{address}/"))).await
        });

        let (server_stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("request should reach the local listener before the real deadline")
            .expect("listener should accept the request");
        tokio::time::pause();
        tokio::time::advance(HTTP_TIMEOUT + Duration::from_secs(1)).await;

        let request_result =
            match tokio::time::timeout(Duration::from_secs(1), &mut request_task).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => panic!("request task should not panic: {error}"),
                Err(_) => {
                    request_task.abort();
                    drop(request_task.await);
                    panic!("request should return after its configured deadline");
                }
            };
        assert!(
            matches!(
                &request_result,
                Err(ScalewayBackendError::Provider { message })
                    if message.starts_with("Request:")
            ),
            "expected a request provider error after the deadline: {request_result:?}"
        );
        drop(server_stream);
    }
}
