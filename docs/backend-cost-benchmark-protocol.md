# Backend cost benchmark protocol

This protocol defines how Mriya measures the cost of one completed build or
test gate on a cloud backend. It exists because a published VM-hour rate does
not establish what a finished gate costs: provisioning, cache restoration,
linking, and the provider's own billing granularity can each dominate a short
run, and nominal vCPU counts are not equivalent throughput.

The protocol is normative. A backend adapter may claim a measured cost
advantage only after an operator has run this protocol against it and published
the resulting records.

## 1. Scope

The protocol covers CPU workloads only. Graphics processing unit (GPU) work is
governed by section 9 and must never be ranked in the same table as a CPU
result.

The protocol does not authorize spending. Section 10 defines the operator
opt-in that a paid run requires.

## 2. Workload set

A campaign measures the same four workloads on every backend it compares. Each
workload pins its command, its input data, and the artefacts it is allowed to
inherit from the cache.

| Identifier         | What it measures                                                | Command shape                                     |
| ------------------ | --------------------------------------------------------------- | ------------------------------------------------- |
| `cold-build`       | Full dependency and workspace compile with an empty build cache | Workspace build with the target directory cleared |
| `warm-incremental` | Recompile after one source file's timestamp changes             | The same build with one touched source            |
| `link-heavy`       | Linking and final artefact production                           | A build forced to relink every test binary        |
| `test-execution`   | Representative test run over an already warm build              | The repository's own test command                 |

Every workload record names the exact command vector it ran. A campaign that
changes a command must start a new campaign identifier, because two different
commands are not the same completed work.

The commands the harness runs are below. Line continuations are shell
continuations; each workload is one command vector of `sh -c` and its script.

```sh
# cold-build
export CARGO_BUILD_JOBS=8
if [ -n "$CARGO_TARGET_DIR" ]; then rm -rf "$CARGO_TARGET_DIR"; \
  else rm -rf target; fi
cargo build --workspace --all-targets --all-features
```

```sh
# warm-incremental
export CARGO_BUILD_JOBS=8
touch src/lib.rs
cargo build --workspace --all-targets --all-features
```

```sh
# link-heavy
export CARGO_BUILD_JOBS=8
touch src/lib.rs
cargo test --workspace --all-features --no-run
```

```sh
# test-execution
export CARGO_BUILD_JOBS=8
cargo nextest run --workspace --all-targets --all-features
```

Each command runs under
`timeout --signal=TERM --kill-after=30 <declared lifetime in seconds>`, so an
overrunning workload is stopped and torn down. The wrapper is part of the
command vector the record pins, because it is part of what ran.

The warm-incremental workload assumes a Rust library crate with a `src/lib.rs`.
A different workload repository needs its own workload definitions and a new
campaign identifier. `test-execution` needs `cargo-nextest` on the guest; the
campaign's cloud-init must install it, and a missing toolchain shows up as a
failed attempt rather than as a fast one.

### 2.1 Input data

The input is the workload repository at a pinned commit. The record carries the
commit identifier, the repository remote, and a digest of the synchronized
tree. A dirty working tree is recorded as `dirty: true` and must not be
compared against a clean tree.

### 2.2 Cache contents

Each attempt declares the cache state it started from:

- `empty` — the cache location held nothing.
- `warm` — the cache location held a previous attempt's artefacts.
- `seed` — the cache location was populated from a declared seed artefact.
- `absent` — the backend cannot persist a cache at all.

The record carries the cache class, the measured cache size in bytes, and the
cache mount status. A comparison between two backends must state their cache
capability difference when the classes are not identical.

## 3. Pinned facts

Every run record carries the following pins. A record missing any of them is
invalid, and the validator reports each missing pin by name.

- Workload revision: repository remote, commit identifier, tree digest, and
  whether the tree was dirty.
- Toolchain: the compiler or runtime identifier and version, and the package
  manager identifier and version.
- Command: the exact argument vector, including any environment assignments
  that affect the build.
- Input data: the synchronized tree size in bytes and the file count.
- Target architecture: the architecture string the provider was asked for and
  the architecture the guest reported.
- Concurrency: the intended worker count and the effective parallelism the
  guest reported.
- Cache: the cache class, size in bytes, and whether it was mounted.
- Provider: the product name, the location or zone, and the resource class
  actually allocated.
- Allocation: central processing unit (CPU) count, random access memory (RAM)
  in bytes, and disk in bytes, as allocated rather than as advertised.

## 4. Phase boundaries

Each attempt records these phases separately. A phase starts when its
initiating call is entered and ends when that call returns, so the phases are
sequential and do not overlap. Time spent waiting outside a phase belongs to
the phase that caused the wait.

| Phase              | Starts                        | Ends                                         |
| ------------------ | ----------------------------- | -------------------------------------------- |
| `allocate`         | The create request is issued  | The provider returns a resource identity     |
| `bootstrap`        | The create call returns       | The guest reports ready for remote execution |
| `cache-restore`    | The cache preparation begins  | The cache is mounted or confirmed absent     |
| `sync`             | The workspace transfer begins | The transfer completes                       |
| `remote-execution` | The remote command starts     | The remote command returns                   |
| `cache-save`       | Cache persistence begins      | Persistence completes or is declined         |
| `teardown`         | The delete request is issued  | The provider confirms no resource remains    |

`teardown` is only `verified` when the provider listing no longer contains the
resource. A delete call that returns without a confirmation produces an
`unverified` teardown, which section 10 treats as a cleanup doubt.

A backend that persists its cache by leaving a volume attached needs no save
step. Such a backend records no `cache-save` phase, and the report says the
phase was not observed rather than inventing a zero-length one.

## 5. Repetition

Each workload repeats at least five times on each backend. The record set for
one workload and backend carries every repetition, including failures. The
report prints the minimum, median, and maximum, and refuses to rank on a single
repetition.

Warm-up runs that exist only to populate a cache are recorded, because they
cost money, and are excluded from the timing summary by their declared role. A
warm-up is declared as such in the campaign plan; the driver does not guess it.

A campaign with fewer than five completed repetitions per workload and backend
is a dry run. Its costs are real and are reported, but no per-gate figure from
it may be published as evidence, and the report says so.

## 6. Billing rules

Cost is computed from the recorded chargeable lifetime using the provider's own
rules. Each rule is loaded from a price sheet that cites its source.

A price sheet declares, per provider and product:

- The list price per unit time, in micro-units of the sheet's currency.
- The billing granularity, in seconds, at which billable time is rounded up.
- The minimum chargeable period, in seconds, applied to each separately billed
  resource.
- Any monthly price cap per resource.
- Any plan or subscription with included usage, and its marginal price beyond
  that usage.
- The price of retained storage, snapshots, and addresses, per unit time.
- Any one-off start charge.

Chargeable lifetime is computed with integer ceiling arithmetic: billable units
are the ceiling of the measured lifetime over the granularity, and the charged
lifetime is that product floored at the minimum period. No floating-point value
enters the computation.

The calculator applies that rule exactly once. A record carries the measured
lifetime and, separately, the chargeable lifetime its writer computed. When the
two agree the writer computed no chargeable figure, and the calculator applies
the provider's rules to the measured lifetime. When the chargeable figure is
longer, it is already billed and is priced as it stands: applying a granularity
to it again could round a minimum that is not a multiple of the granularity
(for example 17 seconds at a 4-second granularity) up to 20.

### 6.1 Published rules used by the example sheet

The sheet at `benchmarks/price-sheet.example.toml` cites these sources:

- Scaleway bills each separately billed resource for a minimum of 60 minutes,
  with distinct start and stop events per resource class, and bills a savings
  plan at its committed monthly rate whether or not it is consumed.
- Hetzner Cloud bills the minimum of the hourly rate multiplied by the hours
  used and the monthly cap, rounds each server's hourly usage up to a whole
  hour, and bills Primary IP addresses and snapshots separately.
- Boat bills per second only while a sandbox runs, includes a number of VM
  hours in each monthly plan, charges $0.056 per saved snapshot per day above
  the plan's included count, and charges $0.03–$0.07 for a start above the
  included allowance.
- Ubicloud bills per minute of virtual machine (VM) usage, pro-rates a public
  IPv4 address per minute, and bills machine images on their archive size.

### 6.2 Total-account and marginal cost

Two figures are reported for every result:

- Marginal cost is what one more identical gate costs once the plan, included
  usage, and subscription are already paid.
- Total-account cost is the whole bill attributable to running the campaign,
  including the minimum subscription, retained storage, snapshots, and
  addresses for the period they were held.

They differ whenever a plan includes usage or a subscription sets a floor. When
they are equal the report says so rather than printing one figure under two
names.

## 7. Equal completed work

Two results may be compared only when all of the following hold:

- The workload identifier and command vector are identical.
- The repository commit and tree digest are identical.
- The cache class is identical, or the difference is stated.
- Both attempts completed, or both failed in the same declared way.
- The target architecture is identical.

A failed attempt is reported with its cost and is never averaged into a
success. A campaign that cannot complete a workload on one backend reports the
failure rather than substituting a different workload.

## 8. Capability differences

Where backends differ in a way that materially changes the measurement, the
report names it. At minimum:

- Cache persistence: whether the cache survives the resource being destroyed,
  and what it costs to retain.
- Cache scope: whether the provider's snapshot mechanism captures the toolchain
  cache directory at all.
- Concurrency: whether the provider shares physical CPU with other tenants.
- Storage: the disk capacity and class actually allocated.

A comparison that silently treats these as equivalent is invalid.

## 9. GPU experiments

A GPU experiment is recorded in its own campaign with its own class marker, and
never appears in a CPU ranking. A GPU record carries the accelerator model, the
accelerator count, the accelerator memory in bytes, the driver version, the
runtime version, and the result of the workload's correctness check. A GPU
result without a passing correctness check is recorded as a failure.

## 10. Safety

Paid execution requires all of the following, and refuses to start otherwise:

- The operator sets the opt-in variable `MRIYA_BENCH_ENABLE_PAID_RUNS` to
  `1`.
- The operator declares, in the campaign file, the maximum spend, the maximum
  number of attempts, and the maximum lifetime of any one attempt.
- The provider credentials are present.

A campaign runs one instance at a time. Before it allocates anything it sweeps
for residual resources carrying the campaign's tag and stops if any remain; the
backend tags the resources it creates with that same campaign tag, so the sweep
and the resources agree. It appends each record immediately after the attempt
it describes and flushes before reporting success, so an interrupted campaign
leaves a readable prefix.

On any cleanup doubt — a failed delete, an unverified teardown, or a resource
listing that cannot be read — the campaign appends a residual record naming
every identifier it could not verify gone and stops. A retained cache volume is
never deleted by a campaign teardown, and it is tagged outside the campaign tag
so it cannot be swept.

## 11. Adapter conformance checklist

A backend adapter passes conformance when every item holds:

- It reports the resource class, location, and allocated CPU, RAM, and disk.
- It exposes the provider identity of every resource it creates, so a residual
  record can name them.
- It reports cache mount status and cache size, or declares that it cannot.
- It reports whether teardown was verified by a listing.
- It records the guest-reported architecture and parallelism.
- It refuses to run when the opt-in or any declared limit is absent.
- It never deletes a retained cache volume during teardown.
- Its price-sheet entry cites a source for every rate, granularity, minimum, and
  cap it declares.

For Scaleway specifically, conformance depends on
[#153](https://github.com/leynos/mriya/issues/153)–[#155](https://github.com/leynos/mriya/issues/155)
landing: reliable Block Storage lifecycle, verified teardown, and an explicit
boot-volume type and size are prerequisites for measuring storage honestly.

## 12. Result format

Results are JSON Lines. Each line is one object with a `schema` version and a
`kind` tag. The kinds are defined in `src/bench/record.rs`; this section states
what a reader must be able to rely on.

Quantities are integers: durations in nanoseconds, sizes in bytes, and money in
micro-units of the sheet's currency. A record never carries a credential, a
token, or a private key.

A campaign file is valid when it has a header record, at least one run record,
and no record that contradicts the protocol. The validator reports every defect
it finds rather than stopping at the first.
