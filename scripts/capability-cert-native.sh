#!/usr/bin/env bash
# Certify the holon-native profile against the REAL substrate.
#
# Separate from `capability-cert.sh` because it needs the `test-helpers`
# feature and boots an in-memory store; same env knobs
# (`HOLON_CAPABILITY_PROFILE`, `HOLON_CAPABILITY_REPORT_DIR`).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || exit 90
cd "$ROOT" || exit 90
export RUSTC_WRAPPER=""
echo "profile:    ${HOLON_CAPABILITY_PROFILE:-$ROOT/assets/default/capability/holon-native.yaml}"
echo "report dir: ${HOLON_CAPABILITY_REPORT_DIR:-$ROOT/target/capability-certification}"
exec cargo nextest run -p holon --features test-helpers \
    -E 'binary(capability_certification)' --no-capture
