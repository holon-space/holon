#!/usr/bin/env bash
# Certify the logseq-db profile against Holon's OWN writer surface.
#
# NO LogSeq oracle: this asks what `kvs_writer::push` carries and refuses, not
# what LogSeq's transactor would do. `just lsqdb-oracle` answers that other
# question and is unaffected.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || exit 90
cd "$ROOT" || exit 90
export RUSTC_WRAPPER=""
echo "profile:    ${HOLON_CAPABILITY_PROFILE:-$ROOT/crates/holon-logseq-db/profile.yaml}"
echo "report dir: ${HOLON_CAPABILITY_REPORT_DIR:-$ROOT/target/capability-certification}"
# `--no-fail-fast` is LOAD-BEARING: the day-29 alarm
# (`the_write_boundary_is_still_closed_to_property_changes`) is the one signal
# written for the day `push` learns to write properties, and nextest's
# fail-fast can pre-empt it — an opened boundary reds the certification first
# and the alarm never runs.
exec cargo nextest run -p holon-logseq-db -E 'binary(profile_certification)' \
    --no-capture --no-fail-fast
