#!/usr/bin/env bash
set -euo pipefail

# Take ownership of volume-mounted cache dirs (created as root by Docker).
sudo chown -R "$(id -u):$(id -g)" \
  "$HOME/.cargo/registry" \
  "$HOME/.cargo/git" \
  "$HOME/.claude" \
  "$(pwd)"/target 2>/dev/null || true

# Trigger rustup to install the toolchain pinned in rust-toolchain.toml.
# postCreateCommand starts with cwd = workspaceFolder, so rustc picks up
# the rust-toolchain.toml at this directory automatically.
rustc --version
cargo --version

echo "devcontainer ready. headless only — GPUI must be run on the host (macOS: native Metal)."
