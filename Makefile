.PHONY: help all clean test build release lint fmt check-fmt markdownlint nixie \
	typecheck scaleway-test scaleway-janitor test-workflow-contracts spelling \
	install-build-tools check-build-tools lint-clippy lint-whitaker


TARGET ?= mriya

CARGO ?= cargo
BUILD_JOBS ?=
RUST_FLAGS ?= -D warnings
RUSTDOC_FLAGS ?= --cfg docsrs -D warnings
# `--workspace` matters: with a root package, Cargo otherwise selects only
# that package and the `crates/` members escape every gate.
CARGO_FLAGS ?= --workspace --all-targets --all-features
CLIPPY_FLAGS ?= $(CARGO_FLAGS) -- $(RUST_FLAGS)
TEST_FLAGS ?= $(CARGO_FLAGS)
MDLINT ?= markdownlint-cli2
# `make fmt` and `make check-fmt` call mdtablefix directly. `--git` selects the
# Markdown files Git tracks and `--include-untracked` adds the untracked files
# Git does not ignore, so a new document is formatted before it is staged.
# Both modes need mdtablefix 0.6.0 or later; CI pins the version at the
# install-mdtablefix step.
MDTABLEFIX ?= mdtablefix
MDTABLEFIX_SELECT = --git --include-untracked
MDTABLEFIX_RULES = --wrap --renumber --breaks --ellipsis --fences
NIXIE ?= nixie
WHITAKER ?= whitaker
# Every uv route goes through the vendored helper, which runs uv from a
# cleaned environment and the global cache, offline first. The one exception
# is `spelling`: see TYPOS_CONFIG_BUILDER below.
UV_GATE ?= python3 scripts/uv_gate.py
UV ?= uv
RUFF_VERSION ?= 0.15.12
# The CV-005 CodeScene contracts live in shared-actions and run from a full
# commit, so a fix is a pin bump. `.github/cv005.toml` holds this repository's
# only parameters.
CV005_CONTRACTS_REF ?= 3754876bc4e94dc65c83af3def37828934f8f65e
CV005_CONTRACTS = $(UV_GATE) tool --python 3.13 \
	--from 'git+https://github.com/leynos/shared-actions@$(CV005_CONTRACTS_REF)\#subdirectory=packages/cv005-contracts' \
	-- cv005-contracts
# The pytest contracts that stay: the mutation-testing caller workflow.
WORKFLOW_CONTRACT_TESTS = tests/workflow_contracts
PYTEST_VERSION ?= 9.2.0
PYYAML_VERSION ?= 6.0.3
HYPOTHESIS_VERSION ?= 6.170.15
# The builder runs directly at a release tag, not through $(UV_GATE): the
# spelling-config-baseline rule requires that shape, and the helper refuses a
# Git spec that is not a full commit (the uv-gate-baseline release-tag exception).
TYPOS_CONFIG_BUILDER_VERSION ?= v0.1.3
TYPOS_CONFIG_BUILDER = $(UV) tool run --python 3.14 --from \
	"git+https://github.com/leynos/typos-config-builder.git@$(TYPOS_CONFIG_BUILDER_VERSION)" \
	typos-config-builder

# The development build standard (concordat rule `rust-build-defaults`):
# the parallel rustc frontend and, on Linux, the mold linker. There are three
# separate layers, and this file owns only the second:
#
# 1. Bare Cargo. `.cargo/config.toml` carries the flags, so `cargo build` gets
#    them with no Make involved.
# 2. Make's restatement. An assigned RUSTFLAGS replaces every `rustflags` table
#    in that file, so each development recipe composes these flags onto any
#    inherited value (CI's setup-rust exports one) itself.
# 3. The explicit non-development routes, which stay off the standard:
#    `release` assigns RUSTFLAGS without the flags; coverage (in CI) assigns its
#    own; and the Whitaker command in `lint` clears CARGO_ENCODED_RUSTFLAGS and
#    assigns the warning policy alone.
#
# The development routes (`build`, `test`, `lint`, `typecheck` and the Scaleway
# targets) depend on `check-build-tools`, which stops with an installation hint
# when `mold` or a C compiler that accepts -fuse-ld=mold is missing. It never falls
# back to another linker. `make install-build-tools` provides the pinned `mold` and
# toolchain. `release` needs neither, so it has no preflight.
BUILD_HOST_OS := $(shell uname -s)
STANDARD_RUSTFLAGS := -Zthreads=8$(if $(filter Linux,$(BUILD_HOST_OS)), -Clink-arg=-fuse-ld=mold)

# `-fuse-ld=mold` resolves the linker by PATH order, so the prefix `make
# install-build-tools` unpacks the pinned release into leads PATH for every
# recipe and outranks a distribution `mold`. A bare `cargo` outside Make needs
# the same directory on PATH.
BUILD_TOOLS_PREFIX ?= $(HOME)/.local
export BUILD_TOOLS_PREFIX
export PATH := $(BUILD_TOOLS_PREFIX)/bin:$(PATH)

build: target/debug/$(TARGET) ## Build debug binary
release: target/release/$(TARGET) ## Build release binary

all: check-fmt lint test spelling test-workflow-contracts ## Perform a comprehensive check of code and prose

clean: ## Remove build artefacts
	$(CARGO) clean

install-build-tools: ## Install the pinned mold linker and Rust toolchain
	@scripts/install-build-tools.sh

check-build-tools: ## Check the `mold` linker, C compiler and toolchain prerequisites
	@scripts/check-build-tools.sh

test: check-build-tools ## Run tests with warnings treated as errors
	$(CARGO) fetch --locked
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(RUST_FLAGS) $(STANDARD_RUSTFLAGS)" $(CARGO) nextest run $(TEST_FLAGS) $(BUILD_JOBS)
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(RUST_FLAGS) $(STANDARD_RUSTFLAGS)" RUSTDOCFLAGS="$(RUSTDOC_FLAGS)" $(CARGO) test --doc --workspace --all-features $(BUILD_JOBS)

test-workflow-contracts: ## Validate the CV-005 coverage contracts and the mutation-testing contract
	$(CV005_CONTRACTS) check --repository .
	$(UV_GATE) tool --python 3.13 --from 'pytest==$(PYTEST_VERSION)' --with 'pyyaml==$(PYYAML_VERSION)' \
		--with 'hypothesis==$(HYPOTHESIS_VERSION)' -- pytest $(WORKFLOW_CONTRACT_TESTS) -q

scaleway-janitor: check-build-tools ## Delete test-run Scaleway resources (requires MRIYA_TEST_RUN_ID)
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) run --bin mriya-janitor

scaleway-test: check-build-tools ## Run Scaleway integration tests with janitor sweep
	@command -v uuidgen >/dev/null 2>&1 || (echo "uuidgen is required" && exit 1)
	@command -v scw >/dev/null 2>&1 || (echo "scw is required" && exit 1)
	@MRIYA_TEST_RUN_ID="$$(uuidgen | tr '[:upper:]' '[:lower:]')" ; \
	echo "MRIYA_TEST_RUN_ID=$$MRIYA_TEST_RUN_ID" ; \
	trap 'MRIYA_TEST_RUN_ID="$$MRIYA_TEST_RUN_ID" RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) run --bin mriya-janitor > /dev/null' EXIT ; \
	MRIYA_TEST_RUN_ID="$$MRIYA_TEST_RUN_ID" RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) run --bin mriya-janitor ; \
	MRIYA_RUN_SCALEWAY_TESTS=1 MRIYA_TEST_RUN_ID="$$MRIYA_TEST_RUN_ID" RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) test --test scaleway_backend --test scaleway_cloud_init -- --test-threads=1

typecheck: check-build-tools ## Typecheck the workspace
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(RUST_FLAGS) $(STANDARD_RUSTFLAGS)" $(CARGO) check $(CARGO_FLAGS) $(BUILD_JOBS)

# The preflight is order-only and debug-only: it must not rebuild the binary, and
# a release build uses neither `mold` nor the frontend flag. The pattern rule below
# supplies the recipe.
target/debug/$(TARGET): | check-build-tools

target/%/$(TARGET): ## Build binary in debug or release mode
	$(if $(findstring release,$(@)),RUSTFLAGS="$${RUSTFLAGS-}",RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)") $(CARGO) build $(BUILD_JOBS) $(if $(findstring release,$(@)),--release) --bin $(TARGET)

# The repository's only Python is the mutation-testing workflow contract test
# under tests/. Interrogate scans each target recursively, subject to its
# default exclusions.
INTERROGATE_VERSION ?= 1.7.0
INTERROGATE_TARGETS ?= tests
# Level 2 prints a MISSED row per undocumented definition, which the
# `interrogate` target fails on: the rounded percentage alone can pass a lone gap.
INTERROGATE_FLAGS ?= -vv --fail-under 100
INTERROGATE ?= $(UV_GATE) tool --python 3.13 --from 'interrogate==$(INTERROGATE_VERSION)' -- interrogate

# Interrogate rounds its percentage to one decimal, so `--fail-under 100` alone
# can pass a lone gap in a large repository; fail on any MISSED row as well.
.PHONY: interrogate
interrogate: ## Enforce complete Python docstring coverage
	@report="$$($(INTERROGATE) $(INTERROGATE_FLAGS) $(INTERROGATE_TARGETS))"; \
	  status=$$?; \
	  printf '%s\n' "$$report"; \
	  test "$$status" -eq 0 || exit "$$status"; \
	  if printf '%s\n' "$$report" | grep -q '| *MISSED *|'; then \
	    printf '%s\n' 'interrogate: undocumented definitions remain; see the MISSED rows above' >&2; \
	    exit 1; \
	  fi

# `lint` is the composite gate: preflight, docstring gate, rustdoc and Clippy, then
# the Whitaker Dylint suite last. A bare prerequisite list is no ordering
# guarantee under `make -j`, so `.NOTPARALLEL` with a prerequisite makes GNU Make
# 4.4 or later build everything `lint` reaches serially, in listed order. The leaf
# targets stay runnable alone: `make lint-clippy` and `make lint-whitaker`. Every
# recipe is a plain command, so the gate's failure is the recipe's exit status,
# with no guard, ignored-error prefix or masking pipeline.
.NOTPARALLEL: lint
.PHONY: lint-clippy lint-whitaker
lint: lint-clippy lint-whitaker ## Run Clippy, the Whitaker Dylint suite and the docstring gate

lint-clippy: check-build-tools interrogate ## Run the docstring gate, rustdoc and Clippy
	RUSTDOCFLAGS="$(RUSTDOC_FLAGS)" RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) doc --workspace --no-deps
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) clippy $(CLIPPY_FLAGS)

# Whitaker runs on its installer-managed toolchain, so it takes the warning policy
# alone and none of the development flags (ADR-001 addendum). `CARGO_FLAGS`
# selects every workspace package: no member needs `rustc_private` plumbing.
lint-whitaker: ## Run the Whitaker Dylint suite over the workspace
	env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS="$(RUST_FLAGS)" $(WHITAKER) --all -- $(CARGO_FLAGS)

fmt: ## Format Rust and Markdown sources
	$(CARGO) fmt --all
	$(MDTABLEFIX) --in-place $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)
	$(MDLINT) --fix "**/*.md"

check-fmt: ## Verify formatting
	$(CARGO) fmt --all -- --check
	$(MDTABLEFIX) --check $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)

markdownlint: spelling ## Lint Markdown files and enforce spelling
	$(MDLINT) '**/*.md'

spelling: ## Enforce en-GB-oxendict spelling
	$(TYPOS_CONFIG_BUILDER) gate --repository . --scope all

nixie: ## Validate Mermaid diagrams
	$(NIXIE) --no-sandbox

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?##' $(MAKEFILE_LIST) | \
	awk 'BEGIN {FS=":"; printf "Available targets:\n"} {printf "  %-20s %s\n", $$1, $$2}'
