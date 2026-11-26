# Mriya development roadmap

This roadmap translates the technical design into phased, measurable work. It
uses phases for strategic milestones, steps for related workstreams, and tasks
for concrete, testable deliverables.

Checkbox states record whether a deliverable exists in the code. Evidence
notes, dated 26 November 2026, record what the test suite actually establishes,
so a ticked box is not mistaken for live provider verification. An
implementation detail that is not exercised by a test is called out in the note
rather than implied by the tick.

## 1. MVP – Scaleway single backend

### 1.1. Instance lifecycle and sync

- [x] Implement `ScalewayBackend` with create/wait/destroy covering token,
  project, image, and instance type inputs, and verify teardown leaves no
  residual resources via an integration check.
  - Evidence: the opt-in live scenario provisions a `DEV1-S` instance, waits
    for a reachable public IPv4, and destroys it. Teardown is verified only by
    polling until the server disappears from the provider's listing; volumes
    and IP addresses are not inventoried. Partial provisioning can leave a
    stopped server when attachment or power-on fails before a handle is
    returned. See [issue #154](https://github.com/leynos/mriya/issues/154).
  - The boot-volume type and size are left to provider defaults and are not
    recorded. See [issue #155](https://github.com/leynos/mriya/issues/155).
- [x] Ship git-aware file sync that uses rsync with `.gitignore` filters and
  avoids deleting ignored cache paths, proven by an end-to-end run where a
  pre-created `target/` on the remote is preserved and the local exit code
  matches the remote command.
  - Evidence: the `.gitignore` filter and `--delete` behaviour run against a
    local copy simulator (`tests/sync/rsync_simulator.rs`), and exit-code
    propagation runs against a scripted command runner. No recorded live run
    inspects a remote cache after sync.

### 1.2. Remote execution flow

- [x] Provide `mriya run` that invokes the system `ssh` client to execute a
  user command, streams stdout/stderr, and returns the remote exit status;
  acceptance: CLI run of `cargo test` returns identical codes locally and via
  Mriya.
  - Evidence: exit-code propagation, sync failure, and teardown failure are
    scripted; no recorded live run executes `cargo test` on an instance.
- [x] Define minimal `mriya.toml` profile schema for Scaleway credentials,
  defaults, and SSH key reference; acceptance: config validation rejects
  missing token/project/image/type/key fields with actionable errors.
  - Evidence: unit tests cover required-field validation and the diagnostic for
    each missing value.

## 2. Persistent cache volume

### 2.1. Volume attachment

- [x] Allow an optional volume ID in config and attach it at VM creation,
  mounting to a stable path; acceptance: two consecutive runs reuse the same
  volume and retain files created in the first run.
  - Evidence: attach and detach request bodies are asserted against a loopback
    HTTP stub, and the mount and routing steps are scripted. No test performs
    two consecutive runs, so file retention across runs is unverified. Volume
    creation still uses the Instances API `b_ssd` contract; see
    [issue #153](https://github.com/leynos/mriya/issues/153).

### 2.2. Cache routing

- [x] Route build caches to the volume by setting `CARGO_HOME`, `RUSTUP_HOME`,
  and `CARGO_TARGET_DIR` (and equivalents for other languages) in the remote
  session; acceptance: repeated `cargo test` runs compile incrementally with
  unchanged source.
  - Evidence: the exported variables are asserted in the rendered remote
    command, and routing is gated on the mount path being a mount point. No
    test compiles twice or measures incremental reuse.

## 3. Configurable instances and cloud-init

### 3.1. Instance configurability

- [x] Support per-run or per-profile overrides for instance type and image,
  with provider-specific validation; acceptance: unsupported type/image names
  yield clear errors and valid overrides take effect in create requests.
  - Evidence: request construction for a valid non-default type and image is
    unit-tested. The opt-in live scenarios reject an unknown type and an
    unknown image label, but none proves that a valid non-default override is
    accepted by the provider. The instance type is validated from the
    create-server response, not by a preflight availability check.

### 3.2. Cloud-init provisioning

- [x] Accept inline or file-based cloud-init user data, pass it through the
  backend, and wait for SSH readiness; acceptance: provisioning script installs
  declared packages before the test command starts, verified by an integration
  run that uses a cloud-config to install a package and then executes it.
  - Evidence: an opt-in live scenario installs `jq` through cloud-config and
    runs `jq --version`; the repository records no executed result for it, only
    that it is skipped without `MRIYA_RUN_SCALEWAY_TESTS`. Readiness ordering,
    timeout, and teardown are scripted.

## 4. `mriya init` for cache volume preparation

### 4.1. Volume lifecycle automation

- [x] Implement `mriya init` to create a provider volume in the configured
  zone, format it (ext4), and detach it cleanly; acceptance: the new volume ID
  is written to `mriya.toml` and can be attached on the next `mriya run`.
  - Evidence: nine scripted scenarios cover success, formatting, readiness,
    detachment, and teardown failures, and `--force` against a fake backend and
    scripted SSH runner. No live `mriya init` and no `init`-then-`run` test
    exists. A failed `init` leaves its created volume unrecorded. Volume
    creation still posts to the Instance API, which Scaleway's changelog says
    has been unable to create block volumes since 30 April 2025, so the
    creation step is expected to fail until the migration lands. See
    [issue #153](https://github.com/leynos/mriya/issues/153) and
    [issue #154](https://github.com/leynos/mriya/issues/154).

### 4.2. Mount conventions

- [x] Establish a default mount point (for example `/mriya` or `/home`) and
  set symlinks or environment overrides so language toolchains and build
  outputs land on the volume; acceptance: after `mriya init`, caches survive VM
  teardown and are discovered automatically on subsequent runs.
  - Evidence: mounting, cache-directory creation, and environment routing are
    scripted. Cache survival across VM teardown is unverified.

## 5. `mriya bake-image`

### 5.1. Snapshot flow

- [ ] Provide a bake command that stops a VM, snapshots its root disk to a
  named custom image, and updates config to use that image; acceptance: the
  next run boots from the baked image without re-running cloud-init package
  installation.

### 5.2. Image hygiene

- [ ] Apply a naming scheme and retention policy guidance for baked images,
  and surface warnings for stale images; acceptance: documentation plus a CLI
  prompt describing storage implications when baking.

## 6. Integrated SSH and robustness

### 6.1. Native SSH client

- [ ] Replace system `ssh` with `russh`, supporting key loading (file and
  agent) and trust-on-first-use host key storage; acceptance: parity test
  showing identical output and exit codes versus the CLI ssh path.

### 6.2. Timeouts and cancellation

- [ ] Add configurable timeouts for VM creation, SSH readiness, and remote
  command execution, plus SIGINT/SIGTERM handling that aborts the remote job
  and destroys the VM; acceptance: manual Ctrl+C leaves no orphaned instances
  or volumes.

### 6.3. Progress visibility

- [ ] Emit structured progress logs for create, sync, run, and teardown, and
  present actionable errors; acceptance: smoke test shows phase-labelled logs
  and a clear message for a forced SSH authentication failure.

## 7. Multi-cloud rollout

### 7.1. Hetzner Cloud

- [ ] Implement `HetznerBackend` with server create/wait/destroy, volume
  attach/mount, and cloud-init pass-through; acceptance: end-to-end run on
  Hetzner using `mriya run` with caches persisted between two runs.

### 7.2. DigitalOcean

- [ ] Implement `DigitalOceanBackend` with droplet lifecycle, volume
  handling, cloud-init, and SSH key injection; acceptance: end-to-end run on
  DigitalOcean matches Scaleway behaviour, including cache reuse.

### 7.3. AWS EC2

- [ ] Implement `AwsBackend` using the official SDK with AMI, key pair,
  security group, subnet selection, EBS volume attachment, and cloud-init;
  acceptance: end-to-end run on AWS returns remote exit codes, reuses the cache
  volume, and enforces resource cleanup on failure.
