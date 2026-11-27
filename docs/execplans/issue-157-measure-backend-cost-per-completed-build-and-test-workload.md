# Measure backend cost per completed build and test workload

This ExecPlan is a living document. The sections `Progress`,
`Surprises & Discoveries`, `Decision Log`, and `Outcomes & Retrospective` must
be kept up to date as work proceeds.

This document must be maintained in accordance with `AGENTS.md` and the
ExecPlan rules in the `execplans` skill.

## Purpose / Big Picture

Published VM-hour rates do not establish what a completed build or test gate
costs. The earlier Boat-versus-Scaleway comparison assumed equal elapsed
lifetimes, so it priced a claimed machine size rather than measured throughput,
cache restoration, linking, or provisioning overhead. Boat documents shared
vCPUs and provider-specific snapshot exclusions, and Mriya already routes build
caches to a persistent volume, so cache and storage capability change the
answer as much as the nominal vCPU count.

After this change an operator can run one explicitly enabled, repeatedly timed
campaign of four pinned CPU workloads against a provider, keep every attempt's
record — successes and failures — in one JSON Lines file, validate that file
offline, and price it with a dated, source-cited price sheet. Two providers can
then be compared on equal completed work with cache and storage capability
differences stated rather than assumed away.

The observable success is:

- `mriya-bench validate results.jsonl` reads a campaign file and reports every
  defect it finds, including a file with no run records, without contacting any
  provider.
- `mriya-bench report results.jsonl --price-sheet prices.toml` prints, offline,
  a marginal and total-account cost per completed gate for each provider and
  workload, with uncertainty and any cache-capability difference named.
- `mriya-bench run --campaign campaign.toml --out results.jsonl` refuses to
  start unless the operator has both enabled paid runs and declared the
  resource limits; on a cleanup doubt it appends a residual-resource record and
  stops the campaign instead of continuing.
- `make test`, `make lint`, `make check-fmt`, `make markdownlint` and
  `make spelling` pass, and no ordinary test or CI job ever provisions a cloud
  resource.

## Constraints

- No ordinary test, and no CI job, may provision a paid resource or launch a
  cloud comparison. Paid execution is reachable only through `mriya-bench run`
  with an explicit opt-in variable plus operator-declared limits.
- `mriya run` behaviour, its return values and its phase order must not change.
  The benchmark observation seams are additive and default to a no-op.
- The `Backend` port is not extended for instrumentation. A lifecycle sink is
  constructor-injected into `ScalewayBackend` only, so alternate backends and
  every existing caller are untouched.
- Records are JSON Lines, one object per line, tagged by `kind`, versioned by a
  single `schema` field, and written with integer units (nanoseconds, bytes,
  micro-units of currency) so no float enters a persisted format.
- Credentials never enter a record: record construction takes identifiers and
  measurements only, and the validator rejects any field that looks like a
  secret.
- Cleanup is fail-closed. A failed or unverified teardown appends a record that
  names the residual provider IDs and ends the campaign; the retained cache
  volume is never deleted by a campaign teardown.
- Every interpolated shell value is escaped; the existing escaping helpers are
  reused rather than duplicated.
- No single code file may exceed 400 lines.
- All comments and documentation use en-GB-oxendict spelling.

## Tolerances (Exception Triggers)

- If a milestone would need more than 3,000 net lines across production and test
  sources, stop and reassess the decomposition.
- If a provider billing rule cannot be established from that provider's
  published documentation, record the uncertainty and carry it as a bounded
  interval rather than guessing a rule.
- If a deterministic gate fails twice for the same root cause, stop and record
  the diagnosis before changing the design.
- If a probe command's output format differs from the documented shape on the
  pinned base image, stop and record the difference rather than widening the
  parser speculatively.
- ExecPlan files do not count towards any size tolerance.

## Risks

- Scaleway bills each separately billed resource for a minimum of 60 minutes,
  so a 90-second gate priced per second understates cost by up to 40×.
  `billing.rs` therefore applies a provider-declared granularity and a
  provider-declared minimum period, and reports the chargeable lifetime it used.
- Hetzner rounds each server's hourly usage up to a whole hour and caps at the
  monthly price, so a campaign that runs many short servers pays per server,
  not per second. The price sheet models a per-resource minimum and a monthly
  cap.
- Boat bills per second while a sandbox runs and includes VM time in a monthly
  plan, so marginal cost is zero until the plan's included hours are exhausted.
  The report therefore prints marginal and total-account costs as separate
  figures rather than one number.
- Shared vCPUs make elapsed time an unstable proxy for throughput. Each
  workload repeats, the report prints the spread, and the protocol forbids
  ranking providers on a single repetition.
- A probe that parses provider or toolchain output can break silently when the
  output changes. Probes fail closed with a typed parse error, and the protocol
  records the exact probe commands so a future reader can re-check them.

## Conformance basis

Upstream: GitHub issue `leynos/mriya#157`, "Measure backend cost per completed
build and test workload", including its Coding Plan (26 November 2026).
Dependencies `#153`–`#155` are open at the time of writing and cover reliable
Scaleway storage and lifecycle measurements; the protocol is written so that
each adapter must satisfy it before claiming a measured advantage.

Provider contracts reviewed 26 November 2026:

- Scaleway billing FAQ, <https://www.scaleway.com/en/docs/billing/faq/>: each
  resource is billed separately for a minimum of 60 minutes, with per-class
  start and stop events, and savings plans bill a committed monthly rate.
- Hetzner Cloud billing FAQ, <https://docs.hetzner.com/cloud/billing/faq/>:
  hourly price rounded up to whole hours, capped at the monthly price; Primary
  IPs billed separately, servers billed while they exist regardless of power
  state; snapshots billed per GB-month.
- Boat pricing, <https://docs.boat.dev/pricing>: per-second billing while a
  sandbox runs, plan-included VM hours, saved snapshots above the included
  count at $0.056 per snapshot-day, extra starts $0.03–$0.07.
- Boat machines, <https://docs.boat.dev/machines>: four sizes with shared
  vCPUs, RAM billed while the sandbox runs, and no promised CPU model.
- Boat snapshot contents, <https://docs.boat.dev/snapshots/whats-saved>:
  `~/.cache` is not saved except Playwright and Deno caches; Docker build cache
  and untagged images are not saved.
- Ubicloud pricing, <https://www.ubicloud.com/docs/about/pricing>: per-minute
  billing, public IPv4 pro-rated per minute, machine images billed on archive
  size at per-minute granularity.
- Mriya user guide, `docs/users-guide.md`: persistent cache routing and the
  one-instance attachment constraint.

Trace chain: `#157-AC-pins` -> `EP-M1` -> validator field checks and protocol
pinning table; `#157-AC-workloads` -> `EP-M1` and `EP-M2` -> four workload
definitions and observer phase events; `#157-AC-phases` -> `EP-M2` -> phase
records around the named calls; `#157-AC-allocation` -> `EP-M2` -> hardware,
cache and toolchain probes; `#157-AC-billing` -> `EP-M1` -> integer billing
calculator and price-sheet tests; `#157-AC-total-vs-marginal` -> `EP-M1` and
`EP-M3` -> plan-aware cost report; `#157-AC-equal-work` -> `EP-M3` ->
equal-work comparison and capability differences; `#157-AC-gpu` -> `EP-M1` ->
GPU class separation and accelerator pins; `#157-AC-opt-in` -> `EP-M3` ->
opt-in and limit refusal paths.

## Verification plan

Invariants:

1. A record carries every pin the protocol requires, and a missing pin is a
   named defect rather than a silently defaulted value.
2. A campaign file always contains at least one run record; a file with only
   headers, or no records at all, is reported as defective.
3. Chargeable lifetime is computed with integer ceiling arithmetic at the
   provider's declared granularity and floored at the provider's declared
   minimum period, so no input can produce a charge below the minimum.
4. Total-account cost is never below marginal cost for the same inputs, and
   they differ exactly when a plan includes usage or a subscription covers it.
5. A campaign refuses to start paid work unless both the opt-in variable and
   every declared limit are present and well-formed.
6. A cleanup doubt produces a residual record naming the IDs that could not be
   verified gone, and no further attempt starts after it.
7. Existing `mriya run` results are unchanged: the no-op observer leaves every
   return value and phase order identical.

Methods and artefacts:

- Invariants 1–2: `src/bench/validate.rs` with valid and defective fixtures
  under `src/bench/fixtures/`, driven by `rstest` case tables that name the
  expected complaint set. Negative control: a fixture with a valid run record
  must report no complaint, so a validator that complains unconditionally fails.
- Invariant 3–4: `src/bench/billing.rs` unit and `rstest` tables for published
  provider rules, plus a `proptest` oracle that recomputes the charge with an
  independent formulation (integer division plus remainder adjustment) and
  checks the minimum and monotonicity properties over generated inputs.
  Negative control: a rule with a zero granularity must be rejected rather than
  dividing by zero.
- Invariant 5–6: `src/bench/campaign.rs` with injected collaborators, driven by
  doubles that record calls and can fail teardown, refusing allocation and
  appending the residual record. Negative control: with the opt-in variable
  absent, no allocation collaborator is called at all.
- Invariant 7: the existing `tests/run_behaviour.rs` scenarios continue to pass
  unchanged; the no-op observer is additionally asserted to emit no record.
- Offline report: `src/bench/report.rs` tests read the Phase 1 fixtures and
  assert the rendered comparison, including the cache-capability note.

Axioms: provider billing rules, provider machine specifications, and probe
output shapes are external contracts. The repository-owned arithmetic and
parsing are tested against them; the campaign tests are the only proof against
a real provider, and they are opt-in.

## Context and orientation

`src/run.rs` holds `RunOrchestrator`, which creates an instance, waits for
readiness, mounts an optional cache volume, syncs, optionally waits for
cloud-init, runs one remote command, and destroys the instance.
`src/backend.rs` defines the `Backend` port and `src/scaleway/mod.rs`
implements it. `src/sync/` owns rsync and SSH execution plus the cache-routing
preamble. `src/janitor/` deletes test-run-tagged Scaleway resources by tag.
`src/lib.rs` exports the library surface. The Makefile owns the gate routes and
`docs/` holds the user, developer, design and roadmap documents.

The benchmark work adds a sibling `src/bench/` module, a `mriya-bench` binary
under `src/bin/`, and a protocol document under `docs/`. It touches
`src/run.rs` and `src/scaleway/mod.rs` only to add observation seams that
default to no-ops.

## Plan of work

Milestone `EP-M1` (offline contract): add the protocol document, the record
types, the validator, the billing calculator and the example price sheet. End
state: a campaign file can be validated and priced with no network access.

Milestone `EP-M2` (observation seams): add the run observer, the Scaleway
lifecycle sink, benchmark-only pre- and post-workload hooks, and the probe
parsers. End state: an observer records every phase separately and `mriya run`
behaves exactly as before.

Milestone `EP-M3` (gated campaign and report): add the `mriya-bench` binary,
the campaign harness with fail-closed cleanup, the offline equal-work report,
the Makefile routes and the documentation updates. End state: an operator can
run a declared campaign and produce a comparison, and nothing paid is reachable
from `make test` or CI.

## Progress

- [x] 2026-11-26: Reconnaissance, provider research and ExecPlan draft.
- [x] 2026-11-26: EP-M1 implemented: protocol document, versioned records,
  validator with valid and defective fixtures, integer billing calculator, and
  the source-cited example price sheet. 60 unit tests pass.
- [x] 2026-11-26: EP-M2 implemented: `RunObserver` with a no-op default,
  `LifecycleSink` injected into `ScalewayBackend`, pre- and post-workload probe
  hooks, and the probe parsers. `tests/bench_observer.rs` proves the phase
  order and that a failed teardown is reported as failed.
- [x] 2026-11-26: EP-M3 implemented: `mriya-bench` with `validate`, `report`
  and paid `run`; the fail-closed campaign harness; the offline equal-work
  report; the Makefile routes; and the user, developer, design and roadmap
  updates. 113 unit tests, 10 CLI tests and 4 observer tests pass.
- [x] 2026-11-26: Independent review of the whole diff; its findings are
  recorded under `Surprises & Discoveries` and were all remediated.
- [x] 2026-11-27: `make check-fmt`, `make lint`, `make markdownlint` and
  `make spelling` pass.
- [ ] `make test` passes on the final tree.
- [x] 2026-11-27: First CodeRabbit review produced 44 findings (17 major). All
  were addressed: a campaign lock, a guest-side lifetime bound, per-attempt
  record identity, retained-storage pricing, per-provider subscription
  attribution, separate groups per cache class, a zero-exchange-rate refusal, a
  single rounding of a billed lifetime, a bounded hook contract, and the
  documentation and test corrections the review named.
- [x] 2026-11-27: Second CodeRabbit review produced 30 findings. All were
  addressed: a campaign lock guard built before its contents are written,
  unique attempt identities enforced by the plan and the validator, phase
  references checked against runs, a whole-attempt deadline with a teardown
  grace, allocation uncertainty carried into the residual, the plan allowance
  valued in money and weighted by product rate, retained storage priced as
  Block Storage, per-provider subscription attribution across all workloads,
  toolchain comparability, structured subscription errors, and the remaining
  documentation and test corrections.
- [x] 2026-11-27: Third CodeRabbit review produced 21 findings, all addressed:
  a thiserror-derived charge error, a zero-granularity guard, a stable lock
  directory, residual identifiers preserved through write failures, teardown
  verified before a write failure is reported, a driver-served-provider check
  before the sweep, completed-only subscription shares, empty run-id and
  complaint-order fixes, volume allocation uncertainty, an exclusive output
  claim, the remaining API contracts, the June 2026 Block Storage rate, the
  mtime-only workload wording, and compile-time fixtures.
- [x] 2026-11-27: Fourth CodeRabbit review produced 19 findings, all addressed:
  an absolute attempt deadline shared by every remote command with a
  teardown-only grace, a reconciliation sweep for an expired attempt, a
  deadline-bounded command runner that kills a stalled `ssh` or `rsync`, an
  `unpriced_extras` declaration so a report states which charges its totals
  exclude, plan-baseline validation, zero-granularity rejection on the billed
  path, residual identifiers for a reason-only doubt, warm-ups excluded from
  comparability, and the remaining documentation, ordering, and test fixes.
- [x] 2026-11-27: Confirmation CodeRabbit review produced 14 findings (and one
  unverified finding). All verified findings were addressed: per-attempt
  flushing, no residual record for a resource-free failure, collision-resistant
  lock names, untracked files included in the tree pin, warm-ups excluded from
  the repetition warning, `cache_mounted` validation, execution statistics
  restricted to measured phases, exact plan allowance accounting, teardown
  excluded from the enclosing phase, definite provider rejections no longer
  reported as uncertain allocations, bounded forwarder cleanup, the Block
  Storage comment and source, and upper-bound labelling for a windowed start
  allowance.
- [x] 2026-11-27: `make test` passes on the final tree: 696 tests and the
  doctests.
- [x] 2026-11-27: Draft pull request #283 opened and linked to the Lody
  session.
- [x] 2026-11-27: Rebased onto `origin/main` at `c903234` from the exclusive
  boundary `200563a`, with no conflicts. The two new `main` commits bump the
  nightly toolchain to 2026-11-26 and the shared GitHub Actions, and introduce
  no shared helper or pattern the benchmark needs. All 20 branch commits
  replayed unchanged (`range-diff` reports `=` throughout), and
  `make check-fmt`, `make typecheck`, `make test` (696 tests plus doctests) and
  `make lint` pass on the rebased result.

## Surprises & Discoveries

- The issue's dependency range `#153`–`#155` is not merged on `main`; the
  Block Storage migration lives on its own branch. The protocol therefore names
  the storage facts it needs instead of assuming the migration's internals.
- Hetzner rounds hourly usage up to a whole hour per server and caps at the
  monthly price, so the billing model needs both a granularity and a cap, not
  just a rate.
- Boat includes VM hours in a monthly plan, which makes marginal cost zero
  until the plan's included hours run out; that is exactly the case the issue
  says must be reported separately from total-account cost.
- Boat does not save `~/.cache` in a snapshot, except Playwright and Deno, so a
  tool cache stored there does not survive a Boat stop unless it is placed under
  `/home/user`. Cargo's default `~/.cargo` sits inside the home directory,
  which Boat documents as saved, so Cargo data does not need relocating for
  that exclusion. The protocol names both locations rather than treating them
  as one.
- A provider's minimum chargeable period need not be a multiple of its billing
  granularity. A property test assumed it was and failed on a 17-second minimum
  at a 4-second granularity; the implementation was right and the property was
  wrong, so the test now allows either a whole number of granularity units or
  exactly the declared minimum.
- The shared spelling gate prefers the British `artefact` and the Oxford
  `-ize`, so a blanket substitution corrupted Cargo's `compiler-artifact`
  message name, which the repository already whitelists in `typos.local.toml`;
  protocol literals must be restored after any spelling pass.
- Whitaker enforces a 400-line module budget that counts doc comments, so the
  benchmark modules had to be split by responsibility rather than by file size
  target: `record/run.rs`, `record/pins.rs`, `billing/{money,sheet/pricing}.rs`,
  `probe/{report,cargo}.rs`, `validate/run_checks.rs`,
  `report/{render,rules}.rs`, and `campaign/{plan,ports,runner,scaleway_*}.rs`.
- A campaign's plan price is charged once per campaign, not once per attempt.
  Attributing it per run would multiply the subscription by the number of
  repetitions and make the total-account figure meaningless.
- The paid driver's `run_attempt` must return phase records as well as the run
  record. An observer that captured durations and then discarded them would
  leave the harness with phase names and no measurements.
- The Scaleway adapter tags resources from `MRIYA_TEST_RUN_ID`, while the
  campaign swept `mriya-test-run-<campaign_id>`. Unless the two agree, the
  fail-closed pre-sweep can never find the campaign's own leftovers. The driver
  now constructs the backend with the campaign id as its test run id, so the
  tag it creates and the tag it sweeps are the same string.
- A failed attempt is a cleanup doubt, not merely a failed measurement: it may
  have allocated before it failed. The harness now writes a residual record
  naming the attempt's provider and any identifiers the lifecycle sink saw,
  then stops.
- A provider's `month_seconds` must actually be used; a sheet that declared a
  shorter month was silently priced against 30 days.
- A record's chargeable lifetime is the figure the provider bills, so the report
  prices from it rather than from the wall-clock lifetime. Re-rounding is not
  idempotent: a one-second measurement at a four-second granularity with a
  seventeen-second minimum yields seventeen billed seconds, and rounding that
  again would yield twenty. A recorded billed lifetime is therefore priced
  unchanged apart from the provider's minimum floor, and only a record whose
  two lifetimes agree is rounded.
- A plan's monthly price is charged once per campaign, but a reader still needs
  to see each workload's share. The report divides the subscription across the
  groups in proportion to their timed attempts and gives the remainder to the
  first group, so the shares always add back to the total.
- The trybuild UI fixture can exceed nextest's 600-second per-test allowance
  when the host's compile queue is saturated. Running
  `cargo test -p mriya-test-macros --test ui` once warms the fixture tree
  outside that limit; the later `make test` passes in about 40 seconds. This is
  a host-load effect, not a defect in the change.

## Decision log

- Persist nanoseconds and micro-units as integers rather than floats: the
  workspace denies float arithmetic outright, and an integer record cannot
  drift between two readers.
- Put the protocol first and make the validator and calculator cloud-free, so
  the contract can be reviewed and tested before any adapter can claim a
  measured advantage.
- Inject the lifecycle sink into `ScalewayBackend` rather than widening the
  `Backend` port, because only the Scaleway adapter needs provider IDs and the
  port is shared with future adapters.
- Keep the paid route out of every aggregate Make goal, because a default goal
  that can spend money is a hazard regardless of how it is guarded.
- Rebase onto `main` rather than merging it: the branch is linear and shares
  its whole base with `main`, so a replay keeps the reviewed commit series and
  leaves no merge commit to explain. Recovery refs preserve the pre-rebase
  head, boundary and target until the rebased branch is accepted.

## Outcomes & Retrospective

Shipped: the protocol, the versioned JSON Lines records, the offline validator,
the integer billing calculator with a source-cited example sheet, the
observation seams, the probe parsers, the gated campaign harness, the offline
equal-work report, the `mriya-bench` binary, the Make routes, and the user,
developer, design, and roadmap documentation.

Deferred: no live campaign was run, because it spends money and the operator's
opt-in is deliberately not something an agent can give itself. The Scaleway
adapter therefore satisfies the protocol in code, and its conformance claim
stays unproven until an operator runs the example campaign with a real price
sheet. The storage-dependent parts of the checklist also wait on #153–#155.

Assumptions that survived: a provider states its granularity and minimum period
independently; a plan's monthly price is a campaign-level cost; and a benchmark
record can be validated and priced without a network. Assumptions corrected
during the work: a failed attempt is a cleanup doubt even when the failure is
not in the teardown; the campaign's sweep tag must be the tag its resources
carry; and a record's chargeable lifetime, not its wall-clock lifetime, is what
the provider bills.

Residual risk: the paid driver is exercised only through doubles, because a
real run needs credentials and money. Its safety policy is tested; its provider
calls are the same ones `mriya run` already uses. The first review also showed
that a plan's start allowance on Boat is a rate limit rather than a period
total; the example sheet says so and charges every start, which overstates
Boat's cost rather than understating it.

## Revision note

Initial draft created 26 November 2026 from issue #157 and its Coding Plan,
after verifying the Scaleway, Hetzner, Boat and Ubicloud billing and capability
documentation.

Revision 2 (27 November 2026) records the implemented milestones, the review
findings and their remediation, the module splits, and the gate evidence.
