# Architectural decision record (ADR) 002: Read the process environment through `mockable::Env`

## Status

Accepted on 11 November 2026. Production code reads the process environment
only through `mockable::Env`; a value the program cannot decode is reported as
an error instead of being treated as absent.

## Date

2026-11-11.

## Context and problem statement

Several boundaries in Mriya need a value from the environment: tilde expansion
for `--cloud-init-file` and the SSH identity path, the test-run identifier the
Scaleway backend tags resources with, and the `MRIYA_FAKE_*` modes the CLI
behaviour suite uses. Before this decision, each read `std::env` directly.

That had three costs. Tests could only reach the failure branches by mutating
the process environment, which is unsound while other tests run in parallel. An
undecodable non-UTF-8 value was either silently converted lossily, which named
a path the user never asked for, or arrived as the same `Option::None` as an
absent one, so the user saw a file error naming no cause. And a reader of a
call site could not tell which environment variables a function actually
depended on.

## Decision drivers

- A test of production logic must not mutate the process environment.
- An undecodable value must be distinguishable from an absent one at every
  boundary that cares, and the error must name the file the user asked for.
- A missing environment must still fall through to the behaviour a shell
  would give: `ssh` resolves `~` itself when `HOME` is unset.
- Environment dependencies must be visible in a signature.

## Options considered

- Keep `std::env` and guard test mutations with a process-wide lock. This is
  what the removed `ENV_LOCK` and `EnvGuard` did. It serializes tests and
  points an arrow at whichever test forgot the guard.
- Resolve every environment value once at startup into configuration. This
  hides the dependency rather than making it injectable, and a test still
  cannot exercise the failure branches.
- Inject `&impl mockable::Env` at each boundary, with `DefaultEnv` at the
  production entry points. This is the option taken.

## Decision outcome

- `mockable::Env` is a regular and build dependency with default features off;
  only the dev-dependency enables `mock`.
- Each boundary takes `&impl Env`: `Syncer::build_ssh_args`,
  `Syncer::common_ssh_options`, `Syncer::build_remote_shell`,
  `ScalewayBackend::new_with_env`,
  `cloud_init::resolve_cloud_init_user_data_with_env`, and the build-standard
  sandbox's `find_on_path_with`. The public constructors keep their arity and
  supply `DefaultEnv`. The pre-existing `sync::expand_tilde` keeps its name,
  signature and arity, and supplies `DefaultEnv` itself. Its undecodable case
  is a behaviour change: the entry used to convert a non-UTF-8 `HOME` lossily,
  and now returns the path unchanged, which is what it already did for an absent
  `HOME`. It reports no error, because its signature carries none and predates
  this decision; production code takes `home_from_env` instead.
- `Env::raw` is used where absent and undecodable must differ;
  `sync::util::home_from_env` is the reference case. It reports
  `HomeUnavailable` for an undecodable `HOME` and `Ok(None)` for an absent one,
  so only the first is an error.
- A boundary that consumes `home_from_env`'s error names the value the user
  configured and what will work instead. `SyncError::HomeUnavailable` is a new
  variant carrying `path` for that reason, so the SSH diagnostic names the
  `ssh_identity_file` and offers an absolute one as the remedy rather than
  naming only the variable. `CloudInitError::HomeUnavailable` does the same for
  `--cloud-init-file`; neither variant existed before this decision.
- `Env::string` is used where a value that cannot be decoded is genuinely
  equivalent to none, as in the `MRIYA_FAKE_*` gates.
- Tests inject `mockable::MockEnv`, built by `test_helpers::mock_env` (one
  decodable table answered by both accessors) or `mock_env_with_raw` (a
  closure, the only way to express `NotUnicode`).

## Consequences

- A boundary that needs a new variable takes an `Env` parameter, which is a
  visible API change at that call site and a compile error until the caller
  supplies one.
- An undecodable `HOME` makes a `~/` cloud-init path or SSH identity path
  fail with a message naming the path, where the base converted the value
  lossily and acted on whatever the replacement character produced. The users'
  guide records this; the 0.2.0 migration note records it as a behaviour change.
- The build-standard sandbox distinguishes an unset `PATH` from an
  undecodable one for the same reason.
- `test_support`'s `EnvGuard` and `ENV_LOCK` are removed. A test that needs a
  process-level value uses `temp_env::with_var`, which is `unsafe` and
  serialized, or sets it on a spawned child.
