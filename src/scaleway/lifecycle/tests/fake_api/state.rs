//! State and failure knobs held by the fake provider.

use std::collections::BTreeMap;

/// Scripted provider failures.
///
/// Each switch injects one independent fault, so they do not form a state
/// machine that a single enum could express.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each switch injects one independent provider fault"
)]
#[derive(Clone, Debug, Default)]
pub(crate) struct Knobs {
    /// Accept a create, apply it, then close without a response.
    pub(crate) drop_create_response: bool,
    /// Answer every server delete with 403.
    pub(crate) deny_server_delete: bool,
    /// Answer the volume attachment PATCH with 403.
    pub(crate) deny_volume_attach: bool,
    /// Answer the power-on action with 400.
    pub(crate) fail_power_on: bool,
    /// Create servers without a root volume.
    pub(crate) omit_root_volume: bool,
    /// Items per list page; zero returns everything on one page.
    pub(crate) page_size: usize,
    /// Return a server whose identifier differs from the stored server.
    pub(crate) wrong_list_id: bool,
}

/// One server held by the fake.
#[derive(Clone, Debug)]
pub(crate) struct FakeServer {
    /// Provider identifier.
    pub(crate) id: String,
    /// Zone the server belongs to.
    pub(crate) zone: String,
    /// Project that owns the server.
    pub(crate) project: String,
    /// Current state.
    pub(crate) state: String,
    /// Provider tags.
    pub(crate) tags: Vec<String>,
    /// Attached volumes keyed by index.
    pub(crate) volumes: BTreeMap<String, FakeVolume>,
    /// Attached public IPs.
    pub(crate) ips: Vec<FakeIp>,
}

/// One Instances API volume held by the fake.
#[derive(Clone, Debug)]
pub(crate) struct FakeVolume {
    /// Provider identifier.
    pub(crate) id: String,
    /// Zone the volume belongs to.
    pub(crate) zone: String,
    /// Provider volume type.
    pub(crate) volume_type: String,
    /// Server the volume is attached to.
    pub(crate) attached_to: Option<String>,
}

/// One public IP held by the fake.
#[derive(Clone, Debug)]
pub(crate) struct FakeIp {
    /// Provider identifier.
    pub(crate) id: String,
    /// Dotted address.
    pub(crate) address: String,
    /// Whether the address is dynamic.
    pub(crate) dynamic: bool,
}

/// Entire fake provider state.
#[derive(Clone, Debug, Default)]
pub(crate) struct FakeState {
    /// Servers by identifier.
    pub(crate) servers: BTreeMap<String, FakeServer>,
    /// Instances API volumes by identifier.
    pub(crate) instance_volumes: BTreeMap<String, FakeVolume>,
    /// Block Storage volumes by identifier.
    pub(crate) block_volumes: BTreeMap<String, FakeVolume>,
    /// Public IPs by identifier.
    pub(crate) ips: BTreeMap<String, FakeIp>,
    /// Request lines observed, as `METHOD /path`.
    pub(crate) requests: Vec<String>,
    /// Identifier counter.
    pub(crate) next_id: u32,
    /// Scripted failures.
    pub(crate) knobs: Knobs,
}

impl FakeState {
    /// Returns the next identifier with `prefix`.
    pub(crate) fn next_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    /// Records `method` and `path` for later assertions.
    pub(crate) fn record(&mut self, method: &str, path: &str) {
        self.requests.push(format!("{method} {path}"));
    }
}

/// Returns the items for one page.
pub(crate) fn page_items<T>(items: &[T], page: usize, page_size: usize) -> Vec<&T> {
    if page_size == 0 {
        return if page == 1 {
            items.iter().collect()
        } else {
            Vec::new()
        };
    }
    let start = (page.saturating_sub(1)).saturating_mul(page_size);
    items.iter().skip(start).take(page_size).collect()
}
