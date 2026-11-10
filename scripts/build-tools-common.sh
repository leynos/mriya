#!/usr/bin/env bash
# Shared helpers for the build tools the development build standard needs.
#
# Sourced by scripts/install-build-tools.sh and scripts/check-build-tools.sh so
# both resolve the pins and word their diagnostics the same way. Every message
# goes to stderr with a `build-tools:` prefix, so a caller can capture a helper's
# stdout without the diagnostics contaminating it.
#
# These are shell rather than Python (docs/scripting-standards.md) on purpose:
# the check runs before every build and must work on a host that has nothing
# but a shell, `rustup` and a C compiler.

set -euo pipefail

# Locate the repository from this file, not the working directory, so the
# scripts behave the same when invoked directly and through `make`.
BUILD_TOOLS_REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

# Overridable so a test can point the scripts at fixtures.
LINKER_VERSION_FILE="${LINKER_VERSION_FILE:-$BUILD_TOOLS_REPO_ROOT/tools/mold/VERSION}"
LINKER_SHA256SUMS_FILE="${LINKER_SHA256SUMS_FILE:-$BUILD_TOOLS_REPO_ROOT/tools/mold/SHA256SUMS}"
RUST_TOOLCHAIN_FILE="${RUST_TOOLCHAIN_FILE:-$BUILD_TOOLS_REPO_ROOT/rust-toolchain.toml}"

# `-fuse-ld=mold` resolves `ld.mold` by PATH order, and the Makefile puts this
# prefix's `bin/` first, so the pinned release outranks a distribution one.
BUILD_TOOLS_PREFIX="${BUILD_TOOLS_PREFIX:-$HOME/.local}"

note() { printf 'build-tools: %s\n' "$*" >&2; }

fail() {
  note "$*"
  exit 1
}

# The host platform, set by `detect_platform`. Empty means not yet detected.
HOST_PLATFORM=

# Detects the host platform into `HOST_PLATFORM`, aborting when `uname` cannot
# answer.
#
# It sets a variable rather than printing one, and callers run it as a bare
# statement, because both alternatives lose the failure. Inside `$(...)` a
# `fail` ends only the subshell, and inside `is_linux`, called as an `if`
# condition, `set -e` is suspended for the whole function. Either way an
# unanswered `uname` would arrive as an empty string, read as a non-Linux host,
# skip every linker check, and report success: exactly the silent pass this
# preflight exists to prevent.
detect_platform() {
  HOST_PLATFORM=$(uname -s) || fail 'could not detect the host platform: uname -s failed'
  [ -n "$HOST_PLATFORM" ] || fail 'could not detect the host platform: uname -s said nothing'
}

# Whether the host links with the mold linker at all; it ships for Linux only.
#
# The argument must be a successful `detect_platform` result. Refusing an empty
# one turns a caller that skipped the detection into a loud bug rather than a
# host that silently is not Linux.
is_linux() {
  local platform=$1
  [ -n "$platform" ] || fail 'is_linux needs a detected platform; call detect_platform first'
  [ "$platform" = 'Linux' ]
}

# Read a one-line pin. A missing, blank or multi-line file aborts rather than
# yielding a version that would build a nonsensical download URL.
read_pin() {
  local file=$1 value
  [ -f "$file" ] || fail "missing version pin: $file"
  [ "$(grep -c '' <"$file")" -le 1 ] || fail "expected one line in version pin: $file"
  value=$(tr -d '[:space:]' <"$file")
  [ -n "$value" ] || fail "empty version pin: $file"
  printf '%s' "$value"
}

linker_version() { read_pin "$LINKER_VERSION_FILE"; }

# The channel `rust-toolchain.toml` pins; the same toolchain every gate uses.
pinned_toolchain() {
  local value
  [ -f "$RUST_TOOLCHAIN_FILE" ] || fail "missing toolchain file: $RUST_TOOLCHAIN_FILE"
  value=$(awk -F'"' '/^[[:space:]]*channel[[:space:]]*=/ { print $2; exit }' "$RUST_TOOLCHAIN_FILE")
  [ -n "$value" ] || fail "no channel found in: $RUST_TOOLCHAIN_FILE"
  printf '%s' "$value"
}

# The version of whichever mold linker PATH resolves, or non-zero when it cannot run.
# `mold --version` prints e.g. "mold 2.41.0 (compatible with GNU ld)".
installed_linker_version() {
  local output
  output=$(mold --version 2>/dev/null) || return 1
  printf '%s' "$output" | awk 'NR == 1 { print $2 }'
}

# The mold linker publishes per-architecture tarballs; map `uname -m` onto their names.
linker_arch() {
  case "$(uname -m)" in
    x86_64 | amd64) printf 'x86_64' ;;
    aarch64 | arm64) printf 'aarch64' ;;
    *) fail "no pinned mold linker release for architecture $(uname -m)" ;;
  esac
}
