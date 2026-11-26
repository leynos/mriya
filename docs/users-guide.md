# Mriya user guide

Mriya provisions a short‑lived Scaleway virtual machine (VM), syncs the working
tree, runs a command over secure shell (SSH), and tears the VM down. This guide
explains how to configure credentials and what guarantees the minimum viable
product (MVP) backend provides.

## Configure credentials (ortho-config)

The command-line interface (CLI) uses `ortho-config` layering: defaults <
config files < environment < CLI flags. Environment variables use the `SCW_`
prefix:

- `SCW_SECRET_KEY` (required) — Scaleway application programming interface
  (API) secret key.
- `SCW_ACCESS_KEY` (optional) — recorded for future audit features.
- `SCW_DEFAULT_PROJECT_ID` (required) — project to bill instances to.
- `SCW_DEFAULT_ORGANIZATION_ID` (optional) — only needed for org-scoped calls.
- `SCW_DEFAULT_ZONE` — defaults to `fr-par-1`.
- `SCW_DEFAULT_INSTANCE_TYPE` — defaults to `DEV1-S`. The default is a
  starting point, not a cost guarantee; see
  [Cost and resource lifetime](#cost-and-resource-lifetime).
- `SCW_DEFAULT_IMAGE` — defaults to `Ubuntu 24.04 Noble Numbat`.
- `SCW_DEFAULT_ARCHITECTURE` — defaults to `x86_64`.
- `SCW_CLOUD_INIT_USER_DATA` — optional cloud-init user-data content.
- `SCW_CLOUD_INIT_USER_DATA_FILE` — optional path to a cloud-init user-data
  file.

For configuration files, place `mriya.toml` under the usual XDG (X Desktop
Group) config locations. Values are merged with the same precedence; CLI flags
override everything. Set `MRIYA_CONFIG_PATH` to point to a specific config file
to override discovery.

Example `mriya.toml`:

```toml
[scaleway]
secret_key = "scw-secret-key-here"
default_project_id = "11111111-2222-3333-4444-555555555555"

[sync]
ssh_identity_file = "~/.ssh/id_ed25519"
ssh_user = "ubuntu"
```

## Configuration validation

Mriya validates all required fields at startup. If a required field is missing,
the error message includes guidance on how to provide the value:

```text
missing Scaleway API secret key: set SCW_SECRET_KEY or add secret_key to [scaleway] in mriya.toml
```

Required fields for Scaleway credentials: `secret_key`, `default_project_id`,
`default_image`, `default_instance_type`, `default_zone`,
`default_architecture`.

Required sync settings: `ssh_identity_file`.

## File sync semantics

Mriya syncs the working tree with `rsync -az --delete --filter=":- .gitignore"`
so only files not matched by `.gitignore` patterns are transferred. Ignored
cache paths such as `target/` are **not** deleted remotely, which keeps
pre-existing build outputs available for incremental runs. The `.git` directory
is excluded from transfer.

Remote commands execute through the system `ssh` client, and Mriya mirrors the
remote exit code. If `cargo test` fails remotely with exit status 101, the
local process will also exit 101. Commands run via `sync_and_run` automatically
`cd` into `MRIYA_SYNC_REMOTE_PATH` before execution, so callers do not need to
prefix their commands with a directory change.

## Run a command remotely

Use `mriya run -- <command>` to provision a VM, sync the working tree, and run
the provided command over SSH. Output streams live to the local terminal using
the system `ssh` client and the configured `rsync` binary. The CLI exits with
the remote command's status code; when the remote process terminates without a
status (for example, due to a signal), Mriya exits with code 1 and reports the
missing status.

To override the Scaleway instance type or image for a single run, pass
`--instance-type` and/or `--image`:

```bash
mriya run --instance-type DEV1-M --image "Ubuntu 24.04 Noble Numbat" -- cargo test
```

The Scaleway backend validates that the instance type exists in the selected
zone from the provider's response to the create-server request; there is no
preflight availability check before the request is sent. Image labels are
resolved for the selected architecture and zone:

1. If the project has any images at all, only those are considered; otherwise
   the public images are used. The two lists are never merged.
2. Candidates are filtered by the requested architecture and to images the
   provider reports as `available`.
3. The newest candidate by creation date is chosen.

Unsupported values yield provider-specific errors (for example, unknown
instance types).

## Cloud-init provisioning

Mriya can pass a cloud-init *user-data* payload through to the provider when
creating the VM. This allows installation of system packages or other
first-boot provisioning before the remote command starts.

Provide user-data either inline or via a local file:

```bash
mriya run --cloud-init-file ./cloud-init.yml -- jq --version
```

Or inline (handy for short scripts):

```bash
mriya run --cloud-init "#!/bin/sh\necho ready" -- sh -lc 'echo ok'
```

A file path starting with `~/` is expanded through `HOME` before the file is
read:

```bash
mriya run --cloud-init-file '~/cloud-init.yml' -- jq --version
```

Quote the path. Unquoted, your shell expands `~` itself before Mriya sees it,
so the path arrives already absolute; quoting hands Mriya the tilde and lets
the rule below apply.

If `HOME` is absent the path is passed through unchanged, which is what a shell
does for a path it cannot expand. If `HOME` is set but is not valid UTF-8, the
run fails with an error naming the file and suggesting an absolute path, rather
than silently reading a path built from replacement characters that may name a
different file. The same rule applies to a `~/.ssh/...` identity path: the
error names the configured `ssh_identity_file` and says to use an absolute one.
`mriya init` reaches the same expansion when it runs its format command over
SSH, and reports the same cause rather than a bare volume-format failure.

When cloud-init user-data is configured, Mriya:

- syncs the workspace as usual
- waits for cloud-init to finish (by checking
  `/var/lib/cloud/instance/boot-finished`)
- only then executes the remote command

Cloud-init settings use `ortho-config` layering with the `SCW_` prefix:

- `SCW_CLOUD_INIT_USER_DATA` — cloud-init user-data content (optional).
- `SCW_CLOUD_INIT_USER_DATA_FILE` — path to a file containing cloud-init
  user-data (optional).

In `mriya.toml`, configure one of the following under `[scaleway]`:

- `cloud_init_user_data = "..."` (inline)
- `cloud_init_user_data_file = "./cloud-init.yml"` (file-based)

Commands always execute from `MRIYA_SYNC_REMOTE_PATH` (default:
`/home/ubuntu/project`) so `mriya run -- cargo test` mirrors running
`cargo test` locally after syncing the workspace. Customize the remote user,
working directory, or SSH flags via the `MRIYA_SYNC_` variables described below.

> Security: host key checking defaults to disabled
> (`MRIYA_SYNC_SSH_STRICT_HOST_KEY_CHECKING=false`)
> with `MRIYA_SYNC_SSH_KNOWN_HOSTS_FILE=/dev/null` to keep ephemeral VM setup
> friction low. This sacrifices MITM protection and is suitable only for
> trusted, short-lived environments. Enable strict checking and a real known
> hosts file when connecting to persistent or untrusted hosts.

Sync settings use `ortho-config` layering with the `MRIYA_SYNC_` prefix:

- `MRIYA_SYNC_SSH_IDENTITY_FILE` (required) — path to the SSH private key file
  for remote authentication. Supports tilde expansion (e.g.,
  `~/.ssh/id_ed25519`).
- `MRIYA_SYNC_RSYNC_BIN` — path to the `rsync` executable (default: `rsync`).
- `MRIYA_SYNC_SSH_BIN` — path to the `ssh` executable (default: `ssh`).
- `MRIYA_SYNC_SSH_USER` — remote user for rsync and SSH (default: `root`).
- `MRIYA_SYNC_REMOTE_PATH` — remote working directory
  (default: `/home/ubuntu/project`).
- `MRIYA_SYNC_SSH_BATCH_MODE` — set to `false` to allow interactive SSH
  prompts (default: `true`).
- `MRIYA_SYNC_SSH_STRICT_HOST_KEY_CHECKING` — set to `true` to enforce host key
  verification (default: `false`).
- `MRIYA_SYNC_SSH_KNOWN_HOSTS_FILE` — path to a known hosts file (default:
  `/dev/null` when host key checking is disabled).
- `MRIYA_SYNC_VOLUME_MOUNT_PATH` — mount path for the persistent cache volume
  (default: `/mriya`).
- `MRIYA_SYNC_ROUTE_BUILD_CACHES` — set to `false` to disable automatic cache
  routing to the mounted volume (default: `true`).
- `MRIYA_SYNC_CREATE_CACHE_DIRECTORIES` — set to `false` to disable automatic
  creation of cache subdirectories on the mounted volume (default: `true`).

## Persistent cache volume

Mriya can attach a Block Storage volume to the ephemeral VM for caching build
artefacts between runs. When configured, Mriya attempts to attach the volume
before the instance starts and to mount it at `/mriya`. Place build caches
(such as Cargo's target directory) on this volume to persist compiled
dependencies across runs.

To enable the cache volume:

1. Run `mriya init` to create and format a Block Storage volume in the
   configured zone. The command writes the new volume ID into `mriya.toml`. Use
   `mriya init --force` to replace an existing configured volume ID. If any
   step after the formatter instance is created fails — including the instance
   read it performs before detaching the volume — the command attempts to
   destroy the formatter instance before reporting the error, and adds a note
   when that teardown also fails. A teardown failure therefore still leaves a
   VM billing, and `mriya init` never deletes the volume it created: a failed
   run can leave an unrecorded volume that only manual cleanup removes, because
   the janitor sweeps only resources carrying a `mriya-test-run-<id>` tag.
2. (Optional) override the volume ID explicitly via configuration:

   ```bash
   export SCW_DEFAULT_VOLUME_ID="11111111-2222-3333-4444-555555555555"
   ```

   Or in `mriya.toml`:

   ```toml
   [scaleway]
   default_volume_id = "11111111-2222-3333-4444-555555555555"
   ```

3. Configure the build tool to use the mounted volume. For Cargo:

   ```bash
   mriya run -- cargo build
   ```

Mriya attempts the mount at the configured path, `/mriya` by default, and keeps
going when it fails. If the mount fails — for example, because the volume has
no filesystem — the run continues without the cache, which allows graceful
degradation for first-time setups.

Cache routing happens only when the configured path **is** a mount point at run
time; the remote command tests it with `mountpoint -q` before exporting
anything. An earlier mount can satisfy that test, and a path that is not a
mount point exports nothing. When routing is active, Mriya:

1. Creates cache subdirectories under the mount point so toolchains can write
   immediately (`cargo`, `rustup`, `target`, `go/pkg/mod`, `go/build-cache`,
   `pip/cache`, `npm/cache`, `yarn/cache`, `pnpm/store`). Creation is attempted
   only: it runs under the same mount-point guard, discards errors, and
   tolerates a read-only or full filesystem, so a toolchain must still cope
   with a missing directory.
2. Exports environment variables in the remote session to route common language
   caches to the volume. These include `CARGO_HOME`, `RUSTUP_HOME`, and
   `CARGO_TARGET_DIR` (Rust/Cargo); `GOMODCACHE` and `GOCACHE` (Go);
   `PIP_CACHE_DIR` (Python/pip); `npm_config_cache`, `YARN_CACHE_FOLDER`, and
   `PNPM_STORE_PATH` (Node tooling).

### Volume size units

`mriya init` uses a temporary formatter instance and runs `mkfs.ext4` against
the new volume. `MRIYA_INIT_VOLUME_SIZE_GB` and `[init].volume_size_gb` default
to `20`, and Mriya multiplies that number by 1024³ (1,073,741,824). The default
is therefore **20 GiB**, or 21,474,836,480 bytes — not 20 decimal gigabytes.
The configured number is always a count of gibibytes.

Zero is rejected at validation time. `mriya init` has no size flag, so the size
comes from configuration or the environment only.

The example below is illustrative: it requests 50 GiB, or 53,687,091,200 bytes.

```toml
[init]
volume_size_gb = 50
```

The `volume_size_gb` and `MRIYA_INIT_VOLUME_SIZE_GB` identifiers keep their
current spelling. A future rename must retain both as accepted aliases, because
they are part of the configuration surface users already have on disk.

**Requirements:**

- The volume must exist in the same zone as the instance.
- The volume should be formatted with a filesystem (ext4 recommended) before
  first use. `mriya init` formats the volume automatically.
- Only one instance can attach a given volume at a time. Mriya has no lock,
  queue, or retry around this: two concurrent runs that share a volume will
  contend, and the loser's attachment is rejected after its server has already
  been created, leaving that stopped server behind. Run one `mriya run` per
  cache volume at a time.

## What the Scaleway backend does now

- Resolves an image by label, architecture, and zone using the order described
  under [Run a command remotely](#run-a-command-remotely): project images
  first, then public images, newest available match wins.
- Creates an instance with a routed public IPv4 address and tags `mriya` and
  `ephemeral`. The instance type is validated from the provider's create-server
  response rather than by a preflight check.
- Requests the cache volume at creation time when `SCW_DEFAULT_VOLUME_ID` is
  configured, and mounts it to `/mriya` after the instance boots. The mount is
  attempted, not guaranteed; see
  [Persistent cache volume](#persistent-cache-volume).
- Polls every five seconds (up to five minutes) until the instance is running
  and reachable via SSH on port 22.
- Sends a delete request for the instance and polls until the API no longer
  lists it, failing when the server is still listed at the deadline.

Teardown verification stops at server disappearance. Mriya does not inventory
attached storage or public IP addresses, and a failure between a successful
create request and the returned handle leaves a stopped server that it cannot
destroy. See
[Cleanup: what is and is not verified](#cleanup-what-is-and-is-not-verified).

Each API request to read or list instances, create a server or volume, power an
instance on, attach or detach a volume, or delete an instance has a 30-second
timeout. That includes the instance read `mriya init` performs before it
detaches the volume, so a hung provider cannot leave the command waiting
indefinitely. If a request times out, Mriya reports a provider error; check
connectivity to the Scaleway API and the instance's actual state before
retrying, as the request may have reached Scaleway even though its response did
not arrive. Image lookup is the exception: it goes through the Scaleway SDK,
which applies no such bound.

The 300-second readiness deadline and the 600-second cloud-init deadline are
polling settings, not guarantees about how long a run takes or how long it is
billed.

## Running the integration check

`make scaleway-test` is opt-in and creates real, billable resources. Without it
the live scenarios skip themselves, so an unconfigured run provisions nothing.
Other tests in the suite do use the network stack, but only against loopback
servers and local files: the Scaleway request tests point the backend at a stub
HTTP server on the loopback interface, and the sync tests copy files locally.
No test contacts a cloud provider unless the live suite is enabled.

```bash
make scaleway-test
```

The target:

- Requires `uuidgen` and `scw` on `PATH`, failing early when either is absent.
- Generates `MRIYA_TEST_RUN_ID` via `uuidgen`, so every created instance and
  volume carries a `mriya-test-run-<id>` tag.
- Runs `mriya-janitor` before the tests, and again from a shell `EXIT` trap so
  the sweep still happens when the test command fails.
- Sets `MRIYA_RUN_SCALEWAY_TESTS=1` to enable the live suite.

The live suite has two behaviours worth knowing before reading a pass as
broader evidence than it is:

- A missing fixture skips the scenario instead of failing it. Absent
  credentials, or a provider response reporting `permissions_denied`, produce a
  skip with the reason, so an unconfigured checkout reports success without
  provisioning anything.
- Lifecycle verification checks that the server disappears from the provider's
  listing. It does not check volumes or IP addresses. See
  [issue #154](https://github.com/leynos/mriya/issues/154).

The underlying `cargo test` uses `--test-threads=1`, which serializes scenarios
within that one invocation. It is not a lock: nothing prevents a second
`make scaleway-test`, or a concurrent `mriya run` that shares a cache volume,
from running at the same time.

The janitor sweeps only resources whose tag matches the current test run. It
deletes tagged servers with `with-volumes=none`, then tagged Block Storage
volumes, and re-lists both to confirm the sweep is clean; IP addresses are not
listed or checked.

## Cost and resource lifetime

A Mriya run bills for the lifetime of the resources it creates, not for the
duration of the remote command. A two-minute test still pays for provisioning,
boot, sync, and teardown, and providers meter each resource class separately
with their own minimum billing period. Treat any estimate built by summing
command runtimes as a lower bound that is usually far too low.

### What a run creates

From the moment the create-server request is accepted, a run can involve:

- A compute instance, from creation through deletion, including the stopped
  interval before power-on and the sync and cloud-init wait after it.
- A root (boot) volume whose type and size Mriya does not choose. See
  [Root storage is implicit](#root-storage-is-implicit).
- A public IP address. Mriya requests a dynamic, routed address without
  specifying its type, so the applicable billing rule is whatever the provider
  attaches to the allocation it makes.
- Optionally, a retained cache volume, billed from its creation until it is
  deleted — across every run, and while no run is active.
- For `mriya init`, a short-lived formatter instance plus the new volume.

Setup and teardown count. The remote command is only one stage of a longer
sequence: create the stopped server, attach the cache volume, power on, wait
for a public IP and SSH, mount, sync, wait for cloud-init, run the command,
request deletion, and poll for the server to disappear. The readiness and
cloud-init deadlines are polling budgets, not durations to budget for.

### Billing minima and granularity

Scaleway's Billing FAQ[^1] states that each resource is billed separately for a
minimum of 60 minutes, and that each uninterrupted period between a start and a
stop billing event counts as a separate resource. Two distinct five-minute runs
are therefore two separately metered resources, not ten minutes of one. The
Instances FAQ[^2] describes CPU instances as billed per hour of uptime and GPU
instances per minute, except RENDER-S, which is billed per hour.

The two pages disagree on when instance billing starts: the Billing FAQ says it
starts at power-on, while the Instance pricing guide[^3] says it starts when
the instance is created. Confirm current terms before relying on either for a
specific product, and do not assume another provider uses the same minimum, the
same granularity, or the same start event.

### Root storage is implicit

The create-server request carries no boot-volume type or size, so the provider
decides both. This has two cost consequences:

- The billed capacity is not a capacity the operator selected, because neither
  setting is expressed in configuration.
- The storage tier is not necessarily the one the instance type suggests. A live
  inspection recorded in
  [issue #155](https://github.com/leynos/mriya/issues/155) found a 10 GB SBS
  root volume, with 5.9 GB free after boot, on an instance type that also
  offers local storage.

Root volumes are billed as storage for as long as they exist. The investigation
recorded in [issue #154](https://github.com/leynos/mriya/issues/154) observed
an SBS root volume surviving server deletion and remaining billable. Making
boot storage explicit is tracked in
[issue #155](https://github.com/leynos/mriya/issues/155); ownership-aware
deletion is tracked in [issue #154](https://github.com/leynos/mriya/issues/154).

### Cache storage is retained and billed

A cache volume is billed from creation until deletion, whether or not a run is
active and whether or not it is attached. Its cost is the capacity times the
storage rate times the hours it exists, so a cache retained for a week costs
far more than the runs that used it.

Two failure modes extend that lifetime unexpectedly:

- `mriya init` never deletes the volume it creates. If a later step fails, the
  volume exists with no configuration entry pointing at it, and nothing sweeps
  it unless it carries a test-run tag.
- `mriya init --force` replaces the configured ID with a new volume. The
  previously configured volume is not deleted, so both are billed until the old
  one is removed by hand.

### IP addresses

Mriya requests a dynamic, routed public IPv4 address and does not specify an IP
type. The IP billing page[^4] bills flexible IPs separately from the resource
they are attached to, from reservation until deletion, whether or not they are
attached. The Billing FAQ instead ties dynamic-IP billing to association with a
running instance. Because Mriya does not assert which type it receives, treat
the address as potentially billable beyond the server's own lifetime and check
the allocation after a run. Do not assume the assigned type from the request.

### Worked example: ten short jobs at the per-run minimum

The symbols below let a reader substitute a provider's published rates without
this guide hard-coding a price:

- `N` — number of runs.
- `s` — setup time per run: create, attach, power on, and reach SSH.
- `c` — remote command duration.
- `t` — teardown time per run: delete request plus polling.
- `M` — the provider's minimum billing period per resource, in minutes.
- `g` — the provider's billing granularity, in minutes.
- `R_vm` — the instance-type rate per billed hour.

`M`, `g`, `s`, `c`, and `t` are all in minutes, so the billed duration must be
divided by 60 before an hourly rate is applied. Per-run billed compute, in
minutes, is `max(M, ceil((s + c + t) / g) * g)`; total compute cost is
`N * (per-run billed compute) / 60 * R_vm`. Root storage, cache storage, and IP
charges are separate terms: per-run root storage is billed for each run's own
root volume; a cache volume is `capacity_GiB * retained_hours * R_storage`; and
the `mriya init` formatter instance is a run of its own for compute purposes.

Take ten runs of a two-minute command with three minutes of setup and three
minutes of teardown. Each run measures eight minutes, so 80 minutes of measured
lifetime cover 20 minutes of command time. With `M = 60` minutes and `g = 60`
minutes, each run is billed as one hour:

```text
per-run billed compute = max(60, ceil((3 + 2 + 3) / 60) * 60) = 60 minutes
total compute cost     = 10 * 60 / 60 * R_vm = 10 * R_vm
```

So 20 minutes of command time costs 10 instance-hours of compute — ten times
`R_vm` — before storage or IP charges are added. Halving the command time
barely changes the total, because the minimum dominates; halving the number of
runs halves it. This example uses the Scaleway CPU-instance terms described
above, as of the cited review dates. It does not apply to every provider or
product, and it quotes no price.

### Quoting a price

Any price stated in a Mriya discussion, issue, or document must name:

- the product and instance type;
- the location or zone;
- the currency;
- whether tax is included;
- the date the price was read;
- whether the figure is an estimate or a measured charge.

Prefer a formula and a dated link over a hard-coded number. A price without
those fields is unusable six months later and misleading immediately.

### Cleanup: what is and is not verified

Mriya sends a delete request for the server after every run and polls until the
provider stops listing it. That is the extent of the guarantee:

- The delete is **attempted** on every path that holds a server handle, and a
  teardown failure is reported rather than swallowed.
- Verification covers server disappearance only. Attached storage and public IP
  addresses are not inventoried, so a residual volume or address will not be
  reported by Mriya. See
  [issue #154](https://github.com/leynos/mriya/issues/154).
- If provisioning fails after the create request succeeded but before a handle
  was returned — volume validation, attachment, or power-on — no handle exists
  to delete, and the stopped server is left behind.
- A failed `mriya init` leaves its created volume unrecorded; see
  [Cache storage is retained and billed](#cache-storage-is-retained-and-billed).
- The janitor used by the live test suite is tag-scoped to a single test run
  and does not cover resources created outside it.

Do not rely on Mriya as a cost control. Check the provider console after
unusual failures, and delete cache volumes that are no longer needed.

### Concurrency

Only one instance can attach a given Block Storage volume at a time, and the
volume must be in the same availability zone as the instance; Scaleway's Block
Storage attach guide[^5] states both constraints. This is a provider rule, so
it applies to every client, not just Mriya.

Mriya enforces nothing on top of it. There is no lock, queue, or retry around
the attachment. Two runs sharing a cache volume contend, and the losing run's
attachment is rejected after its server already exists, leaving a stopped
server that Mriya cannot destroy because it never received a handle. Serialize
runs that share a volume; use separate volumes or separate zones when runs must
overlap.

## Building from source

Building needs `rustup`, which installs the nightly toolchain that
`rust-toolchain.toml` pins, and a C compiler for linking. The repository's
Cargo defaults and the Makefile development targets (`make build`, `make test`,
`make lint` and `make typecheck`) also use the `mold` linker on Linux, so a
Linux development build needs `mold` and a compiler that accepts
`-fuse-ld=mold`: GCC 12.1 or newer, or clang. `make install-build-tools`
installs the pinned `mold` and toolchain, and `make check-build-tools` reports
what is missing; a development target stops with that report instead of falling
back to another linker.

The preflight requires `mold` at exactly the version in `tools/mold/VERSION`
(`2.41.0`) and fails on any other version, so an older distribution release
does not pass. `make install-build-tools` unpacks the pinned release under
`$(BUILD_TOOLS_PREFIX)`, which defaults to `~/.local`. The Makefile puts that
prefix's `bin` first on `PATH`, so every Make target uses the pinned release; a
bare `cargo build` outside Make needs the same directory on `PATH`. The example
reads the variable the Makefile reads, so it follows an override:

```bash
export PATH="${BUILD_TOOLS_PREFIX:-$HOME/.local}/bin:$PATH"
```

A release build (`make release`) needs neither `mold` nor the preflight. It
assigns its own `RUSTFLAGS`, which displaces the configuration's flags, so a
shipped artefact keeps the platform linker. To build with the platform linker
directly, assign an empty `RUSTFLAGS`:

```bash
RUSTFLAGS="" cargo build --release
```

## Using Mriya as a library

The crate exposes the same behaviour the binary uses, so a program can expand a
`~/` path or resolve cloud-init user-data without going through the CLI. Every
entry point that reads the environment has two forms: the default one reads the
real process environment, and a `_with_env` counterpart takes any
`mockable::Env` so your own tests can inject a value.

```rust
use mriya::sync::{expand_tilde, expand_tilde_with_home, home_from_env};
use mockable::{DefaultEnv, Env};

// The compatibility helper reads the live process environment.
let expanded = expand_tilde("~/.ssh/id_ed25519");

// The pure form does the string work with no I/O, so a test needs no environment.
let explicit = expand_tilde_with_home("~/.ssh/id_ed25519", Some("/home/me"));
assert_eq!(explicit, "/home/me/.ssh/id_ed25519");

// `home_from_env` distinguishes an absent HOME from one that cannot be decoded.
match home_from_env(&DefaultEnv, "~/key") {
    Ok(Some(home)) => println!("home is {home}"),
    Ok(None) => println!("HOME is unset; the tilde is left for ssh"),
    Err(err) => eprintln!("HOME cannot be used: {err}"),
}
```

`expand_tilde` returns the path unchanged when `HOME` is absent or cannot be
decoded, because its signature carries no error. Prefer `home_from_env` when
you need to tell those two cases apart or report the cause; the error type
implements `std::error::Error`, so `?` propagates it in a program whose error
type accepts it. Everything to change when upgrading a dependent is listed in
[Migrating to 0.2.0](migration-0.2.0.md).

[^1]: Scaleway, "Billing FAQ",
    <https://www.scaleway.com/en/docs/billing/faq/>, reviewed 9 September 2025.

[^2]: Scaleway, "Instances FAQ",
    <https://www.scaleway.com/en/docs/instances/faq/>, reviewed 19 December
    2025.

[^3]: Scaleway, "Understanding Scaleway Instance pricing",
    <https://www.scaleway.com/en/docs/instances/reference-content/understanding-instance-pricing/>,
    reviewed 19 August 2025.

[^4]: Scaleway, "Understanding billing for Scaleway IP addresses",
    <https://www.scaleway.com/en/docs/ipam/reference-content/understanding-ip-billing/>,
    reviewed 4 November 2025.

[^5]: Scaleway, "How to attach a Block Storage volume to an Instance",
    <https://www.scaleway.com/en/docs/block-storage/how-to/attach-a-volume/>,
    reviewed 21 October 2025.
