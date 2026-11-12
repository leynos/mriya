# Backend RFCs and shared contracts

- Status: proposed.
- Date: 29 October 2026.
- Audience: Mriya implementers, reviewers, and operators.
- Baseline: `fd7d8193fae29d54867cc372dc30efdc6fab35bb`.

## Purpose and proposal index

These requests for comments (RFCs) define three additional Mriya backends. They
preserve one disposable worker per run, Git-aware workspace upload, streamed
command output, and explicit cleanup. They propose implementation contracts;
they do not add working backends or change the default provider.

Table 1 identifies each proposal's first supported workload and storage model.

| RFC                                                  | Initial workload              | Cache and product boundary                                                     |
| ---------------------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------ |
| [0001: Boat](0001-boat-backend.md)                   | Disposable x86-64 CPU sandbox | Cold disk or borrowed seed snapshot; no mutable volume cache                   |
| [0002: Hetzner Cloud](0002-hetzner-cloud-backend.md) | Disposable CPU VM             | Retained, exclusively attached cache volume; dedicated GPU servers excluded    |
| [0003: Ubicloud](0003-ubicloud-backend.md)           | Managed Compute CPU VM        | Disposable local disk; Actions runners and self-hosted control planes excluded |

_Table 1: Proposed backend scope. Each RFC cites the provider contracts and
defines separate gates for additional capabilities._

The proposals share the requirements below. Provider-specific sections refine
them; an implementation must not silently weaken them to fit a provider API.
Acceptance of one RFC does not accept every optional capability in another.

## Existing constraints

The current `Backend` trait separates creation, readiness, and destruction. Its
request still requires image, project, zone, and architecture fields, and its
connection result stores an IP address and SSH port.[^1] The command-line entry
point always loads Scaleway credentials and constructs that backend.[^2]

The existing cache abstraction cannot describe all three providers. Init uses a
Scaleway device identifier and writes Scaleway configuration; run orchestration
mounts `/dev/vdb`, and cloud-init completion follows workspace sync.[^3] The
new adapters therefore need shared changes as well as provider HTTP clients.
The current roadmap's Hetzner entry remains relevant, but the RFCs do not mark
any implementation milestone complete.[^4]

## Selection, configuration, and capabilities

Add a backend selector with precedence: command-line `--backend`,
`MRIYA_BACKEND`, `[mriya].backend`, then the existing Scaleway default. Resolve
selection before loading credentials. Reject an unknown selector locally. The
following example is **proposed**, not current configuration:

```toml
[mriya]
backend = "boat"
```

The proposed command form is `mriya run --backend boat -- cargo test`. Support
the same selection rule for init, with an explicit unsupported error when the
selected provider has no cache-initialization contract. Preserve existing
`[scaleway]`, `SCW_*`, `[sync]`, and `MRIYA_SYNC_*` configuration and the
current default when no selector exists. Keep credentials out of committed
examples and persistent run records.

Use a closed provider-request enum and selected-provider configuration at the
application boundary. Retain typed backend errors through that dispatch.
Validate only the selected variant; do not invent placeholder project or zone
identifiers to satisfy Scaleway-shaped validation. Preserve the existing public
Scaleway request through a compatibility adapter during migration.

Capabilities distinguish raw cloud-init, bounded shell bootstrap, attached
cache volumes, borrowed seed snapshots, CPU architecture, and GPU execution.
Reject a requested unsupported capability before allocating any resource. An
inherited optional cache-routing default may become inactive on a backend
without a mounted cache; an explicit request for persistent storage must fail.
Never substitute a different machine, location, wallet, or backend silently.

## Connection and readiness contract

Generalize `InstanceNetworking` to a validated DNS-or-IP host, SSH port,
provider username, and explicit host-key policy. Ports already exist; the
missing integration concerns host representation and trust. Carry the same
connection settings through readiness probes, `rsync`, and command execution.
Handle IPv6 literals and non-default ports correctly in every transport path.

Provider-authenticated host keys take precedence where available. Otherwise
require a configured trust source or an explicit trust-on-first-use policy;
document that first contact does not provide independent authentication. An
unauthenticated `ssh-keyscan` is not a trusted source. New adapters must not
inherit disabled checking or `/dev/null` known-hosts defaults accidentally.
Preserve legacy Scaleway options while making any security migration explicit.

Readiness means that the worker has a usable endpoint, authenticated SSH,
successful requested bootstrap, a writable workspace, and the tools needed for
transfer. Complete bootstrap before mounting caches or invoking `rsync`.
Provider state or an old `boot-finished` marker alone cannot establish these
conditions. Apply separate bounded deadlines to allocation, readiness,
execution, and cleanup; a provider lifetime budget includes all these phases.

The system SSH client and `rsync` remain the initial transport. Preserve
workspace filters, argument quoting, stdout/stderr streaming, and remote
command semantics. A lost transport leaves command outcome unknown; recovery
must never rerun a potentially non-idempotent command automatically.

## Resource ownership and recovery

Create a durable run ledger before the first allocation. Record provider and
account/project scope, a unique run ID, request digest, creation identity or
idempotency key, resource IDs, lifecycle actions, and cleanup observations.
Store it in an application state directory with owner-only permissions and
atomic updates. Do not store credentials, private keys, or bootstrap secrets.

Table 2 specifies the minimum ownership distinctions.

| Class           | Examples                                                   | Cleanup authority                                         |
| --------------- | ---------------------------------------------------------- | --------------------------------------------------------- |
| Owned transient | Worker, new boot volume, new Primary IP                    | Remove and verify the provider-specific postcondition     |
| Borrowed        | Existing key, subnet, image, seed, supplied cache          | Preserve; undo only this run's attachment or lease        |
| Retained        | Successfully initialized cache or deliberately saved image | Preserve, report ongoing charges, and record its identity |
| Unresolved      | Create response lost before ownership can be established   | Reconcile and report; never adopt or delete by name alone |

_Table 2: Resource ownership governs cleanup. A matching label helps discovery
but does not independently authorize deletion._

The adapter owns rollback after partial creation, including failures before it
can return an `InstanceHandle`. Record each allocation as soon as identity
becomes available. Keep the same creation identity across retries, and respect
each provider's idempotency window. Serialize recovery of a ledger entry. If
provider evidence cannot resolve a lost response safely, retain an explicit
unresolved record instead of retrying with a fresh identity.

After normal execution, failure, or cancellation, perform provider-supported
cleanup in dependency order. Preserve borrowed caches and seed images. Treat
provider-confirmed absence as success only in the expected scope; a permission
error, empty filtered list, or accepted asynchronous request is insufficient.
Keep resource or operation IDs when a deadline expires.

Provide a provider-aware recovery command or janitor before enabling a new
backend for unattended use. The current janitor only handles tagged Scaleway
test resources and cannot recover these providers.[^5] Recovery must support
listing unresolved records without mutation, resuming one owned cleanup, and
reporting resources that still incur charges. A guest shutdown or local
deadline does not imply that provider billing stopped.

## Cache and command outcomes

Replace hard-coded guest device paths with provider-supplied, validated device
metadata. Confirm the mounted filesystem belongs to the requested cache and
that the execution user can write it. Format only a newly allocated volume
owned by the init transaction. A required cache failure fails the run. Scope
leases by provider, account, location, and volume identity; an API attachment
conflict remains authoritative across separate clients.

Keep provider settings separate when writing initialized cache IDs. If a
successfully formatted cache cannot be written to configuration, return its
identifier and retain it for recovery. Do not erase usable data because a local
config update failed. A newly created, unusable cache can be removed only while
the ledger still identifies it as owned by that transaction.

Record remote command status and infrastructure/cleanup status separately.
After successful cleanup, return the remote command's status unchanged. If
cleanup fails or remains unresolved, return an infrastructure failure and
retain the original status in diagnostics and the run record. Do not claim
success after a successful build leaves an owned resource unaccounted for.

## Cost evidence and qualification

Record allocation-to-cleanup lifetime separately from remote execution time.
Apply each product's actual billing intervals, minima, account commitments, and
per-resource caps. Include retained storage, IPs, snapshots, and paid starts
where relevant. Publish currency, tax assumptions, rate source/date, and
unresolved charges with each estimate. A preflight quote does not impose an
atomic provider-side spending limit.

Use equal completed work when comparing providers. Record workload revision,
toolchain, concurrency, actual allocation, and cache state. Compare cold, warm,
link-heavy, and sustained runs, and distinguish total-account from marginal
cost. Issue #157 defines the benchmark work.[^6] No paid resources were created
while preparing these RFCs.

Implementation qualification has three stages:

1. Preserve existing Scaleway selection and configuration through contract
   tests while adding provider dispatch, endpoint handling, capability checks,
   and the ledger.
2. Exercise each real adapter against local HTTP fixtures and injected command
   runners. Cover partial allocation, ambiguous responses, retries, bootstrap
   failures, remote exit 101, cancellation, and delayed or failed deletion.
3. Run an explicitly enabled live test with constrained resources. Verify
   retained-resource preservation and every owned-resource deletion condition.
   Ordinary tests and pull-request checks must not allocate paid resources.

## Related concerns and acceptance boundaries

The following issues track concerns found during the preceding cost review:

- [#153: Supported Scaleway Block Storage API](https://github.com/leynos/mriya/issues/153).
- [#154: Complete teardown and partial provisioning](https://github.com/leynos/mriya/issues/154).
- [#155: Explicit boot-volume type and size](https://github.com/leynos/mriya/issues/155).
- [#156: Status, cache units, and billing documentation](https://github.com/leynos/mriya/issues/156).
- [#157: Cost per completed workload](https://github.com/leynos/mriya/issues/157).

The RFCs reference those issues; merging documentation does not resolve them.
Shared implementation work should land with separate acceptance evidence.
Provider integration remains unimplemented until its own contract and live
qualification pass. No proposal adds delivery dates or paid resource orders.

## References

[^1]: [Backend request, handle, networking, and trait](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/backend.rs).
[^2]: [Current command-line construction](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/main.rs).
[^3]: [Run orchestration](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/run.rs), [init helpers](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/init/helpers.rs),
      and
      [configuration writer](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/config_store/mod.rs).
[^4]: [Existing multi-cloud roadmap](../roadmap.md#7-multi-cloud-rollout).
[^5]: [Scaleway test-resource janitor](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/janitor/mod.rs).
[^6]: [Workload cost qualification issue](https://github.com/leynos/mriya/issues/157).
