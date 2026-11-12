# ortho_config v0.10.0 migration notes

This document records the provenance of the imported `ortho_config` v0.10.0
documentation, the dependency and compiler state before and after the
migration, which migration-guide items apply to Mriya, and the validation
evidence. It is the companion to
[the imported migration guide](ortho-config-v0-10-0-migration-guide.md) and
[the imported users' guide](ortho-config-users-guide.md).

## Imported documentation provenance

Both files were fetched from the immutable commit that the `v0.10.0` tag peels
to, not from `main`, `HEAD`, or a "latest" URL.

| Field               | Value                                           |
| ------------------- | ----------------------------------------------- |
| Upstream repository | `https://github.com/leynos/ortho-config`        |
| Tag                 | `v0.10.0`                                       |
| Tag object          | `996e5a1c9f22ace4f55bf5dcc196d2c0524262bf`      |
| Peeled commit       | `38812c563e3953a490e3efc202609f37e21a0517`      |
| Tag type            | annotated tag (`git cat-file -t` reports `tag`) |

| Source path                       | Destination path                               | Bytes | SHA-256                                                            |
| --------------------------------- | ---------------------------------------------- | ----- | ------------------------------------------------------------------ |
| `docs/users-guide.md`             | `docs/ortho-config-users-guide.md`             | 80278 | `84d1c97b77024224dc9220a12d4b749205d9bb2db54849185da21c969dda5575` |
| `docs/v0-10-0-migration-guide.md` | `docs/ortho-config-v0-10-0-migration-guide.md` | 22980 | `b5331aa6cfcc07b3dcd188c620db98d0a1a1f13b0c8ef96d88913d701bca25bf` |

The imported text is byte-for-byte identical to the upstream files. The
destination names differ from the source names so the repository can keep more
than one imported guide without collisions. The previous
`docs/ortho-config-users-guide.md` was replaced by the v0.10.0 copy. The
repository held no earlier imported `ortho_config` migration guide, so there
was no historical migration guide to retain.

Verification:

```console
$ sha256sum docs/ortho-config-users-guide.md docs/ortho-config-v0-10-0-migration-guide.md
84d1c97b77024224dc9220a12d4b749205d9bb2db54849185da21c969dda5575  docs/ortho-config-users-guide.md
b5331aa6cfcc07b3dcd188c620db98d0a1a1f13b0c8ef96d88913d701bca25bf  docs/ortho-config-v0-10-0-migration-guide.md
```

## Relative-link mapping

The imported prose is unedited, so its relative links still name upstream
paths. This table maps each one to the upstream location at the imported
commit, or to the local destination where an equivalent file exists.

| Link in imported text                                    | Resolves to                                                                                                                                        |
| -------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `../CHANGELOG.md`                                        | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/CHANGELOG.md`                                                |
| `../README.md#install-the-documentation-generator`       | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/README.md#install-the-documentation-generator`               |
| `../examples/hello_world/`                               | `https://github.com/leynos/ortho-config/tree/38812c563e3953a490e3efc202609f37e21a0517/examples/hello_world`                                        |
| `../ortho_l10n/README.md`                                | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/ortho_l10n/README.md`                                        |
| `adrs/adr-0004-cargo-external-subcommand-entry-point.md` | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/docs/adrs/adr-0004-cargo-external-subcommand-entry-point.md` |
| `cargo-orthohelp-api-guide.md`                           | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/docs/cargo-orthohelp-api-guide.md`                           |
| `cargo-orthohelp-design.md`                              | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/docs/cargo-orthohelp-design.md`                              |
| `v0-10-0-migration-guide.md`                             | Local: `docs/ortho-config-v0-10-0-migration-guide.md`                                                                                              |
| `v0-9-0-migration-guide.md`                              | `https://github.com/leynos/ortho-config/blob/38812c563e3953a490e3efc202609f37e21a0517/docs/v0-9-0-migration-guide.md`                              |
| `users-guide.md#...` (in the migration guide)            | Local: `docs/ortho-config-users-guide.md#...`                                                                                                      |

### Reader navigation

Because the imported files keep their upstream relative links, following one
from inside this repository can open the wrong local file. Use these links
instead:

- [ortho_config v0.10.0 users' guide](ortho-config-users-guide.md)
- [ortho_config v0.10.0 migration guide](ortho-config-v0-10-0-migration-guide.md)

The imported users' guide's application example prints a configuration error
and then returns from `main` without a failure status. That is upstream sample
code and is left unedited. Mriya's own entry point does not copy it:
`src/main.rs` computes an exit code and calls `process::exit`, so a
configuration failure reaches the shell as a non-zero status.

The same guide notes that `yaml` requires `serde_json` and that both must be
enabled explicitly if default features are disabled. Mriya keeps the default
features enabled, because upstream issue leynos/ortho-config#627 records that
the crate does not build with `default-features = false`. Do not follow the
imported guide's conditional advice in this repository.

## Dependency state

The working tree already declared and resolved `ortho_config` 0.10.0 before
this migration branch's behavioural work, delivered by the Dependabot update
for the dependency group. The task description's stated baseline
(`ortho_config = "0.9.0"`) was therefore already superseded. No downgrade was
performed.

| Manifest or lockfile  | Before this branch                                                       | After this branch         |
| --------------------- | ------------------------------------------------------------------------ | ------------------------- |
| `Cargo.toml` declared | `ortho_config = { version = "0.10.0", features = ["yaml", "json5"] }`    | unchanged                 |
| `Cargo.lock` resolved | `ortho_config` 0.10.0, `ortho_config_macros` 0.10.0, `ortho_l10n` 0.10.0 | unchanged                 |
| `Cargo.toml` compiler | no `rust-version`                                                        | `rust-version = "1.89.0"` |
| `rust-toolchain.toml` | `nightly-2026-11-10`                                                     | unchanged                 |

`cargo tree -i ortho_config` shows `mriya` as the only dependent, and
`Cargo.lock` contains no Git-sourced package. The YAML and JSON5 features and
the default features remain enabled; `default-features = false` is not used,
because `ortho_config` does not build that way (leynos/ortho-config#627).

## Applicability matrix

| Migration-guide item                         | Status               | Evidence or reason                                                                                                                        |
| -------------------------------------------- | -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| 1. Update dependency versions                | Already done         | `Cargo.toml` and `Cargo.lock` resolve 0.10.0 for all three crates.                                                                        |
| 2. Explicit CLI values win                   | Applicable, verified | `tests/config_sources.rs::explicit_command_line_value_equal_to_default_wins` and `environment_value_applies_when_command_line_is_absent`. |
| 3. Re-key Fluent catalogues for IR 2.0       | Not applicable       | The repository has no Fluent catalogues, localizers, `OrthoConfigLocalization` use, or `ORTHO_DOCS_IR_VERSION` handling.                  |
| 4. `AgentContext` and `DocMetadata` literals | Not applicable       | Neither type is referenced in the repository.                                                                                             |
| 5. Boolean flag spellings                    | Applicable, verified | `boolean_flag_accepts_explicit_false` and `boolean_flag_rejects_space_separated_value`.                                                   |
| 6a. Named profiles                           | Not adopted          | Mriya's configuration is flat and the feature is opt-in; adopting it would add a new user-facing surface beyond the migration.            |
| 6b. Stacked configuration scopes             | Not adopted          | The repository relies on the default `first_wins` discovery order; changing it would change behaviour.                                    |
| 6c. Injected environment sources             | Adopted in tests     | `tests/config_sources.rs`, `tests/config_discovery.rs`, and `src/sync/tests/config.rs` use `MapEnv` and `load_from_iter_with_sources`.    |
| 6d. `cli_default_as_absent`                  | Not applicable       | No generated clap `default_value` restates an `ortho_config` default; the handwritten parser has no defaults.                             |
| 6e. Agent-native metadata and policy         | Not applicable       | The repository does not use `cargo-orthohelp` or the policy checker.                                                                      |
| 6f. Cargo external-subcommand helper         | Not applicable       | `mriya` is not a Cargo subcommand.                                                                                                        |
| 6g. Standalone localization runtime          | Not applicable       | No localization runtime is used.                                                                                                          |

## Compiler floor and toolchain

`Cargo.toml` now declares `rust-version = "1.89.0"`. That is the highest
`rust-version` in the locked non-development graph (`ortho_config`,
`ortho_config_macros`, `ortho_l10n`, and `uuid`). The application, its library,
and its build script were checked against that compiler:

```console
$ RUSTFLAGS="" cargo +1.89.0 check --lib --bins
    Finished `dev` profile [unoptimized] target(s) in 4m 45s
```

The full test suite has a higher floor: the exact-pinned
`typos-cli = "=1.50.2"` dev-dependency requires Rust 1.95. The development
build standard also uses the nightly pinned in `rust-toolchain.toml`
(`nightly-2026-11-10`, `rustc 1.102.0-nightly (ac8310ba6 2026-11-09)`), because
`.cargo/config.toml` passes nightly-only flags. The pin was retained: it builds
the workspace and no evidence showed it incompatible with 0.10.0.

## Repository defect found and fixed

`ortho_config` reads configuration files as root-level keys. The repository's
documentation and `ConfigStore` used `[scaleway]`, `[sync]`, and `[init]`
sections, which the loader has never read (the v0.9.0 source also builds its
file layer from a root-level Figment TOML provider). The practical effect was
that `mriya init` wrote `[scaleway] default_volume_id`, and `mriya run` never
read it.

The migration fixes this:

- `ConfigStore` writes the root-level `default_volume_id` key and reads it back.
- `ConfigStore` still detects a legacy `[scaleway] default_volume_id` value, so
  `mriya init` keeps refusing to replace a volume ID recorded by an earlier
  release unless `--force` is supplied.
- The users' guide and the validation error messages now name root-level keys.
- `tests/config_discovery.rs` proves the store writes a file that
  `ScalewayConfig` reads, and that sectioned files are ignored.

## Test inventory

Baseline (commit `6445bbe`, before this branch's behavioural changes) ran 460
tests across 18 binaries, all passing. The migrated tree runs 488 tests across
20 binaries, all passing. The two new binaries are `config_sources` and
`config_discovery`; the 28 added tests are 12 integration tests in
`tests/config_sources.rs`, 5 integration tests in `tests/config_discovery.rs`,
4 parser tests in `src/cli/mod.rs`, 4 store tests in
`src/config_store/tests.rs`, and 3 spelling-contract checks for the
compiler-hash exemption (two probe cases in `tests/spelling_contract/probes.rs`
and one engine test in `tests/spelling_contract/compiler_hash.rs`). One
existing test,
`src/sync/tests/config.rs::volume_mount_path_uses_default_when_unset`, was
converted from process-environment loading to injected sources without changing
its count. Documentation tests are unchanged at 2 passed and 5 ignored.

```console
$ make test   # baseline, worktree at 6445bbe
Summary [ 161.466s] 460 tests run: 460 passed, 0 skipped
test result: ok. 2 passed; 0 failed; 5 ignored

$ make test   # migrated tree
Summary [   7.138s] 488 tests run: 488 passed, 0 skipped
test result: ok. 2 passed; 0 failed; 5 ignored
```

## Residual transitive versions

`ortho_config` itself is single-version. Older transitive versions remain for
reasons outside this migration:

- `reqwest` 0.11 is required by `scaleway-rs` 0.1.9, while Mriya depends on
  `reqwest` 0.12 directly.
- `thiserror` 1 and 2 coexist because `scaleway-rs` and some dev-dependencies
  still use 1.x.
- `toml` 0.5, 0.8, 0.9, and 1.1 appear through `scaleway-rs`, `figment`, the
  `toml` dev-dependency, and `ortho_config` respectively.
- `cap-std` 3 and 4 coexist because `rstest-bdd-macros` and the build script
  pull 3.x while the application uses 4.x.

None of these is an `ortho_config` version skew, and none is introduced by this
migration.
