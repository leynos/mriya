#!/usr/bin/env bash
# Install the build tools the development build standard needs.
#
# Fetches the pinned mold linker release, verifies it against tools/mold/SHA256SUMS and
# unpacks it under $BUILD_TOOLS_PREFIX (default ~/.local); then installs the
# pinned nightly with the components rust-toolchain.toml lists. Nothing here
# touches the coverage, release or Whitaker toolchains. A mold linker already on PATH
# at the pinned version is left alone, so a runner whose setup action installed
# it does not download it twice.

set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=scripts/build-tools-common.sh
. "$script_dir/build-tools-common.sh"

LINKER_RELEASE_BASE_URL=${LINKER_RELEASE_BASE_URL:-https://github.com/rui314/mold/releases/download}

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
  expected=$(awk -v name="$name" '$2 == name { print $1 }' "$LINKER_SHA256SUMS_FILE")
  [ -n "$expected" ] || fail "no checksum recorded for $name in $LINKER_SHA256SUMS_FILE"
  [ "$(printf '%s\n' "$expected" | grep -c .)" -eq 1 ] ||
    fail "several checksums recorded for $name; refusing to guess"
  printf '%s  %s\n' "$expected" "$archive" | sha256sum --check --status ||
    fail "checksum mismatch for $name; refusing to install"
  note "verified $name against $LINKER_SHA256SUMS_FILE"
}

install_linker() {
  local version=$1 name url
  if ! is_linux "$HOST_PLATFORM"; then
    note "the mold linker is Linux-only; skipping on $HOST_PLATFORM"
    return 0
  fi
  # The compiler reaches the linker through `-fuse-ld=mold`, which resolves the
  # `ld.mold` entry point rather than `mold` itself. A tree that carries the
  # driver but not its linker entry point is not installable-by-skip: the
  # preflight probe would fail and point here, so this check must repair it.
  if [ "$(installed_linker_version 2>/dev/null || true)" = "$version" ] &&
    command -v ld.mold >/dev/null 2>&1; then
    note "mold linker $version already on PATH at $(command -v mold)"
    return 0
  fi
  name="mold-$version-$(linker_arch)-linux.tar.gz"
  url="$LINKER_RELEASE_BASE_URL/v$version/$name"
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
  note "installed mold linker $version into $BUILD_TOOLS_PREFIX"
}

# The component names rust-toolchain.toml lists, one per line.
pinned_components() {
  awk -F'[][]' '/^[[:space:]]*components[[:space:]]*=/ { print $2 }' "$RUST_TOOLCHAIN_FILE" |
    tr -d '" ' | tr ',' '\n' | grep .
}

install_toolchain() {
  local toolchain=$1 component components_text
  local -a components=()
  command -v rustup >/dev/null 2>&1 ||
    fail 'rustup not found on PATH; install it from https://rustup.rs'
  # Read the components first: process substitution discards the status of the
  # reader, so an unreadable or unparsable file would otherwise install the
  # toolchain with no components at all, leaving the lint gate without rustfmt
  # or clippy.
  if ! components_text=$(pinned_components) || [ -z "$components_text" ]; then
    fail "cannot read the components in $RUST_TOOLCHAIN_FILE"
  fi
  while IFS= read -r component; do
    components+=(--component "$component")
  done <<<"$components_text"
  note "installing toolchain $toolchain"
  rustup toolchain install "$toolchain" --profile minimal "${components[@]}" ||
    fail "failed to install toolchain $toolchain"
}

main() {
  local linker_pin toolchain_pin
  linker_pin=$(linker_version) || return 1
  toolchain_pin=$(pinned_toolchain) || return 1
  detect_platform
  # The linker first, so a checksum failure aborts before a toolchain download.
  install_linker "$linker_pin"
  install_toolchain "$toolchain_pin"
  note 'ready; verify with: make check-build-tools'
}

main "$@"
