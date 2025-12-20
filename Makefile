.PHONY: help all clean test build release lint fmt check-fmt markdownlint nixie typecheck scaleway-test scaleway-janitor


TARGET ?= mriya

CARGO ?= cargo
BUILD_JOBS ?=
RUST_FLAGS ?= -D warnings
CARGO_FLAGS ?= --all-targets --all-features
CLIPPY_FLAGS ?= $(CARGO_FLAGS) -- $(RUST_FLAGS)
TEST_FLAGS ?= $(CARGO_FLAGS)
MDLINT ?= markdownlint-cli2
NIXIE ?= nixie

build: target/debug/$(TARGET) ## Build debug binary
release: target/release/$(TARGET) ## Build release binary

all: check-fmt lint test ## Perform a comprehensive check of code

clean: ## Remove build artifacts
	$(CARGO) clean

test: ## Run tests with warnings treated as errors
	RUSTFLAGS="$(RUST_FLAGS)" $(CARGO) test $(TEST_FLAGS) $(BUILD_JOBS)

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

lint: ## Run Clippy with warnings denied
	RUSTDOCFLAGS="$(RUSTDOC_FLAGS)" $(CARGO) doc --no-deps
	$(CARGO) clippy $(CLIPPY_FLAGS)

fmt: ## Format Rust and Markdown sources
	$(CARGO) fmt --all
	mdformat-all

check-fmt: ## Verify formatting
	$(CARGO) fmt --all -- --check

markdownlint: ## Lint Markdown files
	$(MDLINT) '**/*.md'

nixie: ## Validate Mermaid diagrams
	$(NIXIE) --no-sandbox

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?##' $(MAKEFILE_LIST) | \
	awk 'BEGIN {FS=":"; printf "Available targets:\n"} {printf "  %-20s %s\n", $$1, $$2}'
