//! Server reads, paginated listings, and identity-checked absence checks.

use super::{
    HTTP_CLIENT,
    http::{LIST_PAGE_SIZE, ListQuery, checked_json, collect_pages},
    models::{Presence, ServerList, ServerModel, ServerResponse},
};
use crate::{
    ownership::ResourceRef,
    scaleway::{ScalewayBackend, ScalewayBackendError},
};

impl ScalewayBackend {
    /// Reads one server through the substitutable HTTP boundary.
    ///
    /// For example, a `{"server": {...}}` response yields that server, while a
    /// provider denial becomes an `AccessDenied` error.
    pub(in crate::scaleway) async fn get_server(
        &self,
        zone: &str,
        server_id: &str,
    ) -> Result<ServerModel, ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("servers/{server_id}"));
        let response: ServerResponse =
            checked_json("get server", self.authorized(HTTP_CLIENT.get(&url))).await?;
        Ok(response.server)
    }

    /// Lists servers in `zone`, optionally keeping only those tagged `tag`.
    ///
    /// The collected pages are filtered locally: a server must agree with the
    /// requested zone and project when it reports them, and must carry `tag`
    /// exactly. Filtering locally is what makes a provider-side tag or zone
    /// filter advisory rather than trusted, and filtering after pagination
    /// keeps a filtered-out page from ending the listing early.
    ///
    /// # Errors
    ///
    /// Returns the first page failure, or a provider error when pagination does
    /// not terminate.
    pub(in crate::scaleway) async fn list_servers(
        &self,
        zone: &str,
        project_id: &str,
        tag: Option<&str>,
    ) -> Result<Vec<ServerModel>, ScalewayBackendError> {
        let query = ListQuery {
            zone,
            project_id,
            tag,
        };
        let page_items =
            collect_pages("list servers", |page| self.fetch_server_page(query, page)).await?;
        Ok(page_items
            .into_iter()
            .filter(|server| server.is_in_zone(zone))
            .filter(|server| server.is_in_project(project_id))
            .filter(|server| tag.is_none_or(|wanted| server.has_tag(wanted)))
            .collect())
    }

    /// Fetches one page of [`ScalewayBackend::list_servers`].
    async fn fetch_server_page(
        &self,
        query: ListQuery<'_>,
        page: u32,
    ) -> Result<Vec<ServerModel>, ScalewayBackendError> {
        let url = self.instance_url(query.zone, "servers");
        let page_text = page.to_string();
        let mut request = self.authorized(HTTP_CLIENT.get(&url)).query(&[
            ("project", query.project_id),
            ("per_page", &LIST_PAGE_SIZE.to_string()),
            ("page", &page_text),
        ]);
        if let Some(filter) = query.tag {
            request = request.query(&[("tags", filter)]);
        }

        let response: ServerList = checked_json("list servers", request).await?;
        Ok(response.servers)
    }

    /// Checks whether a recorded server still exists.
    ///
    /// A 404 is only evidence: absence is confirmed by a successful,
    /// identity-checked listing in the recorded zone and project. A denial or
    /// any other failure is returned as an error, because cleanup cannot
    /// report a resource gone without proof.
    ///
    /// # Errors
    ///
    /// Returns the provider failure that prevented the check, including an
    /// identity mismatch between the recorded and returned resource.
    pub(in crate::scaleway) async fn server_presence(
        &self,
        resource: &ResourceRef,
        project_id: &str,
    ) -> Result<Presence<ServerModel>, ScalewayBackendError> {
        let zone = resource.zone();
        match self.get_server(zone, resource.id()).await {
            Ok(server) => {
                verify_server_identity(
                    &server,
                    "get server",
                    ExpectedServer {
                        id: resource.id(),
                        zone,
                        project: project_id,
                    },
                )?;
                Ok(Presence::Present(server))
            }
            Err(ScalewayBackendError::NotFound { .. }) => {
                // The unfiltered listing is deliberate: a row whose ID matches
                // but whose scope contradicts the record must be an error, and
                // the filtered listing would have dropped it before the check.
                let servers = self.list_servers_unfiltered(zone, project_id).await?;
                let Some(server) = servers
                    .into_iter()
                    .find(|server| server.id == resource.id())
                else {
                    return Ok(Presence::ConfirmedAbsent);
                };
                verify_server_identity(
                    &server,
                    "list servers",
                    ExpectedServer {
                        id: resource.id(),
                        zone,
                        project: project_id,
                    },
                )?;
                Ok(Presence::Present(server))
            }
            Err(error) => Err(error),
        }
    }

    /// Lists servers in `zone` without applying the local scope or tag filters.
    ///
    /// The presence check uses this so a row that contradicts the recorded
    /// scope is still visible to the identity check.
    async fn list_servers_unfiltered(
        &self,
        zone: &str,
        project_id: &str,
    ) -> Result<Vec<ServerModel>, ScalewayBackendError> {
        let query = ListQuery {
            zone,
            project_id,
            tag: None,
        };
        collect_pages("list servers", |page| self.fetch_server_page(query, page)).await
    }

    /// Deletes a server and leaves verification to the caller.
    ///
    /// # Errors
    ///
    /// Returns a classified provider error; a 404 means the server is already
    /// gone, which the caller must still verify by listing.
    pub(in crate::scaleway) async fn delete_server(
        &self,
        zone: &str,
        server_id: &str,
    ) -> Result<(), ScalewayBackendError> {
        let url = self.instance_url(zone, &format!("servers/{server_id}"));
        super::http::checked_response("delete server", self.authorized(HTTP_CLIENT.delete(&url)))
            .await?;
        Ok(())
    }
}

/// Identity of a fetched resource, checked against the request that named it.
#[derive(Clone, Copy, Debug)]
pub(in crate::scaleway) struct FetchedIdentity<'a> {
    /// Operation that returned the resource.
    pub(in crate::scaleway) operation: &'a str,
    /// Identifier the provider returned.
    pub(in crate::scaleway) id: &'a str,
    /// Zone the provider reported, possibly empty.
    pub(in crate::scaleway) zone: &'a str,
    /// Project the provider reported, possibly empty.
    pub(in crate::scaleway) project: &'a str,
}

/// The server a read was asked for, as recorded by the run.
#[derive(Clone, Copy, Debug)]
pub(in crate::scaleway) struct ExpectedServer<'a> {
    /// Identifier the run recorded.
    pub(in crate::scaleway) id: &'a str,
    /// Zone the run recorded.
    pub(in crate::scaleway) zone: &'a str,
    /// Project the run recorded.
    pub(in crate::scaleway) project: &'a str,
}

/// Rejects a fetched server whose identifier or scope contradicts the record.
///
/// The identifier check matters because a zone- and project-scoped read that
/// returns a different server would otherwise let cleanup power off, detach
/// from, and delete a server the run never created.
pub(in crate::scaleway) fn verify_server_identity(
    server: &ServerModel,
    operation: &str,
    expected: ExpectedServer<'_>,
) -> Result<(), ScalewayBackendError> {
    if server.id != expected.id {
        return Err(ScalewayBackendError::Provider {
            message: format!(
                "{operation} returned {}, but {expected_id} was requested",
                server.id,
                expected_id = expected.id
            ),
        });
    }
    ensure_identity(
        FetchedIdentity {
            operation,
            id: &server.id,
            zone: &server.zone,
            project: &server.project,
        },
        expected.zone,
        expected.project,
    )
}

/// Rejects a fetched resource whose zone or project contradicts the request.
///
/// An omitted field is accepted: the request URL and query already scope the
/// call, and some provider responses omit these fields. A *contradicting* field
/// is a hard error, because deleting a same-identifier resource from another
/// project would be catastrophic.
pub(in crate::scaleway) fn ensure_identity(
    actual: FetchedIdentity<'_>,
    expected_zone: &str,
    expected_project: &str,
) -> Result<(), ScalewayBackendError> {
    let FetchedIdentity {
        operation,
        id,
        zone: actual_zone,
        project: actual_project,
    } = actual;
    if !actual_zone.trim().is_empty() && actual_zone != expected_zone {
        return Err(ScalewayBackendError::Provider {
            message: format!(
                "{operation} returned {id} in zone {actual_zone}, but {expected_zone} was requested"
            ),
        });
    }
    if !actual_project.trim().is_empty() && actual_project != expected_project {
        return Err(ScalewayBackendError::Provider {
            message: format!(
                "{operation} returned {id} in project {actual_project}, but {expected_project} was requested"
            ),
        });
    }
    Ok(())
}
