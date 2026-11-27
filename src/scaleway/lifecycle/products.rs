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

use std::collections::{HashMap, HashSet};

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
            // A published range with no positive capacity is not local
            // storage: treating it as such would select it by default and then
            // fail, instead of falling back to Block Storage.
            Some(by_type) => match by_type.l_ssd {
                Some(sizes) if sizes.max_size > 0 => Some(sizes),
                _ => None,
            },
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

/// The `products/servers` page: commercial type name to raw catalogue entry.
///
/// Entries stay undecoded until one is actually needed. The page describes
/// every type in the zone, including shapes this backend does not model, and
/// deserializing all of them would let one unfamiliar entry fail a lookup for
/// a perfectly ordinary type.
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
    /// Catalogue entries keyed by commercial type, still undecoded.
    pub(crate) servers: HashMap<String, serde_json::Value>,
}

/// What one catalogue page contributed.
enum PageOutcome {
    /// The requested type was on this page, still undecoded.
    Found(serde_json::Value),
    /// The page added nothing new, so pagination is finished.
    Exhausted,
    /// The page added types; another page may hold more.
    More,
}

/// Records one page's keys and reports whether the requested type was on it.
///
/// `seen` accumulates every key returned so far, which is how a map-shaped
/// response detects the end of pagination.
fn take_page(
    servers: HashMap<String, serde_json::Value>,
    instance_type: &str,
    seen: &mut HashSet<String>,
) -> PageOutcome {
    let mut added_any = false;
    for (name, product) in servers {
        if name == instance_type {
            return PageOutcome::Found(product);
        }
        added_any |= seen.insert(name);
    }
    if added_any {
        PageOutcome::More
    } else {
        PageOutcome::Exhausted
    }
}

/// Decodes one catalogue entry, naming the type when the shape is unfamiliar.
///
/// # Errors
///
/// Returns [`ScalewayBackendError::ProductCatalogueUnavailable`] with the
/// decoding failure attached, so an operator can tell "the provider does not
/// list this type" from "the entry changed shape".
fn decode_server_type(
    raw: &serde_json::Value,
    zone: &str,
    instance_type: &str,
) -> Result<ProductServerType, ScalewayBackendError> {
    serde_json::from_value(raw.clone()).map_err(|error| {
        ScalewayBackendError::ProductCatalogueUnavailable {
            instance_type: format!("{instance_type} (unreadable catalogue entry: {error})"),
            zone: zone.to_owned(),
        }
    })
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
        let mut seen: HashSet<String> = HashSet::new();
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

            // The catalogue is a map rather than a list, so progress is
            // measured by keys the page had not already shown. A page that
            // adds nothing means the endpoint either finished or ignored the
            // page parameter; either way, continuing would loop forever.
            match take_page(decoded.servers, instance_type, &mut seen) {
                PageOutcome::Found(raw) => return decode_server_type(&raw, zone, instance_type),
                PageOutcome::Exhausted => break,
                PageOutcome::More => page += 1,
            }
        }

        Err(ScalewayBackendError::ProductCatalogueUnavailable {
            instance_type: instance_type.to_owned(),
            zone: zone.to_owned(),
        })
    }
}
