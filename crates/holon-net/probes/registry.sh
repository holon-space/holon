#!/usr/bin/env bash
# One fact per record of the local runtime registry: every
# <runtime>/<repo>/<kind>/*.json, where <kind> is agents, jobs or sessions
# and <runtime> is the second argument, by default the registry of $HOME.
set -euo pipefail
kind="$1"
runtime="${2:-$HOME/.cache/holon-flow/runtime}"
[[ -d "$runtime" ]] || { echo "no runtime registry at $runtime" >&2; exit 1; }
shopt -s nullglob
for record in "$runtime"/*/"$kind"/*.json; do
  jq -c . "$record"
done
