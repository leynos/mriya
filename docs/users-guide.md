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
override everything.

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

## Cloud-init provisioning

Mriya can pass a cloud-init *user-data* payload through to the provider when
creating the VM. This allows you to install system packages or perform other
first-boot provisioning before the remote command starts.

Provide user-data either inline or via a local file:

```bash
mriya run --cloud-init-file ./cloud-init.yml -- jq --version
```

Or inline (handy for short scripts):

```bash
mriya run --cloud-init "#!/bin/sh\necho ready" -- sh -lc 'echo ok'
```

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

## Persistent cache volume

Mriya can attach a Block Storage volume to the ephemeral VM for caching build
artefacts between runs. When configured, the volume is attached before the
instance starts and mounted at `/mriya`. Place build caches (such as Cargo's
target directory) on this volume to persist compiled dependencies across runs.

To enable the cache volume:

1. Create a Block Storage volume in the same zone as the instances via the
   Scaleway console or CLI.
2. Set the volume ID in configuration:

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

When the volume is mounted successfully, Mriya automatically routes common
language caches to it by exporting environment variables in the remote session,
including:

- `CARGO_HOME`, `RUSTUP_HOME`, and `CARGO_TARGET_DIR` (Rust/Cargo)
- `GOMODCACHE` and `GOCACHE` (Go)
- `PIP_CACHE_DIR` (Python/pip)
- `npm_config_cache`, `YARN_CACHE_FOLDER`, and `PNPM_STORE_PATH` (Node tooling)

If mounting fails (for example, when the volume has no filesystem), the run
continues without the cache — this allows graceful degradation for first-time
setups.

**Requirements:**

- The volume must exist in the same zone as the instance.
- The volume should be formatted with a filesystem (ext4 recommended) before
  first use.
- Only one instance can attach a given volume at a time.

## What the Scaleway backend does now

- Resolves the freshest public image matching `SCW_DEFAULT_IMAGE` and
  architecture in the chosen zone.
- Ensures the requested instance type is available before provisioning.
- Creates an instance with a routed public IPv4 address and tags `mriya` and
  `ephemeral`.
- Attaches a Block Storage volume if `SCW_DEFAULT_VOLUME_ID` is configured,
  then mounts it to `/mriya` after the instance boots.
- Polls every five seconds (up to five minutes) until the instance is running
  and reachable via SSH on port 22.
- Destroys the instance and polls until the API no longer lists it, failing if
  any residual resource remains.

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
