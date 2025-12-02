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

For configuration files, place `mriya.toml` under the usual XDG (X Desktop
Group) config locations. Values are merged with the same precedence; CLI flags
override everything.

## What the Scaleway backend does now

- Resolves the freshest public image matching `SCW_DEFAULT_IMAGE` and
  architecture in the chosen zone.
- Ensures the requested instance type is available before provisioning.
- Creates an instance with a routed public IPv4 address and tags `mriya` and
  `ephemeral`.
- Polls every five seconds (up to five minutes) until the instance is running
  and reachable via SSH on port 22.
- Destroys the instance and polls until the API no longer lists it, failing if
  any residual resource remains.

## Running the integration check

The behavioural suite provisions a real DEV1-S instance to prove create → wait
→ destroy works end to end and that teardown leaves no residue. Ensure the
`SCW_*` variables are set, then run:

```bash
make test -- scaleway_backend -- --test-threads=1
```

The extra `--test-threads=1` keeps only one instance alive at a time. Cleanup
is built into the backend, but cancelling the run may still leave resources;
rerun `make test` to let the teardown step finish.
