# Developers' guide

This guide records development practices specific to maintaining mriya. Follow
the project-wide guidance in `AGENTS.md` first.

## Spelling policy

Run `make spelling` to enforce en-GB-oxendict spelling across every tracked
file. The gate regenerates `typos.toml` from the live shared dictionary,
refreshing its untracked local cache only when the authority is newer, applies
the narrow repository policy in `typos.local.toml`, runs the pinned `typos`
binary, and enforces the shared phrase corrections `typos` cannot express.

Because `typos.toml` is rewritten on every run, never edit its entries by hand
and never check it for drift in continuous integration. Add narrow
repository-specific terminology, identifiers, and deliberate fixtures to
`typos.local.toml`; the shared dictionary carries everything else.

The gate ships as the `typos-config-builder` command, pinned by the Makefile's
`TYPOS_CONFIG_BUILDER_VERSION` variable and run in an isolated Python 3.14
environment. Bump that variable to adopt a newer release.

## Coverage ownership

The trunk owns both persistent coverage outputs. On a push to `main`,
`.github/workflows/coverage-main.yml` measures coverage, writes the ratchet
baseline, and uploads the report to CodeScene. Pull-request CI measures the
same selection only to compare it with that baseline: it archives no report,
never calls CodeScene, and never receives `CS_ACCESS_TOKEN`. The call is what
moves to the trunk, not the archive: the uploader pins the `cs-coverage`
archive by digest, but the client refuses to run whenever CodeScene's API
changes shape, and on the trunk such a change no longer fails every pull
request.

The publisher never binds the token in an `env` block, because the uploader is
a composite action that would pass a step's environment on to its nested steps.
A check step writes whether the secret is set, the upload runs only when it is
and only for `refs/heads/main`, and the token reaches the uploader solely as its
`access-token` input. Runs share one concurrency group per ref and are never
cancelled, so they never overlap, and a newer trigger replaces any run still
pending. GitHub does not promise to start runs in trigger order, so no commit
order is promised. A manual re-run keeps its `run_id`, so it republishes that
commit's coverage but replaces no baseline unless the original run saved none.

Two gaps are known and accepted. Merges made by the Dependabot automerge
workflow with `GITHUB_TOKEN` fire no push, so they reach the publisher only
through a dispatch or the next ordinary push. A dispatch that replaces a
pending push uploads the same or a newer commit, but the shared action writes
the baseline only on a push, so the baseline stays one push behind until the
next one.

`make test-workflow-contracts`, which pull-request CI runs, holds this shape by
running `cv005-contracts check`, the shared contract library in
`leynos/shared-actions` (`packages/cv005-contracts`), from a full commit named
by `CV005_CONTRACTS_REF` in the Makefile, and then the mutation-testing pytest
contract in `tests/workflow_contracts/`. A fix to the rules is therefore a pin
bump. The target needs `uv`, which fetches the Python 3.13 the library runs
under. The repository's parameters are in `.github/cv005.toml`: its
`repository` name and the `[selection]` inputs the baseline measures, which the
publisher's generator must carry and every pull-request lane must match. The
library's own suite proves each rule refuses the shape it exists to refuse, so
this repository keeps no copy of the readers or the refusal cases. It reads
every workflow strictly (a repeated key is an error) and follows local
reusable-workflow calls transitively, so a called workflow cannot reach
CodeScene on a pull request's behalf. For the same reason the release dry run,
which runs for every pull request, calls the release workflow without
`secrets: inherit`: the release workflow reads only `GITHUB_TOKEN`, which a
called workflow receives anyway.

## The build standard

Development, test, lint, and typecheck builds use the parallel `rustc` frontend
(`-Zthreads=8`) and, on Linux, the `mold` linker (`-Clink-arg=-fuse-ld=mold`).
These are defaults in `.cargo/config.toml`, which Cargo discovers on its own,
so a bare `cargo build` gets them. `mold` ships for Linux only, so the linker
flag lives in a Linux-only table and macOS and Windows keep their platform
linker. Cargo selects one `rustflags` source rather than merging them, so every
source repeats the same flags apart from the linker.

An assigned `RUSTFLAGS` replaces the configuration's flags, so the Makefile
recipes that set it compose the standard's flags onto any inherited value (CI's
`setup-rust` exports one). Two builds are deliberately excluded: coverage
assigns `RUSTFLAGS` without the fast flags, because a measurement should not
depend on them, and the release recipe and workflow keep the platform linker,
because they assign `RUSTFLAGS` (even an empty value displaces the
configuration). Cargo has no per-profile `rustflags`, so a direct
`cargo build --release` takes the configuration's flags unless `RUSTFLAGS` is
assigned too.

On Linux, install `mold` before building: the configuration names it, so a
build without it fails at link time. CI installs it through `setup-rust`'s
`install-mold` input. `tests/build_standard_contract.rs` holds the standard. It
reads the configuration sources, the commands `make -n` prints for each
development target on a Linux host and a macOS host (each keeping the caller's
own `RUSTFLAGS`) and for each coverage and release target on a Linux host, and
the `setup-rust` steps of the CI workflows (each must pass `install-mold`), so
a flag lost through a recipe or workflow edit fails there.

### Cranelift

Exception: Cranelift is not the development-profile backend. The full suite was
measured under it on the pinned `nightly-2025-11-26` on 2026-10-02, and these
tests fail there while passing under LLVM: `scenario_provision_and_destroy`,
`scenario_reject_unknown_image`, `scenario_reject_unknown_type` and
`scenario_cloud_init_install_jq`. Re-measure the whole suite on the next
toolchain bump, and adopt the backend when it passes.
