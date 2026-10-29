#!/usr/bin/env bash
# Fixture check for scripts/keystone-known-reds.sh.
#
# This check replays archived full-depth corpora through the classifier and
# asserts each still-registered key's hit count is exactly what that corpus
# contains.
#
# What it CAN catch: a `Match pattern` edited so it no longer matches the very
# payload that motivated its row, and an over-broad pattern that starts
# swallowing a neighbouring signature.
#
# What it CANNOT catch — do not rely on it for this: an assertion message
# REWORDED in production or test code. A corpus is frozen text, so a reword
# leaves it classifying exactly as before while the pattern silently stops
# matching what the code now emits. Only a fresh full-depth run surfaces that.
#
# Corpora are committed zstd-compressed next to the hand-authored regressions
# because they are evidence, not scratch: /tmp is cleared on reboot, and these
# logs are the only decoded record of several families' actual failure payloads.
#
# There are TWO, kept in separate dated directories with separate expected
# files, because a corpus pins patterns against the wording that existed when it
# was captured and the two wordings differ. Merging them into one frozen set
# would make it impossible to say which log a count came from — the exact
# confusion that let `org-blocks-ref-diverge` sit overbroad:
#
#   fixture-logs-2026-07-31  the original nightly corpus. Its block-divergence
#                            payloads predate `render_block_diff`, so they carry
#                            a whole-snapshot dump and cannot pin any pattern
#                            that reads the structured diff body.
#   fixture-logs-2026-09-19  wave-14/15b land-gate logs in the CURRENT format.
#                            This is what pins the narrowed
#                            `org-blocks-ref-diverge` and the three
#                            `ref-diverge-*` rows split out of it.
#
# Usage:
#   scripts/keystone-known-reds-fixture.sh            # check against expected
#   scripts/keystone-known-reds-fixture.sh --bless    # regenerate expected
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
regressions="$repo_root/crates/holon-integration-tests/hand-authored-regressions"
corpus="$regressions/fixture-logs-2026-07-31"
corpus_current="$regressions/fixture-logs-2026-09-19"

# The four runs that exited non-zero. The corpus also holds the four green runs
# of the same nights (as the base-rate record: 4 red / 8 total = 50%), but only
# failed runs are classifier input.
red_runs=(
    keystone-nightly-20260731-083505-run2
    keystone-nightly-20260731-191108-run1
    keystone-nightly-20260731-191108-run2
    keystone-nightly-20260731-193535-run1
)

# Two wave-14/15b land-gate logs, together covering every shape the
# block-divergence family emits in the CURRENT `render_block_diff` format: the
# keystone run carries the `parent_id` shape, the composed run carries both
# `content` shapes AND the two genuine set-membership excesses that the narrowed
# `org-blocks-ref-diverge` still claims.
red_runs_current=(
    land-w14-keystone-full-1789644096
    A2-lib-and-composed-1789580360
)

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
: >"$work/empty.log"

bless=0
[ "${1-}" = "--bless" ] && bless=1

# Rows the LIVE registry still carries as `known-red`. Expectations for any
# other key are dropped at check time — a fixed family is absence, not drift.
live="$work/live-keys.txt"
awk -F'|' '/^\| *`/ {
    gsub(/^ *`|` *$/, "", $2); gsub(/^ *| *$/, "", $3)
    if ($3 == "known-red") print $2
}' "$repo_root/docs/Testing/KeystoneKnownReds.md" >"$live"

# Replay one corpus and pin its per-key hit counts.
#
# Only per-key hit counts are pinned. The novel count is deliberately NOT
# pinned: fixing a family removes its row, after which that family's archived
# panics correctly classify as novel. Pinning the tally would make every
# successful fix look like a fixture regression — the guard would punish
# exactly the work it exists to protect.
check_corpus() {
    local label="$1" dir="$2"
    shift 2
    local exp="$dir/expected-classification.txt"
    local logs=() run src
    for run in "$@"; do
        src="$dir/$run.log.zst"
        [ -f "$src" ] || { echo "[fixture] missing corpus log: $src" >&2; exit 2; }
        zstd -dqf -o "$work/$run.log" "$src"
        logs+=("$work/$run.log")
    done

    # The classifier exits 1 while any novel signature remains; that is a
    # verdict about the corpus, not about this check, so capture it rather than
    # inherit it.
    local raw="$work/$label-classification.txt"
    set +e
    "$repo_root/scripts/keystone-known-reds.sh" "${logs[@]}" >"$raw" 2>&1
    set -e

    local summary="$work/$label-summary.txt"
    grep -E '^WARN known-red' "$raw" \
        | sed -E 's/^(WARN known-red \[[a-z-]+\] x[0-9]+).*/\1/' >"$summary"

    if [ "$bless" -eq 1 ]; then
        cp "$summary" "$exp"
        echo "[fixture] blessed $label:"
        sed 's/^/          /' "$exp"
        return
    fi

    [ -f "$exp" ] || {
        echo "[fixture] no expected file for $label — run with --bless" >&2
        exit 2
    }

    local filtered="$work/$label-expected-filtered.txt" key
    : >"$filtered"
    while read -r line; do
        key=$(printf '%s' "$line" | sed -E 's/^WARN known-red \[([a-z-]+)\].*/\1/')
        if grep -qx -- "$key" "$live"; then
            printf '%s\n' "$line" >>"$filtered"
        else
            echo "[fixture] skipping [$key] — no longer a known-red row (fixed)."
        fi
    done <"$exp"

    if ! diff -u "$filtered" "$summary"; then
        echo ""
        echo "[fixture] FAIL: a still-registered known-red row no longer classifies the"
        echo "          ARCHIVED payload it was written for ($label). The corpus is"
        echo "          immutable, so this is a Match pattern in"
        echo "          docs/Testing/KeystoneKnownReds.md that drifted from its own"
        echo "          evidence. Fix the pattern, then --bless."
        echo "          (Removing a row because its family is FIXED does not land here —"
        echo "          those keys are skipped, see the lines above.)"
        exit 1
    fi
    echo "[fixture] PASS — classifier verdict on the $label corpus is unchanged."
}

check_corpus 2026-07-31 "$corpus" "${red_runs[@]}"
check_corpus 2026-09-19 "$corpus_current" "${red_runs_current[@]}"

if [ "$bless" -eq 1 ]; then
    exit 0
fi

# ---------------------------------------------------------------------------
# Outcome classification. Distinct from the pattern-drift check above: this
# pins what the classifier says about a log BEFORE any signature matching —
# whether it decides the run failed, passed, or said nothing. Nothing here is
# blessable; each case asserts one fixed verdict and exit code.
#
# The green cases are the same nights' passing runs, not synthetic text: a green
# keystone log is thousands of lines of invariant chatter, and it was exactly
# that chatter the classifier had to stop reading as a failure.
outcome_fail=0
expect_outcome() {
    local label="$1" want_code="$2" want_line="$3"
    shift 3
    local out code=0
    out=$("$repo_root/scripts/keystone-known-reds.sh" "$@" 2>&1) || code=$?
    if [ "$code" -ne "$want_code" ] || ! grep -q -- "$want_line" <<<"$out"; then
        echo "[fixture] FAIL outcome/$label: want exit $want_code and a line matching"
        echo "          '$want_line'; got exit $code:"
        printf '%s\n' "$out" | tail -20 | sed 's/^/          | /'
        outcome_fail=1
        return
    fi
    echo "[fixture] ok outcome/$label (exit $code)"
}

green_runs=(
    keystone-nightly-20260731-080542-run1
    keystone-nightly-20260731-081152-run1
    keystone-nightly-20260731-081152-run2
    keystone-nightly-20260731-083505-run1
)
green_logs=()
for run in "${green_runs[@]}"; do
    src="$corpus/$run.log.zst"
    [ -f "$src" ] || { echo "[fixture] missing corpus log: $src" >&2; exit 2; }
    zstd -dq -o "$work/$run.log" "$src"
    green_logs+=("$work/$run.log")
done

# Synthetic shapes no real corpus run happens to have. Committed `.log.zst` like
# the corpus: `.gitignore`'s `*.log` would otherwise drop them from every clean
# checkout, taking the cases that depend on them with it.
for run in failed-no-signature no-outcome green-with-stray-error; do
    src="$corpus/synthetic/$run.log.zst"
    [ -f "$src" ] || { echo "[fixture] missing synthetic log: $src" >&2; exit 2; }
    zstd -dq -o "$work/$run.log" "$src"
done

expect_outcome green-single 0 '^\[known-reds\] PASS: 1 green run' "${green_logs[0]}"
expect_outcome green-all 0 '^\[known-reds\] PASS: 4 green run' "${green_logs[@]}"
# A green log alongside a red one must not dilute the red one's verdict.
expect_outcome green-plus-red 1 '^\[known-reds\] FAIL: ' \
    "${green_logs[0]}" "$work/${red_runs[0]}.log"
# The constraint this fix must not break: a run that genuinely failed with no
# extractable panic stays NOVEL.
expect_outcome failed-no-signature 1 'run failed but no panic signature' \
    "$work/failed-no-signature.log"
# A test printing an `error: ` line on its own stdout has not failed. Only the
# harness's own failure vocabulary outranks an explicit `test result: ok.`.
expect_outcome green-with-stray-error 0 '^\[known-reds\] PASS: 1 green run' \
    "$work/green-with-stray-error.log"
# Truncated and missing both exit 3, so each pins the MESSAGE only its own path
# produces — asserting the shared exit code would let either case stand in for
# the other, and truncation detection could be deleted unnoticed.
expect_outcome truncated 3 'the log states no pass/fail' "$work/no-outcome.log"
expect_outcome empty 3 'the log states no pass/fail' "$work/empty.log"
expect_outcome missing 3 '^\[known-reds\] UNREADABLE: ' "$work/does-not-exist.log"

# nextest's summary counts a killed test as `timed out`, not `failed`, and the
# test printed no panic: the status line is the only failure evidence.
cat >"$work/timeout-only.log" <<'EOF'
        PASS [   0.010s] (1/2) holon-gpui::main layout_smoke::a
     TIMEOUT [ 120.034s] (2/2) holon-gpui::main gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed
     Summary [ 120.355s] 2 tests run: 1 passed, 1 timed out, 0 skipped
     TIMEOUT [ 120.034s] (2/2) holon-gpui::main gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed
EOF
expect_outcome timeout-only 1 '^PRIMARY: \[novel\] .* TIMEOUT: holon-gpui::main gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed$' \
    "$work/timeout-only.log"
# cargo test prints the expected panic of a passing `#[should_panic]` test.
cat >"$work/should-panic-only.log" <<'EOF'
thread 'mistyped_initial_state_key_is_loud' (1) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:166:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state"
test mistyped_initial_state_key_is_loud - should panic ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
EOF
expect_outcome should-panic-only 0 '^\[known-reds\] PASS: 1 green run' "$work/should-panic-only.log"
# The output starts with the input log's BASE:/TREE: lines, wherever the log has them.
{ echo "[flow] job c1"; echo "BASE: e213097550e6"; echo "TREE: 627b5fff5da2"; cat "$work/should-panic-only.log"; } >"$work/headed.log"
headed_code=0
headed_out=$("$repo_root/scripts/keystone-known-reds.sh" "$work/headed.log" 2>&1) || headed_code=$?
if [ "$headed_code" -eq 0 ] && [ "$(head -2 <<<"$headed_out")" = $'BASE: e213097550e6\nTREE: 627b5fff5da2' ]; then
    echo "[fixture] ok outcome/headed-log-keeps-header"
else
    echo "[fixture] FAIL outcome/headed-log-keeps-header: the output does not start with the log's BASE:/TREE: lines:"
    { echo "exit $headed_code"; head -5 <<<"$headed_out"; } | sed 's/^/          | /'
    outcome_fail=1
fi
# A second panic on the thread of a passing should_panic test is not the
# expected one, whichever of the two it is.
cat >"$work/should-panic-name-collision.log" <<'EOF'
thread 'dup_name' panicked at crates/x/src/lib.rs:7:1:
expected loud refusal
test dup_name - should panic ... ok
thread 'dup_name' panicked at crates/y/src/other.rs:99:3:
REAL PRODUCT BUG: the projection vanished
test result: FAILED. 1 passed; 1 failed; 0 ignored
EOF
expect_outcome should-panic-name-collision 1 '^ *1 REAL PRODUCT BUG: the projection vanished$' \
    "$work/should-panic-name-collision.log"
# Test names repeat across binaries. The passing should_panic test's own panic
# was captured, so the one printed panic belongs to the other binary's test.
cat >"$work/should-panic-other-binary.log" <<'EOF'
     Running tests/a.rs (target/debug/deps/a-0000000000000001)
test tests::refuses - should panic ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/b.rs (target/debug/deps/b-0000000000000002)
thread 'tests::refuses' (2) panicked at crates/y/tests/b.rs:12:5:
REAL PRODUCT BUG: the refusal was never raised
test tests::refuses ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
EOF
expect_outcome should-panic-other-binary 1 '^PRIMARY: \[novel\] .*REAL PRODUCT BUG: the refusal was never raised$' \
    "$work/should-panic-other-binary.log"
# Stdout can land between `thread '` and the thread name of a panic header,
# leaving the name at the start of the next line. The panic is still that
# test's: expected when it reports `- should panic ... ok`, else a failure.
split_header_log() {
    cat <<EOF
     Running tests/main.rs (target/debug/deps/integration-0000000000000001)

running 3 tests

thread 'test recorded_env_flag_mismatch_is_reported ... ok
$1' (2) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:255:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state"
test $1 $2
thread 'hand_authored_keystone_regressions' (3) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  field deltas (1):\n    block:a: content: sut=\"x\" ref=\"y\"\n")]
test hand_authored_keystone_regressions ... FAILED
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
EOF
}
split_header_log mistyped_initial_state_key_is_loud '- should panic ... ok' >"$work/should-panic-split-header.log"
expect_outcome should-panic-split-header 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic(s), 0 novel' \
    "$work/should-panic-split-header.log"
split_header_log mistyped_initial_state_key_is_loud '- should panic ... FAILED' >"$work/should-panic-split-header-failed.log"
expect_outcome should-panic-split-header-failed 1 '^PRIMARY: \[novel\] .*unknown top-level key "initail_state"$' \
    "$work/should-panic-split-header-failed.log"
split_header_log mistyped_initial_state_key_is_loud '... FAILED' >"$work/split-header-plain-failed.log"
expect_outcome split-header-plain-failed 1 '^PRIMARY: \[novel\] .*unknown top-level key "initail_state"$' \
    "$work/split-header-plain-failed.log"
# libtest writes a test's verdict apart from its `test <t> ... ` head, so
# stdout of another thread can glue text after the `ok` or push the verdict
# onto a later line. The verdict is still that test's.
split_verdict_log() {
    cat <<EOF
     Running tests/hand_authored_regressions.rs (target/debug/deps/h-0000000000000001)

running 2 tests
thread 'malformed_also_check_after_is_refused' (2) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:212:9:
hand-authored regression "<inline>":1 case "n": \`also_check_after\` holds "2", not a transition number
$1
thread 'hand_authored_keystone_regressions' (3) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  field deltas (1):\n    block:a: content: sut=\"x\" ref=\"y\"\n")]
test hand_authored_keystone_regressions ... FAILED
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
EOF
}
split_verdict_log 'test malformed_also_check_after_is_refused - should panic ... ok[inv-sql-budget] UndoLastMutation: not judged — started from an unswept state' \
    >"$work/should-panic-ok-trailing-text.log"
expect_outcome should-panic-ok-trailing-text 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic(s), 0 novel' \
    "$work/should-panic-ok-trailing-text.log"
split_verdict_log $'test malformed_also_check_after_is_refused - should panic ... \nApplying transition 4/6: ()\n\nok' \
    >"$work/should-panic-ok-later-line.log"
expect_outcome should-panic-ok-later-line 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic(s), 0 novel' \
    "$work/should-panic-ok-later-line.log"
split_verdict_log $'test malformed_also_check_after_is_refused - should panic ... \nApplying transition 4/6: ()\nFAILED\nok' \
    >"$work/should-panic-failed-later-line.log"
expect_outcome should-panic-failed-later-line 1 '^PRIMARY: \[novel\] .*not a transition number$' \
    "$work/should-panic-failed-later-line.log"
# nextest status lines, in the shapes nextest-runner's reporter prints them
# (reporter/displayer/imp.rs `status_str` / `short_status_str`). A test killed
# or leaking after a passing body printed no panic, so the status line is its
# signature, under the long label; a log cut off before `Summary` still says so.
cat >"$work/nextest-labels-truncated.log" <<'EOF'
        PASS [   0.010s] (1/6) holon-gpui::main layout_smoke::a
   LEAK-FAIL [   0.312s] (2/6) holon-gpui::main layout_smoke::leaky
       XFAIL [   0.001s] (3/6) holon-gpui::main layout_smoke::unexecutable
     SIGSEGV [   0.312s] (4/6) holon-gpui::main layout_smoke::boom
 ABORT SIG 9 [   0.312s] (5/6) holon-gpui::main layout_smoke::killed
 TERMINATING [>120.000s] (─────────) holon-gpui::bin timed
     TIMEOUT [ 120.034s] (6/6) holon-gpui::bin timed
Canceling due to test failure
EOF
for want in 'LEAK-FAIL: holon-gpui::main layout_smoke::leaky' 'XFAIL: holon-gpui::main layout_smoke::unexecutable' \
    'SIGSEGV: holon-gpui::main layout_smoke::boom' 'ABORT SIG 9: holon-gpui::main layout_smoke::killed' \
    'TIMEOUT: holon-gpui::bin timed'; do
    expect_outcome "nextest-label/${want%%:*}/${want##* }" 1 "^ *1 $want\$" "$work/nextest-labels-truncated.log"
done
# A `FAIL + LEAK` test failed in its body: its own panic is the signature, and a
# known one stays known.
cat >"$work/fail-leak-known-panic.log" <<'EOF'
 FAIL + LEAK [   6.486s] (197/682) holon-app::main integration_toggle_round_trip::a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection
  stderr ───
    thread 'a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection' (2) panicked at crates/holon-app/tests/integration_toggle_round_trip.rs:83:9:
    the seeded Integrations section never showed 'todoist' as true — it is still false. The operation wrote the state file, so the break is between the store's signal and the mirror.
     Summary [  25.469s] 682 tests run: 681 passed (1 leaky), 1 failed, 1 skipped
 FAIL + LEAK [   6.486s] (197/682) holon-app::main integration_toggle_round_trip::a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection
EOF
expect_outcome fail-leak-known-panic 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red' "$work/fail-leak-known-panic.log"
# A fixed-pending-soak row does not cover its signature, so its return is novel.
cat >"$work/move-between-files-read-repeat.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1553:13:
reconciled composed sequence diverged from the oracle: [("inv-sql-budget", "[inv-sql-budget] 1 budget violation(s):\n  MoveBlockBetweenFiles.sql_read_repeat: one binding-set of `SELECT b.id, b.parent_id, b.sort_key, b.content, b.content_type, b.source_language, b.sour…` re-executed 124x, over the redundancy ratchet 64 — the re-execution defect GREW; find the new consumer, do not raise the ratchet")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome move-between-files-read-repeat 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' \
    "$work/move-between-files-read-repeat.log"
# A row names ONE violation: the signature is known only when each of its
# violations is, and a pattern starts where its violation starts.
cat >"$work/violation-one-known.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-sql-budget", "[inv-sql-budget] 1 budget violation(s):\n  OpenTabViaModifierClick.sql_writes: 2 exceeds expected 1 + tolerance 0 = 1; writes: [`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>; `UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>]")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/one-known 0 '^PRIMARY: \[known-red:cmd-click-open-tab-integration-state-write\]' "$work/violation-one-known.log"
# The integration_state row takes an overshoot of exactly one write at expected
# counts 0-8, and only beside its own `<no-parent>` status write.
cmd_click_log() {
    local name="$1" counts="$2" writes="$3"
    {
        printf '%s\n' "thread 'hand_authored_keystone_regressions' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1481:13:"
        printf '%s\n' "reconciled composed sequence diverged from the oracle: [(\"inv-sql-budget\", \"[inv-sql-budget] 1 budget violation(s):\\n  OpenTabViaModifierClick.sql_writes: $counts; writes: [$writes]\")]"
        printf '%s\n' 'test hand_authored_keystone_regressions ... FAILED'
        printf '%s\n' 'test result: FAILED. 18 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s'
    } >"$work/$name.log"
}
status_write='`UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>'
unwatch_write='`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>'
cmd_click_log cmd-click-overshoot-one-at-two "3 exceeds expected 2 + tolerance 0 = 2" \
    "$unwatch_write; $status_write; $unwatch_write"
expect_outcome violation/cmd-click-overshoot-one-at-two 0 '^PRIMARY: \[known-red:cmd-click-open-tab-integration-state-write\]' \
    "$work/cmd-click-overshoot-one-at-two.log"
cmd_click_log cmd-click-overshoot-two "3 exceeds expected 1 + tolerance 0 = 1" \
    "$status_write; $unwatch_write; $unwatch_write"
expect_outcome violation/cmd-click-overshoot-two 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' \
    "$work/cmd-click-overshoot-two.log"
cmd_click_log cmd-click-without-status-write "3 exceeds expected 2 + tolerance 0 = 2" \
    "$unwatch_write; $unwatch_write; $unwatch_write"
expect_outcome violation/cmd-click-without-status-write 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' \
    "$work/cmd-click-without-status-write.log"
cmd_click_log cmd-click-status-write-with-parent "3 exceeds expected 2 + tolerance 0 = 2" \
    "$unwatch_write; \`UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  \` origin=sync_integration ▸ set_status; $unwatch_write"
expect_outcome violation/cmd-click-status-write-with-parent 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' \
    "$work/cmd-click-status-write-with-parent.log"
cat >"$work/violation-longer-transition-name.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-sql-budget", "[inv-sql-budget] 1 budget violation(s):\n  ReOpenTabViaModifierClick.sql_writes: 2 exceeds expected 1 + tolerance 0 = 1; writes: [`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>; `UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>]")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/longer-transition-name 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-longer-transition-name.log"
cat >"$work/violation-known-plus-novel-budget.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-sql-budget", "[inv-sql-budget] 2 budget violation(s):\n  OpenTabViaModifierClick.sql_writes: 2 exceeds expected 1 + tolerance 0 = 1; writes: [`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>; `UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>]\n  SplitBlock.sql_writes: 3 exceeds expected 1 + tolerance 0 = 1; writes: [`INSERT INTO block (id, parent_id, content) VALUES (?, ?, ?)` origin=<no-parent>]")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/known-plus-novel-budget 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-known-plus-novel-budget.log"
cat >"$work/violation-second-novel-invariant.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/loro", "[inv-blocks-match-ref/loro] fields diverge from reference"), ("inv-sql-budget", "[inv-sql-budget] 1 budget violation(s):\n  OpenTabViaModifierClick.sql_writes: 2 exceeds expected 1 + tolerance 0 = 1; writes: [`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>; `UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>]")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/second-novel-invariant 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-second-novel-invariant.log"
# A companion counts only beside its row's own violation.
cat >"$work/violation-companions-beside-their-row.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  inv-blocks-match-ref/org: 14 blocks, reference: 14 blocks\n  only in inv-blocks-match-ref/org (0): []\n  only in reference (0): []\n  field deltas (1):\n    block:wbi7-a: content: sut=\"Buy milk\" ref=\"Buy milk oat\"\n"), ("inv-blocks-match-ref/loro", "[inv-blocks-match-ref/loro] fields diverge from reference\n  inv-blocks-match-ref/loro: 17 blocks, reference: 17 blocks\n  only in inv-blocks-match-ref/loro (0): []\n  only in reference (0): []\n  field deltas (1):\n    block:wbi7-a: content: sut=\"Buy milk\" ref=\"Buy milk oat\"\n"), ("inv-blocks-match-ref/block_raw", "[inv-blocks-match-ref/block_raw] block block:wbi7-a diverges from reference on Content\n  inv-blocks-match-ref/block_raw: Block {\n    id: EntityUri(\"block:wbi7-a\"),\n    parent_id: EntityUri(\"block:ref-doc-0\"),\n    tags: Tags(\n        {},\n    ),\n    requires: [],\n    advice_suppressed: [],\n    contributes_to: [],\n    content: \"Buy milk\",\n    content_type: Text,\n    source_language: None,\n    source_name: None,\n    properties: {},\n    marks: None,\n    collapsed: false,\n    widget_only: false,\n    block_type: None,\n    created_at: 0,\n    updated_at: 0,\n}\n  reference: Block {\n    id: EntityUri(\"block:wbi7-a\"),\n    parent_id: EntityUri(\"block:ref-doc-0\"),\n    tags: Tags(\n        {},\n    ),\n    requires: [],\n    advice_suppressed: [],\n    contributes_to: [],\n    content: \"Buy milk oat\",\n    content_type: Text,\n    source_language: None,\n    source_name: None,\n    properties: {},\n    marks: None,\n    collapsed: false,\n    widget_only: false,\n    block_type: None,\n    created_at: 0,\n    updated_at: 0,\n}"), ("inv-blocks-match-ref/matview", "[inv-blocks-match-ref/matview] fields diverge from reference\n  inv-blocks-match-ref/matview: 17 blocks, reference: 17 blocks\n  only in inv-blocks-match-ref/matview (0): []\n  only in reference (0): []\n  field deltas (1):\n    block:wbi7-a: content: sut=\"Buy milk\" ref=\"Buy milk oat\"\n"), ("inv-block-content/block_raw", "[inv-block-content/block_raw] block block:wbi7-a content diverges:\n  ref:       \"Buy milk oat\"\n  block_raw: \"Buy milk\""), ("inv-block-content/sql", "[inv-block-content/sql] block block:wbi7-a content diverges:\n  ref: \"Buy milk oat\"\n  sql: \"Buy milk\"")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/companions-beside-their-row 0 '^PRIMARY: \[known-red:ref-diverge-content-text-drift\]' "$work/violation-companions-beside-their-row.log"
cat >"$work/violation-companions-alone.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/block_raw", "[inv-blocks-match-ref/block_raw] block block:wbi7-a diverges from reference on Content\n  inv-blocks-match-ref/block_raw: Block {\n    id: EntityUri(\"block:wbi7-a\"),\n    parent_id: EntityUri(\"block:ref-doc-0\"),\n    tags: Tags(\n        {},\n    ),\n    requires: [],\n    advice_suppressed: [],\n    contributes_to: [],\n    content: \"Buy milk\",\n    content_type: Text,\n    source_language: None,\n    source_name: None,\n    properties: {},\n    marks: None,\n    collapsed: false,\n    widget_only: false,\n    block_type: None,\n    created_at: 0,\n    updated_at: 0,\n}\n  reference: Block {\n    id: EntityUri(\"block:wbi7-a\"),\n    parent_id: EntityUri(\"block:ref-doc-0\"),\n    tags: Tags(\n        {},\n    ),\n    requires: [],\n    advice_suppressed: [],\n    contributes_to: [],\n    content: \"Buy milk oat\",\n    content_type: Text,\n    source_language: None,\n    source_name: None,\n    properties: {},\n    marks: None,\n    collapsed: false,\n    widget_only: false,\n    block_type: None,\n    created_at: 0,\n    updated_at: 0,\n}"), ("inv-block-content/block_raw", "[inv-block-content/block_raw] block block:wbi7-a content diverges:\n  ref:       \"Buy milk oat\"\n  block_raw: \"Buy milk\""), ("inv-block-content/sql", "[inv-block-content/sql] block block:wbi7-a content diverges:\n  ref: \"Buy milk oat\"\n  sql: \"Buy milk\"")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/companions-alone 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-companions-alone.log"
cat >"$work/violation-page-without-own-file.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-every-page-has-its-own-file", "[inv-every-page-has-its-own-file] 1 page(s) not homed to exactly one own file: [\"page `61133fe7-d4d5-ab1c-24e4-110da5f42293` owns NO file (fileless — content lives only in the store; writeback must MATERIALIZE it into `…/61133fe7-d4d5-ab1c-24e4-110da5f42293.org`)\"]")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/page-without-own-file 0 '^PRIMARY: \[known-red:page-without-own-file\]' "$work/violation-page-without-own-file.log"
# An empty created file whose `#+ID:` header never reaches disk fires both
# file invariants; the one-byte echo-loop shape beside a fileless page must not
# be absorbed by that row.
cat >"$work/violation-empty-created-file-header-never-written.log" <<'EOF'
thread 'joining_a_received_page_away_is_refused_and_leaves_it_placed' (143066426) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1607:13:
reconciled composed sequence diverged from the oracle: [("inv-org-render-fixed-point", "[inv-org-render-fixed-point] render != disk PERSISTED for 5s — not a transient projection lag but a real echo-loop / oscillation: the next re_render_all_tracked would keep rewriting the file.\n--- disk (0 bytes) [/private/var/folders/hc/2q6czxpx6j9_87bq787752jw0000gn/T/.tmpheOSSG/doc_0.org] ---\n\n--- rendered from SQL (43 bytes) ---\n#+ID: 317c2395-de93-fe93-d5fa-5db9c57da4db\n"), ("inv-every-page-has-its-own-file", "[inv-every-page-has-its-own-file] 1 page(s) not homed to exactly one own file: [\"page `317c2395-de93-fe93-d5fa-5db9c57da4db` owns NO file (fileless — content lives only in the store; writeback must MATERIALIZE it into `…/317c2395-de93-fe93-d5fa-5db9c57da4db.org`)\"]")]
test joining_a_received_page_away_is_refused_and_leaves_it_placed ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 37 filtered out; finished in 33.21s
EOF
expect_outcome violation/empty-created-file-header-never-written 0 '^PRIMARY: \[known-red:empty-created-file-header-never-written' "$work/violation-empty-created-file-header-never-written.log"
cat >"$work/violation-one-byte-echo-loop-beside-fileless-page.log" <<'EOF'
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1607:13:
reconciled composed sequence diverged from the oracle: [("inv-org-render-fixed-point", "[inv-org-render-fixed-point] render != disk PERSISTED for 5s — not a transient projection lag but a real echo-loop / oscillation: the next re_render_all_tracked would keep rewriting the file.\n--- disk (277 bytes) [/private/var/folders/hc/2q6czxpx6j9_87bq787752jw0000gn/T/.tmpfaBZ5X/forward-edge-page.org] ---\n#+ID: forward-edge-page\n* fe-parent\n:PROPERTIES:\n:ID: fe-parent\n:END:\n* fe-blocked\n:PROPERTIES:\n:ID: fe-blocked\n:REQUIRES: fe-target\n:END:\n** rlJWtK\n:PROPERTIES:\n:ID: block-4\n:column-priority: L\n:END:\nfoVL623I188Qt\ndTL dC Nh8GfJ M\n* fe-target\n:PROPERTIES:\n:ID: fe-target\n:END:\n\n--- rendered from SQL (278 bytes) ---\n#+ID: forward-edge-page\n* fe-parent\n:PROPERTIES:\n:ID: fe-parent\n:END:\n* fe-blocked\n:PROPERTIES:\n:ID: fe-blocked\n:REQUIRES: fe-target\n:END:\n** rlJWtK\n:PROPERTIES:\n:ID: block-4\n:column-priority: L\n:END:\nfoVL623I188Qt\ndTL dC Nh8GfJ M\n\n* fe-target\n:PROPERTIES:\n:ID: fe-target\n:END:\n"), ("inv-every-page-has-its-own-file", "[inv-every-page-has-its-own-file] 1 page(s) not homed to exactly one own file: [\"page `61133fe7-d4d5-ab1c-24e4-110da5f42293` owns NO file (fileless — content lives only in the store; writeback must MATERIALIZE it into `…/61133fe7-d4d5-ab1c-24e4-110da5f42293.org`)\"]")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/one-byte-echo-loop-beside-fileless-page 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-one-byte-echo-loop-beside-fileless-page.log"
# A double-homed page is a different defect from a fileless one, so it stays
# novel beside the empty-file render shape.
cat >"$work/violation-empty-created-file-beside-double-homed-page.log" <<'EOF2'
thread 'joining_a_received_page_away_is_refused_and_leaves_it_placed' (143066426) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1607:13:
reconciled composed sequence diverged from the oracle: [("inv-org-render-fixed-point", "[inv-org-render-fixed-point] render != disk PERSISTED for 5s — not a transient projection lag but a real echo-loop / oscillation: the next re_render_all_tracked would keep rewriting the file.\n--- disk (0 bytes) [/private/var/folders/hc/2q6czxpx6j9_87bq787752jw0000gn/T/.tmpheOSSG/doc_0.org] ---\n\n--- rendered from SQL (43 bytes) ---\n#+ID: 317c2395-de93-fe93-d5fa-5db9c57da4db\n"), ("inv-every-page-has-its-own-file", "[inv-every-page-has-its-own-file] 1 page(s) not homed to exactly one own file: [\"page `317c2395-de93-fe93-d5fa-5db9c57da4db` is DOUBLE-HOMED across 2 files [\\\"doc_0.org\\\", \\\"317c2395-de93-fe93-d5fa-5db9c57da4db.org\\\"]\"]")]
test joining_a_received_page_away_is_refused_and_leaves_it_placed ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 37 filtered out; finished in 33.21s
EOF2
expect_outcome violation/empty-created-file-beside-double-homed-page 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-empty-created-file-beside-double-homed-page.log"
# A budget message also carries the non-SQL ceilings (wall, query, memory).
cat >"$work/violation-known-plus-novel-memory-budget.log" <<'EOF'
[keystone-sample] cases=16 attempted=16 completed=15 floor=8 known-red-hits=1 verdict=OK
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-sql-budget", "[inv-sql-budget] 2 budget violation(s):\n  OpenTabViaModifierClick.sql_writes: 2 exceeds expected 1 + tolerance 0 = 1; writes: [`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>; `UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>]\n  OpenTabViaModifierClick.rss_delta: +60.2MB exceeds limit 50MB (before=300MB, after=360MB)")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome violation/known-plus-novel-memory-budget 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/violation-known-plus-novel-memory-budget.log"
# A cut-and-paste that leaves one child id in two org files: only a verdict
# whose every swallowed problem is that error for those ids and files is known.
cut_paste_dup_problem=$(cat <<'EOF'
\n  [ERROR] holon_filesystem::file_sync_controller (crates/holon-filesystem/src/file_sync_controller.rs:3025): [FileSyncController] DUPLICATE BLOCK ID: 'block:kd-1-a' is declared by BOTH /private/var/folders/hc/T/.tmpDupId/Journals/2026-01-16.org and /private/var/folders/hc/T/.tmpDupId/forward-edge-page.org, so /private/var/folders/hc/T/.tmpDupId/forward-edge-page.org is NOT ingested — QUARANTINING this file from write-back, because ingesting it would merge its headlines into the other file's document and rewrite one file with the other's content. Give the duplicated headline a fresh `:ID:` in one of the two files, or delete the stray copy. The claimant is whichever file was ingested FIRST, and the vault walk order is arbitrary, so which of the two wins can differ between runs. Repeats for this path log at DEBUG. (slug=block:kd-1-a claimed_by=/private/var/folders/hc/T/.tmpDupId/Journals/2026-01-16.org refused=/private/var/folders/hc/T/.tmpDupId/forward-edge-page.org)
EOF
)
cut_paste_dup_log() {
    local out="$1" count="$2" problems="$3"
    {
        echo "thread 'hand_authored_regressions::hand_authored_keystone_regressions' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1607:13:"
        echo "reconciled composed sequence diverged from the oracle: [(\"inv-no-observed-errors\", \"[inv-no-observed-errors] $count swallowed problem(s) (ERROR log / panic) during this transition — a green run is hiding a real failure:$problems\")]"
        echo "test hand_authored_regressions::hand_authored_keystone_regressions ... FAILED"
        echo "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 634 filtered out; finished in 3.68s"
    } >"$out"
}
cut_paste_dup_log "$work/cut-paste-dup-known.log" 1 "$cut_paste_dup_problem"
expect_outcome cut-paste-dup/known 0 '^PRIMARY: \[known-red:cut-paste-duplicate-block-id\]' "$work/cut-paste-dup-known.log"
cut_paste_dup_log "$work/cut-paste-dup-two-known.log" 2 "$cut_paste_dup_problem${cut_paste_dup_problem//kd-1-a/kd-1-b}"
expect_outcome cut-paste-dup/two-known 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic(s), 0 novel' "$work/cut-paste-dup-two-known.log"
cut_paste_dup_log "$work/cut-paste-dup-other-id.log" 1 "${cut_paste_dup_problem//kd-1-a/fe-parent}"
expect_outcome cut-paste-dup/other-id 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/cut-paste-dup-other-id.log"
cut_paste_dup_log "$work/cut-paste-dup-other-files.log" 1 "${cut_paste_dup_problem//forward-edge-page.org/doc_0.org}"
expect_outcome cut-paste-dup/other-files 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/cut-paste-dup-other-files.log"
cut_paste_dup_log "$work/cut-paste-dup-beside-another-error.log" 2 "$cut_paste_dup_problem\n  [ERROR] holon_filesystem::file_sync_controller (crates/holon-filesystem/src/file_sync_controller.rs:3100): write-back of doc_0.org failed"
expect_outcome cut-paste-dup/beside-another-error 1 '^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red' "$work/cut-paste-dup-beside-another-error.log"
# The sample rules (`[keystone-sample]`) belong to the HEADLESS keystone, which
# is the only test that discloses one. The windowed keystone's name contains the
# headless name as a substring, so these three cases pin which log each rule
# reaches: a registered windowed red stays a pass-with-note, a headless run that
# never disclosed its sample is a failure, and so is a disclosed breach.
cat >"$work/windowed-known-red.log" <<'EOF'
    FAIL [  31.004s] (1/19) holon-gpui::main gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed
  stderr ───
    thread 'gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed' (2) panicked at crates/holon-gpui/tests/gpui_composed_windowed_loop.rs:120:9:
    [SplitBlock/keystroke] cannot place the caret on block:abc — editable surface not projected by this driver
     Summary [  31.004s] 19 tests run: 18 passed, 1 failed, 0 skipped
    FAIL [  31.004s] (1/19) holon-gpui::main gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed
EOF
expect_outcome windowed-known-red-discloses-no-sample 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red' \
    "$work/windowed-known-red.log"
# A cargo build line names the keystone's test target without running it, so
# it must not make a log a keystone run that owes a sample disclosure.
cat >"$work/build-warning-names-keystone.log" <<'EOF'
warning: `holon-integration-tests` (test "integration") generated 1 warning (1 duplicate)
 FAIL + LEAK [   6.486s] (197/682) holon-app::main integration_toggle_round_trip::a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection
  stderr ───
    thread 'a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection' (2) panicked at crates/holon-app/tests/integration_toggle_round_trip.rs:83:9:
    the seeded Integrations section never showed 'todoist' as true — it is still false. The operation wrote the state file, so the break is between the store's signal and the mirror.
     Summary [  25.469s] 682 tests run: 681 passed (1 leaky), 1 failed, 1 skipped
 FAIL + LEAK [   6.486s] (197/682) holon-app::main integration_toggle_round_trip::a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection
EOF
expect_outcome build-warning-names-keystone 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red' \
    "$work/build-warning-names-keystone.log"
cat >"$work/sample-undisclosed.log" <<'EOF'
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1553:13:
assertion `left == right` failed: per-tick reconcile: one synthetic per minted real id (syn=[], real=[EntityUri("block:259a9a58-18bb-4559-a589-87aee571c59e")], HeldCreates { parked: 0 })
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome sample-undisclosed 1 '^SAMPLE: UNDISCLOSED' "$work/sample-undisclosed.log"
cat >"$work/sample-undisclosed-nextest.log" <<'EOF'
    FAIL [  31.004s] (1/19) holon-integration-tests::integration general_e2e_composed_pbt::general_e2e_composed_pbt
  stderr ───
    thread 'main' (2) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1553:13:
    assertion `left == right` failed: per-tick reconcile: one synthetic per minted real id (syn=[], real=[EntityUri("block:259a9a58-18bb-4559-a589-87aee571c59e")], HeldCreates { parked: 0 })
     Summary [  31.004s] 19 tests run: 18 passed, 1 failed, 0 skipped
    FAIL [  31.004s] (1/19) holon-integration-tests::integration general_e2e_composed_pbt::general_e2e_composed_pbt
EOF
expect_outcome sample-undisclosed-nextest 1 '^SAMPLE: UNDISCLOSED' "$work/sample-undisclosed-nextest.log"
cat >"$work/sample-floor-breach.log" <<'EOF'
[keystone-sample] cases=16 attempted=4 completed=3 floor=8 known-red-hits=1 verdict=FLOOR-BREACH
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1553:13:
assertion `left == right` failed: per-tick reconcile: one synthetic per minted real id (syn=[], real=[EntityUri("block:259a9a58-18bb-4559-a589-87aee571c59e")], HeldCreates { parked: 0 })
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
expect_outcome sample-floor-breach 1 '^SAMPLE: cases=16 attempted=4 completed=3 floor=8 known-red-hits=1 verdict=FLOOR-BREACH$' \
    "$work/sample-floor-breach.log"
# One case per budget metric (the labels transition_budgets.rs `build_samples`
# writes): a known sql_writes violation joined with that metric's violation
# splits into the known piece and that metric's piece alone as the novel one.
known_budget_piece='OpenTabViaModifierClick.sql_writes: 2 exceeds expected 1 + tolerance 0 = 1; writes: [`DELETE FROM watch_context WHERE watch_key = ? AND nonce = ?` origin=<no-parent>; `UPDATE integration_state SET status = ? WHERE id = ? AND enabled =  ` origin=<no-parent>]'
while IFS='|' read -r metric piece; do
    log="$work/violation-budget-metric-$metric.log"
    cat >"$log" <<EOF
thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-sql-budget", "[inv-sql-budget] 2 budget violation(s):\n  $known_budget_piece\n  OpenTabViaModifierClick.$metric: $piece")]
test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s
EOF
    expect_outcome "violation/budget-metric-$metric" 1 "^ *1 .*\[inv-sql-budget\] 1 budget violation(s):\\\\n  OpenTabViaModifierClick\\.$metric: " "$log"
done <<'EOF'
sql_reads|40 exceeds expected 10 + tolerance 2 = 12
sql_read_repeat|one binding-set of `SELECT id FROM block WHERE parent_id = ?` re-executed 3x, limit 1
sql_writes|3 exceeds expected 1 + tolerance 0 = 1; writes: [`INSERT INTO block (id) VALUES (?)` origin=<no-parent>]
sql_ddl|1 exceeds expected 0 + tolerance 0 = 0
single_query|2400ms exceeds limit 2000ms
wall_time|31000ms exceeds limit 30000ms
settle|2600ms exceeds limit 2500ms (Loro→SQL convergence slow)
rss_delta|+60.2MB exceeds limit 50MB (before=300MB, after=360MB)
rss_cumulative|+520.0MB total growth exceeds limit 500MB (baseline=300MB, current=820MB)
EOF
# A test that returned `Err` printed no panic: its status line stands in, with
# the `Error:` it returned, so two root causes in one binary stay apart.
cat >"$work/fail-leak-no-panic.log" <<'EOF'
 FAIL + LEAK [   0.906s] (36/49) holon::main integration_tests::test_multiple_containers
  stderr ───
    Error: aborted by peer: the cryptographic handshake failed: error 120: peer doesn't support any known protocol
     Summary [  31.396s] 49 tests run: 48 passed, 1 failed, 0 skipped
EOF
expect_outcome fail-leak-no-panic 1 "^ *1 FAIL + LEAK: holon::main integration_tests::test_multiple_containers: Error: aborted by peer: the cryptographic handshake failed: error 120: peer doesn't support any known protocol\$" \
    "$work/fail-leak-no-panic.log"
cat >"$work/fail-two-errors.log" <<'EOF'
        FAIL [   1.973s] (356/893) holon::main e2e_backend_engine_test::test_basic_query_execution
  stdout ───

    running 1 test
    test test_basic_query_execution ... FAILED

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 5 filtered out; finished in 1.95s

  stderr ───
    Error: Failed to insert test data: Database error: Failed to prepare statement: Parse error: cannot modify materialized view block

        FAIL [   2.008s] (357/893) holon::main e2e_backend_engine_test::test_create_and_delete_workflow
  stderr ───
    Error: the certification harness must run

     Summary [ 319.449s] 893 tests run: 891 passed, 2 failed, 0 skipped
        FAIL [   1.973s] (356/893) holon::main e2e_backend_engine_test::test_basic_query_execution
        FAIL [   2.008s] (357/893) holon::main e2e_backend_engine_test::test_create_and_delete_workflow
error: test run failed
EOF
expect_outcome fail-two-errors/first 1 '^ *1 FAIL: holon::main e2e_backend_engine_test::test_basic_query_execution: Error: Failed to insert test data: .*cannot modify materialized view block$' \
    "$work/fail-two-errors.log"
expect_outcome fail-two-errors/second 1 '^ *1 FAIL: holon::main e2e_backend_engine_test::test_create_and_delete_workflow: Error: the certification harness must run$' \
    "$work/fail-two-errors.log"
# A panic belongs to the test whose output holds it: the captured block under
# its status line. A `harness = false` binary panics on `main`, not on a thread
# named after the test, and that panic still stands alone as the signature.
cat >"$work/known-panic-on-main.log" <<'EOF'
        PASS [  51.633s] (2807/6521) holon-gpui::main gpui_compose_sut_windowed::windowed_composed_sut_replays_a_fixture_via_replay_steps_green
        FAIL [  46.890s] (2808/6521) holon-gpui::gpui_sim_replay_capture gpui_sim_replay_capture
  stderr ───
    [Holon Sim Replay Capture] replaying "presskey_loro_split_backspace" (4 steps)
    thread 'main' (75718365) panicked at crates/holon-integration-tests/src/pbt/op_write_cap.rs:381:17:
    [SplitBlock/keystroke] cannot place the caret for content byte 0 on block:c1: editable surface not projected by this driver
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

     Summary [  46.900s] 2 tests run: 1 passed, 1 failed, 0 skipped
        FAIL [  46.890s] (2808/6521) holon-gpui::gpui_sim_replay_capture gpui_sim_replay_capture
error: test run failed
EOF
expect_outcome known-panic-on-main 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic(s), 0 novel' \
    "$work/known-panic-on-main.log"
# The same test name in two binaries, one passing: one failure, one signature.
cat >"$work/same-name-two-binaries.log" <<'EOF'
        PASS [   0.010s] (1/2) holon-app::suite shared_name
        FAIL [   0.010s] (2/2) holon::suite shared_name
  stderr ───
    thread 'shared_name' (2) panicked at crates/holon/tests/suite.rs:7:9:
    the holon copy failed
     Summary [   0.355s] 2 tests run: 1 passed, 1 failed, 0 skipped
error: test run failed
EOF
expect_outcome same-name-two-binaries 1 '^\[known-reds\] FAIL: 1 novel panic(s)' \
    "$work/same-name-two-binaries.log"
# Under `--no-capture` the output runs between the test's `START` and its
# status line, unindented.
cat >"$work/no-capture-worker-panic.log" <<'EOF'
       START [         ] (1/1) holon-integration-tests::integration latency_slo_gate::latency_fence_under_typing_report
[dense-tools] embedded MCP server up in 86.56425ms
thread 'tokio-runtime-worker' (224301153) panicked at crates/holon-integration-tests/tests/latency_slo_gate.rs:261:5:
the fence report is over budget
        FAIL [  13.874s] (1/1) holon-integration-tests::integration latency_slo_gate::latency_fence_under_typing_report
     Summary [  13.875s] 1 test run: 0 passed, 1 failed, 12 skipped
        FAIL [  13.874s] (1/1) holon-integration-tests::integration latency_slo_gate::latency_fence_under_typing_report
error: test run failed
EOF
expect_outcome no-capture-worker-panic 1 '^\[known-reds\] FAIL: 1 novel panic(s)' \
    "$work/no-capture-worker-panic.log"
# nextest indents captured output by 4: a passing should_panic test's own panic
# printed there is no failure, and does not hide a leak of the same test.
cat >"$work/nextest-should-panic.log" <<'EOF'
        PASS [   0.010s] (1/3) holon::sp boom_should_panic
  stdout ───

    running 1 test
    thread 'boom_should_panic' panicked at crates/holon/tests/sp.rs:9:5:
    the expected boom
    test boom_should_panic - should panic ... ok

    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
 FAIL + LEAK [   0.312s] (2/3) holon::leaky boom_should_panic
  stdout ───
    thread 'boom_should_panic' panicked at crates/holon/tests/leaky.rs:9:5:
    the expected boom
    test boom_should_panic - should panic ... ok
    test result: ok. 1 passed; 0 failed; 0 ignored
        PASS [   0.010s] (3/3) holon::other fine
     Summary [   1.000s] 3 tests run: 2 passed, 1 failed, 0 skipped
error: test run failed
EOF
expect_outcome nextest-should-panic/no-panic-sig 1 '^\[known-reds\] FAIL: 1 novel panic(s)' \
    "$work/nextest-should-panic.log"
expect_outcome nextest-should-panic/leak 1 '^ *1 FAIL + LEAK: holon::leaky boom_should_panic$' \
    "$work/nextest-should-panic.log"
# A label outside nextest's vocabulary stops the run instead of being guessed.
cat >"$work/unknown-label.log" <<'EOF'
  TRY 1 FROB [   0.120s] (1/1) holon::flaky_suite sometimes
     Summary [   0.355s] 1 tests run: 0 passed, 1 failed, 0 skipped
EOF
expect_outcome unknown-label 3 'UNKNOWN nextest status label "TRY 1 FROB"' "$work/unknown-label.log"
# A passing retry may end leaky or past its slow bound; neither is a failure.
cat >"$work/try-success-labels.log" <<'EOF'
   TRY 2 TMPASS [ 120.034s] (1/2) holon-gpui::bin slowish
  TRY 2 LEAK [   0.312s] (2/2) holon-gpui::bin leaky
     Summary [ 120.355s] 2 tests run: 2 passed, 0 skipped
EOF
expect_outcome try-success-labels 0 '^\[known-reds\] PASS: 1 green run' "$work/try-success-labels.log"
# `TERMINATING` is nextest killing the test, not an outcome: the final line decides.
cat >"$work/terminating-timeout-pass.log" <<'EOF'
        SLOW [> 60.000s] (─────────) holon-gpui::bin allowed
 TERMINATING [>120.000s] (─────────) holon-gpui::bin allowed
TIMEOUT-PASS [ 120.034s] (1/2) holon-gpui::bin allowed
 SLOW+TMPASS [ 120.034s] (2/2) holon-gpui::bin slowish
     Summary [ 120.355s] 2 tests run: 2 passed, 0 skipped
EOF
expect_outcome terminating-timeout-pass 0 '^\[known-reds\] PASS: 1 green run' "$work/terminating-timeout-pass.log"
# A stress run prefixes the instance with its iteration `[i/n]`; the signature
# names the test without it.
cat >"$work/stress-index.log" <<'EOF'
  Cancelling due to signal: 1 test still running
     SIGTERM [  20.778s] [10/12] (1/1) holon-integration-tests::integration two_instance_composed_pbt::edit_on_receiver_concurrent_with_create_on_owner_converges
 Stress test [  20.778s] iteration 10/12: 1 test run: 0 passed, 1 failed, 37 skipped
     TIMEOUT [  20.778s] [11/12] (1/1) holon::main turso_block_query_source_round_trip_pbt::round_trip
     Summary [ 385.895s] 11/12 stress run iterations: 9 passed; cancelled due to signal
     SIGTERM [  20.778s] [10/12] (1/1) holon-integration-tests::integration two_instance_composed_pbt::edit_on_receiver_concurrent_with_create_on_owner_converges
error: test run failed
EOF
expect_outcome stress-index/signal 1 '^ *1 SIGTERM: holon-integration-tests::integration two_instance_composed_pbt::edit_on_receiver_concurrent_with_create_on_owner_converges$' \
    "$work/stress-index.log"
expect_outcome stress-index/known-row 1 '^WARN known-red \[turso-block-query-source-round-trip\] x1$' \
    "$work/stress-index.log"
# Retries: a retried test's last failing `TRY n` decides. A test that passes on
# a later try (`FLAKY`) did not fail, and the panic of its failed try is no
# signature.
cat >"$work/retry-flaky-pass.log" <<'EOF'
  TRY 1 FAIL [   0.120s] (1/2) holon::flaky_suite sometimes
  stderr ───
    thread 'sometimes' (2) panicked at crates/holon/tests/flaky_suite.rs:10:5:
    REAL ONLY ON TRY 1: the first attempt lost the race
     TRY 2 START [         ] (1/2) holon::flaky_suite sometimes
   FLAKY 2/2 [   0.110s] (1/2) holon::flaky_suite sometimes
        PASS [   0.010s] (2/2) holon::flaky_suite always
     Summary [   0.355s] 2 tests run: 2 passed (1 flaky), 0 skipped
EOF
expect_outcome retry-flaky-pass 0 '^\[known-reds\] PASS: 1 green run' "$work/retry-flaky-pass.log"
cat >"$work/retry-final-fail.log" <<'EOF'
  TRY 1 SLOW [>120.000s] (1/3) holon::main turso_block_query_source_round_trip_pbt::round_trip
  TRY 1 TRMNTG [>120.000s] (1/3) holon::main turso_block_query_source_round_trip_pbt::round_trip
   TRY 1 TMT [ 120.034s] (1/3) holon::main turso_block_query_source_round_trip_pbt::round_trip
   TRY 2 TMT [ 120.034s] (1/3) holon::main turso_block_query_source_round_trip_pbt::round_trip
   TRY 1 LKFAIL [   0.312s] (2/3) holon-gpui::main layout_smoke::leaky
 TRY 2 SIG 9 [   0.312s] (2/3) holon-gpui::main layout_smoke::leaky
  TRY 1 FAIL [   0.120s] (3/3) holon::flaky_suite never
  stderr ───
    thread 'never' (2) panicked at crates/holon/tests/flaky_suite.rs:20:5:
    REAL ON EVERY TRY: the race is lost every time
  TRY 2 FAIL [   0.120s] (3/3) holon::flaky_suite never
  stderr ───
    thread 'never' (2) panicked at crates/holon/tests/flaky_suite.rs:20:5:
    REAL ON EVERY TRY: the race is lost every time
     Summary [ 240.355s] 3 tests run: 0 passed, 1 timed out, 2 failed, 0 skipped
EOF
expect_outcome retry-final-fail/timeout 1 '^WARN known-red \[turso-block-query-source-round-trip\] x1$' \
    "$work/retry-final-fail.log"
expect_outcome retry-final-fail/signal 1 '^ *1 ABORT SIG 9: holon-gpui::main layout_smoke::leaky$' \
    "$work/retry-final-fail.log"
expect_outcome retry-final-fail/panic 1 '^ *2 REAL ON EVERY TRY: the race is lost every time$' \
    "$work/retry-final-fail.log"
# A passing test's own captured output can look like a status line; only
# nextest's fixed label column counts, so it must not read as a label.
cat >"$work/captured-output-lines.log" <<'EOF'
        PASS [   0.010s] (1/2) holon-capability::certify prints_report
    TIGHTENING [org] foo
    VIOLATION  [logseq] bar
 FAIL + LEAK [   6.486s] (2/2) holon-app::main integration_toggle_round_trip::a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection
  stderr ───
    thread 'a_dispatched_switch_reaches_the_seeded_section_without_a_manual_reprojection' (2) panicked at crates/holon-app/tests/integration_toggle_round_trip.rs:83:9:
    the seeded Integrations section never showed 'todoist' as true — it is still false. The operation wrote the state file, so the break is between the store's signal and the mirror.
     Summary [  25.469s] 2 tests run: 1 passed, 1 failed, 0 skipped
EOF
expect_outcome captured-output-lines 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red' "$work/captured-output-lines.log"
# CRLF line endings must not hide a known panic's location.
sed '/panicked at/s/$/\r/' "$work/captured-output-lines.log" >"$work/crlf.log"
expect_outcome crlf 0 '^\[known-reds\] PASS-WITH-NOTE: 1 known-red' "$work/crlf.log"
# A failed setup script fails the run before any test does.
cat >"$work/setup-fail.log" <<'EOF'
  SETUP PASS [   0.100s] db-seed: ./scripts/seed.sh
SETUP LKFAIL [   0.312s] leaky-script: ./scripts/leaky.sh
     Summary [   1.000s] 0 tests run: 0 passed, 0 skipped
EOF
expect_outcome setup-fail 1 '^ *1 SETUP LEAK-FAIL: leaky-script: ./scripts/leaky.sh$' "$work/setup-fail.log"
# A row whose known failure is a budget assertion must not absorb a TIMEOUT of
# the same test: a hang past the nextest cap is a different bug.
cat >"$work/budget-test-timeout.log" <<'EOF'
     TIMEOUT [ 600.012s] (1/1) holon::main turso_storage_repros::tabs_main_panel_delivery::cursor_filtered_main_panel_delivers_at_vault_scale
     Summary [ 600.355s] 1 tests run: 0 passed, 1 timed out, 0 skipped
EOF
expect_outcome budget-test-timeout 1 '^PRIMARY: \[novel\] .*TIMEOUT: holon::main turso_storage_repros::' \
    "$work/budget-test-timeout.log"
cat >"$work/timeout-row.log" <<'EOF'
     TIMEOUT [ 600.012s] (1/1) holon::main turso_block_query_source_round_trip_pbt::round_trip
     Summary [ 600.355s] 1 tests run: 0 passed, 1 timed out, 0 skipped
EOF
expect_outcome timeout-row 0 '^PRIMARY: \[known-red:turso-block-query-source-round-trip\]' \
    "$work/timeout-row.log"

# `bulk-add-sibling-order` matches only a SUT order that OPENS with a bulk block.
# No archived corpus carries that shape (the 2026-09-19 corpus's 52 sibling-order
# lines are swapped uuid children under a bulk PARENT and classify novel), so the
# row is pinned by synthesised panics built on a real panic header.
panic_header="thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1390:13:"
expect_sut_order() {
    local label="$1" want="$2" sut="$3" log="$work/sut-order-$1.log"
    {
        printf '%s\n' "$panic_header"
        printf '%s\n' "reconciled composed sequence diverged from the oracle: [(\"inv-blocks-match-ref/org\", \"[inv-blocks-match-ref/org] sibling order diverges under parent block:journals.\\n  ref order: [\\\"block:c1\\\", \\\"block:bulk-1-0\\\"]\\n  sut order: [$sut]\")]"
        printf '%s\n' "error: recipe \`keystone-full\` failed with exit code 101"
    } >"$log"
    local out
    out=$("$repo_root/scripts/keystone-known-reds.sh" "$log" 2>&1 || true)
    if ! grep -q -- "^PRIMARY: \[$want\]" <<<"$out"; then
        echo "[fixture] FAIL sut-order/$label: want PRIMARY [$want]; got:"
        printf '%s\n' "$out" | tail -5 | sed 's/^/          | /'
        outcome_fail=1
        return
    fi
    echo "[fixture] ok sut-order/$label ($want)"
}
expect_sut_order bulk-first known-red:bulk-add-sibling-order '\"block:bulk-1-0\", \"block:c1\"'
expect_sut_order bulk-second novel '\"block:c1\", \"block:bulk-1-0\"'
expect_sut_order day-page-first novel '\"block:2026-10-01\", \"block:bulk-1-0\"'

# The write-back state machine rows anchor on ids that only their sidecar
# fixture uses, and sit above the broader rows that match the same payloads.
# wb-d7's payload carries no fixture id, so it stays under syn-real-mint.
cat >"$work/wb-d6.log" <<'EOF'
thread 'hand_authored_keystone_regressions' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1553:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/loro", "[inv-blocks-match-ref/loro] fields diverge from reference\n  inv-blocks-match-ref/loro: 13 blocks, reference: 12 blocks\n  only in inv-blocks-match-ref/loro (1): [\"block:wbd6-b\"]\n  only in reference (0): []\n  field deltas (0):\n"), ("inv-blocks-match-ref/block_raw", "[inv-blocks-match-ref/block_raw] block id set diverges from reference\n  spurious in inv-blocks-match-ref/block_raw: [EntityUri(\"block:wbd6-b\")]"), ("inv-blocks-match-ref/matview", "[inv-blocks-match-ref/matview] block id set diverges from reference\n  spurious in inv-blocks-match-ref/matview: [EntityUri(\"block:wbd6-b\")]")]
test hand_authored_keystone_regressions ... FAILED
test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.73s
EOF
expect_outcome wb-d6 0 '^PRIMARY: \[known-red:wb-d6-stall-resurrects-deleted-line\]' "$work/wb-d6.log"
cat >"$work/wb-d7.log" <<'EOF'
thread 'hand_authored_keystone_regressions' (1) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1853:5:
assertion `left == right` failed: per-tick reconcile: one synthetic per minted real id (syn=[], real=[EntityUri("block:8a829aea-4bad-47cf-a735-2990f9d4a655")], HeldCreates { parked: 0, parked_other: 0, newly_failed: 0 }); this tick RETIRED [] and the SUT LOST [] from block_raw
test hand_authored_keystone_regressions ... FAILED
test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.73s
EOF
expect_outcome wb-d7 0 '^PRIMARY: \[known-red:syn-real-mint\]' "$work/wb-d7.log"

# A row whose assert every generated case reaches covers only its observed
# signature: another file, line or boot arm at that assert stays a failure.
hand_authored_log() {
    local name="$1" site="$2" payload="$3"
    {
        printf '%s\n' "thread 'hand_authored_keystone_regressions' (1) panicked at $site:"
        printf '%s\n' "$payload"
        printf '%s\n' 'test hand_authored_keystone_regressions ... FAILED'
        printf '%s\n' 'test result: FAILED. 8 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.73s'
    } >"$work/$name.log"
}
deleted_site=crates/holon-integration-tests/src/pbt/frontend_slice/components.rs
deleted_payload() {
    printf '[SutAppLifecycle::delete_document] %s exists again after its blocks vanished — a projection writer recreated a file the user deleted' "$1"
}
novel_only='^\[known-reds\] FAIL: 1 novel panic(s), 0 known-red'
hand_authored_log deleted-observed "$deleted_site:6067:9" "$(deleted_payload gamma.org)"
expect_outcome deleted-file/observed 0 '^PRIMARY: \[known-red:deleted-file-recreated-by-stale-page-mirror\]' "$work/deleted-observed.log"
hand_authored_log deleted-current-site "$deleted_site:6077:9" "$(deleted_payload gamma.org)"
expect_outcome deleted-file/current-site 0 '^PRIMARY: \[known-red:deleted-file-recreated-by-stale-page-mirror\]' "$work/deleted-current-site.log"
hand_authored_log deleted-other-file-other-line "$deleted_site:1234:5" "$(deleted_payload journal.org)"
expect_outcome deleted-file/other-file-other-line 1 "$novel_only" "$work/deleted-other-file-other-line.log"
hand_authored_log deleted-other-file "$deleted_site:6077:9" "$(deleted_payload journal.org)"
expect_outcome deleted-file/other-file 1 "$novel_only" "$work/deleted-other-file.log"
hand_authored_log deleted-other-line "$deleted_site:1234:5" "$(deleted_payload gamma.org)"
expect_outcome deleted-file/other-line 1 '^\[known-reds\] FAIL: cross-family match: deleted-file-recreated-by-stale-page-mirror' \
    "$work/deleted-other-line.log"
boot_site=crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs
boot_payload() {
    printf 'the composed boot did not finish within 60s with the `fake-mcp` integration peer drawn to answer %s: the session resolves only after every enabled integration connected' "$1"
}
hand_authored_log boot-instant "$boot_site:1402:9" "$(boot_payload Instant)"
expect_outcome boot-timeout/instant 0 '^PRIMARY: \[known-red:composed-boot-timeout-under-load\]' "$work/boot-instant.log"
for timing in AfterSessionResolve Never; do
    hand_authored_log "boot-$timing" "$boot_site:1402:9" "$(boot_payload "$timing")"
    expect_outcome "boot-timeout/$timing" 1 "$novel_only" "$work/boot-$timing.log"
done

# The keystone's `BootFault::TaskPanic` panics on purpose. Only that exact
# payload at that exact site is no failure; anything else stays a signature.
injected_site=crates/holon-integration-tests/src/pbt/composed/boot_fault.rs
injected_payload='keystone boot fault: injected task panic'
if [ "$(sed -n 22p "$repo_root/$injected_site")" != '    injected_task_panic_site(panic_now)' ]; then
    echo "[fixture] FAIL injected-panic/site: $injected_site:22:5 is no longer the injected panic's call;"
    echo "          move the site in scripts/keystone-known-reds.sh with it."
    outcome_fail=1
fi
injected_log() {
    local name="$1" site="$2" payload="$3" verdict="$4"
    {
        printf '%s\n' "thread 'tokio-rt-worker' (311836979) panicked at $site:"
        printf '%s\n' "$payload"
        printf '%s\n' 'note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace'
        if [ "$verdict" = failed ]; then
            printf '%s\n' "thread 'general_e2e_composed_pbt::general_e2e_composed_pbt' (311836306) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1557:13:"
            printf '%s\n' 'REAL PRODUCT BUG: the projection vanished'
            printf '%s\n' 'test general_e2e_composed_pbt::general_e2e_composed_pbt ... FAILED'
            printf '%s\n' 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s'
        else
            printf '%s\n' 'test general_e2e_composed_pbt::general_e2e_composed_pbt ... ok'
            printf '%s\n' 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s'
        fi
    } >"$work/$name.log"
}
injected_log injected-only "$injected_site:22:5" "$injected_payload" green
expect_outcome injected-panic/only-is-green 0 '^\[known-reds\] PASS: 1 green run' "$work/injected-only.log"
expect_outcome injected-panic/only-is-reported 0 "^\[known-reds\] INJECTED: 1 .*$injected_site:22:5" "$work/injected-only.log"
injected_log injected-plus-real "$injected_site:22:5" "$injected_payload" failed
expect_outcome injected-panic/real-is-primary 1 '^PRIMARY: \[novel\] .*REAL PRODUCT BUG: the projection vanished$' \
    "$work/injected-plus-real.log"
injected_log injected-other-line "$injected_site:23:5" "$injected_payload" green
expect_outcome injected-panic/other-line 1 "^PRIMARY: \[novel\] .*$injected_site:23:5: $injected_payload\$" \
    "$work/injected-other-line.log"
injected_log injected-other-file crates/holon-integration-tests/src/pbt/composed/harness.rs:22:5 "$injected_payload" green
expect_outcome injected-panic/other-file 1 "^PRIMARY: \[novel\] .*harness.rs:22:5: $injected_payload\$" \
    "$work/injected-other-file.log"
injected_log injected-other-payload "$injected_site:22:5" 'called `Option::unwrap()` on a `None` value' green
expect_outcome injected-panic/other-payload 1 "^PRIMARY: \[novel\] .*$injected_site:22:5: called .Option::unwrap()." \
    "$work/injected-other-payload.log"
injected_log injected-longer-payload "$injected_site:22:5" "$injected_payload and the store is gone" green
expect_outcome injected-panic/longer-payload 1 "^PRIMARY: \[novel\] .*$injected_site:22:5: $injected_payload and the store is gone\$" \
    "$work/injected-longer-payload.log"
# A grep that cannot read the signatures must stop the classifier, not leave it
# reading a truncated signature file as green.
real_grep=$(command -v grep)
for flags in -cxF -vxF; do
    mkdir -p "$work/grep-fails$flags"
    printf '#!/usr/bin/env bash\n[ "$1" = %s ] && { echo "grep: simulated read error" >&2; exit 2; }\nexec %s "$@"\n' \
        "$flags" "$real_grep" >"$work/grep-fails$flags/grep"
    chmod +x "$work/grep-fails$flags/grep"
    PATH="$work/grep-fails$flags:$PATH" expect_outcome "injected-panic/grep$flags-error-stops" 2 \
        "^\[known-reds\] ERROR: grep $flags " "$work/injected-only.log"
done
if [ "$outcome_fail" -ne 0 ]; then
    echo ""
    echo "[fixture] FAIL: the classifier's outcome verdict changed. A green log read"
    echo "          as a failure is the 2026-08-25 false-NOVEL defect returning; a"
    echo "          failed or unreadable log read as a pass is worse."
    exit 1
fi
echo "[fixture] PASS — outcome classification unchanged."
exit 0
