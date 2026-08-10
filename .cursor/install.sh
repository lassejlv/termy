#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the Termy workspace.
# Safe to run repeatedly: every step is a no-op when already satisfied.
set -euo pipefail

echo "==> Installing Linux desktop build dependencies (GPUI + native)"
export DEBIAN_FRONTEND=noninteractive
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential \
  cmake \
  clang \
  pkg-config \
  libfontconfig-dev \
  libwayland-dev \
  libx11-xcb-dev \
  libxkbcommon-x11-dev \
  libssl-dev \
  libzstd-dev \
  libasound2-dev \
  libvulkan1

echo "==> Ensuring a Rust stable toolchain that supports edition 2024 (>= 1.85)"
# CI pins to rolling `stable`; the workspace requires edition 2024, so an older
# preinstalled stable (e.g. 1.83) must be updated.
rustup update stable
rustup default stable
rustup component add clippy rustfmt

echo "==> Ensuring Bun is available (plugin runtime + website tooling)"
# crates/plugin_runtime shells out to Bun and looks it up in ~/.bun/bin.
BUN_VERSION="bun-v1.3.14"
if [ ! -x "$HOME/.bun/bin/bun" ]; then
  curl -fsSL https://bun.sh/install | BUN_INSTALL="$HOME/.bun" bash -s "$BUN_VERSION"
fi
export PATH="$HOME/.bun/bin:$PATH"
if ! grep -qs 'BUN_INSTALL' "$HOME/.bashrc"; then
  {
    echo 'export BUN_INSTALL="$HOME/.bun"'
    echo 'export PATH="$BUN_INSTALL/bin:$PATH"'
  } >> "$HOME/.bashrc"
fi

echo "==> Warming the Cargo build cache"
cargo fetch
cargo build --workspace

echo "==> Termy environment ready"
