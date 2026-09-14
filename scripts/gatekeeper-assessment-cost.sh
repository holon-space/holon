#!/usr/bin/env bash
# Measure what macOS Gatekeeper costs per freshly-linked binary on this machine.
#
# Each run links binaries at sizes comparable to our test binaries, with a fresh
# nonce so every one has a code-directory hash the system has never assessed, then
# times the first exec (assessment) against the second (cached verdict) and counts
# the "Verifying ..." progress windows macOS put on screen meanwhile.
#
# Run it once before granting the launching app Developer Tools and once after;
# see DEVELOPMENT.md "Gatekeeper 'Verifying ...' windows during test runs".
set -euo pipefail

SIZES_MB="${SIZES_MB:-1 32 128 384}"
OUT="${1:-/tmp/gatekeeper-assessment-cost-$(date +%s).log}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/gk-assess-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }

START_EPOCH="$(date +%s)"
NONCE="$(date +%s)$RANDOM"

{
  echo "# Gatekeeper assessment cost — $(date '+%F %T')"
  echo "# macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
  echo
  printf '%-14s %-14s %-16s %-16s\n' size_bytes adhoc_signed first_exec_ms second_exec_ms
} | tee "$OUT"

for mb in $SIZES_MB; do
  words=$(( mb * 1024 * 1024 / 8 ))
  src="$WORK/pad$mb.c"
  bin="$WORK/probe$mb"
  cat > "$src" <<C_SRC
#include <stdio.h>
const unsigned long long nonce = ${NONCE}ULL + ${mb}ULL;
const unsigned long long blob[$words] = {1};
int main(void) { printf("%llu\\n", blob[0] + nonce); return 0; }
C_SRC
  cc -O0 -o "$bin" "$src"

  size=$(stat -f '%z' "$bin")
  signed=$(codesign -dv "$bin" 2>&1 | grep -q 'adhoc' && echo yes || echo no)

  t0=$(now_ms); "$bin" >/dev/null; t1=$(now_ms); "$bin" >/dev/null; t2=$(now_ms)

  printf '%-14s %-14s %-16s %-16s\n' "$size" "$signed" "$((t1-t0))" "$((t2-t1))" | tee -a "$OUT"
done

elapsed=$(( $(date +%s) - START_EPOCH + 1 ))
dialogs=$(log show --last "${elapsed}s" \
  --predicate 'process == "CoreServicesUIAgent"' --style compact 2>/dev/null \
  | grep -c 'startProgressForInfo' || true)
scans=$(log show --last "${elapsed}s" \
  --predicate 'process == "syspolicyd"' --style compact 2>/dev/null \
  | grep -c 'GatekeeperPolicyScanError' || true)
denials=$(log show --last "${elapsed}s" \
  --predicate 'process == "tccd"' --style compact 2>/dev/null \
  | grep -c 'kTCCServiceDeveloperTool' || true)

{
  echo
  echo "window                     ${elapsed}s"
  echo "Verifying windows shown    $dialogs"
  echo "Gatekeeper policy scans    $scans"
  echo "DeveloperTool TCC queries  $denials"
  echo
  echo "Machine-wide counts: other concurrent builds inflate them. The"
  echo "first-vs-second exec columns are per-binary and stay attributable."
  echo "Expected once the launching app holds Developer Tools: first_exec_ms"
  echo "falls to roughly second_exec_ms. Compare two runs rather than assume it."
} | tee -a "$OUT"

echo "log: $OUT"
