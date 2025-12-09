# Mriya

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
runs the command over SSH, and cleans up when finished. The exit code from your
remote command is preserved, so CI integration works out of the box.

## What it does

- **Git-aware sync** – Uses rsync with `.gitignore` filters, so only tracked
  files travel. Build caches like `target/` stay on the remote for incremental
  runs.
- **Live output streaming** – stdout and stderr stream back to your terminal in
  real time.
- **Exit code preservation** – If `cargo test` fails remotely with status 101,
  you get 101 locally.
- **Guaranteed cleanup** – The VM is destroyed after every run, successful or
  not.

## Current status

Mriya is in its **MVP phase** with a working Scaleway backend. You can
provision DEV1-S instances, run commands, and tear them down reliably. We're
actively working on persistent cache volumes, cloud-init provisioning, and
multi-cloud support.

See the [roadmap](docs/roadmap.md) for what's coming next.

## Documentation

For configuration, credentials, and detailed usage:

- [User guide](docs/users-guide.md) – Credentials, sync behaviour, and backend
  guarantees.
- [Roadmap](docs/roadmap.md) – Phased development plan.

## Requirements

- Rust 2024 edition (for building from source)
- A Scaleway account with API credentials
- `rsync` and `ssh` available on your PATH

## Licence

See [LICENSE](LICENSE) for details.
