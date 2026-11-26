# Migrate Scaleway cache volumes to the Block Storage API

This ExecPlan is a living document. The sections `Progress`,
`Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must
be kept up to date as work proceeds.

This document must be maintained in accordance with `AGENTS.md` and the
ExecPlan rules in the `execplans` skill.

## Purpose / Big Picture

Scaleway removed new Block Storage volume creation from the Instances API on
30 April 2025. `mriya init` still posts `volume_type = "b_ssd"` to
`/instance/v1/zones/{zone}/volumes`, so the advertised cache-initialization
path fails against the live provider with
`b_ssd volumes are no longer supported. Use Scaleway Block Storage (SBS)
volumes instead.`
The attachment payload also omits `volume_type = "sbs_volume"`, so even an
externally created SBS volume cannot be attached.

After this change a user can run `mriya init` against a current Scaleway
account, receive an SBS volume in the configured zone, and then run
`mriya run -- cargo build` twice. The second run must find the first run's
files on the same volume, and every run must detach the volume before deleting
the VM so the provider does not destroy the cache with the server.

The observable success is:

- `mriya init` creates a volume through `POST /block/v1/zones/{zone}/volumes`,
  waits for `available`, attaches it with `volume_type: sbs_volume`, formats
  only the volume whose guest serial is `volume-<uuid>`, detaches it, and then
  destroys the helper VM.
- A configured cache volume is mounted only when the mount source is verified
  to be that volume; otherwise the run fails and tears the VM down rather than
  silently writing build output to the root disk.
- A configured legacy `b_ssd` volume is refused with migration guidance that
  preserves its data.

## Constraints

- Keep the dependency rule: provider HTTP contracts live in `src/scaleway/`;
  orchestrators in `src/init/` and `src/run.rs` depend on the `VolumeBackend`
  port, not on Scaleway request types.
- Never create, reformat, or reinterpret a legacy `b_ssd` volume. Refuse it and
  point at the snapshot migration guide.
- Never destroy a server that still has the cache volume attached. Scaleway
  deletes every attached volume when a server is deleted through the API.
- Volume sizes are decimal GB (10^9 bytes) with a 5 GB minimum; `sbs_5k` is
  the default tier and `sbs_15k` is accepted.
- Every interpolated shell value is escaped, and volume IDs are validated as
  canonical UUIDs before any shell command is built.
- No single code file may exceed 400 lines.
- All comments and documentation use en-GB-oxendict spelling.

## Tolerances (Exception Triggers)

- If implementing a milestone would require more than 2,500 net lines of code
  across production and test sources, stop and reassess decomposition.
- If a provider contract cannot be verified from Scaleway's published OpenAPI
  schema or documentation, stop and record the uncertainty rather than guessing.
- If a gate fails twice for the same root cause, stop and record the diagnosis
  before changing the design.
- ExecPlan files do not count towards any size tolerance.

## Risks

- Scaleway's published minimum size is 1 GB in the Block Storage schema but
  5 GB in product documentation. The plan enforces 5 GB and documents why.
- The legacy `b_ssd` volume may not be visible through the Block Storage API.
  The implementation treats a 404 as "not in this zone" and documents that a
  legacy ID is refused by type when the API returns it. The migration guide is
  linked from the typed legacy error.
- Attach and detach require a stopped server. Teardown therefore powers off,
  waits for `stopped`, detaches, waits for `available`, and only then destroys.
- A failed detach must retain the server. This can incur cost, which is safer
  than deleting the cache; the error names both IDs and the recovery command.

## Conformance basis

Upstream: GitHub issue `leynos/mriya#153`, "Migrate Scaleway cache volumes to
the supported Block Storage API", including its Coding Plan (12 November 2026).

Governing contracts, reviewed 26 November 2026:

- Scaleway Block Storage API v1 schema,
  <https://www.scaleway.com/en/developers/api/block/v1/schema.yml>:
  `POST/GET/DELETE /block/v1/zones/{zone}/volumes[/{id}]`, the unwrapped
  `Volume` response, statuses, and the `volume-types` gate.
- Scaleway Instances API v1 schema,
  <https://www.scaleway.com/en/developers/api/instance/v1/schema.yml>:
  `POST /instance/v1/zones/{zone}/servers/{id}/attach-volume` and
  `.../detach-volume`, with `AttachServerVolumeRequest.VolumeType` including
  `sbs_volume`.
- Scaleway device identification guide,
  <https://www.scaleway.com/en/docs/instances/reference-content/identify-devices/>:
  SBS disks report vendor `SCW`, model `sbs`, serial `volume-<uuid>`.
- Scaleway changelog,
  <https://www.scaleway.com/en/docs/changelog/>: Block Storage creation was
  removed from the Instances API on 30 April 2025.
- Scaleway Block Storage FAQ,
  <https://www.scaleway.com/en/docs/block-storage/faq/>: deleting a server via
  the API deletes attached volumes; SBS sizes are 5 GB to 15 TB.

Trace chain: `#153-AC-create` -> `EP-M1` -> loopback create/tier tests;
`#153-AC-attach` -> `EP-M2` -> attach/detach map tests; `#153-AC-device` ->
`EP-M3` -> fake-tool shell tests; `#153-AC-persist` -> `EP-M4` -> BDD teardown
order and live three-VM suite; `#153-AC-legacy` and `#153-AC-docs` -> `EP-M4`
and `EP-M5` -> migration guide and error-message tests.

## Verification plan

Invariants:

1. Every volume create, lookup, readiness and delete request targets
   `/block/v1/zones/{zone}/...` and carries `X-Auth-Token`.
2. A create request never contains `volume_type` and always contains
   `perf_iops` plus decimal `from_empty.size`.
3. Attach and detach target the dedicated Instances endpoints, send
   `sbs_volume` on attach, and leave the root map entry intact.
4. No server is destroyed while the cache is attached, and no volume created by
   a failed init survives.
5. The formatting script cannot run `mkfs.ext4` unless the serial matched
   exactly one `SCW`/`sbs` disk, the disk is not the root disk or its ancestor,
   and `blkid` reports no existing signature.
6. A configured cache is reported usable only when `findmnt` confirms the mount
   source is the resolved device.
7. A legacy volume is refused before any server POST.

Methods and artefacts:

- Invariants 1-3 and 7: loopback `StubApi` tests in
  `src/scaleway/lifecycle/tests/volumes.rs` and `instances.rs`, asserting exact
  URLs, JSON bodies, headers, and request counts. Negative control: a missing
  tier suppresses the POST, and a changed root entry fails attach.
- Invariants 4: init/run BDD scenarios with a shared ordered event log in
  `tests/init/` and `tests/run/`, plus rollback loopback tests. Negative
  control: detach failure must retain the server and skip DELETE.
- Invariants 5-6: `tests/cache_device_shell.rs` executes the generated scripts
  under a restricted fake-tool `PATH` and inspects call logs. Negative control:
  root-disk, duplicate-serial, existing-signature and wrong-source fixtures
  must show no `mkfs.ext4` call or a refusal status.
- Live persistence: opt-in `tests/scaleway_cache_volume.rs` writes sentinel A
  on VM 1, reads it and writes B on VM 2, and reads B on VM 3.

Axioms: Scaleway's documented request/response shapes and device attributes are
treated as external contracts; tests exercise the repository-owned request
construction against them. The fake utilities implement the documented output
shapes (`lsblk -P`, `findmnt -no SOURCE`), and the live suite is the only proof
against the real provider.

## Context and orientation

`src/scaleway/` is the production backend. `ScalewayBackend` uses direct
`reqwest` calls for instance and volume operations and uses the `scaleway-rs`
SDK only for image lookup. `src/scaleway/lifecycle/` holds the HTTP helpers and
their loopback tests. `src/volume.rs` defines the volume port. `src/init/`
orchestrates cache preparation, and `src/run.rs` mounts the cache before
running a remote command. `src/sync/remote_command.rs` builds the cache routing
preamble.

Current behaviour to replace: `volume_create.rs` posts `b_ssd` to the Instances
API; `volume_attach.rs` PATCHes a partial volume map; `volume_detach.rs` GETs
then PATCHes; `init/helpers.rs` formats `/dev/disk/by-id/scsi-0SCW_BSSD_<id>`;
`run.rs` mounts `/dev/vdb` best-effort and destroys the server without
detaching.

## Plan of work

Milestone `EP-M1` (provider volume lifecycle): add `VolumeTier`, decimal size
semantics, SBS models, typed errors, `block_api_base`, Block API create with
tier gating, get, status polling, deletion, and the tier/size configuration.
End state: loopback tests prove every Block API request and error mapping.

Milestone `EP-M2` (server boundary): replace the PATCH payloads with the
dedicated attach and detach endpoints, verify the returned volume map, add
preflight volume validation, the `create_with_image_id` seam, rollback, power
off, and a detach-before-destroy teardown. End state: no server can be created
with an invalid volume and no server is destroyed with the cache attached.

Milestone `EP-M3` (guest device safety): add `src/cache_device/` with escaped
script builders and a status-line parser, serial-based resolution, root
ancestry guard, empty-signature check, labelled formatting, ext4 validation,
occupied-path rejection, mount-source verification, ownership, and verified
cache routing. End state: fake-tool shell tests prove no destructive command
can run on the root disk.

Milestone `EP-M4` (orchestration and ownership): reorder init, track the owned
volume, delete it only on failure, guard the effective configured ID, apply the
ordered teardown to every run path, and update the BDD suites and doubles.

Milestone `EP-M5` (live proof and documentation): add the opt-in three-VM live
suite, the migration guide, the dated price notes, and the documentation
updates.

## Progress

- [x] 2026-11-26: Reconnaissance and provider contract research complete.
- [x] EP-M1 and EP-M2 implemented: the Block Storage lifecycle, dedicated
  attach and detach endpoints, preflight validation, rollback, power-off and
  ordered teardown are in place with loopback tests.
- [x] EP-M3 implemented: `src/cache_device/` builds guarded scripts and the
  fake-tool suite exercises them.
- [x] EP-M4 implemented: init owns and rolls back its volume, the effective-ID
  guard is in place, and run uses the ordered teardown on every path.
- [x] EP-M5 implemented: the live three-VM suite, migration guide and price
  notes are written.
- [ ] Deterministic gates pass (`make check-fmt`, `make lint`, `make test`).
- [ ] CodeRabbit review concerns cleared.
- [ ] Draft pull request opened.

## Surprises & discoveries

- Scaleway's quickstart shows `/block/v1/zone/` (singular), but the OpenAPI
  path is `/block/v1/zones/{zone}/...`; the schema is authoritative.
- Deleting a server through the API deletes every attached volume, so the
  existing run path silently destroys the cache volume it just used.
- The Block Storage create response is the unwrapped `Volume`, unlike the
  Instances API's `{"volume": ...}` wrapper.
- `list_instance` paginates until an empty page, so every instance fetch in a
  loopback test consumes two responses: the page carrying the server and the
  empty page that ends pagination. Tests that power off an already-stopped
  server must script both.
- `power_off_if_needed` returns immediately for an already-stopped server
  rather than entering the separate stopped-state wait, which keeps teardown to
  one fetch in the common case.

## Decision log

- Use the dedicated attach/detach endpoints rather than rebuilding the PATCH
  map: each request names one volume, so the provider owns map preservation,
  and the returned map is verified.
- Refuse legacy `b_ssd` volumes with a migration link rather than keeping a
  legacy attach path: no ID-preserving conversion exists, and refusing avoids
  reformatting or reinterpreting data.
- Treat a configured cache as required: a run that cannot verify the mount
  fails and tears down, because silently writing to the root disk defeats the
  feature.
- Enforce a 5 GB minimum and decimal GB: product documentation states 5 GB,
  and the API uses 10^9-byte granularity.
- Retain the server when detach fails: a visible paid server is safer than
  deleting the cache volume with the server.
- Keep the status-polling loop separate from the HTTP call
  (`poll_for_volume_status`) so the timeout and terminal-error paths are
  unit-testable without scripting an unbounded number of loopback responses.
- Validate the volume use in `Backend::create` rather than only in `mriya run`,
  so `mriya init` gets the same preflight for free.
- Map every init formatting refusal to its own `InitError` variant while
  keeping the BDD failure kind `format`, so operators get a precise message
  without changing the scenario vocabulary.

## Outcomes & Retrospective

To be completed when the work is done. Record what shipped, what was deferred,
and which assumptions survived contact with the implementation.

## Revision note

Initial draft created 26 November 2026 from issue #153 and its Coding Plan,
after verifying the Block Storage and Instances v1 schemas, the device
identification guide, the changelog, and the FAQ.

Revision 2 (26 November 2026) records the implemented milestones, the
pagination discovery that affects loopback scripts, the power-off fast path,
and the decision to expose the polling loop and the preflight at the backend
boundary. Remaining work is gate evidence, review remediation and the pull
request.
