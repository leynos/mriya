//! HTTP boundary for every direct Scaleway request.
//!
//! The base URLs are fields on the backend rather than constants so a test can
//! point the whole lifecycle at a loopback server. Every request is bounded by
//! the shared per-request timeout, and every failure is classified once here so
//! the rest of the lifecycle can distinguish a 404 (evidence that a resource
//! may be absent) from a 401 or 403 (a failure to act) and from a 5xx (an
//! outcome that may still have been applied).

use std::future::Future;

/// A paginated listing's zone, project, and optional tag filter.
#[derive(Clone, Copy, Debug)]
pub(in crate::scaleway) struct ListQuery<'a> {
    /// Zone the listing is scoped to.
    pub(in crate::scaleway) zone: &'a str,
    /// Project the listing is scoped to.
    pub(in crate::scaleway) project_id: &'a str,
    /// Optional tag every returned item must carry.
    pub(in crate::scaleway) tag: Option<&'a str>,
}

/// Number of items requested per list page.
pub(in crate::scaleway) const LIST_PAGE_SIZE: u32 = 100;

/// Safety bound on pagination, so a provider that ignores `page` cannot make a
/// listing loop forever.
pub(in crate::scaleway) const MAX_LIST_PAGES: u32 = 100;

/// Collects pages until the provider returns an empty page.
///
/// The bound exists so a provider that ignores `page` cannot loop forever. One
/// terminal probe follows the last permitted populated page, which lets a
/// listing whose final content page is the bound itself still end normally.
///
/// # Errors
///
/// Returns the first page failure, and a provider error when the listing does
/// not terminate within [`MAX_LIST_PAGES`].
pub(in crate::scaleway) async fn collect_pages<T, F, Fut>(
    operation: &str,
    mut fetch: F,
) -> Result<Vec<T>, ScalewayBackendError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<Vec<T>, ScalewayBackendError>>,
{
    let mut page = 1;
    let mut items = Vec::new();
    loop {
        let batch = fetch(page).await?;
        if batch.is_empty() {
            return Ok(items);
        }
        items.extend(batch);
        page += 1;
        if page > MAX_LIST_PAGES + 1 {
            return Err(ScalewayBackendError::Provider {
                message: format!("{operation} did not terminate after {MAX_LIST_PAGES} pages"),
            });
        }
    }
}

use reqwest::{RequestBuilder, Response, StatusCode};
use scaleway_rs::ScalewayApiError;
use serde::de::DeserializeOwned;

use super::super::{ScalewayBackend, ScalewayBackendError};

/// Adds the operation name to a transport-level request failure.
///
/// For example, a connection timeout names the operation before
/// `Request: <cause>`, so a report can say which call failed.
pub(in crate::scaleway) fn request_error(
    operation: &str,
    error: &reqwest::Error,
) -> ScalewayBackendError {
    ScalewayBackendError::Provider {
        message: format!("{operation}: Request: {error}"),
    }
}

/// Extracts the provider's own message from a response body.
///
/// For example, `{"type":"denied","message":"no access"}` yields
/// `denied - no access`, while a non-JSON body is returned trimmed.
pub(in crate::scaleway) fn provider_message(body: &str) -> String {
    serde_json::from_str::<ScalewayApiError>(body).map_or_else(
        |_| body.trim().to_owned(),
        |error| format!("{} - {}", error.etype, error.message),
    )
}

/// Maps a non-success response to the backend error that describes it.
///
/// For example, a 404 becomes [`ScalewayBackendError::NotFound`], a 403
/// becomes [`ScalewayBackendError::AccessDenied`], and any other status
/// becomes a [`ScalewayBackendError::Provider`] naming the operation and
/// status.
pub(in crate::scaleway) fn classify_error(
    operation: &str,
    status: StatusCode,
    body: &str,
) -> ScalewayBackendError {
    let message = provider_message(body);
    match status.as_u16() {
        401 | 403 => ScalewayBackendError::AccessDenied {
            operation: operation.to_owned(),
            status: status.as_u16(),
            message,
        },
        404 => ScalewayBackendError::NotFound {
            operation: operation.to_owned(),
            message,
        },
        _ => ScalewayBackendError::Provider {
            message: format!(
                "{operation} failed with HTTP {}: {message}",
                status.as_u16()
            ),
        },
    }
}

/// Sends `request`, mapping a non-success status through [`classify_error`].
///
/// # Errors
///
/// Returns a transport [`ScalewayBackendError::Provider`] when no response
/// arrives, and the classified error for a non-success status.
pub(in crate::scaleway) async fn checked_response(
    operation: &str,
    request: RequestBuilder,
) -> Result<Response, ScalewayBackendError> {
    let response = request
        .timeout(super::HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|error| request_error(operation, &error))?;

    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    // Keep the status even when the body cannot be read; the read failure is
    // added to the provider message instead of replacing the classification.
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("response body could not be read: {error}"));
    Err(classify_error(operation, status, &body))
}

/// Sends `request` and decodes a successful JSON response.
///
/// # Errors
///
/// Returns the same failures as [`checked_response`], and a provider error
/// when a successful body cannot be decoded.
pub(in crate::scaleway) async fn checked_json<T: DeserializeOwned>(
    operation: &str,
    request: RequestBuilder,
) -> Result<T, ScalewayBackendError> {
    let response = checked_response(operation, request).await?;
    response
        .json::<T>()
        .await
        .map_err(|error| request_error(operation, &error))
}

impl ScalewayBackend {
    /// Builds an Instances API URL for `zone` and a path suffix.
    pub(in crate::scaleway) fn instance_url(&self, zone: &str, suffix: &str) -> String {
        format!("{}/zones/{zone}/{suffix}", self.instance_api_base)
    }

    /// Builds a Block Storage API URL for `zone` and a path suffix.
    pub(in crate::scaleway) fn block_url(&self, zone: &str, suffix: &str) -> String {
        format!("{}/zones/{zone}/{suffix}", self.block_api_base)
    }

    /// Adds the authentication header every provider call needs.
    pub(in crate::scaleway) fn authorized(&self, request: RequestBuilder) -> RequestBuilder {
        request.header("X-Auth-Token", &self.config.secret_key)
    }
}

#[cfg(test)]
mod tests {
    //! Exercises error classification and the request budget.

    use std::time::Duration;

    use reqwest::StatusCode;
    use tokio::net::TcpListener;

    use super::{checked_response, classify_error};
    use crate::scaleway::ScalewayBackendError;

    #[test]
    fn a_denied_response_is_classified_as_access_denied() {
        let error = classify_error(
            "delete server",
            StatusCode::FORBIDDEN,
            r#"{"type":"denied","message":"no access"}"#,
        );
        assert_eq!(
            error,
            ScalewayBackendError::AccessDenied {
                operation: String::from("delete server"),
                status: 403,
                message: String::from("denied - no access"),
            }
        );
    }

    #[test]
    fn a_not_found_response_is_classified_as_not_found() {
        let error = classify_error("get volume", StatusCode::NOT_FOUND, "missing");
        assert_eq!(
            error,
            ScalewayBackendError::NotFound {
                operation: String::from("get volume"),
                message: String::from("missing"),
            }
        );
    }

    #[test]
    fn an_unexpected_status_keeps_the_provider_message() {
        let error = classify_error("create server", StatusCode::BAD_REQUEST, "bad request");
        assert!(
            matches!(
                error,
                ScalewayBackendError::Provider { ref message }
                    if message.contains("400") && message.contains("bad request")
            ),
            "{error:?}"
        );
    }

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
            checked_response("probe", client.get(format!("http://{address}/"))).await
        });

        let (server_stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("request should reach the local listener before the real deadline")
            .expect("listener should accept the request");
        tokio::time::pause();
        tokio::time::advance(super::super::HTTP_TIMEOUT + Duration::from_secs(1)).await;

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
                    if message.contains("Request:")
            ),
            "expected a request provider error after the deadline: {request_result:?}"
        );
        drop(server_stream);
    }
}
