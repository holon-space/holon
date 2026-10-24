#!/usr/bin/env bash
# One fact per live process: its pid, its process group and its command name.
set -euo pipefail
ps -A -o pid=,pgid=,comm= | jq -Rc '
  (capture("^ *(?<pid>[0-9]+) +(?<pgid>[0-9]+) +(?<command>.*)$") // error("cannot parse ps line: " + .))
  | .pid |= tonumber | .pgid |= tonumber'
