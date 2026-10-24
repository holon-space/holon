#!/usr/bin/env bash
# One fact per volume that holds one of the given paths: its free space.
set -euo pipefail
df -Pk "$@" | tail -n +2 | jq -Rc '
  (capture("^(?<filesystem>.+?) +(?<total_kb>[0-9]+) +(?<used_kb>[0-9]+) +(?<free_kb>[0-9]+) +[0-9]+% +(?<mount>/.*)$") // error("cannot parse df line: " + .))
  | .total_kb |= tonumber | .used_kb |= tonumber | .free_kb |= tonumber'
