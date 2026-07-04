#!/usr/bin/env bash
# Sample resident-set size (RSS) of the running soak test process over time.
#
# The headless keystone does not emit MemoryMonitor `RSS ...MB` log lines (that is a
# live-app component), so `analyze-log-metrics.py` finds nothing to plot. This sampler
# polls the OS instead: every INTERVAL seconds it sums the RSS of every process whose
# command line matches PATTERN (the compiled test binary) and appends `epoch_s rss_mb`
# to OUT. It self-terminates once no matching process remains (the run finished) or when
# it receives SIGTERM.
#
# Usage: soak_rss_sampler.sh <out.csv> [pattern] [interval_s]
set -euo pipefail
OUT="${1:?usage: soak_rss_sampler.sh <out.csv> [pattern] [interval_s]}"
PATTERN="${2:-general_e2e_composed_pb[t]}"
INTERVAL="${3:-2}"

echo "epoch_s,rss_mb" > "$OUT"
# Wait briefly for the test process to appear (compiled binary launch).
appeared=0
for _ in $(seq 1 60); do
    if pgrep -f "$PATTERN" >/dev/null 2>&1; then appeared=1; break; fi
    sleep 1
done
[ "$appeared" = 1 ] || { echo "sampler: no process matched '$PATTERN'"; exit 0; }

while pgrep -f "$PATTERN" >/dev/null 2>&1; do
    # ps RSS is in KB on macOS and Linux; sum across matching PIDs.
    rss_kb=$(ps -o rss= -p "$(pgrep -f "$PATTERN" | tr '\n' ',' | sed 's/,$//')" 2>/dev/null \
        | awk '{s+=$1} END{print s+0}')
    printf '%s,%.1f\n' "$(date +%s)" "$(echo "$rss_kb / 1024" | bc -l)" >> "$OUT"
    sleep "$INTERVAL"
done

# Summary: start / peak / end and delta.
awk -F, 'NR>1{v=$2; if(NR==2)start=v; if(v>peak)peak=v; end=v; n++}
    END{ if(n>0) printf "RSS samples=%d  start=%.0fMB  peak=%.0fMB  end=%.0fMB  growth=%+.0fMB\n", n, start, peak, end, end-start;
         else print "RSS: no samples captured" }' "$OUT"
