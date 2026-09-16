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
