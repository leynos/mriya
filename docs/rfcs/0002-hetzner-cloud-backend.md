# RFC 0002: Hetzner Cloud backend

- Status: proposed.
- Date: 29 October 2026.
- Audience: Backend implementers, reviewers, and operators.
- Depends on: [Shared backend contracts](README.md).

## Summary and scope

Add a `HetznerBackend` for disposable CPU virtual machines (VMs), persistent
cache volumes, and cloud-init provisioning. Reuse Mriya's workspace sync,
remote execution, and exit-status handling. The existing roadmap already
includes Hetzner Cloud; this proposal defines its implementation contract.[^1]

The first release supports public-network Cloud servers and existing SSH keys.
It excludes private-network routing, automatic image baking, server resizing,
and automatic selection of a different instance type or location. Reject GPU
requirements before provisioning. Hetzner's dedicated GPU servers belong to a
separate product family; ordering or cancelling them requires a separate
proposal, not an implicit fallback from Cloud.[^2]

## Current implementation and provider contract

The baseline is Mriya commit `fd7d8193fae29d54867cc372dc30efdc6fab35bb`.
`Backend` already defines create, readiness, and destroy operations, but
`main.rs`, configuration loading, and `mriya init` instantiate Scaleway
directly. `InstanceRequest` requires a Scaleway-style project identifier.
`InstanceHandle` stores only an identifier and zone. The run path mounts
`/dev/vdb`, while init formats a Scaleway-specific device path and writes
`[scaleway].default_volume_id`.[^1] These are integration prerequisites, not
Hetzner API properties.

Hetzner authenticates API requests with a project-scoped Bearer token. The
backend needs a token with read and write permission; a project identifier in
the request cannot select another token's project.[^3] Keep the token on the
local machine and redact it from errors, logs, and the run ledger.

Table 1 maps the required operations to the published Cloud API and official
client schema.[^4][^5]

| Mriya operation    | Provider operation                                           | Proposed completion condition                         |
| ------------------ | ------------------------------------------------------------ | ----------------------------------------------------- |
| Validate selection | Read server types, locations, images, and SSH keys           | Resolve exact identifiers and compatible architecture |
| Allocate address   | `POST /primary_ips`                                          | Record identifier and explicit `auto_delete` policy   |
| Create server      | `POST /servers` with `user_data`, SSH keys, and `public_net` | Record server and all returned action identifiers     |
| Wait for provider  | Read resource actions and server                             | Every required action succeeds and server is running  |
| Attach cache       | Create with `volumes` or attach a volume                     | Volume reports the expected server attachment         |
| Remove server      | `DELETE /servers/{id}`                                       | Delete action succeeds and server is absent           |
| Remove address     | `DELETE /primary_ips/{id}` when still present                | Owned address is absent                               |

Actions represent asynchronous work and can fail after initial allocation.
Treat a successful HTTP response as acceptance of the request, not completion
of the operation.[^4] Inspect every returned action, including follow-on
actions. Catalogue validation establishes compatibility; it cannot reserve
capacity.

Use direct HTTPS requests through Mriya's existing HTTP stack. Keep typed
provider requests, responses, errors, and action polling under `src/hetzner/`.
Do not add a CLI subprocess or a third-party Rust SDK solely to avoid a small
set of API mappings. Reconsider that choice if implementation demonstrates a
maintenance advantage for an SDK.

## Proposed configuration

The following TOML is a proposal, not configuration accepted by the current
binary. The numeric identifiers illustrate existing resources in the selected
project; operators must replace them. Resolve `cx33` and `ubuntu-24.04` against
the live catalogue rather than treating them as permanent defaults.

```toml
[mriya]
backend = "hetzner"

[hetzner]
token_env = "HCLOUD_TOKEN"
location = "fsn1"
server_type = "cx33"
image = "ubuntu-24.04"
architecture = "x86_64"
ssh_key_ids = [123456]
ipv4 = true
ipv6 = false
# default_volume_id = "12345678"
# cloud_init_user_data_file = "provision.yaml"

[sync]
ssh_user = "root"
ssh_identity_file = "~/.ssh/id_ed25519"
remote_path = "/root/project"
volume_mount_path = "/mriya"
```

`mriya run --backend hetzner -- cargo test` overrides `MRIYA_BACKEND`, then
`[mriya].backend`. An absent selector retains Scaleway behaviour and its
`SCW_*` settings. Select the provider before loading credentials. A Hetzner run
must not require Scaleway credentials or fabricate a project identifier.

`token_env` names an environment variable; it does not contain a token or
perform shell expansion. Provider settings use `MRIYA_HETZNER_*` overrides,
while the existing `MRIYA_SYNC_*` settings continue to control SSH and sync.
The provider request variant holds resolved identifiers and explicit network
choices. Map Mriya's `x86_64` and `aarch64` values to provider architecture
values in one adapter function.

Require at least one public address in the first release. IPv6-only runs
require a compatible local route and guest package sources; do not silently add
a billed IPv4 address after connection failure. Resolve the connection address
from the server response and preserve its family, SSH port, username, identity
file, and host-key policy across readiness, rsync, and execution. The IPv6
response supplies an allocated `/64`, not a guest host address. For qualified
system images, derive and validate the documented `::1` guest address; never
pass the prefix itself to SSH. Test IPv6 quoting and bracket handling in both
SSH and rsync.[^6]

## Creation, readiness, and execution

1. Validate the selected provider, image, architecture, location, keys, cache,
   and network policy before allocating anything. Read the configured cache's
   attachment and location. Reject a cache already attached elsewhere. Match at
   least one provider SSH key's public material or fingerprint to the
   configured local identity; key existence alone does not prove access.
2. Persist a run identifier, request fingerprint, and intended resources in
   the shared ledger. Derive a unique server name from the run identifier. Add
   `mriya-run`, `mriya-owner`, and `mriya-role` labels to owned resources.
3. Explicitly create each requested Primary IP with `auto_delete=true`, the
   selected location, and the run labels. Record each returned identifier
   before the next mutation. Explicit allocation makes partial-create cleanup
   auditable and avoids adopting an unrelated unassigned address.
4. Create the server with those addresses, existing SSH key identifiers,
   selected image, and `start_after_create=true`. Pass user data unchanged. For
   a cache, set `automount=false` and attach its identifier explicitly; Mriya
   owns the mount convention. Persist server and action identifiers.
5. Wait for provider actions, then authenticate over SSH using the shared
   host-key policy. A listening TCP port does not establish SSH readiness. Use
   a bounded deadline for each phase and cancellation-aware polling.
6. Where provisioning was requested, require successful cloud-init completion
   before mounting, syncing, or running commands that depend on its packages.
   Distinguish failure from incomplete status; a boot marker alone does not
   establish that requested provisioning succeeded. Confirm rsync and the
   selected shell are available.
7. Mount and verify the cache, if selected, then reuse git-aware rsync,
   streaming output, and remote exit-status handling. Always enter cleanup
   after readiness, provisioning, sync, execution, timeout, or cancellation
   failure. Preserve the primary failure alongside any cleanup failure.

Supported images must provide the expected SSH and cloud-init environment.
Hetzner document cloud-init support for their system images and describe
different behaviour for snapshots that lack the required datasource.[^6]
Therefore, the initial compatibility test uses a provider system image;
custom-image support requires equivalent readiness evidence.

## Cache lifecycle and resource ownership

Hetzner Volumes permit one server attachment, use capacities expressed in GB,
and remain separate from server snapshots.[^7] Validate location and attachment
through the API. Hold a local lease for a configured cache and treat an API
attachment conflict as authoritative across machines. Never detach a cache from
another live run to acquire it.

Use the API's volume device metadata to identify the guest device, then verify
its filesystem and mount at `[sync].volume_mount_path`. Remove the generic
`/dev/vdb` assumption. Validate provider-returned paths before shell quoting,
verify that the mounted source matches the selected volume, and check that the
execution user can write the required cache directories. A requested cache that
fails to mount must fail the run, rather than consume root space and report
persistence that did not occur.

Extend `VolumeBackend` and the configuration writer for provider-specific
creation, device discovery, deletion, and persistence.
`mriya init --backend hetzner` creates a new ext4 cache, verifies its
filesystem, detaches it, and writes only `[hetzner].default_volume_id`. Format
only a newly created volume owned by that init transaction. Treat a supplied
existing volume as borrowed; never infer permission to reformat it from
`--force` on a configuration write.

Convert requested bytes to whole provider GB with checked ceiling arithmetic
and report the resulting allocated capacity. Validate the provider's current
integer-GB limits, documented as 10 GB to 10 TB, before allocation. Reject
requests outside those limits rather than silently expanding them.[^7] Preserve
the existing Scaleway configuration's interpretation rather than changing old
values implicitly. If configuration persistence fails after successful
initialization, retain the usable cache and return its identifier with recovery
instructions. Remove an unusable volume created by the failed transaction, with
verified deletion.

Table 2 defines cleanup ownership. Labels aid reconciliation; the persisted
ledger and exact resource identity authorize deletion.

| Resource                            | Ownership            | Run cleanup                                           |
| ----------------------------------- | -------------------- | ----------------------------------------------------- |
| Server and its primary disk         | Owned transient      | Delete server and verify absence                      |
| Run-created Primary IPs             | Owned transient      | Verify auto-deletion; explicitly delete any remainder |
| Configured cache volume             | Borrowed or retained | Unmount and detach; preserve contents and resource    |
| Existing SSH keys and source images | Borrowed             | Leave intact                                          |
| Cache created by successful init    | Retained             | Preserve and record recurring storage charge          |

Flush and unmount the cache while SSH remains usable, then await its detach
action before deleting the server. If the guest is unreachable, power the
server off and reconcile its state before detaching. Record the cache as
requiring filesystem validation on next use after forced shutdown. Continue
with server deletion even when guest cleanup fails; verify the cache survives
and ends detached. Never report complete teardown until every owned transient
resource is absent. Record residual identifiers when the API prevents that.

An address created before a failed server request has no server whose deletion
can trigger auto-delete. The ledger must therefore clean up standalone IPs as
well as addresses found on a server. Do not disable deletion protection on an
unrecognized or borrowed resource. The first release accepts no borrowed
Primary IPs, avoiding an implicit change to their retention policy.

## Billing, retry, and recovery behaviour

Hetzner round server lifetimes up to whole hours and apply a monthly cap to
each server. Stopped servers continue to incur charges until deletion. Primary
IPv4 addresses incur separate charges even while unassigned; Primary IPv6
addresses are free. Retained volumes have their own hourly billing and monthly
cap.[^8][^7] The primary disk belongs to the server plan; do not price it as an
additional cache volume.[^6]

For one disposable server within one billing month, estimate compute as
`min(ceil(lifetime_hours) * hourly_rate, monthly_cap)`. Add each IP's actual
lifetime, retained volume allocation, and any applicable transfer charges.
Compute the cap per resource, not across unrelated short-lived servers. As a
consequence, 100 separate 15-minute runs consume at least 100 billed server
hours, before provisioning and cleanup overhead. This is billing arithmetic,
not a throughput prediction.

Retrieve or require current region-specific rates, currency, and tax basis when
presenting costs. Record the estimate's observation date. Benchmark a
representative cold-cache and warm-cache workload before making provider
recommendations. The RFC promises no cross-provider speed equivalence or fixed
price. An idle server pool would alter both billing and isolation; it needs a
separate decision.

Use bounded backoff with jitter for safe reads and action polling, respecting
provider rate-limit signals. An ambiguous mutation response must trigger
reconciliation by recorded identifiers, unique name, and run labels. Labels are
not an atomic idempotency mechanism. Serialize local resume attempts, compare
the request fingerprint, and stop on multiple matches. Do not issue a fresh
create merely because one list response contains no match. Unresolved outcomes
remain visible for recovery; they do not authorize replacement VMs.

Keep the run ledger until cleanup proves its postconditions. Recovery checks
the provider scope, ownership, action outcome, and current resource identity
before retrying deletion. A 404 is success only for the expected resource in
the expected scope. Authentication, permission, protection, and quota errors
remain distinct from temporary capacity failure. Never resubmit a failed job on
a different machine or location without an explicit selection policy.

## Alternatives and implementation phases

Calling `hcloud` would reuse a maintained operator interface but add binary
installation and subprocess contracts. A Rust SDK would reduce mapping code but
introduce a dependency whose supported API and error behaviour need review.
Direct HTTP requests keep the initial surface bounded and consistent with the
current Scaleway adapter. Revisit the choice using implementation evidence,
without changing the provider-neutral lifecycle contract.

Implement the proposal in three increments:

1. Complete the shared selector, provider request variants, endpoint contract,
   ownership ledger, cancellation, and partial-create recovery. Preserve
   existing Scaleway configuration and remote exit codes.
2. Add Hetzner CPU execution without a cache. Verify catalogue selection,
   authenticated readiness, user data, address ownership, and failure cleanup.
3. Add provider-aware cache init and reuse. Verify device identity, capacity
   conversion, cache write access, one-writer exclusion, and retained-resource
   reporting. Add the matching user-guide examples and recovery instructions.

## Acceptance criteria and open questions

- Mock API tests cover action failure after server allocation, lost create
  responses, pagination during reconciliation, rate limits, duplicate names, IP
  allocation followed by create failure, and repeated destroy requests.
- Behavioural tests preserve output and a non-zero remote exit code, prevent
  commands after provisioning failure, reject GPU requirements before writes,
  and route identical SSH identity and host-key settings through every phase.
  Cover IPv6 prefix-to-guest mapping, non-default ports, and mismatched keys.
- Cache tests reject wrong location, existing attachment, wrong device,
  failed mount, and unwritable cache directories. They preserve borrowed data
  and expose configuration-write failure with a recoverable volume ID.
- An explicitly enabled live test runs twice with one cache, checks a marker
  from the first run, then inventories the project. Both servers and all
  run-created IPs must be absent; the retained cache must remain detached.
- Cancellation and failed readiness use the same inventory assertion. The
  report identifies the resources, elapsed lifetimes, and retained charges.
  Paid live tests never form an implicit documentation-validation step.

Implementation must confirm action retention and lookup behaviour after server
deletion, the selected image's cloud-init status interface, and cache detach
behaviour after forced shutdown. Capture these observations in fixture
provenance and the live-test report. They are compatibility gates, not reasons
to weaken ownership or cleanup checks. No paid instance was launched to prepare
this RFC.

## References

[^1]: Mriya baseline: [roadmap](../roadmap.md),
    [backend contracts](../../src/backend.rs),
    [run orchestration](../../src/run.rs),
    [init helpers](../../src/init/helpers.rs), and
    [configuration writer](../../src/config_store/mod.rs), inspected at
    `fd7d8193fae29d54867cc372dc30efdc6fab35bb`.
[^2]: Hetzner,
      [dedicated GPU server catalogue](https://www.hetzner.com/dedicated-rootserver/matrix-gpu/),
    accessed 29 October 2026.
[^3]: Hetzner,
      [using the API](https://docs.hetzner.com/cloud/api/getting-started/using-api/)
    and [generating an API token](https://docs.hetzner.com/cloud/api/getting-started/generating-api-token/),
    accessed 29 October 2026.
[^4]: Hetzner,
      [Cloud API reference](https://docs.hetzner.cloud/reference/cloud),
    accessed 29 October 2026.
[^5]: Official Hetzner Go client at `9adc140755042b41d7a1d865d8c1afe94535b5d6`:
    [server request schema](https://github.com/hetznercloud/hcloud-go/blob/9adc140755042b41d7a1d865d8c1afe94535b5d6/hcloud/schema/server.go),
    [Primary IP client](https://github.com/hetznercloud/hcloud-go/blob/9adc140755042b41d7a1d865d8c1afe94535b5d6/hcloud/primary_ip.go),
    and [volume client](https://github.com/hetznercloud/hcloud-go/blob/9adc140755042b41d7a1d865d8c1afe94535b5d6/hcloud/volume.go).
[^6]: Hetzner, [server FAQ](https://docs.hetzner.com/cloud/servers/faq/),
    accessed 29 October 2026.
[^7]: Hetzner,
      [volume overview](https://docs.hetzner.com/cloud/volumes/overview/)
    and [volume FAQ](https://docs.hetzner.com/cloud/volumes/faq/),
    accessed 29 October 2026.
[^8]: Hetzner, [billing FAQ](https://docs.hetzner.com/cloud/billing/faq/),
    accessed 29 October 2026.
