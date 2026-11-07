# Architectural decision record (ADR) 001: Adopt the Rust build standard for development builds

## Status

Accepted on 8 October 2026, amended on 7 November 2026. Development, test, lint
and typecheck builds use the parallel `rustc` frontend (`-Zthreads=8`) and, on
Linux, the `mold` linker, while coverage and release builds stay off the fast
flags.

## Date

2026-10-08.

## Context and problem statement

Edit-compile cycles are dominated by frontend time and by link time. The
estate's build standard (`rust-build-defaults`) addresses both. Cargo applies
exactly one `rustflags` source, and an assigned `RUSTFLAGS` replaces every
configuration source, so a recipe or workflow step that assigns `RUSTFLAGS`
silently loses the flags unless it restates them. Measurements and shipped
artefacts also need a baseline that the fast flags would disturb.

## Decision drivers

- Faster local and CI development builds without changing what the code means.
- Coverage numbers that do not depend on the fast flags.
- A release that ships from the platform linker.
- A flag lost through a recipe or workflow edit must fail a test, not pass
  quietly.

## Options considered

- Configure the flags only in `.cargo/config.toml`. This leaves every recipe
  and CI step that assigns `RUSTFLAGS` without them.
- Restate the flags in each recipe and step, held by a contract test. This is
  the option taken.
- Adopt Cranelift for development builds. This repository does not: on the
  current pin Cranelift cannot unwind, which fails four tests that use
  `rstest_bdd::skip!`. The developers' guide records the scope and the
  reproduction command.

## Decision outcome

`.cargo/config.toml` carries the flags in every `rustflags` source, and adds the
`mold` linker flag to the Linux table only. The Makefile composes the same
flags into each development recipe, keeping the caller's own `RUSTFLAGS`.
Coverage assigns its own flags and ignores the caller's. The release recipe
keeps the caller's flags and names neither fast flag, so it ships from the
platform linker; a direct `cargo build --release` takes the configuration's
flags unless `RUSTFLAGS` is assigned. Cranelift is not adopted, as an exception
that needs maintainer approval; the developers' guide records the measurement,
its scope and the reason.

`tests/build_standard_contract.rs` reads the Cargo configuration sources and
the commands `make -n` prints for each target, and the `setup-rust` steps of
the workflows, so a flag lost through a recipe or workflow edit fails there.

## Consequences

- Linux builds require `mold`. The flag reaches the linker through `cc`, which
  must be GCC 12.1 or newer, or clang.
- A change to a recipe or workflow step that assigns `RUSTFLAGS` must restate
  the flags, and the contract says which clause fails when it does not.
- Revisit Cranelift when the toolchain or the failing tests change. The
  `rstest-bdd` 0.6 migration may change `skip!`, so recheck the exception then;
  the result is a measurement to take, not one to assume.

## Addendum (2026-11-06)

Whitaker is an exception to the flag standard above. It runs on its
installer-managed toolchain, which cannot take the repository's frontend or
linker flag, so its command assigns the warning policy alone, never expands the
caller's `RUSTFLAGS`, and clears `CARGO_ENCODED_RUSTFLAGS`, which Cargo reads
in preference to it. Every other recipe still restates the standard flags. The
development build footprint joins the standard: `[profile.dev]` sets
`incremental = false` and `debug = 0`, and `.cargo/config.toml` sets
`[unstable] embed-metadata = false`. Debugging needs
`CARGO_PROFILE_DEV_DEBUG=true`.

## Addendum (2026-11-07)

The standard gained a preflight and an explicit release exclusion. `build`,
`test`, `lint`, `typecheck` and the Scaleway targets depend on
`check-build-tools`, which stops with an installation hint when `mold`, a C
compiler that accepts `-fuse-ld=mold` (GCC 12.1 or newer, or clang), or the
pinned toolchain is missing, and never falls back to another linker.
`make install-build-tools` installs the checksum-verified `mold` pinned in
`tools/mold` and the pinned toolchain with `rustfmt`, `clippy` and
`rust-analyzer`. Every Linux job that runs the suite, including the coverage
job whose own build never links with `mold`, runs that installer first and
unconditionally. The release workflow passes `rustflags: '-D warnings'` to the
shared release action, so the exclusion no longer rests on the action's
default. Cranelift remains an exception, now measured on the current pin.
