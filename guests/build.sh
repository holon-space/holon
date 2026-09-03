#!/usr/bin/env bash
# Build one guest to `wasm32-unknown-unknown` and install it beside its
# sidecar. `wasm-opt` is applied when binaryen is on PATH; without it the
# artifact is `opt-level="z"` + LTO + `strip` only, and the size printed here
# is the un-post-processed one.
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6

GUEST=${1:?usage: build.sh <guest-dir-name> [install-dir]}
HERE=$(cd "$(dirname "$0")" && pwd)
# Resolved before the `cd` below, so a relative install dir means what the
# caller typed rather than a path under the guest's own directory.
INSTALL_DIR=$(cd "${2:-$HERE/../crates/holon-plugin-host/plugins}" && pwd)
OUT=$INSTALL_DIR/$GUEST.wasm

cd "$HERE/$GUEST"
cargo build --release --target wasm32-unknown-unknown
BUILT=$(ls target/wasm32-unknown-unknown/release/*.wasm | head -1)

if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz --strip-debug --strip-producers "$BUILT" -o "$OUT"
  echo "wasm-opt: applied"
else
  cp "$BUILT" "$OUT"
  echo "wasm-opt: NOT INSTALLED (binaryen absent) — size below is un-post-processed"
fi

echo "guest $GUEST: $(wc -c < "$OUT") bytes raw, $(gzip -c "$OUT" | wc -c) bytes gzipped"
