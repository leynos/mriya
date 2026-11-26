# Mriya

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](
https://deepwiki.com/leynos/mriya)

**Teleport your builds to the cloud and back.**

Mriya is a command-line tool that offloads heavyweight builds and test suites
to short-lived cloud machines. It takes your working directory, syncs it to a
fresh remote VM, runs your command, streams the output back, and tears the
machine down—all without changing your local workflow.

The name is an homage to the Antonov An-225 *Mriya*, the world's largest cargo
aircraft. Just as that magnificent plane carried massive payloads across
continents, this tool carries your entire workspace to remote infrastructure
and brings the results home.

## Quick start

```bash
mriya run -- cargo test --workspace --all-features
```

That's it. Mriya provisions a VM, syncs your project (respecting `.gitignore`),
runs the command over SSH, and then attempts to tear the VM down and report any
failure to do so. When teardown succeeds, the exit code from your remote
command is preserved, so CI integration works out of the box. A teardown
failure makes the run fail and reports the teardown error instead of that exit
code.

## What it does

- **Git-aware sync** – Uses rsync with `.gitignore` filters, so ignored paths
  are excluded from the transfer and are not deleted on the remote. The filter
  reads `.gitignore` patterns rather than Git's index, so an untracked but
  unignored file still travels. Persistence *across* runs needs a cache volume:
  a later run never reuses the previous VM's own disk. The root volume itself
  can outlive the server and stay billable; see the
  [cleanup notes](docs/users-guide.md#cleanup-what-is-and-is-not-verified).
- **Live output streaming** – stdout and stderr stream back to your terminal in
  real time.
- **Exit code preservation** – If `cargo test` fails remotely with status 101,
  you get 101 locally, provided teardown succeeds; a teardown failure reports
  its own error instead.
- **Configurable instances and images** – Set a default instance type and image
  in configuration, or override either for one run with `--instance-type` and
  `--image`.
- **Cloud-init provisioning** – Pass cloud-config YAML or a first-boot script
  inline or from a file, and Mriya waits for it to finish before running your
  command.
- **Persistent cache volume** – Attach a Block Storage volume to each ephemeral
  VM and route Cargo, Go, Python, and Node caches onto it so builds stay
  incremental between runs. Volume creation through `mriya init` currently
  fails against Scaleway; supply an existing volume ID instead. See
  [Use an existing volume](docs/users-guide.md#use-an-existing-volume).
- **Attempted teardown with reported failures** – Whenever provisioning has
  returned a server handle, Mriya asks the provider to delete that server after
  the run, successful or not, and surfaces a teardown failure instead of
  discarding it. A provisioning failure that never yields a handle is the
  exception, and can leave a stopped server. What this establishes is narrower
  than "guaranteed cleanup"; see
  [Cleanup: what is and is not verified](docs/users-guide.md#cleanup-what-is-and-is-not-verified)
  and the
  [cost and resource lifetime](docs/users-guide.md#cost-and-resource-lifetime)
  guidance.

## Current status

Mriya is an **MVP with a single backend: Scaleway**.

Implemented in code:

- Provisioning a Scaleway instance with a configurable type and image, from
  configuration defaults or per-run `--instance-type` / `--image` overrides.
- Inline and file-based cloud-init user-data, with Mriya waiting for cloud-init
  to finish before the remote command runs.
- Attaching, mounting, and routing language caches onto a configured Block
  Storage volume, with scripted coverage only.
- Git-aware rsync sync, streamed output, exit-code preservation, and attempted
  teardown.

Implemented but **not currently functional against Scaleway**:

- `mriya init`, which formats a new cache volume and records its ID. Volume
  creation still posts to the Instances API volume endpoint with the `b_ssd`
  type. Scaleway's [changelog](https://www.scaleway.com/en/docs/changelog/)
  states that creating new block volumes through the Instances API has been
  impossible since 30 April 2025, so the creation step should be expected to
  fail until the migration in
  [issue #153](https://github.com/leynos/mriya/issues/153) lands. Configuring
  an already-created volume skips that step, though live attachment of an
  `sbs_volume` through the Instances API is also unverified. The user guide
  carries the same limitation.

Planned, not implemented:

- `mriya bake-image` for pre-baked images.
- Backends other than Scaleway — Hetzner Cloud, DigitalOcean, and AWS EC2.
- A native SSH client in place of the system `ssh` binary.
- Configurable lifecycle timeouts and signal handling.
- Structured progress reporting.

See the [roadmap](docs/roadmap.md) for the phased plan behind each item, and the
[user guide](docs/users-guide.md) for configuration and cost guidance.

### Cleanup: what is and is not verified

Mriya deletes the server through the provider's delete endpoint and polls until
the provider stops listing it; the integration test asserts exactly that. It
does not inventory every resource a run can create, and it cannot destroy a
server when provisioning fails after the create request succeeded but before a
handle was returned — for example, while attaching a volume or powering the
instance on. Root storage is chosen by the provider rather than by Mriya, so
its type and size are implicit.

Three open issues track the gap:

- [#153](https://github.com/leynos/mriya/issues/153) — migrate cache volumes to
  the supported Block Storage API.
- [#154](https://github.com/leynos/mriya/issues/154) — verify complete teardown
  and recover partial provisioning.
- [#155](https://github.com/leynos/mriya/issues/155) — make boot-volume type and
  size explicit.

Read
[Cost and resource lifetime](docs/users-guide.md#cost-and-resource-lifetime)
before relying on a run estimate.

## Documentation

For configuration, credentials, and detailed usage:

- [User guide](docs/users-guide.md) – Credentials, sync behaviour, cache
  volume setup, cost and resource lifetime, and the backend's verified
  behaviour.
- [Migrating to 0.2.0](docs/migration-0.2.0.md) – Install the pinned build
  tools and put the prefix on `PATH`.
- [Roadmap](docs/roadmap.md) – Phased development plan.

## Requirements

- Rust 2024 edition (for building from source)
- A Scaleway account with API credentials
- `rsync` and `ssh` available on your PATH

## Licence

See [LICENSE](LICENSE) for details.
