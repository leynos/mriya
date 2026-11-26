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
- `SCW_DEFAULT_INSTANCE_TYPE` — defaults to `DEV1-S` (smallest, cheapest).
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
zone during provisioning, and resolves image labels for the selected
architecture. Unsupported values yield provider-specific errors (for example,
unknown instance types).

## Boot volume

Choosing an instance type chooses CPU and RAM. It does not, on its own, choose
the disk the instance starts from: Scaleway picks a default that depends on how
the image was named, whether the instance type offers local storage, and the
marketplace image's own type. Mriya therefore resolves the boot volume
explicitly and sends it in the create request.

Set the boot volume in `[scaleway]`:

```toml
boot_volume_type = "l_ssd"
boot_volume_size_gb = 20
```

Or through the environment:

- `SCW_BOOT_VOLUME_TYPE` — `l_ssd` (local SSD) or `sbs_volume` (Block Storage).
- `SCW_BOOT_VOLUME_SIZE_GB` — capacity in **decimal gigabytes** (10^9 bytes).

Only those two storage types are accepted. The deprecated `b_ssd`, `scratch`,
`unified`, and snapshot identifiers are refused by name rather than silently
mapped onto something else.

### Size units

Three different units appear on this path, and they are not interchangeable:

- `boot_volume_size_gb` and `SCW_BOOT_VOLUME_SIZE_GB` are decimal gigabytes
  (10^9 bytes), matching Scaleway's published examples.
- The provider payload and every API response use plain bytes.
- Mriya's reports state both: `20 GB (20000000000 bytes)`.

The persistent cache volume's `MRIYA_INIT_VOLUME_SIZE_GB` / `volume_size_gb`
setting is a *different* setting and still uses binary gibibytes (1024^3
bytes). Keep the two apart: the boot disk is destroyed with the instance, while
`default_volume_id` names a retained cache volume.

### Compatibility default

When a profile names neither setting, Mriya applies a documented, deterministic
default rather than guessing per run:

1. If the instance type offers local storage and the image can be booted from
   local storage, use `l_ssd` at the instance type's maximum local capacity.
   This mirrors Scaleway's own behaviour for a marketplace image ID.
2. Otherwise use `sbs_volume` at the larger of the image's own root size and
   10 GB, rounded up to a whole gigabyte. The 10 GB floor mirrors the
   provider's documented default OS size.

When the type is named but the size is not, local storage still defaults to the
type's maximum and Block Storage still defaults to the larger of the image root
and 10 GB. A named type is never replaced by the other type.

Each defaulted setting is recorded and printed, so a profile that meant to be
explicit can see that it was not:

```text
boot volume: l_ssd 40 GB (40000000000 bytes) [compatibility default: type, size]
cache volume: none
```

A fully explicit plan prints `[configured]` instead.

### Validation before allocation

Everything below is checked before the create POST, so an impossible
combination costs nothing:

- the instance type's published architecture matches the requested one;
- the instance type offers the requested storage type;
- the image's own root volume can back the requested storage type;
- the requested size covers the image's root volume;
- the requested size sits inside the instance type's range for that storage
  type, and inside Scaleway's Block Storage range for `sbs_volume`;
- the size is a multiple of the provider's 512-byte increment.

A request that names a size and type is never silently adjusted. A refusal
names both routes a user can change, for example:

```text
boot volume of 41000000000 bytes is outside the 20000000000..=40000000000
byte range instance type 'DEV1-S' accepts for l_ssd
```

### What a run reports

After the instance exists and before readiness checks begin, both `mriya run`
and `mriya init` print two separate lines to standard error:

```text
boot volume: vol-root l_ssd 20 GB (20000000000 bytes) [configured]
cache volume: vol-cache
```

The boot line comes from the root volume the provider reported, and the cache
line from the configured `default_volume_id`; neither is derived from the
other. A request with no cache prints `cache volume: none`, and a handle with
no recorded root prints `boot volume: unknown` rather than omitting the line.

Mriya also compares the reported root volume with the resolved plan. A provider
substitution in type or size fails the create, and a post-create failure
destroys the server best-effort before reporting the original reason.

`mriya init` resolves this plan before it creates the cache volume, so an
impossible boot configuration leaves no volume behind.

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
artefacts between runs. When configured, the volume is attached before the
instance starts and mounted at `/mriya`. Place build caches (such as Cargo's
target directory) on this volume to persist compiled dependencies across runs.

To enable the cache volume:

1. Run `mriya init` to create and format a Block Storage volume in the
   configured zone. The command writes the new volume ID into `mriya.toml`. Use
   `mriya init --force` to replace an existing configured volume ID. If any
   step after provisioning fails — including the instance read it performs
   before detaching the volume — the command destroys the formatter instance
   before reporting the error, so a failed run does not leave a VM billing.
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

When the volume is mounted successfully, Mriya:

1. Creates cache subdirectories under the mount point so toolchains can write
   immediately (`cargo`, `rustup`, `target`, `go/pkg/mod`, `go/build-cache`,
   `pip/cache`, `npm/cache`, `yarn/cache`, `pnpm/store`).
2. Exports environment variables in the remote session to route common language
   caches to the volume. These include `CARGO_HOME`, `RUSTUP_HOME`, and
   `CARGO_TARGET_DIR` (Rust/Cargo); `GOMODCACHE` and `GOCACHE` (Go);
   `PIP_CACHE_DIR` (Python/pip); `npm_config_cache`, `YARN_CACHE_FOLDER`, and
   `PNPM_STORE_PATH` (Node tooling).

If mounting fails (for example, when the volume has no filesystem), the run
continues without the cache — this allows graceful degradation for first-time
setups.

`mriya init` uses a temporary formatter instance and runs `mkfs.ext4` against
the new volume. Adjust the default size with `MRIYA_INIT_VOLUME_SIZE_GB` or in
`mriya.toml`:

```toml
[init]
volume_size_gb = 50
```

**Requirements:**

- The volume must exist in the same zone as the instance.
- The volume should be formatted with a filesystem (ext4 recommended) before
  first use. `mriya init` formats the volume automatically.
- Only one instance can attach a given volume at a time.

## What the Scaleway backend does now

- Reads the zone's product catalogue, then resolves the freshest public image
  matching `SCW_DEFAULT_IMAGE` and architecture in the chosen zone.
- Validates the boot-volume plan against that catalogue entry and the image.
- Ensures the requested instance type is available before provisioning.
- Creates an instance with a routed public IPv4 address, an explicit
  `volumes["0"]` boot entry, and tags `mriya` and `ephemeral`.
- Reads the root volume back — the size of a Block Storage root comes from the
  Block Storage API — and compares it with the plan.
- Attaches a Block Storage volume if `SCW_DEFAULT_VOLUME_ID` is configured,
  then mounts it to `/mriya` after the instance boots.
- Polls every five seconds (up to five minutes) until the instance is running
  and reachable via SSH on port 22.
- Destroys the instance and polls until the API no longer lists it, failing if
  any residual resource remains.

Each API request to read or list instances, create a server or volume, power an
instance on, attach or detach a volume, or delete an instance has a 30-second
timeout. That includes the instance read `mriya init` performs before it
detaches the volume, so a hung provider cannot leave the command waiting
indefinitely. If a request times out, Mriya reports a provider error; check
connectivity to the Scaleway API and the instance's actual state before
retrying, as the request may have reached Scaleway even though its response did
not arrive. Image lookup is the exception: it goes through the Scaleway SDK,
which applies no such bound.

## Running the integration check

The behavioural suite provisions a real DEV1-S instance to prove create → wait
→ destroy works end to end and that teardown leaves no residue. Ensure the
`SCW_*` variables are set, then run:

```bash
make scaleway-test
```

The target:

- Sets `MRIYA_RUN_SCALEWAY_TESTS=1` to enable the Scaleway behavioural suite.
- Generates `MRIYA_TEST_RUN_ID` via `uuidgen` and tags created instances.
- Runs `mriya-janitor` before and after tests so leaked instances are cleaned
  up even when the test command fails.

The underlying `cargo test` uses `--test-threads=1` to keep only one instance
alive at a time.

### Opt-in boot-volume live check

`make scaleway-test` proves create → wait → destroy. It does not prove that the
root volume the provider created is the disk the plan asked for, because a
provider default can satisfy a create. The boot-volume check does, and it is
separate because it creates real servers of the shapes you name.

List the cases in `MRIYA_TEST_BOOT_CASES` as
`instance_type:volume_type:size_gb` triples separated by semicolons, then run:

```bash
MRIYA_TEST_BOOT_CASES='DEV1-S:l_ssd:20;DEV1-S:sbs_volume:20' \
  make scaleway-boot-test
```

The target sets `MRIYA_RUN_SCALEWAY_BOOT_TESTS=1`, generates
`MRIYA_TEST_RUN_ID` via `uuidgen`, and runs `mriya-janitor` before and after,
so leaked resources are swept even when the check fails.

When enabled, the check never skips silently. Missing credentials, an
unreadable configuration, a provider permission error, or an empty case list is
a failure. For each case it clears the cache volume, sets the boot type and
size, creates the server, compares the handle's record with the resolved plan,
and independently re-reads the server and — for Block Storage — the Block
Storage volume to compare identifier, type, and size.

Every created server is destroyed even when an assertion fails. The captured
root volume is then checked for survival: if it remains, the check deletes it
by its captured identifier and prints a note pointing at issue #154, which
tracks ownership-aware boot-volume cleanup. Cleanup failures are reported
separately from assertion failures, and the run ends with an executed-case
summary.

```text
boot-volume live check: 2 case(s) executed, 0 assertion failure(s), 0 cleanup error(s)
```

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
