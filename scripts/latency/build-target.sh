#!/usr/bin/env bash
set -euo pipefail
cd "$1"
export RUSTC_WRAPPER=
command -v rustup >/dev/null || { echo "build-target.sh: rustup not on PATH; install rustup or put it on PATH" >&2; exit 1; }
rustup show active-toolchain
cargo build -p holon-gpui --features pbt
echo "BUILD_SUMMARY_OK $1"
