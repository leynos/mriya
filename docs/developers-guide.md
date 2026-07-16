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
