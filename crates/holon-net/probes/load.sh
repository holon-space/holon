#!/usr/bin/env bash
# One fact for this host: its load averages and its swap use, in MiB.
set -euo pipefail
[[ "$(uname)" == Darwin ]] || { echo "load.sh reads sysctl names of macOS only" >&2; exit 1; }
jq -nc --arg host "$(hostname -s)" --arg load "$(sysctl -n vm.loadavg)" --arg swap "$(sysctl -n vm.swapusage)" '
  ($load | capture("(?<l1>[0-9.]+) +(?<l5>[0-9.]+) +(?<l15>[0-9.]+)") // error("cannot parse vm.loadavg: " + $load)) as $l
  | ($swap | capture("total = (?<total>[0-9.]+)M +used = (?<used>[0-9.]+)M") // error("cannot parse vm.swapusage: " + $swap)) as $s
  | {host: $host, load1: ($l.l1 | tonumber), load5: ($l.l5 | tonumber), load15: ($l.l15 | tonumber),
     swap_total_mib: ($s.total | tonumber), swap_used_mib: ($s.used | tonumber)}'
