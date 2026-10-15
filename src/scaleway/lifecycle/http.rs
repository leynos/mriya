//! HTTP boundary for snapshot polling, instance deletion, and power-on actions.
//!
//! Keeping the endpoint private lets tests substitute a loopback API without
//! altering global environment variables or calling the live provider.

use reqwest::{RequestBuilder, Response};
use scaleway_rs::{ScalewayApiError, ScalewayError, ScalewayInstance};
use serde::Deserialize;

use crate::backend::InstanceHandle;
use crate::scaleway::types::Zone;
use crate::scaleway::{ScalewayBackend, ScalewayBackendError};

#[derive(Deserialize)]
struct ListInstancesResponse {
    servers: Vec<ScalewayInstance>,
}

/// Adds request context when a transport or response-body operation fails.
///
/// For example, a connection timeout becomes a provider error prefixed
/// `Request:`.
fn request_error(error: &reqwest::Error) -> ScalewayBackendError {
    ScalewayBackendError::Provider {
        message: format!("Request: {error}"),
    }
}

/// Preserves the SDK's structured client-error messages and HTTP status checks.
///
/// For example, a 403 `{"type":"denied","message":"no access"}` response
/// becomes a provider error with the message `Api: denied - no access`.
async fn checked_response(request: RequestBuilder) -> Result<Response, ScalewayBackendError> {
    // Retain the per-request budget if the shared client fell back to an
    // unconfigured client after its builder failed.
    let response = request
        .timeout(super::HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|error| request_error(&error))?;
    if response.status().is_client_error() {
        let error = response
            .json::<ScalewayApiError>()
            .await
            .map_err(|error| request_error(&error))?;
        return Err(ScalewayError::Api(error).into());
    }
    response
        .error_for_status()
        .map_err(|error| request_error(&error))
}

impl ScalewayBackend {
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
        let url = format!(
            "{}/zones/{}/servers/{instance_id}/action",
            self.instance_api_base,
            zone.as_str(),
        );
        checked_response(
            super::HTTP_CLIENT
                .post(&url)
                .header("X-Auth-Token", &self.config.secret_key)
                .json(&serde_json::json!({"action": "poweron"})),
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

    use super::super::HTTP_TIMEOUT;
    use super::checked_response;
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
