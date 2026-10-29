# RFC 0001: Boat backend

- Status: proposed.
- Date: 29 October 2026.
- Audience: Mriya implementers and reviewers.
- Depends on: [shared backend contracts](README.md).

## Summary and scope

Add a `boat` backend for disposable CPU build and test workers. Use Boat's HTTP
application programming interface (API) for lifecycle operations and keep
Mriya's system SSH and `rsync` transport for workspace synchronization, live
output, and remote exit status. The proposal includes cold workers and an
optional, explicitly selected seed snapshot. It does not implement a mutable
shared cache, image baking, integrated coding agents, or GPU execution.

This RFC proposes configuration and behaviour; the current binary does not
support Boat. Acceptance permits implementation without changing the default
Scaleway backend or promising a particular cost per completed build.

## Current implementation and provider contract

At `fd7d8193fae29d54867cc372dc30efdc6fab35bb`, `Backend` already separates
`create`, `wait_for_ready`, and `destroy`. However, `InstanceRequest` requires
an image, zone, and project, while `InstanceNetworking` accepts an IP address
and SSH port. Boat needs provider-specific validation and a connection endpoint
that can carry a DNS name, username, and trusted host key.[^1]

The command-line entry point constructs `ScalewayBackend` directly. The sync
configuration defaults to user `root`, workspace `/home/ubuntu/project`, and
disabled host-key checking. Backend selection and provider connection defaults
must therefore reach both command execution and `rsync`; adding one trait
implementation would leave the command-line path incomplete.[^2]

Boat publishes four fixed x86_64 sizes, from 2 vCPU/4 GB to 16 vCPU/32 GB, with
usable disk from 51 GB to 251 GB. It offers no GPU or custom machine size.[^3]
Its public pricing uses running seconds, a monthly compute commitment, and
separate charges for excess starts and saved snapshots.[^4] These properties
make short CPU jobs a useful target, subject to measured throughput and the
account's total usage.

The adapter uses `https://boat.dev/api/v1` with a scoped bearer key. A `ci` key
can manage sandboxes; the operator creates and funds the account before Mriya
runs. Keep the key in the local environment or secret manager, and select the
billing organization explicitly. The `ci` preset grants deletion of sandboxes
created by that key; a replacement key may lack that authority. Record a
non-secret credential reference in the ledger. Recovery needs the still-valid
creating key or an explicitly provisioned recovery credential with sufficient
rights. Verify actual actions: older keys retain their original scopes and may
lack newer SSH or deletion permissions.[^5]

Table 1 records the provider operations needed by the first implementation.

| Operation | Published contract                                                     | Mriya mapping                              |
| --------- | ---------------------------------------------------------------------- | ------------------------------------------ |
| Create    | `POST /sandboxes` accepts type, labels, lifetime, and optional `from`  | Create one run-owned worker                |
| Inspect   | `GET /sandboxes/{id}` exposes lifecycle state                          | Wait with a deadline                       |
| Access    | `POST /sandboxes/{id}/sshkey` supplies connection details and host key | Authorize a public key; pin the response   |
| Delete    | `DELETE /sandboxes/{id}` starts permanent deletion                     | Dispose of the worker and track completion |

_Table 1: The proposed lifecycle mapping. The cited sections below specify the
request and completion rules._

## Proposed configuration

The shared selector follows command-line flag, `MRIYA_BACKEND`, then
`[mriya].backend` precedence; an absent selector retains Scaleway. The
following TOML is **proposed**, not configuration accepted by the current
binary:

```toml
[mriya]
backend = "boat"

[boat]
api_key_env = "BOAT_API_KEY"
organization = "personal"
machine_type = "default"
ttl_seconds = 7200
fail_fast = true
# seed_snapshot = "mriya-rust-v1"

[sync]
ssh_identity_file = "~/.ssh/id_ed25519"
ssh_user = "user"
remote_path = "/home/user/project"
```

The corresponding **proposed** invocation is:

```bash
mriya run --backend boat -- cargo test --workspace
```

Require the referenced public key alongside the private identity, or provide an
explicit public-key path. Do not extract a key interactively during a run. The
provider default username is `user`; an explicit incompatible username fails
validation. A configured workspace must be writable by that user.

Reject cloud-init input, a block-volume cache, non-x86_64 architecture, and a
GPU requirement before allocation. Do not populate fake Scaleway project or
zone values. Treat `seed_snapshot` as Boat-specific input; do not silently
reinterpret the existing `--image` option. Reject `mriya init --backend boat`
until a separate cache initialization contract exists.

The adapter always sends `noEnv: true`, `env: {}`, `snapshots: false`, explicit
private visibility, and a finite `ttlSeconds`. The empty environment prevents
inheriting a seed's per-sandbox environment; `noEnv` alone does not clear it.
These fields suppress implicit environment injection and retained output
snapshots. They do not sanitize arbitrary credentials already on the seed disk.
Boat documents that disabled snapshots prevent resume, fork, and saving a new
snapshot. A seed selects the starting disk; it does not opt the output worker
into snapshot retention.[^6] Contract tests and the paid smoke test must verify
the combined `from` and `snapshots: false` request before enabling seeds.

## Provisioning, readiness, and transport

Use a native Rust HTTP adapter with typed requests and responses. Reuse Mriya's
injected HTTP testing patterns and secret redaction. A CLI wrapper would add
installation, version, shell integration, and output-parsing dependencies
without preserving more of Mriya's existing transport.

Before creation, persist the run identifier, billing scope, immutable request
digest, intended resource ownership, and idempotency key in the shared
lifecycle ledger. Submit one sandbox per run, with labels for Mriya ownership
and the run identifier. Record the sandbox ID as soon as the response arrives.

Poll until the provider reports a usable state, then authorize the public SSH
key. Prefer `sshEndpoint` when present; otherwise use `machineIp` and port 22.
Use the API's `sshUser` and `hostKey`, with an isolated known-hosts file and
`StrictHostKeyChecking=yes`. Apply the same settings to SSH probes, command
execution, and the SSH transport used by `rsync`.[^7] Never fall back to
`StrictHostKeyChecking=no` after a key mismatch or API failure.

A successful state poll does not establish that the workspace is usable. Probe
SSH, verify `rsync` availability, and create the workspace before syncing. Keep
bootstrap separate from cloud-init: if a required utility is missing, report an
actionable failure or use an explicitly configured seed. Do not run unrequested
package installers inside the worker.

The create request carries `Idempotency-Key`. Boat retains keys for 24 hours
and reuses the original result only for the same account, key, and request
body; an in-progress request and a changed body produce distinct conflicts.[^8]
Retries preserve all three. After the retention window, reconcile ownership
labels and the ledger before any new create. An uncertain result must remain an
unresolved allocation, not become permission to issue a fresh request.

Once transport is ready, preserve existing workspace filters and command
quoting. Stream stdout and stderr through system SSH and preserve the remote
exit status. An interrupted transport has an unknown command outcome; do not
automatically rerun a potentially non-idempotent command.

## Lifetime and cancellation

The finite lifetime bounds an abandoned run's exposure. Boat's timer starts at
creation or resume, and ordinary commands do not extend it. Reaching the
deadline can interrupt a build. The provider exposes `archiveAfter`, and a TTL
update starts a new interval from the update time.[^9]

Treat `ttl_seconds` as a run budget including provisioning, sync, execution,
and result collection. Compare `archiveAfter` with the provider's creation time
plus the requested TTL, allowing documented timestamp precision; do not compare
it with a new full budget after readiness. Report the remaining budget. Never
disable auto-stop or extend the lifetime implicitly. A controller that needs a
longer run must request that budget before allocation.

On a signal, stop local upload and SSH activity, then begin bounded cleanup
using the recorded ID. Cancellation must not discard the ledger or abort the
cleanup task. A killed client cannot run cleanup, so the recovery command and
provider-aware janitor must reconcile abandoned records. Auto-stop limits
running time; it does not prove that deletion finished.

## Cache model and ownership

A seed snapshot is a borrowed, read-only starting point. New sandboxes consume
it without modifying it. Boat snapshot names identify the latest save under
that name; operators should publish a new versioned name for each seed and
avoid overwriting names used by reproducible runs.[^10] Record the name and
resolved snapshot metadata in run evidence where the API exposes it.

Keep Cargo, compiler, and package caches in a dedicated saved directory such as
`/home/user/mriya-cache` when preparing a seed. Boat excludes `~/.cache`,
`/tmp`, and Docker's build cache from snapshots; an exported BuildKit cache
needs a saved location.[^11] Seed preparation must verify a sentinel and one
representative warm build after restoring the seed. Do not assume a directory
survives because it existed before saving.

Qualify only trusted, sanitized seeds. Remove embedded cloud credentials,
registry logins, project secrets, and unintended SSH authorizations during seed
preparation. Boat's credential cleanup does not remove arbitrary credential
files or old `authorized_keys` entries.[^11] Test that a source environment
sentinel and unintended credentials or authorized keys do not reach a worker.
Later environment forwarding needs an explicit allowlist.

The first implementation neither saves each completed worker nor overwrites a
shared seed. Parallel workers start independently, and their cache changes
expire with them. This sacrifices incremental cache write-back to avoid
concurrent publication and unintended snapshot charges. A later mutable-cache
RFC must define publication, locking, retention, and cleanup before enabling it.

Do not enter the existing `/dev/vdb` mount path for Boat. Its seed disk is not
a detachable block volume. A request for persistent volume semantics must fail
clearly instead of falling back to the disposable root filesystem.

## Cleanup and recovery

`destroy` means permanent deletion of the run-owned sandbox. It does not mean
`stop`. Send `DELETE /sandboxes/{id}` with `X-Ascii-Confirm-Delete` set to that
exact ID. Boat returns an asynchronous deletion operation; persist its ID and
poll `GET /deletion-operations/{id}` for completion. List disappearance alone
does not establish data erasure. Separately saved snapshots survive deletion of
their source sandbox.[^12]

Use the shared ownership ledger for all recovery paths. Never delete a seed,
another run's sandbox, or a resource inferred only from a human-readable name.
If deletion exceeds the cleanup deadline, retain the operation ID and report
cleanup pending, including the provider's stage and next inspection time when
available. The janitor resumes that operation rather than creating a duplicate
deletion workflow. Treat a 404 cautiously when an earlier delete response was
lost: confirm scope and retain the missing completion evidence.

Capture usage before deletion where available. A completed command followed by
pending or failed cleanup must preserve both outcomes in the run record. Report
the cleanup failure to the caller using the shared outcome contract.

## Cost model and failure handling

Model monthly account cost as the greater of the compute commitment and pooled
compute usage, plus applicable start and saved-snapshot charges. Allocate the
commitment separately when estimating a single job. The published entry plan
currently costs $20 per month; `default` compute costs $0.036 per hour. These
are a dated planning input, not constants to embed in runtime code.[^4]

Before creation, inspect `GET /limits` for allowed machine types, capacity, and
`nextStartDollars`. Boat's paid starts automatically consume wallet funds above
included rolling start limits.[^13] Show a non-zero prospective start charge in
the allocation summary. A preflight check cannot enforce a hard charge cap:
another client can consume the remaining allowance before creation. Do not
describe local pacing as an atomic account spending limit.

Retry provider-declared transient failures with bounded backoff and jitter.
Respect response-specific retry delays. Stop immediately for invalid input,
authentication, authorization, or payment prerequisites. Do not buy credit,
upgrade a plan, switch wallets, select a different size, or fall back to
another provider automatically. Preserve provider request IDs with redacted
errors. Use the same explicit organization scope for limits, allocation, usage,
and cleanup; never depend on a mutable active wallet selection.

Boat shares CPU capacity between sandboxes, so rental price does not establish
build throughput.[^14] Benchmark cold and restored-cache Rust builds under the
intended concurrency. Record provision time, sync bytes, execution time,
cleanup time, actual machine size, and total account charges attributable to
the run. Include link-heavy gates and a sustained build before recommending
Boat for long CPU-saturated jobs.

The HTTP command API offers a separate implementation option, but synchronous
commands have a 600-second ceiling; longer commands require detached execution
and streamed or polled results.[^15] Keeping SSH avoids introducing a second
execution and output-recovery protocol in this RFC.

## Alternatives and trade-offs

- **Wrap the Boat CLI:** expedient for a spike, but adds a runtime dependency
  and couples recovery to CLI output. Prefer the documented HTTP API.
- **Use Boat's execution and file APIs:** removes direct SSH, but requires new
  transfer, command recovery, and output handling. Defer until a workload
  demonstrates a benefit over the existing transport.
- **Keep a stopped sandbox per project:** retains mutable state for a limited
  period but changes Mriya's disposable-worker model and needs exclusive
  access. Do not treat this as equivalent to the current persistent volume.
- **Save after every run:** can preserve caches, but also retains workspace
  data and introduces concurrent writers and recurring storage charges. Defer.

## Delivery and acceptance

1. Land the shared selector, provider request validation, connection trust,
   and lifecycle ledger contracts. Verify that existing Scaleway configuration
   and command status behaviour remain unchanged.
2. Implement cold Boat workers with mocked HTTP lifecycle responses and the
   existing injected SSH runner. Test an endpoint with a non-default port, a
   DNS hostname, a missing host key, and a key mismatch.
3. Cover lost create responses, same-key retries, changed-body conflicts,
   expired idempotency records, readiness failure, sync failure, command exit
   101, cancellation, TTL expiry, and delayed deletion. Every allocation must
   end in confirmed cleanup or an actionable recovery record.
4. Add borrowed seed support only after verifying the combined create request,
   restored cache sentinels, cleared inherited environment, sanitized seed
   credentials, concurrent consumers, and seed survival after worker deletion.
   Reject unsupported cache and cloud-init requests before any billable call.
5. Run an opt-in paid smoke test with explicit credentials and a fixed lifetime.
   Capture command status, deletion operation completion, and retained-resource
   inventory. Compare one cold and one seeded build; publish the observations
   without inferring universal throughput from a single sample.

No paid instance was launched to prepare this RFC. Provider documentation and
source inspection establish the proposed contract; live behaviour remains an
implementation acceptance gate.

## Open questions

- Confirm the `ci` preset's coverage of limits, seed metadata, and deletion
  operation reads. Enumerate the minimum additional actions if required.
- Verify seed creation with output snapshots disabled and record the exact
  API revision used by contract fixtures.
- Decide whether output artefact retrieval belongs in a shared Mriya change;
  this RFC preserves the current workspace upload and command-output workflow.
- Resolve conflicting numeric API request limits in Boat's documentation
  before setting client defaults. Prefer returned rate-limit metadata over a
  hard-coded request quota.[^13]

## References

Provider documentation was checked on 2026-10-29.

[^1]: [Mriya backend contract at the reviewed revision](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/backend.rs).
[^2]: [Command-line construction](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/main.rs)
      and
      [sync configuration](https://github.com/leynos/mriya/blob/fd7d8193fae29d54867cc372dc30efdc6fab35bb/src/sync/config.rs).
[^3]: [Boat machine sizes](https://docs.boat.dev/machines).
[^4]: [Boat pricing](https://docs.boat.dev/pricing).
[^5]: [Boat API keys and tokens](https://docs.boat.dev/api-keys) and
      [key scopes](https://docs.boat.dev/api-keys/scopes).
[^6]: [Boat create-sandbox API](https://docs.boat.dev/api/reference/sandboxes/create-sandbox).
[^7]: [Boat SSH access](https://docs.boat.dev/ssh-access) and
      [SSH key API](https://docs.boat.dev/api/reference/agent/configure-sandbox-ssh-key).
[^8]: [Boat API overview: idempotency and lifecycle](https://docs.boat.dev/api/v1).
[^9]: [Boat auto-stop and keep alive](https://docs.boat.dev/long-running-tasks/auto-stop).
[^10]: [Boat saved snapshots](https://docs.boat.dev/snapshots/templates).
[^11]: [What a Boat snapshot saves](https://docs.boat.dev/snapshots/whats-saved)
       and
       [access and credential retention](https://docs.boat.dev/environments/access).
[^12]: [Boat permanent deletion and operation tracking](https://docs.boat.dev/data-retention/delete).
[^13]: [Boat limits, paid starts, and retry rules](https://docs.boat.dev/limits).
[^14]: [Boat FAQ: shared CPU and sustained compute](https://docs.boat.dev/faq).
[^15]: [Boat background commands](https://docs.boat.dev/long-running-tasks/background-commands).
