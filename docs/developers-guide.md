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
`typos.local.toml`; the shared dictionary carries everything else. The shared
base masks fenced code blocks, but not inline code: backticks do not exempt a
word, so spell identifiers in en-GB-oxendict unless an external system fixes
the spelling. Where one does, add an exact pattern to `typos.local.toml` for
that name, never a blanket word or a mask over all inline code. The current
exemptions are the Cargo JSON message kind `compiler-artifact` and the `mold`
linker, matched as `` `mold` ``, "mold linker", the `install-mold` input of
`setup-rust`, and the `-fuse-ld=mold` argument. The build-tools scripts and
contracts add its command line and output (`command -v mold`, `mold --version`,
its version banner), release archive names (`mold-2.41.0-x86_64-linux.tar.gz`),
the `bin/mold` and `ld.mold` paths, and the one declaration per language of the
quoted tool name the contract tests pass as an argument. Test each new
exemption in both directions: the exact name passes and the same misspelling in
ordinary prose fails.

`tests/spelling_contract.rs` runs that rule as an ordinary test, in two halves
that answer different questions. The first half reads the generated
`typos.toml`, compiles every `extend-ignore-re` entry with the `regex` crate
and removes the matches before looking for the spelling, which names a lost or
widened exemption directly. That is a model of the gate rather than the gate:
Typos masks by span and drops a finding whose span starts or ends inside a
match, so the two rules can differ for a spelling that only partly overlaps a
mask. The second half closes that gap. It loads the same generated
configuration into `typos-cli` — the crate the gate's pinned console script is
built from, pinned to the version `typos-config-builder` names — and runs the
same `FileChecker` the binary runs over isolated fixture files. It covers the
exempt forms a fixture exercises: the backticked name, the `mold linker`
phrase, the `install-mold` input, the `-fuse-ld=mold` argument and the
`.contains("mold")` probe. It does not cover every entry in `typos.local.toml`:
`CARGO_TERM_COLOR`, `organisation_id` and the workflow lines have no probe in
either half, so nothing here judges their behaviour, and the configuration test
only confirms each pattern reaches the generated file. It also covers the
negative direction — the bare name, and the same spelling in prose or in a code
span — together with the anchored declarations in their LF, CRLF and indented
forms and the Tokio attribute fixtures. When adding an exemption, add it to
both halves: a form with no probe is covered by neither. Neither half needs the
network, a fixed toolchain, or the `typos` binary.

A property test covers the shared base's Tokio grammar beyond the arrangements
the probes pin: a spacing run of up to four characters drawn from spaces, tabs
and newlines, any argument order, either quote form, a `main` runtime with a
worker pool, and a malformed attribute as the opposite edge. It asserts two
things about the text it generates — the runtime key survives the masks exactly
when the attribute is malformed, and a spelling placed beside the attribute
survives either way. That is a check on the key and on the surroundings, not on
every character of the attribute: an arrangement that masked the key while
leaving another part of the attribute visible would pass it. The probes and the
real-engine half cover the rest, keeping `local` on `main` and rejecting
runtime kinds the base does not name. The contract checks the reach of the
exemptions, not the shared dictionary, so it does not prove that a spelling the
dictionary accepts today is still accepted tomorrow.

The test files must carry the misspellings they judge, so each reported
spelling is written once, on a `const` declaration line, and every probe and
fixture body interpolates that constant; `typos.local.toml` exempts exactly
those three declaration lines, anchored with `(?m)^…$` so a differently named
or indented declaration is still reported. Keep that shape when adding a probe:
put the spelling in a constant and let the pattern anchor its declaration,
rather than loosening the mask or excluding the file.

The gate ships as the `typos-config-builder` command, pinned by the Makefile's
`TYPOS_CONFIG_BUILDER_VERSION` variable and run in an isolated Python 3.14
environment. Bump that variable to adopt a newer release.

The dev-dependency `typos-cli` carries an exact `=` requirement, which is a
recorded exception to this workspace's caret-requirement rule. The exception is
narrow and deliberate: `typos-config-builder` pins its own `typos` engine
version (currently `typos==1.50.2`), and the contract test must compile the
same engine the gate runs. The pin keeps the two engines aligned without an
inference: an ordinary `cargo update` leaves `typos-cli` at `1.50.2` rather
than drifting it, and so cannot fail the build. Nothing ties the two versions
together, though, and the builder's pin can move on its own, so check them
against each other when you bump either. Move the pin in `Cargo.toml` and this
paragraph together, and only when the builder's pin moves.

## Running Python tools with uv

Every uv route in the Makefile except `spelling` goes through `$(UV_GATE)`,
which is `python3 scripts/uv_gate.py`. The helper is vendored byte-for-byte from
`uv_gate/uv_gate.py` in `leynos/shared-actions` (version 0.2.1, commit
`2ddf19b3fd52c0f1089db0d57b015f859b80ab67`, SHA-256
`fe7671d8663ec89ce1076ebb857847802e46c8d6607ab1e012daa443e335e66e`). Do not
edit it: the concordat rule `uv-gate-baseline` compares its digest with the
canonical ones. To adopt a newer helper, replace the file with the published
copy once its digest is listed in the rule.

The helper runs uv from a cleaned environment and the global uv cache, offline
first, with at most one online step when uv proves a cache miss. Consequently
the Makefile must not set `UV_CACHE_DIR` or `UV_TOOL_DIR`, call `uv run`,
`uv sync` or `uvx` directly, or pass `--refresh` or `--upgrade`. This
repository has no `pyproject.toml`, so the helper's `tool` subcommand runs each
tool at an exact pin: `interrogate==$(INTERROGATE_VERSION)`, pytest and PyYAML
for the workflow contract tests, and `cv005-contracts` at a full commit. The
`prepare` and `run` subcommands are unused until the repository gains a
`pyproject.toml` and `uv.lock`.

`spelling` is the exception. The `spelling-config-baseline` rule requires
`typos-config-builder` at a release tag, and the helper refuses any Git spec
that is not a full commit, so that recipe runs `uv tool run` directly. The
`uv-gate-baseline` rule accepts a release tag for that one tool and no other.

## Fetching dependencies before tests

The CLI tests build the `mriya` binary with their own `cargo`. Cargo resolves
dependencies while it holds the package-cache lock in `$CARGO_HOME`, so a nested
`cargo` that resolves cold can wait on another repository's build for many
minutes. The fix is to fetch once, before any test starts, and to run the
nested build offline:

- `make test` runs `cargo fetch --locked` first. A failed fetch stops the run.
- `ci.yml` and `coverage-main.yml` run `cargo fetch --locked` as an
  unconditional step before the coverage step, with no `|| true`.
- The test binary build sets `CARGO_NET_OFFLINE=true`. Prepare a direct or
  offline run with `cargo fetch --locked` first; a crate the fetch missed then
  fails at once instead of waiting.

The estate lint rule `nested-cargo-offline-tests` checks all three.

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
bump. The target needs `uv`, which it runs through `$(UV_GATE)`, and which
fetches the Python 3.13 the library runs under. The repository's parameters are
in `.github/cv005.toml`: its `repository` name and the `[selection]` inputs the
baseline measures, which the publisher's generator must carry and every
pull-request lane must match. The library's own suite proves each rule refuses
the shape it exists to refuse, so this repository keeps no copy of the readers
or the refusal cases. It reads every workflow strictly (a repeated key is an
error) and follows local reusable-workflow calls transitively, so a called
workflow cannot reach CodeScene on a pull request's behalf. For the same reason
the release dry run, which runs for every pull request, calls the release
workflow without `secrets: inherit`: the release workflow reads only
`GITHUB_TOKEN`, which a called workflow receives anyway.

## Scaleway lifecycle tests

The production `poll_for_public_ip` and `poll_until_gone` loops use
`tokio::time::Instant` with Tokio sleeps. Their `wait_for_public_ip` and
`wait_until_gone` wrappers provide settings and an instance-fetch closure; the
readiness wrapper groups its SSH port, poll interval, and timeout in
`PollSettings`. Unit and property tests pause runtime time using Tokio's
`test-util` feature, enabled on the Tokio dev-dependency, so timeout and
polling behaviour can be exercised without wall-clock delays. Property tests
generate histories of up to 16 observations and compare the loop's first
terminal outcome with a small model. Histories cover instance states, both IPv4
and IPv6 addresses, invalid or missing addresses, absence, provider errors, and
different observation orderings. This is sampled coverage, not a formal proof.

The private `instance_api_base` field defaults to Scaleway's official Instance
API. Tests replace it with a loopback HTTP script; every Instance API path the
backend formats itself uses it: the single-instance `GET`, the paginated list
and snapshot `GET`, instance `DELETE`, power-on `POST`, server `POST` during
creation, the volume `POST`, and the attachment `PATCH` that both attach and
detach send. Those tests exercise the backend's `create`, `wait_for_ready`,
`destroy`, and volume paths, including the SDK's full server response type,
authentication header, query parameters, pagination, and provider error
handling. A loopback SSH listener covers readiness through the SSH check. The
shared HTTP client applies a 30-second per-request timeout to these calls; the
SDK's own helpers apply none, so a detachment read that stays on the SDK could
wait indefinitely. The base URL is an internal test seam, not a user
configuration option.

Image lookup is the one remaining SDK helper that does not honour
`instance_api_base`: `resolve_image_id` calls
`ScalewayListInstanceImagesBuilder`, which hard-codes the production host. That
path is covered by the pure selection and filtering cases rather than by a
loopback script.

### Behavioural test support

The behavioural suites under `tests/` share two support modules rather than
each keeping a private copy:

- `tests/common/cli_support.rs` supplies the binary-driving helpers the
  `cloud_init` and `instance_overrides` suites both need, including
  `DEFAULT_IMAGE_LABEL` — the label the CLI resolves when a scenario sets none,
  which must track `ScalewayConfig::default_image` — and
  `check_run_failed_with`, whose failure message reports both streams so a run
  that fails before writing to stderr still explains itself.
- `tests/sync/rsync_simulator.rs` derives `RsyncFlags` from the real argument
  vector the sync code built. `--delete` and `--filter=:- .gitignore` are
  matched exactly, so a scenario fails when production stops passing either
  rule rather than when the simulator's copy logic changes. `simulate_rsync`
  then copies under whatever behaviours those flags name.

The run suite classifies each recorded invocation against the command the
scenario requested, carried on `RunContext::requested_command`, so its ordering
assertions fail when the remote command is skipped instead of passing on any
trailing SSH call.

Mutation exclusions in `.cargo/mutants.toml` remain limited to test-backdoor
scaffolding and the equivalent instance-request builder mutant. Scoped mutation
counts recorded before these lifecycle tests are historical results; rerun
mutation testing against the current candidate before using counts as evidence.
The loopback tests exercise the network-boundary survivors reported by the
earlier run.

### The environment boundary

Production code reads environment *variables* through `mockable::Env`, never
through `std::env`: `DefaultEnv` is the real process environment, and `MockEnv`
is what the tests use, available only under the dev-dependency's `mock`
feature. The one exception is not a variable: `std::env::current_dir` has no
`Env` equivalent and `main.rs` still calls it. Two accessors matter:

- `string` reports a variable as `None` when it is absent or not valid
  Unicode. Code that only wants a value when one is usable, such as the
  `MRIYA_FAKE_*` gates, uses it.
- `raw` returns `VarError`, so it can tell an absent variable from one that
  is present but undecodable. Code that must not silently fall back, such as
  tilde expansion, uses it.

`src/sync/util/expand.rs` is where the distinction is spent. `home_from_env`
reads `HOME` only when the path it is given starts with `~/`, returns
`Ok(None)` for an absent `HOME` (leaving the caller its own fallback), and
reports `HomeUnavailable` for an undecodable one. `expand_tilde_with_home` then
does the pure string work with no environment access at all. Each caller names
the path in its own error: `Syncer::common_ssh_options` takes the `Env` it is
handed, and `config.rs` and `main.rs` turn the cloud-init `HomeUnavailable`
into a message naming the file the user asked for.

Each boundary therefore has a default and an injected entry point. The default
one supplies `DefaultEnv` and keeps the arity a caller already depends on; the
injected one takes `&impl Env` and is what a test targets:

| Default entry point                                                        | Injected counterpart                                                   | Reads                           |
| -------------------------------------------------------------------------- | ---------------------------------------------------------------------- | ------------------------------- |
| `ScalewayBackend::new(config)`                                             | `ScalewayBackend::new_with_env(config, env)`                           | the test-run identifier         |
| `cloud_init::resolve_cloud_init_user_data(inline, file)`                   | `cloud_init::resolve_cloud_init_user_data_with_env(inline, file, env)` | `HOME` for a `~/` file path     |
| `Syncer::build_ssh_args(..)` / `common_ssh_options` / `build_remote_shell` | the same methods with a trailing `&impl Env`                           | `HOME` for a `~/` identity path |
| `expand_tilde(path)`                                                       | `home_from_env(env, path)` plus `expand_tilde_with_home`               | `HOME`                          |

The public `expand_tilde(path)` entry point is kept for compatibility, so an
existing library caller needs no change. It reads the live process environment
through `DefaultEnv`; its undecodable case falls back to the path unchanged
rather than reporting the cause, which is why production code takes
`home_from_env` instead. The build-standard sandbox's non-UTF-8 `PATH` case
cannot inject either: the injected boundary answers with `String`, so that case
re-executes the test binary with the value in its real environment, and the
child asserts the report rather than the parent parsing prose.

`src/test_helpers.rs` builds the `MockEnv` cases from a variable table, so a
test never mutates the process environment. `mock_env` answers both accessors
from one decodable table; `mock_env_with_raw` takes a closure, which is the
only way to express `NotUnicode` and therefore the way the undecodable-`HOME`
cases are written. A test that genuinely needs the live process environment —
`expand_tilde`'s own process-level cases, `config_tests`' `as_request` case, or
a CLI behaviour scenario that sets `HOME` for the child — uses
`temp_env::with_var` or `Command::env`, never `set_var`.

The CLI behaviour suite sets `HOME` on the spawned binary rather than on the
test process, which is why the tilde cloud-init scenario can assert an expanded
path without a guard or a lock. The undecodable-`HOME` scenario goes the same
way, handing the child an `OsString` built from non-UTF-8 bytes; the child
reports the failure, so no test process is ever left with an unusable `HOME`.
That step assembles the value from bytes, which this workspace does only on
Unix, so the scenario carries `@unix` and is filtered out at registration on
other platforms.

The build-standard contract's sandbox reads `PATH` through the same boundary
and with the same rule: `find_on_path` reports an unset `PATH` and an
undecodable one as different failures, because "not set" would send a reader
looking for a variable that is in fact present. `find_on_path_with` takes the
`Env` so `sandbox_cases.rs` can prove both reports, and the partial-`PATH`
case, without touching the process environment.

### Internal module boundaries

Two sibling modules exist to keep their parents inside the 400-line budget
without changing behaviour, and each publishes only what its parent needs:

- `src/janitor/scw.rs` holds the `scw` command runner, JSON list parsing, and
  the four resource operations `list_servers`, `delete_server`, `list_volumes`,
  and `delete_volume`, all `pub(super)`. `janitor/mod.rs` keeps the sweep
  policy and re-exports nothing, so a caller still reaches a sweep only through
  `Janitor`. The delete operations take their zone from the listed resource
  rather than from configuration, which is what lets one sweep remove a server
  and a volume that report different zones.
- `src/test_backdoors.rs` separates deciding from doing. `select_fake_outcome`
  reads the injected environment and returns a `FakeOutcome` value; it has no
  side effects, so the precedence rules and the `MRIYA_FAKE_RUN_ENABLE` gate
  are testable on their own. `run_fake_outcome` is the command that performs
  the selection, and it takes its stdout and stderr as arguments rather than
  reaching for the process streams, so a unit test asserts exactly what a fake
  run emits. `run_command` is the only caller that supplies the real streams.
  The module is compiled for unit tests and for the `test-backdoors` feature
  the CLI integration tests enable. `select_fake_outcome` takes `&impl Env`
  rather than reading `std::env` directly; `run_fake_outcome` takes the
  selected outcome and the two streams it writes to, and reads no environment
  at all. The `MRIYA_FAKE_*` variables are read only after `enable_fake_modes`
  gates them, so an ordinary run cannot be diverted by a stray variable.
  `.cargo/mutants.toml` excludes this scaffolding from mutation testing by
  function name, not by module: `exclude_re` lists each function whose mutants
  are noise because no production path reaches it. Adding or renaming one of
  these functions therefore means editing that list too, or its mutants start
  surviving as unrecorded gaps rather than recorded exceptions.

## The build standard

Development, test, lint, and typecheck builds use the parallel `rustc` frontend
(`-Zthreads=8`) and, on Linux, the `mold` linker (`-Clink-arg=-fuse-ld=mold`).
Four mechanisms deliver and protect that standard. They are separate, and a
change to one does not stand in for the others.

### Bare Cargo

`.cargo/config.toml` carries the flags, and Cargo discovers it on its own, so a
bare `cargo build` gets them with no Make involved. `mold` ships for Linux
only, so the linker flag lives in a Linux-only table and macOS and Windows keep
their platform linker. Cargo selects one `rustflags` source rather than merging
them, so every source repeats the same flags apart from the linker. A direct
`cargo build --release` takes the configuration's flags too, because Cargo has
no per-profile `rustflags`; assign `RUSTFLAGS` to leave them (even an empty
value displaces the configuration).

### Make's restatement

An assigned `RUSTFLAGS` replaces the configuration's flags, so each Makefile
recipe that compiles composes the standard's flags (`STANDARD_RUSTFLAGS`) and
the warning policy onto any inherited value (CI's `setup-rust` exports one). A
recipe that assigned only `-D warnings` would pass while quietly using the
platform linker and the single-threaded frontend, which is why
`tests/build_standard_contract.rs` reads what `make -n` prints for every
development target on a Linux and a macOS host. Every Cargo command uses
`$(CARGO)`, so the executable stays injectable; the contract evaluates the
recipes with `CARGO=probe-cargo` to prove it.

### Explicit non-development routes

Three builds are deliberately off the standard, each by assigning `RUSTFLAGS`
without the fast flags:

- Coverage. The `generate-coverage` steps in `ci.yml` and `coverage-main.yml`
  set `RUSTFLAGS: -D warnings`, because a measurement should not depend on the
  fast flags.
- Release. `make release` assigns the caller's `RUSTFLAGS` and nothing else.
  In `release.yml` the `rust-build-release` step passes
  `rustflags: '-D warnings'`. That input is exported as `RUSTFLAGS` before the
  action's toolchain setup, so the exclusion is written down at the step
  instead of resting on the nested `setup-rust` default.
  `tests/workflow_contracts/release_flags_test.py` holds it.
- Whitaker. The Whitaker half of `make lint` runs on its installer-managed
  toolchain, which must not inherit the development flags. Its command assigns
  `RUSTFLAGS` the warning policy alone (`$(RUST_FLAGS)`), never expands the
  caller's value, clears `CARGO_ENCODED_RUSTFLAGS` with `env -u` (Cargo reads
  it in preference to `RUSTFLAGS`), and names neither `-Zthreads=8` nor the
  `mold` flag. The Concordat rule checks its isolation.

### Installed tooling and the preflight

On Linux the configuration names `mold`, and rustc reaches it through
`cc -fuse-ld=mold`. A build without it fails at link time, deep inside Cargo. So
`build`, `test`, `lint`, `typecheck`, `scaleway-test`, and `scaleway-janitor`
depend on `check-build-tools`, which runs `scripts/check-build-tools.sh` before
any compilation. It checks, and reports every gap in one pass:

- `mold` is on `PATH` at the version pinned in `tools/mold/VERSION`;
- `cc` links a trivial program with `-fuse-ld=mold` (GCC 12.1 or newer, or
  clang; `clang` itself is not required);
- the toolchain pinned in `rust-toolchain.toml` is installed.

A missing tool stops the route with an installation hint. There is no fallback
to another linker. `release` has no preflight, because it needs none of these
tools. The platform is read once, by `detect_platform`, and a `uname -s` that
cannot answer stops both scripts instead of reading as "not Linux": treating it
that way would skip every linker check and report success, which is the silent
pass the preflight exists to prevent.

`make install-build-tools` provides them: it downloads the pinned `mold`
release, verifies it against `tools/mold/SHA256SUMS`, unpacks it under
`$(BUILD_TOOLS_PREFIX)` (default `~/.local`), and installs the pinned nightly
with the components `rust-toolchain.toml` lists (`rustfmt`, `clippy`, and
`rust-analyzer`). A `mold` already on `PATH` at the pinned version is left
alone. The Makefile puts `$(BUILD_TOOLS_PREFIX)/bin` first on `PATH`, so the
pinned release outranks a distribution one for every recipe; a bare `cargo`
outside Make needs the same directory on `PATH`. These are shell scripts, not
the Python of `docs/scripting-standards.md`, because the preflight must run
before anything else is installed.

Installed tooling need not be active in every job. The coverage build never
links with `mold`, but the suite it runs starts Cargo, so every Linux job that
runs the suite runs `make install-build-tools` in its own step, with no `if:`
and no `continue-on-error`, before the suite. Those jobs get `mold` from
`setup-rust` first, so their install sees the pinned version and returns early.
The `install-build-tools` job in `ci.yml` is the end-to-end counterpart: it
starts with nothing but the runner's own `rustup`, no linker on `PATH` and an
empty prefix, then asserts that the download, checksum check, unpack and
toolchain install happened and that `make check-build-tools` accepts what the
installer produced.

### The contracts

- `tests/build_standard_contract.rs` reads the configuration sources and
  the commands `make -n` prints for each development target on a Linux and a
  macOS host (each keeping the caller's own `RUSTFLAGS`) and for the release
  target, plus the `setup-rust` steps of the workflows. It also evaluates every
  route with `CARGO=probe-cargo`: the preflight must be printed before the
  first Cargo command of each development route and absent from `release`, and
  no command may hard-code `cargo`.
- The same crate runs the preflight and installer scripts in a sandbox `PATH`
  (`build_standard_support/sandbox.rs`) holding exactly the tools a case
  chooses, and runs the real Make routes there with a fake Cargo, to prove that
  a missing `mold` stops every development route before Cargo starts and that a
  complete toolset reaches it. The fake `cc` links only when it receives
  `-fuse-ld=mold`, so one case rewrites a copy of the preflight to drop that
  argument and requires the complete toolset to fail: the probe is proved to
  depend on the flag rather than pass whatever the driver does. A `uname -s`
  that fails is a case of its own, for both scripts.
- `tests/workflow_contracts/suite_lane_install_test.py` derives the Linux jobs
  that run the suite (through the coverage action, `cargo nextest run`, or a
  Make goal that reaches it) and requires the installer, unconditionally,
  before the suite. An empty reading is refused.
- `tests/workflow_contracts/release_flags_test.py` holds the release step's
  explicit `rustflags`.

The contract runs `make -n`, so a direct `cargo test` needs GNU make on the
`PATH`; it fails when `make` is missing instead of skipping. The decision is
recorded in [ADR 001](adrs/adr-0001-rust-build-standard.md).

### The development footprint

The Concordat `rust-dev-build-footprint` rule keeps default development builds
small. `Cargo.toml` sets `incremental = false` and `debug = 0` in
`[profile.dev]`, which the test profile inherits, and `.cargo/config.toml` sets
`[unstable] embed-metadata = false`, the native Cargo setting for dated
nightlies from 2026-07-31; read the pin from `rust-toolchain.toml`, because
stable Cargo ignores the table. The trade-off is that a debugger sees no
source-level debug information or line tables in a default development build
and every rebuild recompiles whole crates. To debug, run with
`CARGO_PROFILE_DEV_DEBUG=true`; to rebuild incrementally, set
`CARGO_INCREMENTAL=1`.

### CLI integration-test builds

CLI behavioural tests share `tests/common/test_binary.rs`. It invokes Cargo from
`CARGO_MANIFEST_DIR`, enables `test-backdoors` explicitly, and selects the
executable from Cargo's `compiler-artifact` JSON. Running from the manifest
root preserves the configured target directory and inherited Cargo settings.

The helper uses `std::process::Command::output()` to drain stdout and stderr
concurrently. A nested build can fill stderr while its caller waits for stdout,
blocking compilation. Build errors retain both streams so compiler diagnostics
remain available when a build fails.

### Test runner policy

`make test` and the CI coverage step run the tests under `cargo-nextest`, and
`.config/nextest.toml` sets the policy (install the runner with
`cargo binstall cargo-nextest`):

- A test is warned after 60 s and terminated after five warning periods
  (300 s); the whole run is bounded at 30 minutes.
- The `cli_run`, `cli_smoke`, `cloud_init_behaviour` and
  `instance_overrides_behaviour` binaries start a child Cargo, which contends
  for the package-cache lock and for build parallelism, so they run one at a
  time in the `nested-cargo-builds` group. Each gets ten warning periods (600
  s), because a cold build looks like a hang.
- Documentation tests are outside nextest's limits: `make test` runs them
  separately with `cargo test --doc`, and the coverage action does the same.

Property coverage uses `proptest` for 256 generated Cargo message histories of
0–31 records, varying matching fields, distractors, repeated matches, and
missing executable paths. An independent record-oriented oracle selects the
last executable on a matching record and retains an earlier path when a later
match has no executable. A spawn regression also uses a guaranteed-missing
absolute executable and checks for `Spawn(NotFound)`. These generated cases are
sampled coverage, not a formal proof.

### Cranelift

Cranelift is not the development backend. This is an exception to the estate
default, and it needs maintainer approval; until that is recorded here, treat
it as proposed, not granted.

Scope: a dev-profile build with `-Zcodegen-backend=cranelift` on
`nightly-2026-11-06`, the pin at the time of measurement, on 2026-11-07 on
x86-64 Linux with the standard's `-Zthreads=8` and `mold` flags. The pin has
since moved, and nothing has re-measured the backend on the current one, so
read this as a statement about the toolchain named, not about today's pin.
Other platforms are unmeasured. The failure is in unwinding: a panic compiled
by Cranelift does not find its handler, so `catch_unwind`, `resume_unwind`,
`Drop` on unwind, and a panic on a spawned thread fail, and the last aborts the
process with `failed to initiate panic, error 5`. `#[should_panic]` still
passes, because libtest's outermost handler catches the panic, so a probe
limited to it reads as a pass. A dependency-free probe crate shows the same
failures without `mold` and without `-Zthreads=8`, so the backend alone causes
them.

Four tests fail, all through `rstest_bdd::skip!`, which unwinds:
`scenario_provision_and_destroy`, `scenario_reject_unknown_image` and
`scenario_reject_unknown_type` in `tests/scaleway_backend.rs`, and
`scenario_cloud_init_install_jq` in `tests/scaleway_cloud_init.rs`. With the
command below, 369 of the 373 tests nextest ran passed when this was measured,
and the four failures were exactly these. `make test` stops there, so the
doctests run apart (`cargo test --doc --all-features` passed under Cranelift in
the earlier measurement). The tests rely on unwinding, and that reliance stays.

Reproduce it on the pin. Cargo needs `CARGO_UNSTABLE_CODEGEN_BACKEND=true` to
accept the profile setting (without it Cargo stops with
``feature `codegen-backend` is required``), the toolchain needs the
`rustc-codegen-cranelift-preview` component, and `--no-fail-fast` reaches all
four failures, because without it nextest stops at the first failing test
binary:

```bash
CARGO_UNSTABLE_CODEGEN_BACKEND=true \
CARGO_PROFILE_DEV_CODEGEN_BACKEND=cranelift \
make test TEST_FLAGS="--all-targets --all-features --no-fail-fast"
```

Recheck the exception when `rstest-bdd` moves to 0.6: that migration may change
`skip!`, and whether the four tests still fail then is a question to measure,
not a conclusion to carry over. Re-measure on every toolchain bump too, and
adopt the backend when the whole suite passes. Adopting a backend also means
checking the exclusion routes: the backend is a profile setting, which assigning
`RUSTFLAGS` need not deselect, so verify the actual `rustc` invocation of the
coverage and release builds.
