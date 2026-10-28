#!/usr/bin/env bash
# Classify FAILED keystone full-depth runs against the known-reds registry.
#
# Usage:
#   scripts/keystone-known-reds.sh <run.log> [<run.log> ...]
#
# `just keystone-nightly` passes only the runs that exited non-zero, but logs
# also arrive by hand, so each log's outcome is read from the log itself. For a
# FAILED run the script extracts the failure signatures — the first message line
# of every panic, which is either the composed-keystone oracle verdict
# ("reconciled composed sequence diverged from the oracle: [(...)]") or a
# harness assertion — and matches each one against the `Match pattern` column of
# docs/Testing/KeystoneKnownReds.md. A GREEN run has nothing to classify.
#
# A keystone run also has to have DRAWN something. The keystone keeps drawing
# past a registered known red and discloses its sample as one
# `[keystone-sample] … verdict=OK|FLOOR-BREACH` line (the floor itself lives in
# ONE place, crates/holon-integration-tests/src/pbt/known_reds.rs). A keystone
# log without that line stopped at its first known red or was truncated: its
# sample is unverified, so it is a FAILURE here, not a pass-with-note.
#
# Exit 0  — nothing to triage: every extracted signature matched a `known-red`
#           row (printed as WARN), every keystone run disclosed verdict=OK,
#           and/or every log was green.
# Exit 1  — at least one signature matched nothing, a FAILED run yielded no
#           signature at all, or a keystone run's sample is below the floor /
#           undisclosed. All are regressions to triage, NOT rows to add.
#           Also a CROSS-FAMILY match: a row's wording fired from a site the
#           row does not list, which is a new failure wearing a known red's
#           words until a log proves the same root cause is re-raised there.
# Exit 3  — at least one log admits no verdict: INDETERMINATE (empty, or
#           truncated before the run said anything) or UNREADABLE (no such
#           file). Never silently a pass — the input is broken, so nothing can
#           be said about the run at all. Also a nextest status label this
#           script does not know: guessing its meaning would guess the verdict.
#
# The registry is the single source of truth for the patterns; this script holds
# none of its own. Apart from that, it knows exactly one panic: the keystone's
# injected boot fault (`injected_panic` below), which is no failure at all.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
registry="$repo_root/docs/Testing/KeystoneKnownReds.md"

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <run.log> [<run.log> ...]" >&2
    exit 2
fi
if [ ! -f "$registry" ]; then
    echo "[known-reds] registry missing: $registry" >&2
    exit 2
fi

# The input logs' `BASE:`/`TREE:` lines head the output, so a tee of it names
# the tree it judged. An unreadable log is reported in the loop below.
for log in "$@"; do
    [ -f "$log" ] && [ -r "$log" ] || continue
    awk '/^BASE: / && !b { print; b = 1 } /^TREE: / && !t { print; t = 1 } b && t { exit }' "$log"
done | awk '!seen[$0]++'

# Registry rows come from scripts/check-known-red-evidence.py, the one parser of
# the table (it also lints it); only `known-red` rows classify. A cell cannot
# hold a literal `|` (the table separator), so a pattern writes alternation as
# `&#124;`, decoded by that parser.
#
# `site` is an ERE over `<file>:<line>:<col>` listing every MEASURED site that
# reports the row's root cause — one row per root cause, so the ERE alternates
# over the asserting site and each harness that re-raises its verdict (the
# windowed GPUI loop re-reports the composed keystone's verdict at its own
# site). Empty means the row is not pinned yet (no evidence log on this machine
# to measure it from) and is not site-checked. A pinned row whose wording
# matches a signature from ANOTHER site is a cross-family match: the run fails
# loudly rather than re-attributing the signature (Martin, 2026-10-13).
#
# `companions` are invariant-id globs the row's root cause also trips; see the
# matching loop for when they count.
keys=()
patterns=()
sites=()
companions=()
rows=$(python3 "$repo_root/scripts/check-known-red-evidence.py" --rows "$registry")
while IFS=$'\x1f' read -r key pattern site companion; do
    [ -z "$key" ] && continue
    keys+=("$key")
    patterns+=("$pattern")
    sites+=("$site")
    companions+=("$companion")
done <<<"$rows"

if [ "${#keys[@]}" -eq 0 ]; then
    echo "[known-reds] registry has no known-red rows — every failure is novel."
fi

# Strip volatile payloads that make otherwise-identical panics look distinct
# (per-run UUIDs, span Ids, resolver-map contents, split-block indices) so
# dedup and registry matching operate on the stable shape of the signature.
normalize_sig() {
    printf '%s' "$1" | sed -E \
        -e 's/^Test failed: (.*)\.$/\1/' \
        -e 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<uuid>/g' \
        -e 's/\bId\([0-9]+\)/Id(N)/g' \
        -e 's/Mapped: \[[^]]*\]/Mapped: [...]/g' \
        -e 's/split-[0-9]+/split-N/g'
}

# A registry row names ONE violation, so a signature is known only when each of
# its violations is. Prints one signature per violation, each shaped as the
# panic would read had that violation fired alone: an oracle verdict
# `…diverged from the oracle: [("inv-a", "…"), ("inv-b", "…")]…` splits per
# invariant, and an `inv-sql-budget` message of N violations splits into N
# `1 budget violation(s)` messages. Any other signature prints unchanged.
# Exit 3 when a budget message holds a different number of violations than it
# announces: the split cannot be trusted, so neither can a verdict.
split_violations() {
    printf '%s\n' "$1" | awk '
        function whole() { print sig; exit }
        # The end of the Debug-escaped string that opens at s[p], or 0.
        function string_end(s, p,    c) {
            for (p++; p <= length(s); p++) {
                c = substr(s, p, 1)
                if (c == "\\") p++
                else if (c == "\"") return p
            }
            return 0
        }
        {
            sig = $0
            at = index(sig, "diverged from the oracle: [(")
            if (at == 0) whole()
            at += length("diverged from the oracle: ")
            prefix = substr(sig, 1, at - 1)
            n = 0; p = at + 1
            while (1) {
                if (substr(sig, p, 2) != "(\"") whole()
                id_end = string_end(sig, p + 1)
                if (!id_end || substr(sig, id_end, 3) != "\", ") whole()
                msg_end = string_end(sig, id_end + 3)
                if (!msg_end || substr(sig, msg_end + 1, 1) != ")") whole()
                n++
                head[n] = substr(sig, p, id_end + 4 - p)
                msg[n] = substr(sig, id_end + 4, msg_end - id_end - 4)
                p = msg_end + 2
                if (substr(sig, p, 2) == ", ") { p += 2; continue }
                if (substr(sig, p, 1) == "]") break
                whole()
            }
            suffix = substr(sig, p + 1)
            budget_head = "[inv-sql-budget] 1 budget violation(s):\\n  "
            for (i = 1; i <= n; i++) {
                if (!match(msg[i], /^\[inv-sql-budget\] [0-9]+ budget violation\(s\):\\n  /)) {
                    print prefix "[" head[i] msg[i] "\")]" suffix
                    continue
                }
                announced = substr(msg[i], length("[inv-sql-budget] ") + 1) + 0
                rest = substr(msg[i], RLENGTH + 1)
                k = 0
                # Every metric label, not only sql_*: the budget also writes wall, query and memory ceilings.
                while (match(rest, /\\n  [^ ]+\.[a-z_]+: /)) {
                    one[++k] = substr(rest, 1, RSTART - 1)
                    rest = substr(rest, RSTART + 4)
                }
                one[++k] = rest
                if (k != announced) {
                    printf "[known-reds] ERROR: an inv-sql-budget message announces %d violation(s) but splits into %d: %s\n", announced, k, msg[i] >"/dev/stderr"
                    exit 3
                }
                for (j = 1; j <= k; j++) print prefix "[" head[i] budget_head one[j] "\")]" suffix
            }
        }'
}

# A panic is collateral (unwind noise from a thread tearing down after some
# other panic) when its payload is an opaque `Any { .. }` or it originates in
# tracing-subscriber's span teardown, not in workspace code. Collateral is
# reported but never counts toward novel/matched or the PASS/FAIL verdict.
is_collateral() {
    local loc="$1" sig="$2"
    [[ "$sig" == *'Any { .. }'* ]] && return 0
    [[ "$loc" == *"tracing-subscriber-"*"sharded.rs"* ]] && return 0
    return 1
}

# The keystone's own sample-floor panic is this gate speaking, not a bug: it is
# reported through the sample verdict below, never as a novel signature.
is_sample_floor() {
    [[ "$1" == *'[keystone-sample] FLOOR BREACH:'* ]]
}

# A keystone log must disclose the sample its verdict rests on. Prints the
# disclosure and returns non-zero when the sample is below the floor or absent.
check_sample() {
    local log="$1" line
    # A run of the HEADLESS keystone only, read from the test runner's own
    # lines: cargo build output names the target without running it, and
    # `general_e2e_composed_pbt_windowed` discloses no sample.
    grep -qE "^[[:space:]]*test general_e2e_composed_pbt |thread 'general_e2e_composed_pbt'|::general_e2e_composed_pbt general_e2e_composed_pbt\$" "$log" || return 0
    line=$(grep_selecting -hF '[keystone-sample] cases=' "$log" | tail -1)
    if [ -z "$line" ]; then
        echo "SAMPLE: UNDISCLOSED — this keystone run printed no [keystone-sample] line, so"
        echo "        it stopped at its first failure (or was truncated) instead of drawing on."
        echo "        Nothing can be said about draws it never made."
        return 1
    fi
    echo "SAMPLE: ${line#*\] }"
    [ "$line" != "${line%verdict=OK*}" ]
}

# The keystone's `BootFault::TaskPanic` panics on purpose and asserts the
# disclosure itself. Only this payload at this site is no failure: it is
# removed before the outcome is read, so it can neither fail a green run nor
# take the PRIMARY slot from a real failure.
injected_panic=$'crates/holon-integration-tests/src/pbt/composed/boot_fault.rs:22:5\tkeystone boot fault: injected task panic'
injected=0

# grep whose "no line selected" (exit 1) is a result; a grep error (exit 2) is
# not, and stops the script rather than leave a truncated signature file.
grep_selecting() {
    local rc=0
    grep "$@" || rc=$?
    if [ "$rc" -gt 1 ]; then
        echo "[known-reds] ERROR: grep $* failed (exit $rc); the log is not classified." >&2
        return "$rc"
    fi
}

# What the log says the run DID: `failed`, `green`, or `indeterminate` (it says
# nothing either way). Read from the harness's own verdict lines — cargo test's
# `test result:`, cargo's and just's failure diagnostics, nextest's `Summary` /
# `FAIL` — because the classifier is handed logs by hand as often as by
# `just keystone-nightly`, and a green run's absence of panics is not a missing
# signature.
#
# Precedence is deliberate where a log carries BOTH a green verdict and
# something error-shaped: only the harness's own failure vocabulary outranks an
# explicit `test result: ok.`, because a test that prints a line starting
# `error: ` on its own stdout has not failed, and reading it as one reproduces
# the very false alarm this classifier exists to avoid. With no green verdict to
# weigh it against, any error-shaped line still counts as failure evidence.
harness_failed_re='^test result: FAILED|^ +(FAIL|FAIL \+ LEAK|FLKY-FL [0-9]+/[0-9]+) \[|tests run:.* [1-9][0-9]* (failed|timed out)|^error: (test failed|could not compile|recipe .* failed|process didn.t exit successfully|linking with|failed to run custom build command)'
green_verdict_re='^test result: ok\.|tests run: [0-9]+ passed'
any_error_re='^error(\[[A-Za-z0-9]+\])?: '

log_outcome() {
    local log="$1"
    if grep -qE "$harness_failed_re" "$log"; then
        printf 'failed'
    elif grep -qE "$green_verdict_re" "$log"; then
        printf 'green'
    elif grep -qE "$any_error_re" "$log"; then
        printf 'failed'
    else
        printf 'indeterminate'
    fi
}

novel=0
matched=0
cross=0
collateral=0
sample_bad=0
green=0
indeterminate=0
unreadable=0
# Per-key hit counts + one example each. Parallel indexed arrays, not an
# associative array — macOS ships bash 3.2.
counts=()
examples=()
for i in "${!keys[@]}"; do
    counts[$i]=0
    examples[$i]=""
done
novel_file=$(mktemp)

for log in "$@"; do
    if [ ! -f "$log" ] || [ ! -r "$log" ]; then
        echo "[known-reds] UNREADABLE: $log — not a readable file." >&2
        unreadable=$((unreadable + 1))
        continue
    fi

    # A panic prints "thread '…' panicked at <file>:<line>:" and the message on
    # the NEXT line; that message line IS the signature. Proptest re-panics on
    # every shrink step, so one failing case yields dozens of near-identical
    # lines — hence the aggregation below.
    #
    # In a nextest log every line of test output belongs to one test instance:
    # the captured block (indented by 4) under its status line, or, under
    # `--no-capture`, the lines between its `START` and its status line. Thread
    # names cannot do this: a `harness = false` binary panics on `main`, a test
    # may panic on a worker thread, and names repeat across binaries.
    #
    # A panic on the thread of a test cargo reports as `- should panic ... ok`
    # is that test passing. libtest writes the verdict apart from its head, so
    # stdout of another thread may follow the `ok` or push it onto a later
    # line. Such a panic is matched by thread name within the owning nextest
    # instance, else within one cargo `Running` section, and is
    # dropped only when the panic count there equals the `ok` lines there; any
    # surplus keeps them all, because nothing tells the expected panic from the
    # real one.
    #
    # A failed test whose output holds a panic has that panic as its signature.
    # Otherwise its status line is the signature, under nextest's long label
    # (`TMT` reads as `TIMEOUT`), followed by the first `Error: ` line of its
    # output when it returned one, so two root causes stay two signatures.
    # Labels are nextest-runner's reporter/displayer/imp.rs `status_str` /
    # `short_status_str`; a label outside that set stops the run (exit 3).
    #
    # A retried test's last failing `TRY n` line is its verdict. When a later
    # try passes (`FLAKY n/m`), its failed tries and their panics are not
    # failures. The stress index `[i/n]` is dropped from the signature so a
    # pattern names the test once; nextest also repeats status lines in its
    # closing summary, hence the dedup.
    sigs_file=$(mktemp)
    awk '
        function thread_of(line,    t) {
            sub(/^ +/, "", line)
            # A stdout line written between `thread '"'"'` and the name leaves
            # the name at the start of the next line.
            if (index(line, "thread " q) != 1)
                return match(line, /^[^ '"'"']+'"'"' (\([0-9]+\) )?panicked at /) ? substr(line, 1, index(line, q) - 1) : ""
            t = substr(line, 9)
            return substr(t, 1, index(t, q) - 1)
        }
        # Sets LABEL, STRESS and UNIT from a nextest status line, whose label
        # nextest right-aligns in 12 columns and spells in capitals; returns 0
        # for any other line.
        function parse_status(line,    head) {
            if (!match(line, /^ *[A-Z][A-Za-z0-9 +\/-]* \[[^]]*\] /)) return 0
            head = substr(line, 1, RLENGTH)
            LABEL = head; sub(/^ +/, "", LABEL); sub(/ +\[[^]]*\] $/, "", LABEL)
            if (index(head, " [") != (length(LABEL) < 12 ? 13 : length(LABEL) + 1)) return 0
            if (LABEL ~ /[a-z]/ && LABEL !~ /^(Summary|Stress test)$/) return 0
            UNIT = substr(line, RLENGTH + 1)
            STRESS = ""
            if (match(UNIT, /^\[[0-9]+\/[0-9]+\] /)) {
                STRESS = substr(UNIT, 1, RLENGTH); UNIT = substr(UNIT, RLENGTH + 1)
            }
            sub(/^\([^)]*\) /, "", UNIT)
            return 1
        }
        function unknown_label(label) {
            printf "[known-reds] UNKNOWN nextest status label \"%s\" at %s:%d — the classifier cannot read this run.\n", label, FILENAME, FNR >"/dev/stderr"
            exit 3
        }
        # The long label of a failing status, short or long form.
        function long_label(s) {
            if (s ~ /^(TMT|TIMEOUT)$/) return "TIMEOUT"
            if (s ~ /^(LKFAIL|LEAK-FAIL)$/) return "LEAK-FAIL"
            if (s ~ /^(FL\+LK|FAIL \+ LEAK)$/) return "FAIL + LEAK"
            if (s ~ /^SIG [0-9]+$/) return "ABORT " s
            if (s ~ /^(FAIL|XFAIL|ABORT|ABORT SIG [0-9]+)$/) return s
            if (s ~ ("^(" signals ")$")) return "SIG" s
            if (s ~ ("^SIG(" signals ")$")) return s
            unknown_label(LABEL)
        }
        # "progress", "meta", "pass" or "fail" (setting FAIL_LABEL).
        function outcome(label,    s) {
            if (label ~ /^(Summary|Stress test|SKIP|SETUP|SETUP SLOW|SETUP PASS|SETUP LEAK|SETUP TMPASS)$/) return "meta"
            if (label ~ /^SETUP /) {
                s = label; sub(/^SETUP /, "", s)
                FAIL_LABEL = "SETUP " long_label(s)
                return "fail"
            }
            s = label; sub(/^TRY [0-9]+ /, "", s)
            if (s ~ /^(START|SLOW|TRMNTG|TERMINATING)$/) return "progress"
            if (s ~ /^(PASS|LEAK|TMPASS|TIMEOUT-PASS|SLOW \+ LEAK|SLOW\+TMPASS)$/ || s ~ /^FLAKY [0-9]+\/[0-9]+$/) return "pass"
            if (s ~ /^FLKY-FL [0-9]+\/[0-9]+$/) { FAIL_LABEL = "FLKY-FL"; return "fail" }
            FAIL_LABEL = long_label(s)
            return "fail"
        }
        # OWNER is the test instance whose output the current line is.
        function track_status(o, inst) {
            if (o == "progress") {
                if (LABEL ~ /START$/) live = inst
                captured = ""
            } else if (o == "meta") {
                captured = ""; live = ""
            } else {
                if (inst == live) live = ""
                captured = inst
            }
        }
        function track_line(line) {
            if (captured != "" && line !~ /^(    |  [a-z]+ ───$|$)/) captured = ""
            OWNER = captured != "" ? captured : live
        }
        function scope() { return OWNER != "" ? OWNER : section }
        # Whether the output of test instance `inst` holds a panic that is not
        # a passing should_panic test'"'"'s own.
        function has_real_panic(inst,    k, parts) {
            for (k in panics) {
                split(k, parts, SUBSEP)
                if (parts[1] == inst && !((k in expected) && expected[k] == panics[k])) return 1
            }
            return 0
        }
        function try_of(label,    n) {
            n = label; sub(/^TRY /, "", n); sub(/ .*$/, "", n)
            return n
        }
        function emit(sig) {
            if (!(sig in killed)) { killed[sig] = 1; print "nextest\t" sig }
        }
        FNR == 1 { section = ""; live = ""; captured = "" }
        /^ +Running .* \(.*\)$/ || /^ +Doc-tests / { section = $0 }
        FNR == NR {
            if (parse_status($0)) {
                inst = STRESS UNIT
                o = outcome(LABEL)
                track_status(o, inst)
                if (LABEL ~ /^FLAKY / || (LABEL ~ /^TRY / && o == "pass")) passed[inst] = 1
                else if (LABEL ~ /^FLKY-FL /) flaky_failed[inst] = 1
                else if (LABEL ~ /^TRY [0-9]+ / && o == "fail" && try_of(LABEL) + 0 > last_try[inst] + 0) last_try[inst] = try_of(LABEL)
                next
            }
            track_line($0)
            if (scope() in verdict_due && match($0, /^ *(ok|FAILED|ignored)/)) {
                if ($0 ~ /^ *ok/) expected[scope() SUBSEP verdict_due[scope()]]++
                delete verdict_due[scope()]
            }
            if (match($0, /^ *test .* - should panic \.\.\. /)) {
                name = $0; sub(/^ *test /, "", name); sub(/ - should panic \.\.\. .*$/, "", name)
                if (substr($0, RLENGTH + 1) ~ /^ok/) expected[scope() SUBSEP name]++
                else if (substr($0, RLENGTH + 1) !~ /^(FAILED|ignored)/) verdict_due[scope()] = name
            } else if (/panicked at /) {
                panics[scope() SUBSEP thread_of($0)]++
            } else if (OWNER != "" && /^ *Error: / && !(OWNER in error_of)) {
                error_of[OWNER] = $0; sub(/^ +/, "", error_of[OWNER])
            }
            next
        }
        parse_status($0) {
            inst = STRESS UNIT
            o = outcome(LABEL)
            track_status(o, inst)
            if (o != "fail") next
            if (LABEL ~ /^SETUP /) { emit(FAIL_LABEL ": " UNIT); next }
            if ((inst in passed) && !(inst in flaky_failed)) next
            if (LABEL ~ /^TRY [0-9]+ / && try_of(LABEL) != last_try[inst]) next
            if (FAIL_LABEL !~ /^(FAIL|FAIL \+ LEAK|FLKY-FL)$/) emit(FAIL_LABEL ": " UNIT)
            else if (!has_real_panic(inst)) emit(FAIL_LABEL ": " UNIT ((inst in error_of) ? ": " error_of[inst] : ""))
            next
        }
        { track_line($0) }
        /panicked at / {
            if ((OWNER in passed) && !(OWNER in flaky_failed)) { getline; next }
            key = scope() SUBSEP thread_of($0)
            if ((key in expected) && expected[key] == panics[key]) next
            loc = $0; sub(/^.*panicked at /, "", loc); sub(/\r$/, "", loc); sub(/:$/, "", loc)
            if ((getline msg) > 0) { sub(/^ +/, "", msg); print loc "\t" msg }
            next
        }' q="'" signals="HUP|INT|QUIT|ILL|TRAP|ABRT|BUS|FPE|KILL|USR1|SEGV|USR2|PIPE|ALRM|TERM" "$log" "$log" >"$sigs_file" \
        || { rc=$?; rm -f "$sigs_file" "$novel_file"; exit "$rc"; }

    log_injected=$(grep_selecting -cxF -- "$injected_panic" "$sigs_file") \
        || { rc=$?; rm -f "$sigs_file" "$novel_file"; exit "$rc"; }
    if [ "$log_injected" -gt 0 ]; then
        grep_selecting -vxF -- "$injected_panic" "$sigs_file" >"$sigs_file.real" \
            || { rc=$?; rm -f "$sigs_file" "$sigs_file.real" "$novel_file"; exit "$rc"; }
        mv "$sigs_file.real" "$sigs_file"
        echo "[known-reds] INJECTED: $log_injected keystone boot-fault panic(s) at ${injected_panic%%$'\t'*} in $log — expected, not a failure."
        injected=$((injected + log_injected))
    fi

    # Panics ARE failure evidence, and outrank the verdict lines: a run killed
    # mid-shrink is truncated before cargo ever prints `test result:`.
    if [ -s "$sigs_file" ]; then
        outcome=failed
    else
        outcome=$(log_outcome "$log")
    fi

    if [ "$outcome" = green ]; then
        echo "[known-reds] GREEN: $log — run passed, nothing to classify."
        green=$((green + 1))
        rm -f "$sigs_file"
        continue
    fi

    if [ "$outcome" = indeterminate ]; then
        echo "[known-reds] INDETERMINATE: $log — the log states no pass/fail"
        echo "               outcome (empty, truncated before any verdict, or not a"
        echo "               keystone run log). Tail:"
        tail -20 "$log" | sed 's/^/               | /'
        indeterminate=$((indeterminate + 1))
        rm -f "$sigs_file"
        continue
    fi

    if [ ! -s "$sigs_file" ]; then
        echo "[known-reds] NOVEL: $log — run failed but no panic signature was extracted"
        echo "               (compile error or a new failure shape). Tail:"
        tail -20 "$log" | sed 's/^/               | /'
        novel=$((novel + 1))
        rm -f "$sigs_file"
        continue
    fi

    base=$(basename "$log")
    echo ""
    echo "=== $base ==="

    # The FIRST panic is what actually failed the run; proptest then re-runs
    # the same case (or a genuinely different one) while shrinking, so every
    # panic after it is a shrink-tail re-panic — printed separately so it
    # can't be mistaken for the run's real failure.
    line_no=0
    tail_file=$(mktemp)
    while IFS=$'\t' read -r loc sig; do
        line_no=$((line_no + 1))
        norm=$(normalize_sig "$sig")

        if is_sample_floor "$sig"; then
            class="sample-floor"
        elif is_collateral "$loc" "$sig"; then
            class="collateral"
            collateral=$((collateral + 1))
        else
            pieces=$(split_violations "$norm") \
                || { rc=$?; rm -f "$sigs_file" "$tail_file" "$novel_file"; exit "$rc"; }
            # Each violation is matched on its own; ` i j ` holds the distinct
            # rows hit. A violation no pattern knows is still known when its
            # invariant is a companion of a row hit by ANOTHER violation of
            # the same signature: alone, a companion is novel.
            hits=" "
            cross_hit=-1
            unmatched=()
            while IFS= read -r piece; do
                hit=-1
                for i in "${!keys[@]}"; do
                    if printf '%s\n' "$piece" | grep -qE -- "${patterns[$i]}"; then
                        hit=$i
                        break
                    fi
                done
                if [ "$hit" -ge 0 ] && [ -n "${sites[$hit]}" ] \
                    && ! printf '%s\n' "$loc" | grep -qE -- "${sites[$hit]}"; then
                    [ "$cross_hit" -lt 0 ] && cross_hit=$hit
                elif [ "$hit" -ge 0 ]; then
                    [[ "$hits" == *" $hit "* ]] || hits="$hits$hit "
                else
                    unmatched+=("$piece")
                fi
            done <<<"$pieces"
            novel_pieces=""
            for piece in ${unmatched[@]+"${unmatched[@]}"}; do
                invariant=${piece#*diverged from the oracle: \[(\"}
                if [ "$invariant" = "$piece" ]; then
                    invariant=""
                else
                    invariant=${invariant%%\"*}
                fi
                claimed=0
                for hit in $hits; do
                    read -r -a globs <<<"${companions[$hit]}"
                    for glob in ${globs[@]+"${globs[@]}"}; do
                        # shellcheck disable=SC2254 # a registry glob, matched as one
                        case "$invariant" in $glob) claimed=1 ;; esac
                    done
                done
                [ "$claimed" -eq 1 ] \
                    || novel_pieces="$novel_pieces$(printf '%.240s' "$piece")"$'\n'
            done
            if [ "$cross_hit" -ge 0 ]; then
                echo "[known-reds] FAIL: cross-family match: ${keys[$cross_hit]} at $loc"
                echo "             The row's wording matched, but it asserts from"
                echo "             ${sites[$cross_hit]} — so this signature belongs to another family."
                echo "             Triage it (bug-gap-triage). Add this site to the row ONLY if"
                echo "             the log shows the SAME root cause re-raised there (an outer"
                echo "             harness re-reporting the inner verdict); never to silence it."
                class="cross-family:${keys[$cross_hit]}"
                cross=$((cross + 1))
            elif [ -n "$novel_pieces" ]; then
                class="novel"
                # Grouped by normalized message only, not location: the same
                # bug's panic site and proptest's own terminal re-report of it
                # are two locations for one signature (see normalize_sig's
                # Test-failed-wrapper strip) and must count as one novel hit,
                # the same way known-red counts already ignore location. Only
                # the violations no row knows are listed.
                printf '%s' "$novel_pieces" >>"$novel_file"
                novel=$((novel + 1))
            else
                class="known-red:"
                for hit in $hits; do
                    class="$class${keys[$hit]}+"
                    counts[$hit]=$(( ${counts[$hit]} + 1 ))
                    [ -z "${examples[$hit]}" ] && examples[$hit]="$base @ $loc: $(printf '%.240s' "$norm")"
                done
                class="${class%+}"
                matched=$((matched + 1))
            fi
        fi

        entry="[$class] $base @ $loc: $(printf '%.240s' "$norm")"
        if [ "$line_no" -eq 1 ]; then
            echo "PRIMARY: $entry"
        else
            printf '%s\n' "$entry" >>"$tail_file"
        fi
    done <"$sigs_file"
    rm -f "$sigs_file"

    if [ -s "$tail_file" ]; then
        tail_count=$(wc -l <"$tail_file" | tr -d ' ')
        echo "  -- shrink tail ($tail_count re-panics after the primary; may be a"
        echo "     different bug than the one that actually failed the run) --"
        sed 's/^/  /' "$tail_file"
    fi
    rm -f "$tail_file"

    if ! check_sample "$log"; then
        sample_bad=$((sample_bad + 1))
    fi
done
echo ""

for i in "${!keys[@]}"; do
    if [ "${counts[$i]}" -gt 0 ]; then
        echo "WARN known-red [${keys[$i]}] x${counts[$i]}"
        echo "     ${examples[$i]}"
    fi
done
if [ -s "$novel_file" ]; then
    echo ""
    echo "NOVEL signatures (distinct, with occurrence counts):"
    sort "$novel_file" | uniq -c | sort -rn | sed 's/^/  /'
fi
rm -f "$novel_file"

echo ""
if [ "$injected" -ne 0 ]; then
    echo "[known-reds] $injected injected boot-fault panic(s) excluded — see INJECTED above."
fi
no_verdict=$((indeterminate + unreadable))
if [ "$no_verdict" -ne 0 ]; then
    echo "[known-reds] $indeterminate log(s) stated no outcome, $unreadable unreadable — see above."
fi
if [ "$cross" -ne 0 ]; then
    echo "[known-reds] FAIL: $cross cross-family match(es) — see above."
    echo "             A row's wording fired from a site the row does not list."
    echo "             That is a new failure wearing a known red's words until a"
    echo "             log proves the same root cause is re-raised there."
    exit 1
fi
if [ "$novel" -ne 0 ]; then
    echo "[known-reds] FAIL: $novel novel panic(s), $matched known-red panic(s), $collateral collateral (ignored)."
    echo "             A novel signature is a regression to triage (bug-gap-triage),"
    echo "             not a row to add to $registry."
    exit 1
fi
if [ "$sample_bad" -ne 0 ]; then
    echo "[known-reds] FAIL: $sample_bad keystone run(s) whose sample is below the floor or"
    echo "             undisclosed — see SAMPLE above. $matched known-red panic(s) matched, but a"
    echo "             run that drew too few cases is not a pass-with-note: the invariants it"
    echo "             never reached are untested, not green."
    exit 1
fi
if [ "$no_verdict" -ne 0 ]; then
    echo "[known-reds] NO VERDICT: nothing can be said about $no_verdict of the $# log(s)."
    echo "             Re-run the keystone and keep the whole log."
    exit 3
fi
if [ "$matched" -eq 0 ] && [ "$collateral" -eq 0 ]; then
    echo "[known-reds] PASS: $green green run(s), nothing to classify."
    exit 0
fi
echo "[known-reds] PASS-WITH-NOTE: $matched known-red panic(s), 0 novel, $collateral collateral (ignored)."
if [ "$green" -ne 0 ]; then
    echo "             Plus $green green run(s) with nothing to classify."
fi
echo "             Registry: docs/Testing/KeystoneKnownReds.md"
