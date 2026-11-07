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
# Exit 2  — usage error, missing registry, or a command this script runs
#           failed: the log is not classified, and stderr names the command.
# Exit 3  — at least one log admits no verdict: INDETERMINATE (empty, or
#           truncated before the run said anything) or UNREADABLE (no such
#           file). Never silently a pass — the input is broken, so nothing can
#           be said about the run at all. Also a nextest status label this
#           script does not know: guessing its meaning would guess the verdict.
#
# A hand-authored replay log names the case of each panic. A case's panic is
# known only when it matches exactly the row the case names (`known_red`), so
# one row never covers another case; a red case without a panic is novel.
# A case that names a row and runs green prints a NOTE line, not a failure.
#
# The registry is the single source of truth for the patterns; this script holds
# none of its own. Apart from that, it knows exactly one panic: the keystone's
# injected boot fault (`INJECTED_SITE` below), which is no failure at all.
set -eEuo pipefail
export LC_ALL=C

# Names the command by its first line: an awk program spans many.
fail() {
    echo "[known-reds] ERROR: ${1%%$'\n'*} failed (exit $2); the log is not classified." >&2
    exit 2
}
trap 'fail "$BASH_COMMAND" "$?"' ERR

here=$(dirname "${BASH_SOURCE[0]}")
repo_root=$(cd "$here/.." && pwd)
registry="$repo_root/docs/Testing/KeystoneKnownReds.md"

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <run.log> [<run.log> ...]" >&2
    exit 2
fi
if [ ! -f "$registry" ]; then
    echo "[known-reds] registry missing: $registry" >&2
    exit 2
fi

work=$(mktemp -d)
trap 'rm -rf "$work" || fail "rm -rf $work" "$?"' EXIT

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
# matching awk for when they count.
rows="$work/rows"
python3 "$repo_root/scripts/check-known-red-evidence.py" --rows "$registry" >"$rows"
if [ ! -s "$rows" ]; then
    echo "[known-reds] registry has no known-red rows — every failure is novel."
fi

# The keystone's `BootFault::TaskPanic` panics on purpose and asserts the
# disclosure itself. Only this payload at this site is no failure: it is
# removed before the outcome is read, so it can neither fail a green run nor
# take the PRIMARY slot from a real failure.
export INJECTED_SITE=crates/holon-integration-tests/src/pbt/composed/boot_fault.rs:22:5
export INJECTED_PAYLOAD='keystone boot fault: injected task panic'
injected=0

# grep whose "no line selected" (exit 1) is a result; any other exit stops the run.
grep_selecting() {
    local rc=0
    grep "$@" || rc=$?
    [ "$rc" -le 1 ] || fail "grep $*" "$rc"
    return "$rc"
}

# awk whose `exit 3` is a verdict it printed itself (the log admits none); any
# other failure stops the run.
awk_verdict() {
    local rc=0
    awk "$@" || rc=$?
    [ "$rc" -ne 3 ] || exit 3
    [ "$rc" -eq 0 ] || fail "awk" "$rc"
}

# A keystone log must disclose the sample its verdict rests on. Prints the
# disclosure and counts the log into `sample_bad` when the sample is below the
# floor or absent. It runs outside any condition, so a failing command in it
# still stops the run.
check_sample() {
    local log="$1" line
    # A run of the HEADLESS keystone only, read from the test runner's own
    # lines: cargo build output names the target without running it, and
    # `general_e2e_composed_pbt_windowed` discloses no sample.
    grep_selecting -qE "^[[:space:]]*test general_e2e_composed_pbt::general_e2e_composed_pbt |thread 'general_e2e_composed_pbt::general_e2e_composed_pbt'|::integration general_e2e_composed_pbt::general_e2e_composed_pbt\$" "$log" || return 0
    line=$(awk 'index($0, "[keystone-sample] cases=") { l = $0 } END { print l }' "$log")
    if [ -z "$line" ]; then
        echo "SAMPLE: UNDISCLOSED — this keystone run printed no [keystone-sample] line, so"
        echo "        it stopped at its first failure (or was truncated) instead of drawing on."
        echo "        Nothing can be said about draws it never made."
        sample_bad=$((sample_bad + 1))
        return 0
    fi
    echo "SAMPLE: ${line#*\] }"
    [ "$line" != "${line%verdict=OK*}" ] || sample_bad=$((sample_bad + 1))
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
    if grep_selecting -qE "$harness_failed_re" "$log"; then
        printf 'failed'
    elif grep_selecting -qE "$green_verdict_re" "$log"; then
        printf 'green'
    elif grep_selecting -qE "$any_error_re" "$log"; then
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
green_declared=0
indeterminate=0
unreadable=0
: >"$work/novel"
: >"$work/hits"

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
    # Another thread's output can also split a head. Text before the name on
    # the head's line is skipped. A head cut after the name continues the one
    # head its scope has open (a line ending in `thread '<name>`); with two or
    # more open, the name is lost, and a scope with a lost name drops none of
    # its panics.
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
    #
    # The injected boot-fault panic is counted into `injected`, not printed.
    sigs="$work/sigs"
    awk_verdict '
        # The thread name of panic head `line`, or "" when the text lost it.
        function thread_of(line,    t, s) {
            sub(/^ +/, "", line)
            t = substr(line, 9)
            if (index(line, "thread " q) == 1 && index(t, q)) return substr(t, 1, index(t, q) - 1)
            # Text written before the name on its line.
            if (match(line, "[^ " q "]*" q " [(][0-9]+[)] panicked at |[^ " q "]*" q " panicked at ") && substr(line, RSTART, 1) != q)
                return substr(line, RSTART, index(substr(line, RSTART), q) - 1)
            # Text written after the name: the head continues an open one.
            s = scope()
            if (open_heads[s] != 1) return ""
            open_heads[s] = 0
            return head_name[s]
        }
        # A line that ends in `thread '"'"'<name>` opens a panic head; it stays
        # open until a `'"'"' (id) panicked at` line continues it.
        function track_head(line,    rest, i, s) {
            if (line ~ /panicked at /) return
            rest = line
            while ((i = index(rest, "thread " q)) > 0) rest = substr(rest, i + 8)
            if (rest == line || index(rest, q)) return
            s = scope()
            if (!open_heads[s]++) head_name[s] = rest
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
            stopped = 1
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
                if (parts[1] == inst && ((inst in unnamed) || !((k in expected) && expected[k] == panics[k]))) return 1
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
        FNR == 1 { section = ""; live = ""; captured = ""; replay_case = ""; split("", open_heads); split("", head_name) }
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
            track_head($0)
            if (scope() in verdict_due && match($0, /^ *(ok|FAILED|ignored)/)) {
                if ($0 ~ /^ *ok/) expected[scope() SUBSEP verdict_due[scope()]]++
                delete verdict_due[scope()]
            }
            # Unbuffered stderr of another thread can put any text, even part of
            # a word, before the head on its line.
            if (match($0, /test [^ ]+ - should panic \.\.\. /)) {
                name = substr($0, RSTART + 5, RLENGTH - 5); sub(/ - should panic \.\.\. $/, "", name)
                if (substr($0, RSTART + RLENGTH) ~ /^ok/) expected[scope() SUBSEP name]++
                else if (substr($0, RSTART + RLENGTH) !~ /^(FAILED|ignored)/) verdict_due[scope()] = name
            } else if (/panicked at /) {
                name = thread_of($0)
                panics[scope() SUBSEP name]++
                if (name == "") unnamed[scope()] = 1
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
        { track_line($0); track_head($0) }
        # A hand-authored replay names the case it runs and the row that case
        # is registered under; a panic between those lines is that case'"'"'s.
        /\[hand-authored regression\] running case "/ {
            replay_case = $0; sub(/^.*running case "/, "", replay_case); sub(/".*$/, "", replay_case)
            declared[replay_case] = match($0, /\(known red: [^,]*,/) ? substr($0, RSTART + 12, RLENGTH - 13) : ""
        }
        /\[hand-authored regression\] (PASSED|RED) case "/ {
            if (/\] RED case "/) red_case[replay_case] = 1
            else if (declared[replay_case] != "") print "note\t" replay_case "\037" declared[replay_case]
            replay_case = ""
        }
        /panicked at / {
            s = scope(); owner = OWNER; key = s SUBSEP thread_of($0)
            loc = $0; sub(/^.*panicked at /, "", loc); sub(/\r$/, "", loc); sub(/:$/, "", loc)
            if ((getline msg) <= 0) next
            track_line(msg); track_head(msg)
            if ((owner in passed) && !(owner in flaky_failed)) next
            if (!(s in unnamed) && (key in expected) && expected[key] == panics[key]) next
            sub(/^ +/, "", msg)
            if (replay_case != "") { msg = msg "\037" replay_case "\037" declared[replay_case]; judged[replay_case] = 1 }
            if (loc == ENVIRON["INJECTED_SITE"] && msg == ENVIRON["INJECTED_PAYLOAD"]) injected++
            else print loc "\t" msg
            next
        }
        END {
            print injected + 0 >injected_out
            if (!stopped)
                for (c in red_case) if (!(c in judged)) print "-\tthe red case printed no panic\037" c "\037" declared[c]
        }' q="'" signals="HUP|INT|QUIT|ILL|TRAP|ABRT|BUS|FPE|KILL|USR1|SEGV|USR2|PIPE|ALRM|TERM" injected_out="$work/injected" "$log" "$log" >"$sigs"

    read -r log_injected <"$work/injected"
    if [ "$log_injected" -gt 0 ]; then
        echo "[known-reds] INJECTED: $log_injected keystone boot-fault panic(s) at $INJECTED_SITE in $log — expected, not a failure."
        injected=$((injected + log_injected))
    fi

    # Panics ARE failure evidence, and outrank the verdict lines: a run killed
    # mid-shrink is truncated before cargo ever prints `test result:`.
    if [ -s "$sigs" ]; then
        outcome=failed
    else
        outcome=$(log_outcome "$log")
    fi

    if [ "$outcome" = green ]; then
        echo "[known-reds] GREEN: $log — run passed, nothing to classify."
        green=$((green + 1))
        continue
    fi

    if [ "$outcome" = indeterminate ]; then
        echo "[known-reds] INDETERMINATE: $log — the log states no pass/fail"
        echo "               outcome (empty, truncated before any verdict, or not a"
        echo "               keystone run log). Tail:"
        tail -20 "$log" | sed 's/^/               | /'
        indeterminate=$((indeterminate + 1))
        continue
    fi

    if [ ! -s "$sigs" ]; then
        echo "[known-reds] NOVEL: $log — run failed but no panic signature was extracted"
        echo "               (compile error or a new failure shape). Tail:"
        tail -20 "$log" | sed 's/^/               | /'
        novel=$((novel + 1))
        continue
    fi

    base=${log##*/}
    echo ""
    echo "=== $base ==="

    # One awk matches every signature against every row. It prints the
    # PRIMARY and shrink-tail entries, appends novel signatures to `novel` and
    # known-red hits (`<row index>\t<example>`) to `hits`, and writes this
    # log's counts to `counts`.
    #
    # Volatile payloads (per-run UUIDs, span Ids, resolver-map contents,
    # split-block indices) are normalized away, so dedup and registry matching
    # operate on the stable shape of the signature.
    #
    # A registry row names ONE violation, so a signature is known only when each
    # of its violations is: an oracle verdict
    # `…diverged from the oracle: [("inv-a", "…"), ("inv-b", "…")]…` splits per
    # invariant, and an `inv-sql-budget` message of N violations splits into N
    # `1 budget violation(s)` messages. A budget message that holds a different
    # number of violations than it announces stops the run (exit 3): the split
    # cannot be trusted, so neither can a verdict. A violation no pattern knows
    # is still known when its invariant is a companion of a row hit by ANOTHER
    # violation of the same signature: alone, a companion is novel.
    #
    # The FIRST panic is what actually failed the run; proptest then re-runs
    # the same case (or a genuinely different one) while shrinking, so every
    # panic after it is a shrink-tail re-panic — printed separately so it
    # can't be mistaken for the run's real failure.
    #
    # A panic is collateral (unwind noise from a thread tearing down after some
    # other panic) when its payload is an opaque `Any { .. }` or it originates
    # in tracing-subscriber's span teardown, not in workspace code. Collateral
    # is reported but never counts toward novel/matched or the verdict. The
    # keystone's own sample-floor panic is reported through the sample verdict
    # below, and the hand-authored replay's closing report lists cases that
    # are each judged on their own panic: neither is a signature.
    LOG_BASE="$base" WORK="$work" awk_verdict '
        function normalize(s,    out, pre) {
            if (s ~ /^Test failed: .*\.$/) s = substr(s, 14, length(s) - 14)
            gsub(/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/, "<uuid>", s)
            out = ""
            while (match(s, /Id\([0-9]+\)/)) {
                pre = substr(s, RSTART - 1, 1)
                out = out substr(s, 1, RSTART - 1) ((RSTART == 1 || pre !~ /[A-Za-z0-9_]/) ? "Id(N)" : substr(s, RSTART, RLENGTH))
                s = substr(s, RSTART + RLENGTH)
            }
            s = out s
            gsub(/Mapped: \[[^]]*\]/, "Mapped: [...]", s)
            gsub(/split-[0-9]+/, "split-N", s)
            return s
        }
        # The end of the Debug-escaped string that opens at s[p], or 0.
        function string_end(s, p,    c) {
            for (p++; p <= length(s); p++) {
                c = substr(s, p, 1)
                if (c == "\\") p++
                else if (c == "\"") return p
            }
            return 0
        }
        # Fills PIECE[1..n] with the violations of `sig`, each shaped as the
        # panic would read had that violation fired alone; returns n.
        function split_violations(sig,    at, prefix, n, p, id_end, msg_end, head, msg, suffix, budget_head, i, announced, rest, k, one, j, out) {
            PIECE[1] = sig
            at = index(sig, "diverged from the oracle: [(")
            if (at == 0) return 1
            at += length("diverged from the oracle: ")
            prefix = substr(sig, 1, at - 1)
            n = 0; p = at + 1
            while (1) {
                if (substr(sig, p, 2) != "(\"") return 1
                id_end = string_end(sig, p + 1)
                if (!id_end || substr(sig, id_end, 3) != "\", ") return 1
                msg_end = string_end(sig, id_end + 3)
                if (!msg_end || substr(sig, msg_end + 1, 1) != ")") return 1
                n++
                head[n] = substr(sig, p, id_end + 4 - p)
                msg[n] = substr(sig, id_end + 4, msg_end - id_end - 4)
                p = msg_end + 2
                if (substr(sig, p, 2) == ", ") { p += 2; continue }
                if (substr(sig, p, 1) == "]") break
                return 1
            }
            suffix = substr(sig, p + 1)
            budget_head = "[inv-sql-budget] 1 budget violation(s):\\n  "
            out = 0
            for (i = 1; i <= n; i++) {
                if (!match(msg[i], /^\[inv-sql-budget\] [0-9]+ budget violation\(s\):\\n  /)) {
                    PIECE[++out] = prefix "[" head[i] msg[i] "\")]" suffix
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
                    stopped = 1
                    exit 3
                }
                for (j = 1; j <= k; j++) PIECE[++out] = prefix "[" head[i] budget_head one[j] "\")]" suffix
            }
            while (out > 1 && PIECE[out] == "") out--
            return out
        }
        function glob_re(g,    r, i, c) {
            r = "^"
            for (i = 1; i <= length(g); i++) {
                c = substr(g, i, 1)
                if (c == "*") r = r ".*"
                else if (c == "?") r = r "."
                else if (index("\\.[]^$+{}()|/", c)) r = r "\\" c
                else r = r c
            }
            return r "$"
        }
        function row_index(k,    i) {
            for (i = 1; i <= nrows; i++) if (key[i] == k) return i
            return 0
        }
        BEGIN { FS = "\037"; prefix_inv = "diverged from the oracle: [(\"" }
        FNR == NR {
            if ($1 != "") { key[++nrows] = $1; pat[nrows] = $2; site[nrows] = $3; comp[nrows] = $4 }
            next
        }
        {
            tab = index($0, "\t")
            loc = tab ? substr($0, 1, tab - 1) : $0
            sig = tab ? substr($0, tab + 1) : ""
            sub(/^\t+/, "", sig); sub(/\t+$/, "", sig)
            if (loc == "note") {
                cut = index(sig, "\037")
                print "[known-reds] NOTE: known_red " substr(sig, cut + 1) " case \"" substr(sig, 1, cut - 1) "\": declared known red, ran green"
                green_declared++
                next
            }
            line_no++
            replay_case = ""; declared_row = ""
            first = index(sig, "\037")
            if (first) {
                last = first
                while ((cut = index(substr(sig, last + 1), "\037"))) last += cut
                declared_row = substr(sig, last + 1)
                replay_case = last > first ? substr(sig, first + 1, last - first - 1) : substr(sig, first + 1)
                sig = substr(sig, 1, first - 1)
            }
            norm = normalize(sig)
            if (index(sig, "[keystone-sample] FLOOR BREACH:")) {
                class = "sample-floor"
            } else if (replay_case == "" && sig ~ /^hand-authored replay: [0-9]+ red case\(s\): /) {
                class = "replay-report"
            } else if (index(sig, "Any { .. }") || loc ~ /tracing-subscriber-.*sharded\.rs/) {
                class = "collateral"
                collateral++
            } else {
                n = split_violations(norm)
                nhits = 0; cross_hit = 0; nunmatched = 0
                split("", is_hit)
                for (j = 1; j <= n; j++) {
                    hit = 0
                    for (i = 1; i <= nrows; i++) if (PIECE[j] ~ pat[i]) { hit = i; break }
                    if (hit && site[hit] != "" && loc !~ site[hit]) {
                        if (!cross_hit) cross_hit = hit
                    } else if (hit) {
                        if (!(hit in is_hit)) { is_hit[hit] = 1; HIT[++nhits] = hit }
                    } else UNMATCHED[++nunmatched] = PIECE[j]
                }
                novel_pieces = ""
                for (j = 1; j <= nunmatched; j++) {
                    invariant = ""
                    if ((at = index(UNMATCHED[j], prefix_inv))) {
                        invariant = substr(UNMATCHED[j], at + length(prefix_inv))
                        if ((cut = index(invariant, "\""))) invariant = substr(invariant, 1, cut - 1)
                    }
                    claimed = 0
                    for (h = 1; h <= nhits; h++) {
                        ng = split(comp[HIT[h]], globs, " ")
                        for (g = 1; g <= ng; g++) if (invariant ~ glob_re(globs[g])) claimed = 1
                    }
                    if (!claimed) novel_pieces = novel_pieces substr(UNMATCHED[j], 1, 240) "\n"
                }
                if (cross_hit) {
                    print "[known-reds] FAIL: cross-family match: " key[cross_hit] " at " loc
                    print "             The row" q "s wording matched, but it asserts from"
                    print "             " site[cross_hit] " — so this signature belongs to another family."
                    print "             Triage it (bug-gap-triage). Add this site to the row ONLY if"
                    print "             the log shows the SAME root cause re-raised there (an outer"
                    print "             harness re-reporting the inner verdict); never to silence it."
                    class = "cross-family:" key[cross_hit]
                    cross++
                } else if (novel_pieces != "") {
                    class = "novel"
                    if (replay_case != "")
                        print "[known-reds] FAIL: case \"" replay_case "\" matches no row; it names " (declared_row != "" ? declared_row : "no row")
                    # Grouped by normalized message, not location: a panic site and
                    # the terminal re-report proptest prints are one novel signature.
                    printf "%s", novel_pieces >>novel_out
                    novel++
                } else if (replay_case != "" && !(nhits == 1 && HIT[1] == row_index(declared_row))) {
                    class = "novel"
                    matched_rows = ""
                    for (h = 1; h <= nhits; h++) matched_rows = matched_rows (h > 1 ? "+" : "") key[HIT[h]]
                    print "[known-reds] FAIL: case \"" replay_case "\" matches " matched_rows "; it names " (declared_row != "" ? declared_row : "no row")
                    printf "case \"%s\": %s\n", replay_case, substr(norm, 1, 240) >>novel_out
                    novel++
                } else {
                    class = "known-red:"
                    for (h = 1; h <= nhits; h++) {
                        class = class (h > 1 ? "+" : "") key[HIT[h]]
                        print HIT[h] "\t" ENVIRON["LOG_BASE"] " @ " loc ": " substr(norm, 1, 240) >>hits_out
                    }
                    matched++
                }
            }
            entry = "[" class "] " ENVIRON["LOG_BASE"] " @ " loc (replay_case != "" ? " case \"" replay_case "\"" : "") ": " substr(norm, 1, 240)
            if (line_no == 1) print "PRIMARY: " entry
            else tail[++ntail] = entry
        }
        END {
            if (stopped) exit 3
            if (ntail) {
                print "  -- shrink tail (" ntail " re-panics after the primary; may be a"
                print "     different bug than the one that actually failed the run) --"
                for (t = 1; t <= ntail; t++) print "  " tail[t]
            }
            print novel + 0, matched + 0, cross + 0, collateral + 0, green_declared + 0 >counts_out
        }' q="'" novel_out="$work/novel" hits_out="$work/hits" counts_out="$work/counts" "$rows" "$sigs"
    read -r log_novel log_matched log_cross log_collateral log_green_declared <"$work/counts"
    novel=$((novel + log_novel))
    matched=$((matched + log_matched))
    cross=$((cross + log_cross))
    collateral=$((collateral + log_collateral))
    green_declared=$((green_declared + log_green_declared))

    check_sample "$log"
done
echo ""

awk 'BEGIN { FS = "\037" }
    FNR == NR { if ($1 != "") key[++nrows] = $1; next }
    {
        tab = index($0, "\t"); i = substr($0, 1, tab - 1)
        if (!count[i]++) example[i] = substr($0, tab + 1)
    }
    END {
        for (i = 1; i <= nrows; i++)
            if (count[i]) { print "WARN known-red [" key[i] "] x" count[i]; print "     " example[i] }
    }' "$rows" "$work/hits"
if [ -s "$work/novel" ]; then
    echo ""
    echo "NOVEL signatures (distinct, with occurrence counts):"
    sort "$work/novel" | uniq -c | sort -rn | sed 's/^/  /'
fi

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
if [ "$matched" -eq 0 ] && [ "$collateral" -eq 0 ] && [ "$green_declared" -eq 0 ]; then
    echo "[known-reds] PASS: $green green run(s), nothing to classify."
    exit 0
fi
echo "[known-reds] PASS-WITH-NOTE: $matched known-red panic(s), 0 novel, $collateral collateral (ignored)."
if [ "$green_declared" -ne 0 ]; then
    echo "             Plus $green_declared declared known-red case(s) that ran green — see NOTE above."
fi
if [ "$green" -ne 0 ]; then
    echo "             Plus $green green run(s) with nothing to classify."
fi
echo "             Registry: docs/Testing/KeystoneKnownReds.md"
