# Migrating to 0.2.0

0.2.0 makes the development build standard a checked precondition instead of a
link-time surprise. This note lists what an existing checkout must change; a
fresh clone needs only the install step.

## Install the pinned tools

`make build`, `make test`, `make lint`, `make typecheck`, `make scaleway-test`,
and `make scaleway-janitor` now run `scripts/check-build-tools.sh` before any
compilation. The preflight requires:

- `mold` at exactly the version in `tools/mold/VERSION` (currently `2.41.0`);
- a C compiler that links with `-fuse-ld=mold` (GCC 12.1 or newer, or clang);
- the toolchain that `rust-toolchain.toml` pins, with its listed components.

Run `make install-build-tools` once. It downloads the pinned `mold` release,
verifies it against `tools/mold/SHA256SUMS`, unpacks it under
`$(BUILD_TOOLS_PREFIX)` (default `~/.local`), and installs the pinned toolchain.
`make check-build-tools` reports every missing prerequisite in one pass.

## Put the prefix on `PATH` for bare Cargo

The Makefile puts `$(BUILD_TOOLS_PREFIX)/bin` first on `PATH` for every recipe,
so `make` targets find the pinned release even when a distribution `mold`
appears earlier in the shell's `PATH`. A bare `cargo build` outside Make does
not: add that same directory to the shell profile. `BUILD_TOOLS_PREFIX`
defaults to `~/.local`, and the Makefile permits overriding it, so the example
names the variable rather than assuming the default:

```bash
export PATH="${BUILD_TOOLS_PREFIX:-$HOME/.local}/bin:$PATH"
```

## What did not change

- macOS and Windows keep their platform linker; the preflight skips the linker
  checks there and says so.
- `make release` and the release workflow still need none of these tools. They
  assign their own `RUSTFLAGS`, so a shipped artefact keeps the platform linker.
- Coverage builds still run without the fast flags.

## Environment access goes through `mockable`

Production code no longer reads environment variables through `std::env`: it
reads them through `mockable::Env`, and `DefaultEnv` is injected at each entry
point. Reads that are not environment variables are unaffected; `main.rs` still
calls `std::env::current_dir`, which has no `Env` equivalent. Library users
keep every existing entry point and gain injectable ones beside them:

- `mriya::sync::expand_tilde(path)` is unchanged in name, signature and
  arity, and still returns the path unchanged when `HOME` is absent. Its one
  behaviour change is the undecodable case described below;
- `mriya::sync::expand_tilde_with_home(path, home)` — the pure expansion, with
  `home_from_env` supplying the value and reporting why it could not;
- `mriya::cloud_init::resolve_cloud_init_user_data_with_env`;
- `ScalewayBackend::new_with_env`.

`SyncError` and `CloudInitError` each gain a `HomeUnavailable` variant, because
the environment can no longer be read silently at the point of expansion:

```rust
SyncError::HomeUnavailable { path: String, source: sync::HomeUnavailable }
CloudInitError::HomeUnavailable { path: String }
```

Neither variant existed in 0.1.0, so no released variant changes shape. A
downstream exhaustive `match` over either enum needs a new arm; code that only
formats the error is unaffected, and the message names the file.

`mockable` 3.0.0 with default features off is now a regular and build
dependency, and `temp-env` 0.3.6 is a dev-dependency. The decision and its
rationale are recorded in [ADR 002](adrs/adr-0002-environment-access.md).

### Behaviour changes

- A `~/` path with a `HOME` that is not valid UTF-8 is now an error rather than
  a lossily converted path. The message names the file and suggests an absolute
  path: an absolute `--cloud-init-file`, or an absolute `ssh_identity_file`.
  `mriya init` reports the same cause when its format command is reached,
  rather than a bare volume-format failure.
- `expand_tilde`, the compatibility entry point, reports no error by design,
  so for it the undecodable case returns the path unchanged, exactly as an
  absent `HOME` does.
- The test-support `EnvGuard` and `ENV_LOCK` are gone from the public
  `test_support` module. A test that needs the live environment should use
  `temp_env::with_var` for the process, or `Command::env` for a spawned child;
  a test of production logic should inject a `mockable::MockEnv`.
