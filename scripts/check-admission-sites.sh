#!/usr/bin/env bash
# Inventory of every production call into an operation-execution entry
# (`execute_operation`, `execute_operation_with_origin`,
# `execute_with_parsed_carriers`), per file, checked
# against scripts/admission-sites.inventory. Each inventory row classifies its
# file: `entry` sites reach the OperationEngine and must pass the dispatcher's
# admission step before any spawn or await; `below` sites run inside an
# operation that was already admitted (provider-to-provider calls, the
# dispatcher's own routing) or wrap the engine without being a caller.
# A new, moved or vanished call site fails the check until it is classified.
#
# Usage: scripts/check-admission-sites.sh            # check
#        scripts/check-admission-sites.sh --print    # print the observed counts
set -euo pipefail
cd "$(dirname "$0")/.."

inventory=scripts/admission-sites.inventory

observed() {
    rg -l --type rust -e '\.(execute_operation(_with_origin)?|execute_with_parsed_carriers)\(' crates frontends \
        -g '!**/tests/**' -g '!**/benches/**' -g '!**/examples/**' -g '!**/testing/**' \
        -g '!*_test.rs' -g '!*_tests.rs' -g '!crates/holon-integration-tests/**' \
        -g '!crates/*-testing/**' -g '!crates/holon-macros-test/**' \
        | sort \
        | while read -r f; do
            # Production code only: skip each top-level `#[cfg(test)] mod … {`
            # up to its closing `}` in column 0 (rustfmt layout).
            n=$(awk '
                skip { if (/^}/) skip = 0; next }
                /^#\[cfg\(test\)\]/ { pending = 1; next }
                pending && /^(pub(\([a-z]+\))? )?mod .*\{$/ { skip = 1; pending = 0; next }
                { pending = 0 }
                /^[[:space:]]*\/\// { next }
                /\.(execute_operation(_with_origin)?|execute_with_parsed_carriers)\(/ { c++ }
                END { print c + 0 }' "$f")
            [ "$n" -gt 0 ] && echo "$n $f"
        done
}

if [ "${1-}" = "--print" ]; then
    observed
    exit 0
fi

expected=$(grep -v '^#' "$inventory" | awk 'NF { print $1, $2 }')
actual=$(observed)
if [ "$expected" != "$actual" ]; then
    echo "check-admission-sites: production execute_operation call sites differ from $inventory" >&2
    diff <(echo "$expected") <(echo "$actual") >&2 || true
    echo "Classify each changed file (entry | below) in $inventory; an entry site must admit before it spawns or awaits." >&2
    exit 1
fi
entries=$(grep -v '^#' "$inventory" | awk '$3 == "entry" { s += $1 } END { print s + 0 }')
files=$(grep -v '^#' "$inventory" | awk '$3 == "entry"' | wc -l | tr -d ' ')
echo "check-admission-sites: OK — $entries entry call site(s) in $files file(s) must admit"
