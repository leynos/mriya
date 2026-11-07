#!/usr/bin/env bash
# Install the build tools the development build standard needs.
#
# Fetches the pinned mold release, verifies it against tools/mold/SHA256SUMS and
# unpacks it under $BUILD_TOOLS_PREFIX (default ~/.local); then installs the
# pinned nightly with the components rust-toolchain.toml lists. Nothing here
# touches the coverage, release or Whitaker toolchains. A mold already on PATH
# at the pinned version is left alone, so a runner whose setup action installed
# it does not download it twice.

set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/build-tools-common.sh
. "$script_dir/build-tools-common.sh"

MOLD_RELEASE_BASE_URL=${MOLD_RELEASE_BASE_URL:-https://github.com/rui314/mold/releases/download}

# The scratch directory the EXIT trap removes. A named function rather than a
# trap string, so a quote in TMPDIR cannot break the cleanup.
BUILD_TOOLS_WORKDIR=
remove_workdir() {
  [ -n "$BUILD_TOOLS_WORKDIR" ] || return 0
  rm -rf -- "$BUILD_TOOLS_WORKDIR"
}
trap remove_workdir EXIT

# An unlisted or ambiguous artefact is a hard failure, never a silent skip.
verify_archive() {
  local archive=$1 name=$2 expected
  expected=$(awk -v name="$name" '$2 == name { print $1 }' "$MOLD_SHA256SUMS_FILE")
  [ -n "$expected" ] || fail "no checksum recorded for $name in $MOLD_SHA256SUMS_FILE"
  [ "$(printf '%s\n' "$expected" | grep -c .)" -eq 1 ] ||
    fail "several checksums recorded for $name; refusing to guess"
  printf '%s  %s\n' "$expected" "$archive" | sha256sum --check --status ||
    fail "checksum mismatch for $name; refusing to install"
  note "verified $name against $MOLD_SHA256SUMS_FILE"
}

install_mold() {
  local version=$1 name url
  if ! is_linux; then
    note "mold is Linux-only; skipping on $(uname -s)"
    return 0
  fi
  if [ "$(installed_mold_version 2>/dev/null || true)" = "$version" ]; then
    note "mold $version already on PATH at $(command -v mold)"
    return 0
  fi
  name="mold-$version-$(mold_arch)-linux.tar.gz"
  url="$MOLD_RELEASE_BASE_URL/v$version/$name"
  BUILD_TOOLS_WORKDIR=$(mktemp -d)
  note "downloading $url"
  # A stall detector rather than a deadline, so a slow link still finishes
  # while a server that accepts and then goes quiet cannot hang the install.
  curl --fail --silent --show-error --location \
    --connect-timeout 15 --speed-limit 1024 --speed-time 60 \
    --output "$BUILD_TOOLS_WORKDIR/$name" "$url" || fail "failed to download $name"
  verify_archive "$BUILD_TOOLS_WORKDIR/$name" "$name"
  # The tarball root is mold-<version>-<arch>-linux/; strip it so bin/mold and
  # bin/ld.mold land under the prefix.
  mkdir -p "$BUILD_TOOLS_PREFIX"
  tar --extract --gzip --strip-components=1 --directory "$BUILD_TOOLS_PREFIX" \
    --file "$BUILD_TOOLS_WORKDIR/$name" || fail "failed to unpack $name"
  note "installed mold $version into $BUILD_TOOLS_PREFIX"
}

# The component names rust-toolchain.toml lists, one per line.
pinned_components() {
  awk -F'[][]' '/^[[:space:]]*components[[:space:]]*=/ { print $2 }' "$RUST_TOOLCHAIN_FILE" |
    tr -d '" ' | tr ',' '\n' | grep .
}

install_toolchain() {
  local toolchain=$1 component
  local -a components=()
  command -v rustup >/dev/null 2>&1 ||
    fail 'rustup not found on PATH; install it from https://rustup.rs'
  while IFS= read -r component; do
    components+=(--component "$component")
  done < <(pinned_components)
  note "installing toolchain $toolchain"
  rustup toolchain install "$toolchain" --profile minimal "${components[@]}" ||
    fail "failed to install toolchain $toolchain"
}

main() {
  local mold_pin toolchain_pin
  mold_pin=$(mold_version) || return 1
  toolchain_pin=$(pinned_toolchain) || return 1
  # The linker first, so a checksum failure aborts before a toolchain download.
  install_mold "$mold_pin"
  install_toolchain "$toolchain_pin"
  note 'ready; verify with: make check-build-tools'
}

main "$@"
