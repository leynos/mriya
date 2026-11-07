#!/usr/bin/env bash
# Preflight for the development build standard.
#
# Runs before every build, test, lint and typecheck route so a missing tool
# produces an actionable installation hint instead of an opaque link failure
# inside Cargo. It never falls back to a slower linker: an absent or unusable
# prerequisite is a failure. Every check runs, so one pass reports every gap.

set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/build-tools-common.sh
. "$script_dir/build-tools-common.sh"

# The mold linker must be on PATH at exactly the pinned version. A drifted version fails
# rather than warns: an advisory pin is not a pin.
check_linker() {
  local pinned=$1 installed resolved
  if ! resolved=$(command -v mold 2>/dev/null); then
    note "mold linker not found on PATH (pinned $pinned)"
    note 'install it with: make install-build-tools'
    return 1
  fi
  if ! installed=$(installed_linker_version) || [ -z "$installed" ]; then
    note "mold linker at $resolved is on PATH but cannot report its version"
    note 'reinstall it with: make install-build-tools'
    return 1
  fi
  if [ "$installed" != "$pinned" ]; then
    note "mold linker $installed at $resolved does not match the pin $pinned"
    note 'install the pinned release with: make install-build-tools'
    return 1
  fi
  note "mold linker $installed at $resolved"
}

# rustc reaches the mold linker through `cc -fuse-ld=mold`, so the C compiler driver must
# accept the flag (GCC 12.1 or newer, or clang). Linking a trivial program is
# the only reliable way to ask; a version check would guess.
check_linker_driver() {
  local scratch status=0
  if ! command -v cc >/dev/null 2>&1; then
    note 'no C compiler (cc) on PATH; rustc links through it'
    note 'install GCC 12.1 or newer, or clang, with your package manager'
    return 1
  fi
  scratch=$(mktemp -d)
  printf 'int main(void) { return 0; }\n' |
    cc -x c - -fuse-ld=mold -o "$scratch/probe" >"$scratch/log" 2>&1 || status=1
  if [ "$status" -ne 0 ]; then
    note 'cc cannot link with -fuse-ld=mold, which the build standard passes'
    note 'use GCC 12.1 or newer, or clang, and make sure the mold linker is installed'
    sed 's/^/build-tools:   /' "$scratch/log" >&2
  else
    note 'cc links with -fuse-ld=mold'
  fi
  rm -rf -- "$scratch"
  return "$status"
}

# The pinned nightly must be installed: the parallel frontend is nightly-only,
# and rustup will not install it unprompted.
check_toolchain() {
  local toolchain=$1
  if ! command -v rustup >/dev/null 2>&1; then
    note 'rustup not found on PATH; it selects the pinned nightly'
    note 'install it from https://rustup.rs'
    return 1
  fi
  if ! rustup toolchain list | grep -q "^$toolchain"; then
    note "toolchain $toolchain is not installed"
    note 'install it with: make install-build-tools'
    return 1
  fi
  note "toolchain $toolchain available"
}

main() {
  local status=0 linker_pin toolchain_pin
  # Assign first: `fail` exits, but inside `$(...)` that kills only the
  # subshell, so passing the substitution straight in would carry on with an
  # empty pin. An assignment propagates the status.
  linker_pin=$(linker_version) || return 1
  toolchain_pin=$(pinned_toolchain) || return 1
  if is_linux; then
    check_linker "$linker_pin" || status=1
    check_linker_driver || status=1
  else
    note "the mold linker is Linux-only; $(uname -s) keeps its platform linker"
  fi
  check_toolchain "$toolchain_pin" || status=1
  [ "$status" -eq 0 ] || note 'build tools check failed; see the messages above'
  return "$status"
}

main "$@"
