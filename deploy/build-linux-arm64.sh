#!/usr/bin/env bash
# Builds Linux arm64 binaries (Debian bookworm / Raspberry Pi OS 12) in a container.
# Output: target/linux-arm64/release/{scanner-web,scanner}
set -euo pipefail
cd "$(dirname "$0")/.."

ENGINE="${CONTAINER_ENGINE:-podman}"
IMAGE="docker.io/library/rust:1-bookworm"

"$ENGINE" run --rm --platform linux/arm64 \
  -v "$PWD":/src -w /src \
  -v scanner-app-cargo-registry:/usr/local/cargo/registry \
  "$IMAGE" bash -euc '
    apt-get update -qq && apt-get install -y -qq --no-install-recommends libsane-dev pkg-config >/dev/null
    cargo build --release --locked -p scanner-web -p scanner-cli --target-dir target/linux-arm64
  '
ls -la target/linux-arm64/release/scanner-web target/linux-arm64/release/scanner
