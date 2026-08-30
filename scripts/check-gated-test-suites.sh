#!/usr/bin/env bash
# Guard: no feature-gated test binary may silently compile to ZERO tests and rot.
#
# BugFunnel row 78: `holon-orgmode/tests/sync_controller_mutation_pbt.rs` is
# `#![cfg(feature = "di")]`-gated. `di` is NOT a default feature of
# holon-orgmode, so the default `cargo test`/`cargo nextest run` compiled that
# binary to `0 tests, 0 benchmarks` and it rotted invisibly for weeks while a
# real round-trip data-loss bug hid inside it. An empty test binary is a *green*
# test binary, so nothing failed.
#
# The failure mode is precisely: a test suite gated on a feature that NO build
# in the default test path enables. This guard makes that loud, and is
# self-maintaining (auto-discovers every gated `tests/` file — no inventory to
# drift):
#
#   * If the gating feature IS in the crate's default feature set, the default
#     `cargo test --workspace` (CI `rust-checks`) compiles+runs the suite — not
#     at risk. Reported, not compiled.
#   * If the gating feature is NON-DEFAULT, the suite only runs if something
#     opts in. The guard compiles+lists it WITH the feature and asserts >0 tests
#     (catches an emptied / renamed-feature / moved-out suite), AND requires that
#     a CI workflow actually runs that crate with that feature (catches an
#     unwired suite — the row-78 gap itself).
#
# Only the non-default-gated suites are compiled, so the guard stays cheap.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

fail=0
at_risk=0

# Discover top-level `#![cfg(feature = "...")]` files under any `tests/` dir.
# (Portable array fill — macOS ships bash 3.2, which lacks `mapfile`.)
gated_files=()
while IFS= read -r gf; do
    [ -n "$gf" ] && gated_files+=("$gf")
done < <(grep -rlE '^#!\[cfg\(feature' --include='*.rs' \
    crates/*/tests frontends/*/tests 2>/dev/null | sort)

if [ "${#gated_files[@]}" -eq 0 ]; then
    echo "check-gated-test-suites: no feature-gated test files found — nothing to check."
    exit 0
fi

for file in "${gated_files[@]}"; do
    crate_dir="${file%%/tests/*}"
    manifest="$crate_dir/Cargo.toml"
    pkg=$(grep -m1 -E '^name *= *"' "$manifest" | sed -E 's/^name *= *"([^"]+)".*/\1/')
    stem=$(basename "$file" .rs)

    # A `mod`-based suite (tests/<suite>/main.rs + tests/<suite>/<member>.rs)
    # compiles to ONE target named after its DIRECTORY, so `--test` must name
    # the suite while messages still name the member file.
    rel="${file#*/tests/}"
    if [ "$rel" = "${rel##*/}" ]; then
        target="$stem"
    else
        target="${rel%%/*}"
    fi

    # Extract the gating feature. Fail loud on any form this guard can't parse
    # (e.g. `all(feature=..., feature=...)`) rather than silently skipping it —
    # an unparsed gate would reintroduce exactly the blind spot this guards.
    feat_line=$(grep -m1 -E '^#!\[cfg\(feature' "$file")
    feat=$(printf '%s' "$feat_line" | grep -oE 'feature *= *"[^"]+"' | grep -oE '"[^"]+"' | tr -d '"')
    n_feats=$(printf '%s' "$feat_line" | grep -oE 'feature *= *"[^"]+"' | wc -l | tr -d ' ')
    if [ -z "$feat" ] || [ "$n_feats" != "1" ]; then
        echo "FAIL: $file — cannot parse a single gating feature from: $feat_line"
        echo "      extend check-gated-test-suites.sh to handle this cfg form."
        fail=1
        continue
    fi

    # Is the feature in the crate's [features] default list? (Shallow: the
    # literal default array. Our gated features are one hop from default.)
    default_line=$(sed -n '/^\[features\]/,/^\[/p' "$manifest" | grep -m1 -E '^default *=')
    if printf '%s' "$default_line" | grep -qE "\"$feat\""; then
        echo "ok (default feature): $pkg :: $stem  (feature = $feat) — run by default \`cargo test\`"
        continue
    fi

    # NON-default-gated → at risk. Must be (1) non-empty with its feature and
    # (2) actually run by a CI workflow with that feature.
    at_risk=$((at_risk + 1))
    echo "== at-risk: $pkg :: $stem  (NON-default feature = $feat) =="

    # (2) wired in CI?
    if grep -rqE -- "-p +$pkg\b.*--features[ =].*$feat|--features[ =].*$feat.*-p +$pkg\b|nextest run -p $pkg --features $feat" \
        .github/workflows/ 2>/dev/null; then
        echo "   ci: a workflow runs '$pkg' with --features $feat"
    else
        echo "FAIL: no .github/workflows/ step runs '$pkg' with --features $feat —"
        echo "      the gated suite '$stem' would never execute in CI (the row-78 gap)."
        echo "      Add a step, e.g.: cargo nextest run -p $pkg --features $feat"
        fail=1
    fi

    # (1) non-empty with its feature?
    list_out=$(cargo test -p "$pkg" --features "$feat" --test "$target" -- --list 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then
        echo "FAIL: compiling/listing $pkg/$stem with --features $feat exited $rc"
        printf '%s\n' "$list_out" | tail -20
        fail=1
        continue
    fi
    # libtest's summary is singular for a one-test binary ("1 test, 0
    # benchmarks"), so the plural-only match read every such suite as 0 and
    # failed it as dead.
    count=$(printf '%s\n' "$list_out" | grep -oE '[0-9]+ tests?,' | grep -oE '[0-9]+' | tail -1)
    count=${count:-0}
    if [ "$count" -eq 0 ]; then
        echo "FAIL: $pkg/$stem lists 0 tests WITH --features $feat — the gated suite is"
        echo "      dead (renamed feature / emptied file / tests moved out). Reconnect it."
        fail=1
    else
        echo "   ok: $count tests"
    fi
done

echo ""
if [ "$fail" -ne 0 ]; then
    echo "check-gated-test-suites: FAILED ($at_risk non-default-gated suite(s) checked)."
    exit 1
fi
echo "check-gated-test-suites: PASS — every non-default-gated test suite ($at_risk) is non-empty and CI-wired."
