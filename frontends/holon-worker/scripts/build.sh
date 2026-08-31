#!/usr/bin/env bash
# Phase 1 spike build: produce a wasm32-wasip1-threads module via napi build.
#
# Exit non-zero on any step so CI/agents can detect failure. Always tees
# output to a log file so filtering can happen after the fact without a
# re-run (per project convention).
set -euo pipefail

cd "$(dirname "$0")/.."

LOG=/tmp/holon-worker-build.log
: > "$LOG"

echo "[holon-worker] napi build → wasm32-wasip1-threads" | tee -a "$LOG"

# The CLI comes from this package's pinned devDependency, never from a version
# range resolved at run time: the range floats onto CLI releases the lockfile
# was never tested against, and the workflows in .github/workflows build with
# this same local binary.
NAPI=./node_modules/.bin/napi
if [ ! -x "$NAPI" ]; then
    echo "[holon-worker] ERROR: $NAPI missing — run \`npm install\` in $(pwd) first" \
        | tee -a "$LOG" >&2
    exit 1
fi

# `--manifest-path ./Cargo.toml` pins napi build to this crate's out-of-workspace
# manifest. `--no-js` skips the Node-side .js glue (we write our own for the
# browser worker). `--platform` is required by napi build to produce a binary
# with a platform suffix in the filename. EMNAPI_LINK_DIR points napi's wasi
# shim at the emnapi static libs it links against.
EMNAPI_LINK_DIR="$(pwd)/node_modules/emnapi/lib/wasm32-wasi-threads" \
    "$NAPI" build \
    --features browser \
    --profile release-official \
    --platform \
    --target wasm32-wasip1-threads \
    --no-js \
    --manifest-path ./Cargo.toml \
    --output-dir . 2>&1 | tee -a "$LOG"

echo "[holon-worker] build complete. Artifacts:" | tee -a "$LOG"
ls -lh holon_worker*.wasm 2>&1 | tee -a "$LOG" || {
    echo "[holon-worker] ERROR: no .wasm produced" | tee -a "$LOG" >&2
    exit 1
}
