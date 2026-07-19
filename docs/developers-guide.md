# Developers' guide

This guide records development practices specific to maintaining mriya. Follow
the project-wide guidance in `AGENTS.md` first.

## Spelling policy

Run `make spelling` to enforce en-GB-oxendict spelling in tracked text. The
generated `typos.toml` starts from the shared estate dictionary, refreshes its
untracked local cache only when the authority is newer, and then applies the
narrow repository policy in `typos.local.toml`. Edit the local policy and
regenerate the configuration rather than changing generated entries by hand.

The focused `typos-config-builder` CLI is pinned to commit
`b604f198797fdd36a567dd0f8f07b13f9539b241` and runs in an isolated Python 3.14
environment. Use `make spelling-config-write` to refresh the cache and output,
or `make spelling-config` to check them without changing tracked files. The
repository's phrase helper remains on Python 3.13, follows the scripting
standards in this repository, and scans all eligible tracked text. Its test
environment supplies `typing-extensions` explicitly at the version pinned by
the Makefile's `TYPING_EXTENSIONS_VERSION` because the pinned `cmd-mox` revision
imports `typing_extensions.Self` without declaring that compatibility
dependency. This test-only package does not belong in the helper's runtime
metadata or the project dependency lock.

## Mutation-testing workflow contract tests

This repository runs scheduled, informational mutation testing through a thin
caller workflow, [`.github/workflows/mutation-testing.yml`][mutation-workflow],
which delegates to the shared reusable workflow
`leynos/shared-actions/.github/workflows/mutation-cargo.yml`. The heavy
lifting — running `cargo-mutants`, sharding, and summarizing survivors —
lives in `shared-actions`; this repository carries only declarative
configuration. The run is **informational only**: it never gates a pull
request. Survivors are reported through the job summary and downloadable
artefacts so they can be triaged into tests, not enforced as a blocking
check.

The workflow runs in two modes. A **daily schedule** fires a change-scoped
run that mutates only the source files touched within the detection window,
so quiet days are cheap no-ops. A **manual dispatch** (the Actions "Run
workflow" control) mutates the whole crate; select a branch in that control
to exercise a feature branch.

The caller passes two configuration inputs, each carrying intent:

- `exclude-globs` — `src/test_support.rs`, test scaffolding compiled into
  the library unconditionally (unlike the `#[cfg(test)]`-gated
  `test_helpers` and `tests` modules, which `cargo-mutants` skips already),
  whose surviving mutants would otherwise be noise in the survivors table.
- `extra-args` — `--all-features`, matching the `make test` CI baseline so
  the `test-backdoors`-gated tests run against mutants rather than being
  silently skipped.

The `uses:` reference pins the shared workflow to a full 40-character commit
SHA rather than a branch or tag, so a force-push upstream cannot silently
change what runs here. The contract test asserts only that the pin is a
full commit SHA, not a particular value, so Dependabot bumps it
automatically without any accompanying test edit.

Because the caller is configuration rather than code, a contract test pins
the shape it must uphold, failing the pull request when the caller drifts —
repointing the pin at a branch, widening the token scope, or dropping a
configuration input — rather than letting the breakage surface only in a
scheduled run. Run it locally with `make test-workflow-contracts`. The test
validates:

- the `uses:` reference targets `mutation-cargo.yml` pinned to a full commit
  SHA;
- the `with:` block carries exactly the expected configuration (the exclude
  and feature arguments above);
- job permissions are least-privilege (`contents: read`, `id-token: write`)
  and the workflow-level default token scope is empty;
- `concurrency` serializes runs per ref without cancelling one in progress;
  and
- the triggers keep the daily schedule and a plain `workflow_dispatch` with
  no legacy branch input.

[mutation-workflow]: ../.github/workflows/mutation-testing.yml
