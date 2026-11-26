//! Zone product catalogue lookups for the Scaleway backend.
//!
//! The Instances API exposes each zone's instance types at
//! `GET /instance/v1/zones/{zone}/products/servers`, keyed by commercial type,
//! with the volume constraints the create call enforces. Reading the catalogue
//! before a create is what lets Mriya refuse an impossible boot-storage
//! request without allocating anything.
//!
//! Only the fields a boot-storage decision needs are decoded. The response is
//! large and mostly pricing and network metadata, so a minimal model keeps the
//! contract visible and tolerates fields the provider adds later.

use std::collections::HashMap;

use serde::Deserialize;

use super::super::{ScalewayBackend, ScalewayBackendError};

/// One size range reported for a volume constraint.
///
/// # Examples
///
/// ```ignore
/// let sizes: ConstraintSizes = serde_json::from_str(r#"{"min_size":10000000000,"max_size":100000000000}"#)?;
/// assert_eq!(sizes.min_size, 10_000_000_000);
/// assert_eq!(sizes.max_size, 100_000_000_000);
/// ```
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::scaleway) struct ConstraintSizes {
    /// Smallest volume the instance type accepts, in bytes.
    pub(crate) min_size: u64,
    /// Largest volume the instance type accepts, in bytes.
    pub(crate) max_size: u64,
}

/// Volume constraints broken down by storage type.
///
/// The provider documents only `l_ssd` here; Block Storage is bounded by the
/// general `volumes_constraint` range instead.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::scaleway) struct ConstraintsByType {
    /// Constraint for local SSD volumes, when the type offers them.
    #[serde(default)]
    pub(crate) l_ssd: Option<ConstraintSizes>,
}

/// The subset of a zone `ServerType` that a boot decision needs.
///
/// # Examples
///
/// ```ignore
/// let t: ProductServerType = serde_json::from_str(
///     r#"{"volumes_constraint":{"min_size":10000000000,"max_size":100000000000},
///         "arch":"x86_64","ncpus":2,"ram":2147483648,"end_of_service":false,
///         "capabilities":{"block_storage":true}}"#,
/// )?;
/// assert!(t.supports_block_storage());
/// ```
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(in crate::scaleway) struct ProductServerType {
    /// Range every volume on this type must fall within.
    pub(crate) volumes_constraint: ConstraintSizes,
    /// Additional per-type constraints, empty when none are published.
    #[serde(default)]
    pub(crate) per_volume_constraint: Option<ConstraintsByType>,
    /// CPU architecture the type provides.
    #[serde(default)]
    pub(crate) arch: String,
    /// True once Scaleway has announced the type's retirement.
    #[serde(default)]
    pub(crate) end_of_service: bool,
    /// Capabilities advertised for the type.
    #[serde(default)]
    pub(crate) capabilities: ProductCapabilities,
}

/// Capabilities advertised for an instance type.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
pub(in crate::scaleway) struct ProductCapabilities {
    /// True when the type can attach Block Storage volumes.
    ///
    /// Absent in the catalogue for older types, which is why
    /// [`ProductServerType::supports_block_storage`] treats absence as
    /// "unknown" and does not refuse on it alone.
    #[serde(default)]
    pub(crate) block_storage: Option<bool>,
}

impl ProductServerType {
    /// Reports whether the type accepts a local SSD volume.
    ///
    /// A type offers local storage only when the catalogue publishes an
    /// `l_ssd` constraint for it.
    ///
    /// For example, a type with no `per_volume_constraint.l_ssd` returns
    /// `false`, so a local plan against it is refused before the create.
    pub(in crate::scaleway) const fn local_constraint(&self) -> Option<ConstraintSizes> {
        match self.per_volume_constraint {
            Some(by_type) => by_type.l_ssd,
            None => None,
        }
    }

    /// Reports whether the type explicitly advertises Block Storage.
    ///
    /// For example, a type whose catalogue entry says
    /// `"block_storage": true` returns `true`, while one that omits the
    /// capability returns `false` rather than claiming support.
    pub(in crate::scaleway) const fn supports_block_storage(&self) -> bool {
        matches!(self.capabilities.block_storage, Some(true))
    }
}

/// The `products/servers` page: commercial type name to catalogue entry.
///
/// # Examples
///
/// ```ignore
/// let page: ProductServersResponse = serde_json::from_str(
///     r#"{"servers":{"DEV1-S":{"volumes_constraint":{"min_size":1,"max_size":2}}}}"#,
/// )?;
/// assert!(page.servers.contains_key("DEV1-S"));
/// ```
#[derive(Debug, Deserialize)]
pub(in crate::scaleway) struct ProductServersResponse {
    /// Catalogue entries keyed by commercial type.
    pub(crate) servers: HashMap<String, ProductServerType>,
}

impl ScalewayBackend {
    /// Reads the zone's instance-type catalogue, following pagination.
    ///
    /// The endpoint returns a map rather than a list, so a page that repeats
    /// a key merges rather than duplicates. Pagination stops on an empty page,
    /// matching `list_instance`'s contract.
    ///
    /// # Errors
    ///
    /// Returns a provider error when a request fails, or
    /// [`ScalewayBackendError::ProductCatalogueUnavailable`] when the catalogue
    /// never reports the requested type.
    ///
    /// For example, a catalogue listing `DEV1-S` yields its constraints, while
    /// one that omits it reports the type as unavailable in that zone.
    pub(in crate::scaleway) async fn fetch_server_type(
        &self,
        zone: &str,
        instance_type: &str,
    ) -> Result<ProductServerType, ScalewayBackendError> {
        let url = format!("{}/zones/{zone}/products/servers", self.instance_api_base);
        let mut page = 1_u64;
        loop {
            let response = super::http::checked_response(
                super::HTTP_CLIENT
                    .get(&url)
                    .header("X-Auth-Token", &self.config.secret_key)
                    .query(&[("page", page.to_string()), ("per_page", "100".to_owned())]),
            )
            .await?;
            let decoded = response
                .json::<ProductServersResponse>()
                .await
                .map_err(|error| super::http::request_error(&error))?;

            if decoded.servers.is_empty() {
                break;
            }
            if let Some(product) = decoded.servers.get(instance_type) {
                return Ok(product.clone());
            }
            page += 1;
        }

        Err(ScalewayBackendError::ProductCatalogueUnavailable {
            instance_type: instance_type.to_owned(),
            zone: zone.to_owned(),
        })
    }
}
