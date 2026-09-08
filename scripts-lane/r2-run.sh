#!/usr/bin/env bash
# Rev-2 lane runner. $1 = log tag, $2 = one command string.
set -uo pipefail
cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/plaintext-layer || exit 90
tag="$1"; shift
mkdir -p lane-logs
log="lane-logs/r2-${tag}-$$.log"
echo "LOG=$log"
# sccache's daemon wedges under multi-lane load and reports the wedge as a
# compile error; the gate must not read that as a code failure.
export RUSTC_WRAPPER=
bash "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" "$@" >"$log" 2>&1
echo "EXIT=$?"
tail -25 "$log"
