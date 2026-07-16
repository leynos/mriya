.PHONY: help all clean test build release lint fmt check-fmt markdownlint nixie \
	typecheck scaleway-test scaleway-janitor test-workflow-contracts spelling \
	spelling-phrase-check spelling-config spelling-config-write \
	spelling-helper-test


TARGET ?= mriya

CARGO ?= cargo
BUILD_JOBS ?=
RUST_FLAGS ?= -D warnings
CARGO_FLAGS ?= --all-targets --all-features
CLIPPY_FLAGS ?= $(CARGO_FLAGS) -- $(RUST_FLAGS)
TEST_FLAGS ?= $(CARGO_FLAGS)
MDLINT ?= markdownlint-cli2
NIXIE ?= nixie
WHITAKER ?= whitaker
UV ?= uv
UV_ENV = UV_CACHE_DIR=.uv-cache UV_TOOL_DIR=.uv-tools
RUFF_VERSION ?= 0.15.12
TYPOS_VERSION ?= 1.48.0
TYPOS_CONFIG_BUILDER_COMMIT := b604f198797fdd36a567dd0f8f07b13f9539b241
TYPOS_CONFIG_BUILDER_REPOSITORY := https://github.com/leynos/typos-config-builder.git
TYPOS_CONFIG_BUILDER_SOURCE := git+$(TYPOS_CONFIG_BUILDER_REPOSITORY)@$(TYPOS_CONFIG_BUILDER_COMMIT)
TYPOS_CONFIG_BUILDER := $(UV_ENV) $(UV) tool run --python 3.14 \
	--from "$(TYPOS_CONFIG_BUILDER_SOURCE)" typos-config-builder
CMD_MOX_COMMIT := f583c279a15760aba5cfd9bddf1fbbe9b1f8c429
CMD_MOX_REPOSITORY := https://github.com/leynos/cmd-mox
CMD_MOX_SOURCE := git+$(CMD_MOX_REPOSITORY)@$(CMD_MOX_COMMIT)
CYCLOPTS_VERSION := 4.21.1
PLUMBUM_VERSION := 2.0.1
TYPING_EXTENSIONS_VERSION := 4.15.0
SPELLING_PY_SRCS := scripts/typos_rollout_check.py \
	scripts/tests/conftest.py scripts/tests/test_typos_rollout_check.py
SPELLING_PY_TESTS := scripts/tests/test_typos_rollout_check.py
SPELLING_PY_ENV := PYTHONDONTWRITEBYTECODE=1
SPELLING_COVERAGE_FILE ?= /tmp/mriya-spelling-helper.coverage
SPELLING_HELPER_PYTEST := PYTHONPATH=scripts $(SPELLING_PY_ENV) \
	COVERAGE_FILE=$(SPELLING_COVERAGE_FILE) $(UV_ENV) $(UV) run --no-project \
	--python 3.13 --with 'cyclopts==$(CYCLOPTS_VERSION)' \
	--with 'pathspec==1.1.1' \
	--with 'plumbum==$(PLUMBUM_VERSION)' \
	--with 'cmd-mox@$(CMD_MOX_SOURCE)' \
	--with 'typing-extensions==$(TYPING_EXTENSIONS_VERSION)' \
	--with 'pytest==9.0.2' --with 'pytest-cov==7.0.0' python -m pytest

build: target/debug/$(TARGET) ## Build debug binary
release: target/release/$(TARGET) ## Build release binary

all: check-fmt lint test spelling ## Perform a comprehensive check of code and prose

clean: ## Remove build artefacts
	$(CARGO) clean

test: ## Run tests with warnings treated as errors
	RUSTFLAGS="$(RUST_FLAGS)" $(CARGO) test $(TEST_FLAGS) $(BUILD_JOBS)

test-workflow-contracts: ## Validate the mutation-testing caller contract
	uv run --with 'pytest>=8' --with 'pyyaml>=6' pytest tests/workflow_contracts -q

scaleway-janitor: ## Delete test-run Scaleway resources (requires MRIYA_TEST_RUN_ID)
	$(CARGO) run --bin mriya-janitor

scaleway-test: ## Run Scaleway integration tests with janitor sweep
	@command -v uuidgen >/dev/null 2>&1 || (echo "uuidgen is required" && exit 1)
	@command -v scw >/dev/null 2>&1 || (echo "scw is required" && exit 1)
	@MRIYA_TEST_RUN_ID="$$(uuidgen | tr '[:upper:]' '[:lower:]')" ; \
	echo "MRIYA_TEST_RUN_ID=$$MRIYA_TEST_RUN_ID" ; \
	trap 'MRIYA_TEST_RUN_ID="$$MRIYA_TEST_RUN_ID" $(CARGO) run --bin mriya-janitor > /dev/null' EXIT ; \
	MRIYA_TEST_RUN_ID="$$MRIYA_TEST_RUN_ID" $(CARGO) run --bin mriya-janitor ; \
	MRIYA_RUN_SCALEWAY_TESTS=1 MRIYA_TEST_RUN_ID="$$MRIYA_TEST_RUN_ID" $(CARGO) test --test scaleway_backend --test scaleway_cloud_init -- --test-threads=1

typecheck: ## Typecheck the workspace
	RUSTFLAGS="$(RUST_FLAGS)" $(CARGO) check $(CARGO_FLAGS) $(BUILD_JOBS)

target/%/$(TARGET): ## Build binary in debug or release mode
	$(CARGO) build $(BUILD_JOBS) $(if $(findstring release,$(@)),--release) --bin $(TARGET)

lint: ## Run Clippy and the Whitaker Dylint suite with warnings denied
	RUSTDOCFLAGS="$(RUSTDOC_FLAGS)" $(CARGO) doc --no-deps
	$(CARGO) clippy $(CLIPPY_FLAGS)
	RUSTFLAGS="$(RUST_FLAGS)" $(WHITAKER) --all -- $(CARGO_FLAGS)

fmt: ## Format Rust and Markdown sources
	$(CARGO) fmt --all
	mdformat-all

check-fmt: ## Verify formatting
	$(CARGO) fmt --all -- --check

markdownlint: spelling ## Lint Markdown files and enforce spelling
	$(MDLINT) '**/*.md'

spelling: spelling-phrase-check ## Enforce en-GB-oxendict spelling in tracked text
	@git ls-files -z | xargs -0 env $(UV_ENV) \
		$(UV) tool run typos@$(TYPOS_VERSION) \
		--config typos.toml --force-exclude --hidden

spelling-phrase-check: spelling-config ## Reject prohibited spelling phrases
	@PYTHONPATH=scripts $(SPELLING_PY_ENV) $(UV_ENV) $(UV) run --no-project \
		--python 3.13 scripts/typos_rollout_check.py --repository .

spelling-config: spelling-helper-test ## Check generated spelling configuration
	@git ls-files --error-unmatch typos.toml >/dev/null
	@$(TYPOS_CONFIG_BUILDER) --repository . --check

spelling-config-write: spelling-helper-test ## Regenerate spelling configuration
	@$(TYPOS_CONFIG_BUILDER) --repository .

spelling-helper-test: ## Validate the spelling phrase helper
	@$(UV_ENV) $(UV) tool run ruff@$(RUFF_VERSION) format --isolated \
		--target-version py313 --check $(SPELLING_PY_SRCS)
	@$(UV_ENV) $(UV) tool run ruff@$(RUFF_VERSION) check --isolated \
		--target-version py313 $(SPELLING_PY_SRCS)
	@$(SPELLING_HELPER_PYTEST) $(SPELLING_PY_TESTS) -c /dev/null \
		--rootdir=. -p no:cacheprovider --cov=typos_rollout_check --cov-fail-under=90

nixie: ## Validate Mermaid diagrams
	$(NIXIE) --no-sandbox

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?##' $(MAKEFILE_LIST) | \
	awk 'BEGIN {FS=":"; printf "Available targets:\n"} {printf "  %-20s %s\n", $$1, $$2}'
