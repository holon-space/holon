#!/usr/bin/env bash
# Two checks over production code.
#
# 1. No `spawn(`, `spawn_blocking(` or `defer(` call contains a call that admits
# an operation: admitted inside the task, the operation takes its place in the
# write order when the task first runs. Admit first, spawn the returned future.
# A call admits when it names an entry (`execute_operation*`,
# `execute_with_parsed_carriers`, `admit`, `dispatch_intent*`) as a method or by
# path, names a free function whose body admits, or names a method whose body
# admits and that is defined in a file the inventory classifies `entry`; a body
# admits when it calls an admitting function, transitively. An admitting call
# whose spawn's own event is the admission point carries
# `// ALLOW(admit-in-spawn): <reason>` in the comment directly above its line;
# one inside `defer` carries `// ALLOW(admit-in-defer): <reason>`.
#
# 2. Inventory of every production call into an entry other than `admit`, per
# file, checked against scripts/admission-sites.inventory. Each inventory row
# classifies its file: `entry` sites reach the OperationEngine and must pass
# the dispatcher's admission step before any spawn or await; `below` sites run
# inside an operation that was already admitted (provider-to-provider calls,
# the dispatcher's own routing) or wrap the engine without being a caller.
# A new, moved or vanished call site fails the check until it is classified.
#
# Usage: scripts/check-admission-sites.sh            # check
#        scripts/check-admission-sites.sh --print    # print the observed counts
set -euo pipefail
cd "$(dirname "$0")/.."

inventory=scripts/admission-sites.inventory
entries='execute_operation(_with_origin)?|execute_with_parsed_carriers|dispatch_intent(_sync|_awaitable|_awaiting_result|_chain)?'
admits="$entries|admit"

observed() {
    rg -l --type rust -e "(^|[^A-Za-z0-9_])($entries)\(" crates frontends \
        -g '!**/tests/**' -g '!**/benches/**' -g '!**/examples/**' -g '!**/testing/**' \
        -g '!*_test.rs' -g '!*_tests.rs' -g '!crates/holon-integration-tests/**' \
        -g '!crates/*-testing/**' -g '!crates/holon-macros-test/**' \
        | sort \
        | while read -r f; do
            # Production code only: skip each top-level `#[cfg(test)] mod … {`
            # up to its closing `}` in column 0 (rustfmt layout).
            n=$(awk -v call="(^|[^A-Za-z0-9_])($entries)\\\\(" -v def="fn ($entries)[<(]" '
                skip { if (/^}/) skip = 0; next }
                /^#\[cfg\(test\)\]/ { pending = 1; next }
                pending && /^(pub(\([a-z]+\))? )?mod .*\{$/ { skip = 1; pending = 0; next }
                { pending = 0 }
                /^[[:space:]]*\/\// { next }
                $0 ~ def { next }
                $0 ~ call { c++ }
                END { print c + 0 }' "$f")
            [ "$n" -gt 0 ] && echo "$n $f"
        done
}

scan() {
    ast-grep scan --json=stream --inline-rules "$1" crates frontends \
        --globs '!**/tests/**' --globs '!**/benches/**' --globs '!**/examples/**' \
        --globs '!**/testing/**' --globs '!*_test.rs' --globs '!*_tests.rs' \
        --globs '!crates/holon-integration-tests/**' --globs '!crates/*-testing/**' \
        --globs '!crates/holon-macros-test/**'
}

not_in_test_mod='
    inside:
      stopBy: end
      kind: mod_item
      follows:
        kind: attribute_item
        regex: "cfg\\(test\\)"'

# A call to an entry or to one of the methods `$2` by method or path, or to
# one of the free functions `$1`.
admitting_call() {
    cat <<EOF
kind: call_expression
any:
  - has:
      field: function
      regex: "(^|::|\\\\.)($admits|${2:-NO_SUCH_METHOD})\$"
  - has:
      field: function
      any: [{kind: identifier}, {kind: scoped_identifier}]
      regex: "(^|::)(${1:-NO_SUCH_FN})\$"
EOF
}

indent() { sed "s/^/$1/"; }

# A function that admits when called: in its body, outside any closure the body
# builds (a handler admits at its own event), a call to an entry, to one of the
# admitting free functions `$1` by bare name, or to one of the admitting methods
# `$2` through `self.` or `Self::`. Other receivers are not followed: a method
# name alone cannot tell two types' methods apart.
admitting_body() {
    cat <<EOF
  has:
    field: name
    pattern: \$NAME
  all:
    - has:
        field: body
        stopBy: end
        kind: call_expression
        any:
          - has:
              field: function
              regex: "(^|::|\\\\.)($admits)\$"
          - has:
              field: function
              kind: identifier
              regex: "^(${1:-NO_SUCH_FN})\$"
          - has:
              field: function
              regex: "^(self\\\\.|Self::)(${2:-NO_SUCH_METHOD})\$"
        not:
          inside:
            kind: closure_expression
            stopBy: {kind: function_item}
EOF
}

admitting_fns() {
    scan "
id: admitting-fn
language: rust
rule:
  kind: function_item
  not:
    any:
      - inside: {kind: impl_item, stopBy: end}
      - inside: {kind: trait_item, stopBy: end}
      - inside:
          stopBy: end
          kind: mod_item
          follows:
            kind: attribute_item
            regex: \"cfg\\\\(test\\\\)\"
$(admitting_body "$1" "$2")
" | jq -r '.metaVariables.single.NAME.text' | sort -u | paste -sd '|' -
}

# Methods and trait default methods in `entry` files only: a `below` file's
# methods (`set_field`, `delete`, …) run inside an admitted operation.
admitting_methods() {
    scan "
id: admitting-method
language: rust
rule:
  kind: function_item
  any:
    - inside: {kind: impl_item, stopBy: end}
    - inside: {kind: trait_item, stopBy: end}
  not:
$not_in_test_mod
$(admitting_body "$1" "$2")
" | jq -r '"\(.file) \(.metaVariables.single.NAME.text)"' \
        | awk 'NR == FNR { if ($3 == "entry") entry[$2] = 1; next } entry[$1] { print $2 }' "$inventory" - \
        | sort -u | paste -sd '|' -
}

# Calls `$3` matches that contain an admitting call, minus those whose line is
# directly preceded by a comment carrying `$4`.
admits_inside() {
    scan "
id: admit-inside
language: rust
rule:
$(admitting_call "$1" "$2" | indent '  ')
  inside:
    stopBy: end
    kind: call_expression
    has:
      field: function
      regex: \"(^|::|\\\\.)($3)\$\"
  not:
$not_in_test_mod
" \
        | jq -r '"\(.file) \(.range.start.line + 1)"' \
        | sort -u \
        | while read -r f line; do
            allowed=$(awk -v n="$line" -v marker="$4" '
                NR < n { if (/^[[:space:]]*\/\//) { block = block $0 "\n" } else { block = "" } }
                NR == n { print (index(block, marker) > 0) ? 1 : 0; exit }' "$f")
            [ "$allowed" = 1 ] || echo "$f:$line"
        done
}

helpers=""
methods=""
while :; do
    next_helpers=$(admitting_fns "$helpers" "$methods")
    next_methods=$(admitting_methods "$helpers" "$methods")
    [ "$next_helpers" != "$helpers" ] || [ "$next_methods" != "$methods" ] || break
    helpers=$next_helpers
    methods=$next_methods
done
[ -n "$helpers" ] || { echo "check-admission-sites: found no admitting free function; the scan is broken" >&2; exit 1; }
[ -n "$methods" ] || { echo "check-admission-sites: found no admitting method; the scan is broken" >&2; exit 1; }
spawned=$(admits_inside "$helpers" "$methods" 'spawn(_local|_blocking)?' 'ALLOW(admit-in-spawn)')
if [ -n "$spawned" ]; then
    echo "check-admission-sites: a spawned task admits an operation; admit before the spawn:" >&2
    echo "$spawned" >&2
    exit 1
fi
deferred=$(admits_inside "$helpers" "$methods" 'defer' 'ALLOW(admit-in-defer)')
if [ -n "$deferred" ]; then
    echo "check-admission-sites: a deferred closure admits an operation; mark it ALLOW(admit-in-defer) with the reason it may run after its gesture:" >&2
    echo "$deferred" >&2
    exit 1
fi

if [ "${1-}" = "--print" ]; then
    observed
    exit 0
fi

expected=$(grep -v '^#' "$inventory" | awk 'NF { print $1, $2 }')
actual=$(observed)
if [ "$expected" != "$actual" ]; then
    echo "check-admission-sites: production entry call sites differ from $inventory" >&2
    diff <(echo "$expected") <(echo "$actual") >&2 || true
    echo "Classify each changed file (entry | below) in $inventory; an entry site must admit before it spawns or awaits." >&2
    exit 1
fi
entry_sites=$(grep -v '^#' "$inventory" | awk '$3 == "entry" { s += $1 } END { print s + 0 }')
files=$(grep -v '^#' "$inventory" | awk '$3 == "entry"' | wc -l | tr -d ' ')
echo "check-admission-sites: OK — $entry_sites entry call site(s) in $files file(s) must admit; $(echo "$helpers" | tr '|' '\n' | wc -l | tr -d ' ') admitting free function(s) and $(echo "$methods" | tr '|' '\n' | wc -l | tr -d ' ') method(s) followed into spawns and defers"
