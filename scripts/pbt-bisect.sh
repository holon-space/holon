#!/usr/bin/env bash
# Component-bisect a failing PBT capture (ADR 0009 §3).
#
# On a red wide PBT, the slice's Drop writes a concrete failing sequence to
# crates/holon-integration-tests/tests/.captures/<slice>.captured.json. This
# script replays that capture across the ComponentSet lattice and reports the
# smallest set that still reproduces — the "where ref and projections disagree"
# localization — so triage starts from a 2-component answer, not a 50-step trace.
#
# Usage:
#   scripts/pbt-bisect.sh <slice>            # full bisection (many SUT builds)
#   scripts/pbt-bisect.sh <slice> --probe    # cheap: does the ceiling reproduce?
#   HOLON_BISECT_CEILING=loro_vm_fast scripts/pbt-bisect.sh <slice>
#
# <slice> is the test_fn name, e.g. general_e2e_pbt, split_block_content_pbt.
# Ceiling defaults to full_headless (a safe universal ceiling for headless
# captures — unset transitions simply never gate).
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <slice> [--probe]" >&2
  exit 2
fi

slice="$1"
probe_env=()
if [[ "${2:-}" == "--probe" ]]; then
  probe_env=(HOLON_BISECT_PROBE=1)
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
capture="$repo_root/crates/holon-integration-tests/tests/.captures/${slice}.captured.json"
if [[ ! -f "$capture" ]]; then
  echo "[pbt-bisect] no capture for slice '${slice}' at $capture" >&2
  echo "[pbt-bisect] run the slice to a failure first (it writes the capture on panic)." >&2
  exit 1
fi

log="${TMPDIR:-/tmp}/pbt-bisect-${slice}.log"
echo "[pbt-bisect] localizing ${slice} (ceiling=${HOLON_BISECT_CEILING:-full_headless}) — log: $log" >&2

# `tee` before filtering (project rule); surface only the bisect lines.
env "${probe_env[@]}" HOLON_BISECT_SLICE="$slice" \
  cargo test -p holon-integration-tests --features pbt \
    --test bisection_pbt bisect_capture_from_env -- --nocapture 2>&1 \
  | tee "$log" \
  | grep -E '\[bisect\]'
