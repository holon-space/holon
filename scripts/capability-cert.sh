#!/usr/bin/env bash
# Run one capability certification of the org format and print its report.
#
# `HOLON_CAPABILITY_PROFILE` points the harness at a profile yaml other than
# the crate's own — the flip sweep certifies a MUTATED COPY that way, so a
# sweep never writes into the source tree.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || exit 90
cd "$ROOT" || exit 90
export RUSTC_WRAPPER=""
echo "profile:    ${HOLON_CAPABILITY_PROFILE:-$ROOT/crates/holon-org-format/profile.yaml}"
echo "report dir: ${HOLON_CAPABILITY_REPORT_DIR:-$ROOT/target/capability-certification}"
# `--no-fail-fast` is uniform across the certification scripts: a red in one
# test must never PRE-EMPT another. Proven once on the logseq script, where
# fail-fast skipped the day-29 boundary alarm; the hazard is milder here (a
# cross-format assertion rather than an alarm) and the fix is the same.
exec cargo nextest run -p holon-org-format -E 'binary(profile_certification)' --no-capture --no-fail-fast
