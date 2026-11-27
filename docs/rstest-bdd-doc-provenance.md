# rstest-bdd documentation provenance

This file is repository-authored. It records where the three imported
`rstest-bdd` documents came from, how they are kept byte-for-byte, and what
each breaking change in `rstest-bdd` 0.3 to 0.6 meant for Mriya. The
conventions that follow from the migration are in
[`developers-guide.md`](developers-guide.md#bdd-testing-with-rstest-bdd).

## Source

- Repository: <https://github.com/leynos/rstest-bdd>
- Tag: `v0.6.0`
- Commit: `72fb22635670e456545ca368805ba4c1c9d7bd69`
- Crate versions adopted: `rstest-bdd` 0.6.0 and `rstest-bdd-macros` 0.6.0

| Upstream path                    | Destination                                 | SHA-256                                                            |
| -------------------------------- | ------------------------------------------- | ------------------------------------------------------------------ |
| `docs/users-guide.md`            | `docs/rstest-bdd-users-guide.md`            | `1e9f4d1b6607fdf83df979676f1a682218cd6751381d374cb98b701778b798b2` |
| `docs/v0-6-0-migration-guide.md` | `docs/rstest-bdd-v0-6-0-migration-guide.md` | `6e76c10028f9962134f6158732cf1645ae1b0beba3018bccd3bc3e88b2c38985` |
| `docs/v0-5-0-migration-guide.md` | `docs/rstest-bdd-v0-5-0-migration-guide.md` | `508d5b55cb2025915ced366b917a2327e572a6aa920fd290684b858b7afa2abc` |

Verify them with `sha256sum docs/rstest-bdd-*-guide.md`. The files are copied
unchanged. The users' guide replaces the stale copy that described 0.2.

## Version boundary

The v0.6.0 text describes some v0.6.1-beta and v0.7.0 interfaces, such as
guard-based `StepContext` borrowing and the GPUI stateful-step variants. Mriya
adopts nothing beyond 0.6.0, and the upstream `main` v0.7.0 guide is not
imported.

## Repository-local handling

### Relative links

The imported text links to upstream documents that do not exist here. They are
left as they are, and no gate checks them: `markdownlint` and `nixie` pass on
the imports. A reader following one of these finds no file.

| Imported file          | Link target                                                  | Where upstream keeps it                                      |
| ---------------------- | ------------------------------------------------------------ | ------------------------------------------------------------ |
| v0.5.0 migration guide | `adr-006-fallible-scenario-functions.md` (link and footnote) | `docs/`                                                      |
| v0.6.0 migration guide | `adr-006-fallible-scenario-functions.md`                     | `docs/`                                                      |
| v0.6.0 migration guide | `adr-007-harness-context-injection.md`                       | `docs/`                                                      |
| v0.6.0 migration guide | `adr-009-consistent-implicit-fixture-name-normalization.md`  | `docs/`                                                      |
| v0.6.0 migration guide | `adr-012-guard-based-stepcontext-borrowing.md`               | `docs/`                                                      |
| v0.6.0 migration guide | `rstest-bdd-design.md` and three section anchors             | `docs/`                                                      |
| v0.6.0 migration guide | `developers-guide.md`                                        | resolves to Mriya's own guide, which is a different document |

In-page anchors resolve inside each file. None of the targets is imported, to
stay inside the three documents the migration called for. If a link checker is
added later, give it a map from these targets to the upstream tag rather than
editing the imports.

### Gate exclusions

None. At import, `mdtablefix --check`, `markdownlint-cli2`, `make spelling` and
`make nixie` pass on the three files without a per-path exclusion.

## Disposition of breaking changes

Counts are over `tests/`, `src/` and `crates/` Rust files at the migration
commit. The suites have 39 `#[scenario]` functions (plus 3 in the new
regression test) and 87 step definitions, all synchronous.

### v0.6.0

| Change                                                                                | Applies?              | Evidence and action                                                                                                                                                                                                                                                                |
| ------------------------------------------------------------------------------------- | --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| One leading underscore on implicit fixture names is dropped                           | Not before; used once | No `#[from(_...)]` and no underscore fixture parameters existed. The new `tests/scaleway_cloud_init.rs` scenario uses `_scaleway_backend`, `_syncer` and `_temp_source_dir` deliberately, and the suite passes.                                                                    |
| `runtime = "tokio-current-thread"` becomes an alias of `TokioHarness`                 | No                    | Zero uses of `runtime =`, `scenarios!` or `harness =`. Steps that need a runtime build one with `Runtime::new()` and `block_on`.                                                                                                                                                   |
| `HarnessAdapter` implementations return `HarnessResult`                               | No                    | Zero custom harnesses.                                                                                                                                                                                                                                                             |
| Feature paths are manifest-relative                                                   | No action             | Every `#[scenario]` path is already manifest-relative (`tests/features/...`). No reporter, JUnit or `cargo bdd --dump-steps` consumer exists.                                                                                                                                      |
| `StepContext::insert_value` returns `InsertOutcome`                                   | No                    | Zero calls.                                                                                                                                                                                                                                                                        |
| Step `Result` aliases propagate `Err`, inject `T`, and need `Display` errors          | Yes                   | Zero steps return an alias directly. The init, run and sync steps return `Result<XContextResult, StepError>`, where the alias is the payload and `StepError` implements `Display`. `tests/step_return_propagation.rs` pins the rules, with mutation proof. No scenario turned red. |
| `RustStepIndexResult`, removal of `index_feature_file` and `publish_rust_diagnostics` | No                    | Zero uses of the language-server APIs.                                                                                                                                                                                                                                             |
| `record_bypassed_steps` takes a `BypassedScenario`                                    | No                    | Zero calls.                                                                                                                                                                                                                                                                        |
| `find_feature_files` returns a `Result`                                               | No                    | Zero calls.                                                                                                                                                                                                                                                                        |
| MSRV 1.88                                                                             | Satisfied             | `rust-toolchain.toml` pins `nightly-2026-11-06`, which is newer than 1.88.                                                                                                                                                                                                         |
| `tracing` gains the `log` feature through unification                                 | Checked               | `cargo tree -e features -i tracing` shows `log` enabled; `make lint` (Clippy and Whitaker) passes.                                                                                                                                                                                 |

### v0.5.0

| Change                                                                                            | Applies?              | Evidence and action                                                                                                                                                          |
| ------------------------------------------------------------------------------------------------- | --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `#[scenario]` returns only `Result<(), E>` or `StepResult<(), E>`                                 | Yes, for one scenario | No scenario returned a value. `scenario_cloud_init_install_jq` declared a `Result` fixture and returned `()`, which no longer compiles. It now returns `Result<(), String>`. |
| Scenario return types are not resolved through aliases                                            | No                    | No scenario returns an alias.                                                                                                                                                |
| Fixtures are recreated per scenario; `#[once]` opts in                                            | No action             | Zero `#[once]`. Mriya's fixtures were already built per test and none carries state into another scenario.                                                                   |
| Async-only steps under a sync scenario use a per-step Tokio fallback; nested runtimes are guarded | No                    | Zero `async fn` or `tokio::test` in BDD files. Steps that `block_on` run in synchronous scenarios outside any runtime, so no nested runtime arises.                          |
| Async wrapper helpers (`sync_to_async`)                                                           | No                    | Zero manual async wrappers.                                                                                                                                                  |

### v0.4.0

The 0.4.0 tag has no release notes or migration guide. The commit log between
`v0.3.1` and `v0.4.0` lists these user-facing items.

| Change                                             | Applies? | Evidence and action                       |
| -------------------------------------------------- | -------- | ----------------------------------------- |
| Async step execution and Tokio current-thread mode | No       | Zero async steps; see v0.5.0.             |
| `expr` syntax in step attributes                   | No       | Zero uses.                                |
| `scenarios!` gains `fixtures`                      | No       | Zero uses of `scenarios!`.                |
| Scenario Outline support                           | No       | No Scenario Outline in `tests/features/`. |
| Language-server diagnostics, Go to Implementation  | No       | Editor tooling only.                      |
| Skip logic consolidated                            | Checked  | `skip!` behaves as before; see below.     |

### v0.3.x

The 0.3.0 and 0.3.1 releases list no breaking changes.

| Change                                                   | Applies? | Evidence and action                                                |
| -------------------------------------------------------- | -------- | ------------------------------------------------------------------ |
| Stable return-kind inference                             | Yes      | Subsumed by the 0.6 step-return rules above.                       |
| Underscore-prefixed step parameter matching              | No       | No underscore step parameters existed.                             |
| Implicit fixture injection through metadata-aware lookup | Checked  | Steps already named their fixtures; all 410 tests pass.            |
| Specificity-based step ranking                           | Checked  | No ambiguous step patterns; every scenario ran its intended steps. |
| Borrowed `&str` parameters, string type hint             | No       | Not used.                                                          |

## Behaviour checks

- `skip!` is unchanged for Mriya. The live suites skip when
  `MRIYA_RUN_SCALEWAY_TESTS` is unset, and the four Cranelift failures
  (`scenario_provision_and_destroy`, `scenario_reject_unknown_image`,
  `scenario_reject_unknown_type` and `scenario_cloud_init_install_jq`) recur
  exactly under 0.6.0 because `skip!` still unwinds. The exception in
  [`developers-guide.md`](developers-guide.md#cranelift) stands.
- 0.6 unwraps `Result` fixtures in the scenario prelude, so a live-suite
  fixture that returned `Err` for "switched off" would fail the scenario rather
  than skip it. `tests/scaleway_cloud_init.rs` now returns a `Gated` wrapper
  and skips in the step. This leaves the silent skip of #210 as it was.
