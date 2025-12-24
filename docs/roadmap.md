# Mriya development roadmap

This roadmap translates the technical design into phased, measurable work. It
uses phases for strategic milestones, steps for related workstreams, and tasks
for concrete, testable deliverables.

## Phase 0: MVP – Scaleway single backend

- Step: Instance lifecycle and sync

  - [x] Implement `ScalewayBackend` with create/wait/destroy covering token,
    project, image, and instance type inputs, and verify teardown leaves no
    residual resources via an integration check.
  - [x] Ship git-aware file sync that uses rsync with `.gitignore` filters and
    avoids deleting ignored cache paths, proven by an end-to-end run where a
    pre-created `target/` on the remote is preserved and the local exit code
    matches the remote command.

- Step: Remote execution flow

  - [x] Provide `mriya run` that invokes the system `ssh` client to execute a
    user command, streams stdout/stderr, and returns the remote exit status;
    acceptance: CLI run of `cargo test` returns identical codes locally and via
    Mriya.
  - [x] Define minimal `mriya.toml` profile schema for Scaleway credentials,
    defaults, and SSH key reference; acceptance: config validation rejects
    missing token/project/image/type/key fields with actionable errors.

## Phase 0.1: Persistent cache volume

- Step: Volume attachment

  - [x] Allow an optional volume ID in config and attach it at VM creation,
    mounting to a stable path; acceptance: two consecutive runs reuse the same
    volume and retain files created in the first run.

- Step: Cache routing

  - [x] Route build caches to the volume by setting `CARGO_HOME`, `RUSTUP_HOME`,
    and `CARGO_TARGET_DIR` (and equivalents for other languages) in the remote
    session; acceptance: repeated `cargo test` runs compile incrementally with
    unchanged source.

## Phase 0.2: Configurable instances and cloud-init

- Step: Instance configurability

  - [x] Support per-run or per-profile overrides for instance type and image,
    with provider-specific validation; acceptance: unsupported type/image names
    yield clear errors and valid overrides take effect in create requests.

- Step: Cloud-init provisioning

  - [x] Accept inline or file-based cloud-init user data, pass it through the
    backend, and wait for SSH readiness; acceptance: provisioning script
    installs declared packages before the test command starts, verified by an
    integration run that uses a cloud-config to install a package and then
    executes it.

## Phase 0.3: `mriya init` for cache volume preparation

- Step: Volume lifecycle automation

  - [x] Implement `mriya init` to create a provider volume in the configured
    zone, format it (ext4), and detach it cleanly; acceptance: the new volume
    ID is written to `mriya.toml` and can be attached on the next `mriya run`.

- Step: Mount conventions

  - [ ] Establish a default mount point (for example `/mriya` or `/home`) and
    set symlinks or environment overrides so language toolchains and build
    outputs land on the volume; acceptance: after `mriya init`, caches survive
    VM teardown and are discovered automatically on subsequent runs.

## Phase 0.4: `mriya bake-image`

- Step: Snapshot flow

  - [ ] Provide a bake command that stops a VM, snapshots its root disk to a
    named custom image, and updates config to use that image; acceptance: the
    next run boots from the baked image without re-running cloud-init package
    installs.

- Step: Image hygiene

  - [ ] Apply a naming scheme and retention policy guidance for baked images,
    and surface warnings for stale images; acceptance: documentation plus a
    CLI prompt describing storage implications when baking.

## Phase 1.0: Integrated SSH and robustness

- Step: Native SSH client

  - [ ] Replace system `ssh` with `russh`, supporting key loading (file and
    agent) and trust-on-first-use host key storage; acceptance: parity test
    showing identical output and exit codes versus the CLI ssh path.

- Step: Timeouts and cancellation

  - [ ] Add configurable timeouts for VM creation, SSH readiness, and remote
    command execution, plus SIGINT/SIGTERM handling that aborts the remote job
    and destroys the VM; acceptance: manual Ctrl+C leaves no orphaned
    instances or volumes.

- Step: Progress visibility

  - [ ] Emit structured progress logs for create, sync, run, and teardown, and
    present actionable errors; acceptance: smoke test shows phase-labelled
    logs and a clear message for a forced SSH authentication failure.

## Phase 1.x: Multi-cloud rollout

- Step: Hetzner Cloud

  - [ ] Implement `HetznerBackend` with server create/wait/destroy, volume
    attach/mount, and cloud-init pass-through; acceptance: end-to-end run on
    Hetzner using `mriya run` with caches persisted between two runs.

- Step: DigitalOcean

  - [ ] Implement `DigitalOceanBackend` with droplet lifecycle, volume
    handling, cloud-init, and SSH key injection; acceptance: end-to-end run on
    DigitalOcean matches Scaleway behaviour, including cache reuse.

- Step: AWS EC2

  - [ ] Implement `AwsBackend` using the official SDK with AMI, key pair,
    security group, subnet selection, EBS volume attachment, and cloud-init;
    acceptance: end-to-end run on AWS returns remote exit codes, reuses the
    cache volume, and enforces resource cleanup on failure.
