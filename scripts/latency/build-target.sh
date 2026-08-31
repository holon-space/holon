#!/usr/bin/env bash
set -euo pipefail
cd "$1"
export RUSTC_WRAPPER=
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
rustup show active-toolchain
cargo build -p holon-gpui --features pbt
echo "BUILD_SUMMARY_OK $1"
