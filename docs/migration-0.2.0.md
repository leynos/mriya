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
appears earlier in your shell's `PATH`. A bare `cargo build` outside Make does
not: add the same directory to your shell profile.

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## What did not change

- macOS and Windows keep their platform linker; the preflight skips the linker
  checks there and says so.
- `make release` and the release workflow still need none of these tools. They
  assign their own `RUSTFLAGS`, so a shipped artefact keeps the platform linker.
- Coverage builds still run without the fast flags.
