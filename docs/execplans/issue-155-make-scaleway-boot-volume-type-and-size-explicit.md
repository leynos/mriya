# Make the Scaleway boot volume type and size explicit

This ExecPlan is a living document. The sections `Progress`,
`Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must
be kept up to date as work proceeds.

This document must be maintained in accordance with `AGENTS.md` and the
ExecPlan rules in the `execplans` skill.

## Purpose / Big Picture

Mriya resolves a Scaleway image label to an image UUID and then creates a
server without saying anything about the boot volume. Scaleway fills the gap
with a default that depends on how the image was named, on whether the
instance type offers local storage, and on the marketplace image's own type.
A run that names a CPU/RAM shape therefore does not fully specify what it
buys, nor how much workspace the remote command gets.

After this change a profile can state the boot volume type and capacity
explicitly, Mriya validates that combination against the zone's product
catalogue and the chosen image *before* it creates anything billable, sends
the resolved plan as an explicit `volumes["0"]` entry, reads the actual root
volume back, records it on the handle, and prints the boot and cache
allocations on separate lines. When a profile omits the settings, Mriya picks
a documented deterministic default and says on stderr that it did so.

Observable success:

- `SCW_BOOT_VOLUME_TYPE=l_ssd` plus `SCW_BOOT_VOLUME_SIZE_GB=20` sends
  `"volumes": {"0": {"boot": true, "size": 20000000000, "volume_type": "l_ssd"}}`
  in the create POST.
- A request whose size is below the instance type's minimum, above its
  maximum, below the image's own root size, or in a storage type the type or
  image cannot back, fails before the POST with an error naming the rule.
- With both settings unset, stderr says which default was chosen and why.
- `mriya run` prints one `boot volume:` line and one `cache volume:` line.

## Constraints

- Boot storage is separate from the retained cache volume
  (`default_volume_id` / `InstanceRequest::volume_id`). A field, a variable or
  a report line must never stand in for both.
- Every size is a decimal gigabyte (10^9 bytes) on the configuration and
  provider path. The cache volume's existing binary GiB conversion stays as it
  is. Provider payloads and reports state their units.
- Provider HTTP contracts live in `src/scaleway/`; orchestrators depend on the
  `Backend` port. `src/boot_volume.rs` holds the provider-neutral domain model
  and must not import Scaleway types.
- No single code file may exceed 400 lines.
- All comments and documentation use en-GB-oxendict spelling.
- Lints are not silenced except as a last resort, with a written reason.

## Tolerances (Exception Triggers)

- If the change exceeds 3,500 net lines of code across production and test
  sources, stop and reassess decomposition. ExecPlan files do not count.
- If a provider contract cannot be verified from Scaleway's published OpenAPI
  schema or documentation, stop and record the uncertainty rather than
  guessing.
- If a gate fails twice for the same root cause, stop and record the diagnosis
  before changing the design.

## Risks

- The `volumes["0"]` template for an `sbs_volume` boot disk created from an
  image may require `base_snapshot` rather than being derivable from `image`.
  The documented "Create a volume from a snapshot of an image" row leaves both
  `name` and `base_snapshot` optional, so the plan sends only `volume_type`,
  `size` and `boot`, and the live check is what proves it.
- Reading an SBS root volume's size needs the Block Storage API; the Instances
  API reports the size and type of an attached SBS volume inconsistently. The
  plan reads local size from the server entry and SBS size from
  `GET /block/v1/zones/{zone}/volumes/{id}`, and treats a failure there as a
  post-POST failure that destroys the server.
- The `sbs_snapshot`/`sbs_volume`/`unified` image-root vocabulary is only
  partly documented. Anything unrecognised is refused by name rather than
  guessed at.
- Attachment PATCHes for cache volumes must carry `volume_type: "sbs_volume"`
  only for SBS volumes; sending it for a local volume would be wrong.

## Conformance basis

Upstream: GitHub issue `leynos/mriya#155`, "Make Scaleway boot-volume type and
size explicit", including its Coding Plan (26 November 2026). Related:
`#153` (SBS cache volumes) and `#154` (ownership-aware boot-volume cleanup).

Governing contracts, reviewed 26 November 2026:

- Scaleway Instances API v1 schema,
  <https://www.scaleway.com/en/developers/api/instance/v1/schema.yml>:
  `POST /instance/v1/zones/{zone}/servers` with a `volumes` object of
  `VolumeServerTemplate`; `GET /instance/v1/zones/{zone}/products/servers`
  returning `ServerType` with `volumes_constraint.min_size`,
  `volumes_constraint.max_size`, and `per_volume_constraint.l_ssd`; the
  `Volume.VolumeType` enum `l_ssd | b_ssd | unified | scratch | sbs_volume |
  sbs_snapshot`.
- Scaleway Instances API "Creating an Instance: the volumes object":
  an image *label* defaults to an `sbs_volume` the size of the OS; a
  marketplace image *ID* on a type with local storage defaults to an `l_ssd`
  at the instance's maximum local capacity.
- Scaleway Block Storage API v1 schema,
  <https://www.scaleway.com/en/developers/api/block/v1/schema.yml>:
  `GET /block/v1/zones/{zone}/volumes/{volume_id}` returning an unwrapped
  `Volume` with `size` in bytes and `type`.

Trace chain: `#155-AC-config` -> `EP-M1` -> config and builder tests;
`#155-AC-validate` -> `EP-M2` -> resolver rule table; `#155-AC-request` ->
`EP-M3` -> create payload loopback tests; `#155-AC-record` -> `EP-M3` ->
root-capture tests; `#155-AC-docs` -> `EP-M5` -> users guide and design doc;
`#155-AC-live` -> `EP-M5` -> `make scaleway-boot-test`.

## Verification plan

Invariants:

1. No create POST is sent unless the resolved plan satisfies the instance
   type's size bounds, the image's root size, the storage type's availability
   on that instance type, and 512-byte size alignment.
2. A request that names a boot type and size is never silently replaced by a
   default; a default is used only where the corresponding setting is unset,
   and each defaulted setting is recorded on the plan.
3. The `volumes["0"]` entry carries exactly the plan's type and size, with
   `boot: true`, and never carries a cache volume ID.
4. A captured root volume that disagrees with the plan in type or size fails
   the create, and the created server is destroyed best-effort first.

Lemmas:

- `BootVolumeType::from_str` and `Display` round-trip every supported
  provider identifier, and reject unknown ones.
- Decimal-GB conversion is exact and overflow-checked for every `u32` input.

Axioms (external interfaces, taken as given): the published OpenAPI schemas
above; that `products/servers` lists every type the zone will accept; that a
server created with `stopped: true` reports its root volume in the create
response.

Verification methods:

- Parameterized `rstest` tables for the resolver rules, the conversion
  boundaries, and the type parse/display round trip, because these are finite
  partitions with named cases.
- Loopback HTTP stubs for the create payload, the SBS root read, and the
  attachment PATCH, because the contract is the exact request body.
- BDD scenarios for the orchestration orderings (init preflight before cache
  allocation).
- An opt-in live check that compares request and actual, and fails rather than
  skips when enabled.

Non-vacuity: each resolver rule table includes at least one row that must be
accepted and one that must be refused for that rule's own reason, so a rule
that never fires, or fires always, fails a row. The live check asserts a
non-empty case list when enabled.

## Context and orientation

`src/backend.rs` owns `InstanceRequest` and `InstanceHandle`, the provider
neutral request and result types. `src/config.rs` maps `ScalewayConfig` to an
`InstanceRequest` in `as_request_with_env`. `src/scaleway/mod.rs` implements
`Backend`; its `create` resolves an image (`lifecycle/image.rs`), posts a
server (`lifecycle/create.rs`), attaches a cache volume
(`lifecycle/volume_attach.rs`), and powers the server on. `src/init/mod.rs`
orchestrates cache-volume creation. `src/run.rs` orchestrates a remote run.
`src/test_backdoors.rs` (test-only) renders `dump-request` output.

## Milestones

### EP-M1: boot-storage domain model and configuration

Create `src/boot_volume.rs` with `BootVolumeType`, `BootVolumeRequest`,
`BootVolumePlan` and `BootVolumeSource`, `RootVolume`, and
`BootVolumeAllocation`. Carry optional profile settings through
`ScalewayConfig` into `InstanceRequest`, and record the allocation on
`InstanceHandle`. End state: the settings parse, validate, and appear on the
request; `dump-request` prints them.

### EP-M2: preflight and deterministic plan resolution

Add a paginated zone `products/servers` lookup, return the image root type and
size from the resolver, and implement the pure resolver. Add
`Backend::preflight` and use the same resolution in `create`.

### EP-M3: explicit create payload, root capture, and post-POST cleanup

Send `volumes["0"]`, read the actual root (local from the server, SBS from the
Block API), verify it against the plan, record it on the handle, and destroy
the server best-effort when a post-POST step fails.

### EP-M4: init preflight and allocation reporting

Preflight in `InitOrchestrator` before cache allocation, and add optional
reporters to both orchestrators, wired to stderr in `src/main.rs`.

### EP-M5: documentation and the opt-in live check

Update `docs/users-guide.md` and `docs/mriya-technical-design.md`, add
`tests/scaleway_boot_volume.rs` and the `make scaleway-boot-test` target.

## Progress

- [x] Read the repository, the Scaleway OpenAPI schemas, and the issue.
- [ ] EP-M1: domain model and configuration.
- [ ] EP-M2: preflight and resolver.
- [ ] EP-M3: create payload, root capture, cleanup.
- [ ] EP-M4: init preflight and reporting.
- [ ] EP-M5: documentation and live check.

## Surprises & Discoveries

- Observation: this branch is based on `200563a`, which predates the `#153`
  Block Storage migration. The cache volume here is still created through the
  Instances API with `b_ssd`, and the SBS migration lands separately.
  Evidence: `git merge-base HEAD main` is `4a3ceda`; `git branch --contains`
  shows `5c2a8c2` only on the `#153` branch.
  Consequence: this work uses the Instances volume API as it exists on this
  base and does not depend on `#153`'s new module layout. Where a boot-storage
  decision overlaps `#153` (SBS volume types), the identifier chosen here is
  the one both branches must agree on: `sbs_volume`.

## Decision Log

- Decision: hold the provider-neutral domain model in `src/boot_volume.rs`
  rather than in `src/scaleway/`.
  Rationale: the `Backend` port must be able to name a boot plan without
  importing Scaleway types, and a future backend should reuse it.

## Outcomes & Retrospective

To be completed.

## Revision note

Initial draft, 26 November 2026.
