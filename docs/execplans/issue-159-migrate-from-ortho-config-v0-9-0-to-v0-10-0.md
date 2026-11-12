# Migrate mriya to ortho_config v0.10.0

This ExecPlan (execution plan) is a living document. The sections `Constraints`,
`Tolerances`, `Risks`, `Progress`, `Surprises & Discoveries`, `Decision log`,
`Outcomes & retrospective`, `Conformance basis`, and `Verification plan` must
be kept up to date as work proceeds.

Status: COMPLETE (draft pull request
[#239](https://github.com/leynos/mriya/pull/239) open; not merged)

## Purpose / big picture

Mriya is a Rust command-line tool that provisions a short-lived Scaleway
virtual machine, syncs the working tree, runs a command over SSH, and tears the
machine down. Its configuration is layered by the `ortho_config` crate:
defaults, then configuration files, then environment variables, then
command-line arguments.

This plan migrates Mriya from `ortho_config` v0.9.0 to the published v0.10.0
release and proves the migration with tests. After the change a user can:

1. run `mriya run -- <command>` and `mriya init` with the same configuration
   precedence as before, including the v0.10.0 fix that an explicit
   command-line value wins even when it equals the struct default;
2. rely on `mriya.toml` keys being read from the locations the documentation
   names, and on `mriya init` writing a volume ID that `mriya run` actually
   reads back;
3. read imported upstream documentation that matches the release actually
   compiled against, with a provenance record naming the exact upstream commit
   and checksums.

The migration is observable by running the repository gates (`make check-fmt`,
`make lint`, `make test`, `make markdownlint`, `make nixie`,
`make test-workflow-contracts`) and by the new regression tests described in
`Verification plan`.

## Constraints

- Do not disable `ortho_config` default features. The YAML and JSON5 features
  must stay enabled
  (`ortho_config = { version = "0.10.0", features = ["yaml", "json5"] }`).
  Upstream issue leynos/ortho-config#627 records that the crate does not build
  with `default-features = false`.
- Do not downgrade `ortho_config`. The working tree already declares and
  resolves 0.10.0; the remaining work is behavioural validation, documentation,
  and the compiler floor.
- Preserve the configuration precedence contract: defaults < configuration
  files < environment < command-line arguments.
- Preserve the exit-status and stream contract: remote exit codes are mirrored,
  errors go to standard error, and `mriya init` success text goes to standard
  output.
- Remote commands after `--` must never be reparsed as Mriya or configuration
  flags.
- Do not leak secrets. Tests must use obviously fake credentials and must not
  print real values.
- Do not provision billable cloud resources, invoke live remote commands, or
  expose credentials during validation. Use the existing scripted backends and
  runners.
- Imported upstream documents must be byte-for-byte identical to the files at
  the immutable upstream commit. Do not edit their prose.
- Do not suppress warnings, add `#[ignore]`, or pass `--ignore-rust-version`.
- Do not use `/tmp` as a build target; use it only for logs and scratch.
- Do not run `build-limits-recover` or signal build-limits services. Queued
  builds are normal.
- Use `/usr/bin/gh` authenticated as `leynos` and `/usr/bin/git` over SSH for
  remote operations, with a clean environment as described in the global
  instructions.
- Never use bare `git stash`; use WIP commits or a uniquely tagged stash.

## Tolerances (exception triggers)

- Scope: if implementation requires changes to more than 20 non-ExecPlan files
  or 1200 net lines of code and other documentation, stop and escalate. Exclude
  this ExecPlan file from both counts.
- Interface: if a public API signature must change, stop and escalate. The
  `ConfigWriter` trait and the public configuration structs must keep their
  current shape.
- Dependencies: if a new external dependency is required, stop and escalate.
- Iterations: if a gate still fails after three focused fix attempts, stop and
  escalate with the captured log.
- Time: if a milestone takes more than four hours of wall-clock work, stop and
  escalate.
- Ambiguity: if the correct persisted configuration format cannot be determined
  from the upstream guide and a probe, stop and present options.

## Risks

- Risk: the repository's documented `[scaleway]` and `[sync]` TOML sections are
  not read by `ortho_config`, which reads root-level keys. Severity: high.
  Likelihood: high (already observed). Mitigation: probe the real loader, fix
  the store and documentation, and add a round-trip test.
- Risk: changing the persisted volume-ID location could surprise users who
  already have `[scaleway] default_volume_id` files. Severity: medium.
  Likelihood: medium. Mitigation: keep a read-only fallback for the legacy
  section while writing the root-level key, and document it.
- Risk: declaring `rust-version = "1.89.0"` while a dev-dependency requires
  Rust 1.95 could mislead contributors. Severity: medium. Likelihood: high.
  Mitigation: document the application versus development split in
  `docs/developers-guide.md` and in a comment next to the manifest field.
- Risk: the pinned nightly toolchain may not be the one CI uses.
  Severity: low. Likelihood: low. Mitigation: inspect CI configuration and keep
  the existing pin unless evidence requires a change.
- Risk: imported documentation links may point outside the repository.
  Severity: low. Likelihood: high. Mitigation: record a link-mapping table in
  the provenance document and leave the imported text untouched.

## Progress

- [x] (2026-11-12) Confirm the working tree already declares and resolves
  `ortho_config` 0.10.0; record the discrepancy with the task's stated baseline.
- [x] (2026-11-12) Verify the upstream tag object `996e5a1c...` peels to commit
  `38812c56...` and download the two documents from that commit.
- [x] (2026-11-12) Establish that the source compiles unchanged against 0.10.0
  (`make typecheck`, exit 0).
- [x] (2026-11-12) Probe the real file layer: root-level keys load; `[sync]` and
  `[scaleway]` sections are ignored.
- [x] (2026-11-12) Write this ExecPlan.
- [x] (2026-11-12) Add regression tests for file shape, precedence, explicit CLI
  defaults, boolean `--flag=false`, discovery errors, and the `--` contract
  (`tests/config_sources.rs` and `tests/config_discovery.rs`, 18 tests;
  `src/cli/mod.rs`, 4 tests; `src/config_store/tests.rs`, 4 tests). The store
  round-trip test was written first and failed (`got None`) before the fix.
- [x] (2026-11-12) Fix `ConfigStore` to write and read the root-level volume
      key,
  with a legacy read fallback (`src/config_store/mod.rs`, 3 new tests).
- [x] (2026-11-12) Update repository documentation to the root-level file shape
  (`docs/users-guide.md`, error text in `src/config.rs` and
  `src/sync/config.rs`).
- [x] (2026-11-12) Import the two upstream documents and write the provenance
  record (`docs/ortho-config-v0-10-0-migration-notes.md`).
- [x] (2026-11-12) Declare the compiler floor and document the
  application/development split (`Cargo.toml`, `docs/developers-guide.md`).
- [x] (2026-11-12) Run all gates and compare test inventories. Baseline
  `make test` ran 460 tests across 18 binaries; the migrated tree runs 489
  across 20 binaries, all passing. `make check-fmt`, `make lint`, `make test`,
  `make markdownlint`, `make nixie`, and `make test-workflow-contracts` all exit
  0. Logs are under `/tmp/*-mriya-issue-159*.out`.
- [x] (2026-11-12) Inspect the final dependency graph.
      `cargo tree -i ortho_config` shows only `mriya`; all three `ortho_*`
      crates resolve 0.10.0;
      `Cargo.lock` has no Git-sourced package. The remaining duplicates
      (`reqwest` 0.11/0.12, `thiserror` 1/2, `toml` 0.5/0.8/0.9/1.1, `cap-std`
      3/4) are transitive and predate this migration.
- [x] (2026-11-12) Commit, push, open a draft pull request, and clear CodeRabbit
  concerns. Draft pull request [#239](https://github.com/leynos/mriya/pull/239)
  is open against `main`; five review rounds were answered, with the imported
  guides' relative links left unedited by design and recorded as a known
  limitation.

## Surprises & discoveries

- Observation: the working tree already declares `ortho_config = "0.10.0"` and
  `Cargo.lock` resolves 0.10.0, while the task description says the baseline
  declares 0.9.0. Evidence: `Cargo.toml`, `Cargo.lock`, and
  `cargo tree -i ortho_config`. Impact: the dependency bump was already
  delivered (Dependabot pull request #167). The migration work is therefore
  behavioural validation, documentation, and the compiler floor, not a version
  edit.
- Observation: `ortho_config` reads configuration files as root-level keys, not
  as `[scaleway]` or `[sync]` sections. Evidence: a probe using
  `load_from_iter_with_sources` with a `MapEnv` file source loaded
  `volume_mount_path = "/root"` and `default_volume_id = "root-volume"`, but
  ignored `[sync]` and `[scaleway]` sections. The v0.9.0 source in the Cargo
  registry also builds its file layer from a root-level Figment TOML provider.
  Impact: the repository's documentation and `ConfigStore` have always used a
  shape the loader never read. This is a pre-existing defect surfaced by the
  migration validation, and it must be fixed for loading and writing to select
  compatible locations.
- Observation: the pinned nightly is `nightly-2026-11-10`, not the
  `nightly-2025-11-26` named in the task description. Evidence:
  `rust-toolchain.toml` and `rustc -Vv` (`1.102.0-nightly`). Impact: no nightly
  change is needed; record the actual identity.

## Decision log

- Decision: treat the sectioned TOML shape as a defect and fix it rather than
  only documenting it. Rationale: the task requires validating that loading and
  writing select compatible locations. Leaving `mriya init` writing a key that
  `mriya run` ignores would fail that requirement and leave a real functional
  bug. Date/Author: 2026-11-12, implementation agent.
- Decision: keep a read-only fallback for the legacy
  `[scaleway] default_volume_id` key. Rationale: files written by earlier
  `mriya init` runs exist in the field. Reading the legacy key preserves the
  "already configured; rerun with --force" behaviour without writing the
  unsupported shape again. Date/Author: 2026-11-12, implementation agent.
- Decision: declare `rust-version = "1.89.0"` and document that the full test
  suite needs Rust 1.95 because of the pinned `typos-cli` dev-dependency.
  Rationale: 1.89.0 is the highest `rust-version` in the non-development
  dependency graph, and the application builds on it. Advertising 1.95 would
  overstate the application floor; advertising 1.89 without the note would
  mislead contributors running tests. Date/Author: 2026-11-12, implementation
  agent.
- Decision: keep the existing nightly pin.
  Rationale: the pin already builds the workspace, and no evidence shows it is
  incompatible with 0.10.0. Date/Author: 2026-11-12, implementation agent.
- Decision: apply the CodeRabbit finding that a blank `default_volume_id`
  should count as unset, and reject its two findings that ask for edits inside
  the imported upstream guides. Rationale: the imported files must stay
  byte-for-byte identical to the upstream commit, so their sample code and
  relative links are not ours to rewrite; the provenance document's link
  mapping already redirects every relative link, including the reciprocal
  `users-guide.md` and `v0-10-0-migration-guide.md` links. Mriya's own `main`
  already exits with a non-zero status on failure. Date/Author: 2026-11-12,
  implementation agent.
- Decision: answer the second review round by adding a reader-navigation
  section to the provenance notes, enabling `pipefail` in the recorded gate
  commands, and making the default-value tests select an absent configuration
  path so project-file discovery cannot read the working directory. Rationale:
  these address the findings without editing the byte-identical imported text.
  The two missing-file tests were merged because selecting an absent path makes
  them the same scenario. Date/Author: 2026-11-12, implementation agent.
- Decision: for the third review round, record the imported guides' stale
  in-document links as a known limitation of keeping them byte-identical,
  rather than editing them. Rationale: no edit is possible without breaking the
  provenance requirement, and the reader-navigation section gives working
  links. The round's other findings were applied: project-file discovery is
  isolated in the sync default test, the imported feature guidance is corrected
  in the provenance notes, and a spelling-contract probe now judges the
  compiler-hash exemption in both the model and the real engine. Date/Author:
  2026-11-12, implementation agent.
- Decision: for the fourth review round, fall through to the legacy
  `[scaleway]` value when a root-level volume key is present but blank, and
  bind the `TempDir` guard in the two tests that matched it with `..`.
  Rationale: the blank-key guard from round one otherwise skipped the legacy
  check, so a blank root key could let `mriya init` replace an ID an earlier
  release recorded; and an unbound `TempDir` field is dropped at the end of the
  `let` statement, deleting the fixture directory before the write and leaking
  its recreated replacement. Both fixes are minimal and covered by tests.
  Date/Author: 2026-11-12, implementation agent.

## Outcomes & retrospective

Delivered. The branch compiles against `ortho_config` 0.10.0 with the YAML and
JSON5 features and default features intact, and the three observable outcomes
in the purpose hold:

1. `mriya run` and `mriya init` keep the documented precedence, including the
   v0.10.0 rule that an explicit command-line value equal to the default still
   wins (`tests/config_sources.rs`).
2. `mriya.toml` is read from root-level keys, which is what the loader has
   always read, and `mriya init` now writes a volume ID that `mriya run` reads
   back (`src/config_store/mod.rs`, `tests/config_discovery.rs`).
3. The imported guides match the release compiled against, with tag object,
   peeled commit, byte counts, checksums, and a relative-link map recorded in
   `docs/ortho-config-v0-10-0-migration-notes.md`.

Undischarged or deliberately out-of-scope obligations:

- The imported guides keep upstream relative links, so a handful of in-document
  links point at names this repository does not use. The provenance notes' link
  map and reader-navigation section give the working targets; the bytes stay
  unedited to preserve provenance.
- Named profiles, stacked scopes, localization runtimes, `cargo-orthohelp`, and
  agent metadata are v0.10.0 capabilities the repository does not need, so they
  were not adopted.
- Validation that needs billable cloud resources or live remote commands was
  not performed, as the migration manifest requires.

Lessons:

- The dependency bump had already landed through Dependabot, so the real work
  was behavioural validation. Checking the declared and resolved versions
  before editing any manifest avoided a pointless downgrade-and-rebump.
- The most valuable finding was a pre-existing defect: documentation and
  `ConfigStore` used `[section]` tables the loader never read. A migration is a
  good moment to exercise the documented file shape end to end.
- Blank values and legacy records interact. Treating a blank root key as unset
  is right for the root key, but it must not short-circuit the legacy guard.
- Destructuring a fixture with `..` silently drops the unbound fields at the
  end of the `let` statement; bind a temporary-directory guard by name when the
  test writes through it.
- Review findings that ask for edits inside imported, checksum-pinned text are
  not actionable. Record the limitation and map the links instead.

## Context and orientation

The repository is a single Rust package named `mriya` at the repository root.
The relevant files are:

- `Cargo.toml` and `Cargo.lock`: dependency declarations and the resolved graph.
- `rust-toolchain.toml`: the pinned nightly toolchain.
- `src/config.rs`: `ScalewayConfig`, derived with `OrthoConfig`, prefix `SCW`,
  discovery app name `mriya`, environment override `MRIYA_CONFIG_PATH`, file
  names `mriya.toml` and `.mriya.toml`.
- `src/sync/config.rs`: `SyncConfig`, prefix `MRIYA_SYNC`, same discovery.
- `src/init/mod.rs`: `InitConfig`, prefix `MRIYA_INIT`, same discovery.
- `src/config_store/mod.rs`: `ConfigStore`, which uses `ConfigDiscovery` to find
  `mriya.toml` and writes the cache volume ID.
- `src/cli/mod.rs`: the handwritten `clap` parser for `mriya run` and
  `mriya init`.
- `src/main.rs`: the binary entry point, which loads configuration without CLI
  arguments and applies per-run overrides.
- `tests/`: integration and behavioural tests.
- `docs/users-guide.md`: the repository user guide.
- `docs/ortho-config-users-guide.md`: the previously imported upstream guide.
- `docs/developers-guide.md`: build and toolchain guidance.

Terms used in this plan:

- "Root-level key": a TOML key at the top of the file, such as
  `secret_key = "..."`, as opposed to a key inside a `[section]` table.
- "Source-injected loading": calling a generated loader such as
  `load_from_iter_with_sources` with an explicit environment map and file
  discovery source instead of reading the process environment.
- "MSRV": the minimum supported Rust version, declared as `rust-version` in
  `Cargo.toml`.

## Conformance basis

The upstream artefacts are the `leynos/ortho-config` repository at tag
`v0.10.0` (tag object `996e5a1c9f22ace4f55bf5dcc196d2c0524262bf`, peeled commit
`38812c563e3953a490e3efc202609f37e21a0517`), specifically `docs/users-guide.md`
and `docs/v0-10-0-migration-guide.md`, plus the release changelog. The
repository has no Terms of Reference or ADR set for this migration; the
governing documents are `AGENTS.md`, `docs/users-guide.md`, and
`docs/mriya-technical-design.md`.

Trace links:

```plaintext
UPSTREAM-USERS-GUIDE (root-level TOML keys) -> EP-M2 -> tests/config_sources.rs
UPSTREAM-MIGRATION-GUIDE (explicit CLI wins) -> EP-M2 -> tests/config_sources.rs
UPSTREAM-MIGRATION-GUIDE (boolean --flag=false) -> EP-M2 -> tests/config_sources.rs
UPSTREAM-MIGRATION-GUIDE (source-injected loading) -> EP-M2 -> tests/config_sources.rs, tests/config_discovery.rs
REPO-USERS-GUIDE (volume ID persistence) -> EP-M3 -> src/config_store/tests.rs
REPO-AGENTS (MSRV honesty) -> EP-M4 -> Cargo.toml, docs/developers-guide.md
TASK-S1 (document provenance) -> EP-M5 -> docs/ortho-config-v0-10-0-migration-notes.md
```

## Verification plan

The change introduces no new algorithmic invariant. The obligations are
contract-level and are discharged with parameterized integration tests and
focused unit tests.

- Obligation: `FILE-SHAPE`. A configuration file with root-level keys supplies
  values to `ScalewayConfig` and `SyncConfig`; a file with `[scaleway]` or
  `[sync]` sections does not. Method: parameterized integration test with
  `load_from_iter_with_sources` and a `MapEnv` file source. Rationale: the
  loader is third-party; the repository-owned obligation is that its documented
  interface is used correctly. A real call against the real crate is the
  faithful boundary. Domain: root-level and sectioned files for both
  configuration structs. Artefact: `tests/config_sources.rs`. Evidence:
  `cargo test --test config_sources`; the sectioned cases must show the default
  value, proving the section is ignored. Non-vacuity: the root-level case must
  observe the file value, not the default, so a loader that ignored files
  entirely would fail the test.
- Obligation: `PRECEDENCE`. For each tier, a higher tier wins: default < file <
  environment < command line. Method: parameterized integration test.
  Rationale: finite truth table with distinct semantic rows. Domain:
  `SyncConfig` fields with defaults, plus `ScalewayConfig` required fields.
  Artefact: `tests/config_sources.rs`. Evidence:
  `cargo test --test config_sources`. Non-vacuity: each row asserts a distinct
  value per tier, so a loader that dropped a tier would fail.
- Obligation: `EXPLICIT-CLI-DEFAULT`. An explicit command-line value equal to
  the struct default still wins over a lower tier (the v0.10.0 fix). Method:
  focused test with a small locally derived configuration struct whose command
  line covers every defaulted field. Rationale: the upstream bug required the
  command line to cover every defaulted field; a small struct makes that
  condition reachable and the test decisive. Domain: one boolean field
  defaulting to `true`, with the environment set to `false` and the flag passed
  explicitly. Artefact: `tests/config_sources.rs`. Evidence:
  `cargo test --test config_sources`; the test fails on 0.9.0 and passes on
  0.10.0. Non-vacuity: the environment value differs from the default, so a
  skipped CLI layer would produce `false` and fail the assertion.
- Obligation: `BOOLEAN-FALSE`. `--flag=false` supplies an explicit `false` that
  clears a `true` from a file or environment; `--flag false` is rejected.
  Method: focused test with the same local struct. Rationale: finite surface
  with two spellings. Domain: `--flag=false` accepted, `--flag false` rejected.
  Artefact: `tests/config_sources.rs`. Evidence:
  `cargo test --test config_sources`. Non-vacuity: the accepted case asserts
  `false`; the rejected case asserts a parse error.
- Obligation: `DISCOVERY-ERRORS`. A missing optional configuration file is not
  an error; a malformed file and an unreadable file are distinct, visible
  errors. Method: parameterized integration test. Rationale: finite error
  partition. Domain: absent path, malformed TOML, directory in place of a file.
  Artefact: `tests/config_discovery.rs`. Evidence:
  `cargo test --test config_discovery`. Non-vacuity: each case asserts a
  different outcome, so collapsing the distinctions would fail.
- Obligation: `STORE-ROUND-TRIP`. A volume ID written by `ConfigStore` is read
  back by `ScalewayConfig` from the same file, and a legacy `[scaleway]` value
  is still detected by `current_volume_id`. Method: unit test in
  `src/config_store/tests.rs` plus an integration test in
  `tests/config_discovery.rs`. Rationale: the store and the loader are separate
  repository-owned components; the obligation is their compatibility. Domain:
  fresh write, legacy read, forced overwrite. Artefact:
  `src/config_store/tests.rs`, `tests/config_discovery.rs`. Evidence:
  `cargo test --lib config_store` and `cargo test --test config_discovery`.
  Non-vacuity: the integration test asserts the loaded value equals the written
  ID, so a store writing an unread location fails.
- Obligation: `DASH-CONTRACT`. Arguments after `--` are captured as the remote
  command and never parsed as Mriya flags. Method: unit test on the handwritten
  parser. Rationale: the parser is repository-owned and small. Domain:
  `run -- --instance-type X`, and
  `run --instance-type BIG -- --instance-type SMALL`. Artefact:
  `src/cli/mod.rs` test module. Evidence: `cargo test --lib cli`. Non-vacuity:
  the first case asserts `instance_type` is `None` while the command contains
  the flag text.

Axioms: `ortho_config` 0.10.0 implements the documented layering and
source-injection interfaces; `clap` 4 parses `--flag=false` for boolean flags
generated by the derive; the filesystem behaves normally for temporary
directories. These are treated as documented third-party interfaces and are
exercised through real calls rather than mocked.

## Plan of work

Stage A (complete): understand the current state. Confirm the declared and
resolved versions, the compiler identity, the upstream tag resolution, and the
real file-layer shape.

Stage B: add failing tests. Create `tests/config_sources.rs` (file shape,
precedence, explicit CLI defaults, boolean flags) and
`tests/config_discovery.rs` (discovery errors, store round-trip), sharing the
hermetic workspace fixture from `tests/config_support/mod.rs` so neither file
exceeds the 400-line limit. Add the `--` parser tests to `src/cli/mod.rs`, and
add the store round-trip test to `src/config_store/tests.rs`. Run the focused
tests and confirm they fail for the expected reasons (the store test fails
because the store writes a section; the file-shape test fails because the
section is ignored).

Stage C: implement. Change `src/config_store/mod.rs` to read and write the
root-level `default_volume_id` key, with a legacy `[scaleway]` read fallback.
Update the error-message text in `src/config.rs` and `src/sync/config.rs` to
name root-level keys instead of sections. Add `rust-version = "1.89.0"` to
`Cargo.toml`.

Stage D: documentation and validation. Replace
`docs/ortho-config-users-guide.md` with the imported v0.10.0 guide, add
`docs/ortho-config-v0-10-0-migration-guide.md`, write
`docs/ortho-config-v0-10-0-migration-notes.md` with provenance and the
applicability matrix, update `docs/users-guide.md` and
`docs/developers-guide.md`, then run every gate.

## Milestones and plateaus

- EP-M1: baseline recorded. Outcome: the declared and resolved versions, the
  compiler identity, the upstream tag resolution, and the file-layer shape are
  documented in this plan. Requirements: TASK-S2. Acceptance evidence: the
  `Surprises & discoveries` entries. Conformance check: no code changed; no
  interface or dependency change. Recovery: re-run the probes. Remaining gaps:
  all implementation. Compatibility decision: none.
- EP-M2: source-aware regression tests. Outcome: `tests/config_sources.rs`,
  `tests/config_discovery.rs`, and the CLI parser tests exist and pass.
  Requirements: UPSTREAM-USERS-GUIDE, UPSTREAM-MIGRATION-GUIDE. Acceptance
  evidence: `cargo test --test config_sources --test config_discovery` and
  `cargo test --lib cli`. Conformance check: tests use source injection, not
  process-environment mutation; no public interface change. Recovery: revert
  the test commit. Remaining gaps: the store fix. Compatibility decision: none.
- EP-M3: compatible persistence. Outcome: `ConfigStore` writes a root-level key
  that `ScalewayConfig` reads, and legacy sectioned files are still detected.
  Requirements: REPO-USERS-GUIDE. Acceptance evidence: the store unit tests and
  the integration round-trip test. Conformance check: `ConfigWriter` signature
  unchanged; persisted format change is deliberate and documented. Recovery:
  revert the commit; the legacy fallback keeps old files readable. Remaining
  gaps: documentation. Compatibility decision: deployed `mriya.toml` files
  written by earlier `mriya init` runs require the read fallback.
- EP-M4: compiler floor. Outcome: `Cargo.toml` declares
  `rust-version = "1.89.0"` and the developer guide explains the
  application/development split. Requirements: REPO-AGENTS. Acceptance evidence:
  `cargo metadata` reports the field; the gates pass on the pinned nightly.
  Conformance check: no dependency change. Recovery: revert the commit.
  Remaining gaps: none. Compatibility decision: none.
- EP-M5: documentation provenance. Outcome: the imported guides match the
  upstream commit byte-for-byte and the provenance record names the commit,
  paths, and checksums. Requirements: TASK-S1. Acceptance evidence: `sha256sum`
  matches the recorded values. Conformance check: imported prose is unedited.
  Recovery: re-download from the immutable commit. Remaining gaps: none.
  Compatibility decision: none.

## Concrete steps

All commands run from the repository root
`/home/leynos/.lody/repos/github---leynos---mriya/worktrees/647696b7-99aa-4b65-a0be-aa60d16a5716`.

1. Run the focused tests:

   ```console
   cargo test --test config_sources --test config_discovery
   cargo test --lib cli
   cargo test --lib config_store
   ```

2. Run the gates sequentially, logging each. Enable `pipefail` first so a
   failing gate is not masked by a successful `tee`:

   ```console
   set -o pipefail
   make check-fmt 2>&1 | tee /tmp/check-fmt-mriya-issue-159.out
   make lint 2>&1 | tee /tmp/lint-mriya-issue-159.out
   make test 2>&1 | tee /tmp/test-mriya-issue-159.out
   make markdownlint 2>&1 | tee /tmp/markdownlint-mriya-issue-159.out
   make nixie 2>&1 | tee /tmp/nixie-mriya-issue-159.out
   make test-workflow-contracts 2>&1 | tee /tmp/workflow-contracts-mriya-issue-159.out
   ```

3. Verify the imported documents:

   ```console
   sha256sum docs/ortho-config-users-guide.md docs/ortho-config-v0-10-0-migration-guide.md
   ```

## Validation and acceptance

Acceptance is behavioural:

- `cargo test --test config_sources --test config_discovery` passes, and the
  file-shape cases show that root-level keys are read while sectioned keys are
  ignored, and that the store's written ID is read back by `ScalewayConfig`.
- `cargo test --lib cli` passes, and the `--` cases show that remote flags are
  not parsed as Mriya flags.
- `cargo test --lib config_store` passes, and the legacy-section case shows
  that an earlier release's volume ID is still detected.
- `make check-fmt`, `make lint`, `make test`, `make markdownlint`, `make nixie`,
  and `make test-workflow-contracts` all exit 0.
- The imported documents' SHA-256 checksums match the provenance record.

Red-Green-Refactor evidence will be recorded here as each stage completes.

## Idempotence and recovery

Every step is re-runnable. Tests use temporary directories and source-injected
environments, so they leave no process state behind. If a gate fails, read the
captured log rather than re-running blindly. If a commit must be undone, use
`git revert` on the specific commit; do not use bare `git stash`.

## Artefacts and notes

The probe that established the file shape:

```rust
let env = Arc::new(MapEnv::new().with_var("MRIYA_CONFIG_PATH", path));
let discovery: SharedEnvSource = env.clone();
let merge: SharedScanEnvSource = env;
SyncConfig::load_from_iter_with_sources(["mriya"], discovery, merge)
```

Observed: `volume_mount_path = "/root"` loaded as `/root`;
`[sync] volume_mount_path = "/section"` loaded as the default `/mriya`.

## Interfaces and dependencies

No new dependencies. The public interfaces that must remain stable are:

```rust
pub trait ConfigWriter {
    fn current_volume_id(&self) -> Result<Option<String>, ConfigStoreError>;
    fn write_volume_id(&self, volume_id: &str, force: bool)
        -> Result<Utf8PathBuf, ConfigStoreError>;
}
```

The configuration structs keep their fields and derive attributes. The only
manifest change is the addition of `rust-version = "1.89.0"`.

## Revision note

2026-11-12: initial draft, created after the baseline probes established that
the working tree already resolves 0.10.0 and that the file layer reads
root-level keys. Remaining work is behavioural validation, the store fix,
documentation, and the compiler floor.
