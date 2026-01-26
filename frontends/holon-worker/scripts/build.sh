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

# `--manifest-path ./Cargo.toml` pins napi build to this crate's out-of-workspace
# manifest. `--no-js` skips the Node-side .js glue (we write our own for the
# browser worker). `--platform` is required by napi build to produce a binary
# with a platform suffix in the filename.
npx --yes @napi-rs/cli@^3.1.5 napi build \
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
