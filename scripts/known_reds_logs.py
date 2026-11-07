#!/usr/bin/env python3
"""Test-run logs drawn from a model, and the verdict scripts/keystone-known-reds.sh owes each.

A model run is a list of tests, each with a final status and the panics its
threads print. `render` prints a run in one of four shapes:
- `libtest`: a cargo `Running` section, `thread 'n' (id) panicked at` heads,
  `test n ... ok` and `test n - should panic ... ok` verdicts;
- `captured`: nextest status lines, each test's output indented under its own;
- `serial`: nextest `--no-capture` behind its marker line, each test's output
  between its `START` and its status line;
- `parallel`: `--no-capture` without the marker, all `START`s first.
With `interleave` it adds what concurrent threads do to the text: foreign lines
between lines, and a libtest panic head split before or after the thread name.
While it prints, it records the owner and thread name the text still states for
each panic; `verdict` reads only that record.

`python3 scripts/known_reds_logs.py write-examples <dir>` writes each of
EXAMPLES to <dir>/<name>.log and prints `<name>\t<exit>\t<line regex>`.
"""
import re
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

INJECTED_SITE = "crates/holon-integration-tests/src/pbt/composed/boot_fault.rs:22:5"
INJECTED_MSG = "keystone boot fault: injected task panic"
# The panic of the `ref-diverge-content` row, at the site the row names.
KNOWN_SITE = "crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13"
KNOWN_MSG = r'reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  field deltas (1):\n    block:a: content: sut=\"x\" ref=\"y\"\n")]'

PASS_LABELS = ["PASS", "LEAK", "TMPASS", "TIMEOUT-PASS", "SLOW + LEAK", "SLOW+TMPASS"]
# Each failing nextest label, short or long, with the long label the classifier prints.
FAIL_LABELS = {
    "FAIL": "FAIL", "FAIL + LEAK": "FAIL + LEAK", "FL+LK": "FAIL + LEAK",
    "LKFAIL": "LEAK-FAIL", "LEAK-FAIL": "LEAK-FAIL", "TMT": "TIMEOUT", "TIMEOUT": "TIMEOUT",
    "XFAIL": "XFAIL", "ABORT": "ABORT", "SIG 9": "ABORT SIG 9", "ABORT SIG 6": "ABORT SIG 6",
    "SEGV": "SIGSEGV", "SIGSEGV": "SIGSEGV", "SIGABRT": "SIGABRT",
}
# Labels whose test ran its body to a libtest verdict: `ok`, or `FAILED` for these.
BODY_FAILED = {"FAIL", "FAIL + LEAK", "FL+LK"}
BODY_OK = set(PASS_LABELS) | {"LKFAIL", "LEAK-FAIL"}
SERIAL_MARKER = "warning: ignoring --test-threads because --no-capture is specified"
FOREIGN = [
    "Applying transition 4/4: ()",
    "",
    "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
    "[wide-e2e wiring] drawn: storage={Loro, Org}",
    "  left: Object {}",
]
# Text another thread prints right after `thread '` or right before the name.
SPLIT_AFTER_QUOTE = ["", "()"]
SPLIT_BEFORE_NAME = ["", "Applying transition 1/4: ", ": "]
WORKER = "tokio-runtime-worker"


@dataclass
class Panic:
    kind: str  # novel | known | injected
    thread: str
    site: str
    msg: str


@dataclass
class Test:
    name: str
    status: str  # a nextest label, or libtest `ok` / `FAILED`
    should_panic: bool
    panics: list


@dataclass
class Run:
    shape: str  # libtest | captured | serial | parallel
    binary: str
    tests: list
    # A serial run killed in its last test, after that test's panics.
    cut: bool = False


@dataclass
class Rendered:
    lines: list = field(default_factory=list)
    # (scope, thread name as the text states it, Panic); scope is ("section", run) or ("inst", run, unit).
    panics: list = field(default_factory=list)
    # (scope, name) of each `- should panic ... ok` line.
    expects: list = field(default_factory=list)
    # (("inst", run, unit), label) of each nextest status line.
    statuses: list = field(default_factory=list)
    # Indexes into `panics` whose owner the text proves (a captured block, a
    # serial run's START to status) and whose status in that run passes.
    in_passing: set = field(default_factory=set)
    failed: bool = False

    @property
    def text(self):
        return "\n".join(self.lines) + "\n"


@dataclass
class Verdict:
    exit: int
    novel: int
    matched: int
    listed: Counter  # the NOVEL signature lines with their counts
    note: int  # panics inside passing tests


def word(rng, used):
    while True:
        w = "".join(rng.choice("abcdefghijklmnopqrstuvwxyz") for _ in range(rng.randint(4, 8)))
        if w not in used:
            used.add(w)
            return w


def draw_panic(rng, used, thread):
    kind = rng.choices(["novel", "known", "injected"], [5, 3, 2])[0]
    if kind == "known":
        return Panic(kind, thread, KNOWN_SITE, KNOWN_MSG)
    if kind == "injected":
        return Panic(kind, thread, INJECTED_SITE, INJECTED_MSG)
    site = f"crates/{word(rng, used)}/tests/{word(rng, used)}.rs:{rng.randint(1, 900)}:{rng.randint(1, 40)}"
    return Panic(kind, thread, site, "novel defect " + " ".join(word(rng, used) for _ in range(3)))


def draw_test(rng, used, shape, name=None):
    name = name or f"{word(rng, used)}::case_{word(rng, used)}"
    should_panic = rng.random() < 0.5
    if shape == "libtest":
        status = rng.choice(["ok", "ok", "FAILED"])
        passing = status == "ok"
    else:
        status = rng.choice(PASS_LABELS * 2 + list(FAIL_LABELS))
        passing = status in BODY_OK
    panics = []
    if should_panic and passing:
        panics.append(draw_panic(rng, used, name))
    for _ in range(rng.choice([0, 0, 1, 2]) if not panics else rng.choice([0, 0, 0, 1])):
        panics.append(draw_panic(rng, used, name if rng.random() < 0.85 else WORKER))
    # shortcut: a libtest test fails here only by a novel panic of its own; a failure
    # without one reads as "no signature", which a printed known red masks. Draw it once the classifier attributes libtest failures.
    if status == "FAILED" and not any(p.kind == "novel" and p.thread == name for p in panics):
        panics.append(Panic("novel", name, f"crates/{word(rng, used)}/tests/{word(rng, used)}.rs:1:1", "novel defect " + word(rng, used)))
    return Test(name, status, should_panic, panics)


def draw(rng):
    """One log's runs: one or two runs of one to five tests.

    A nextest run after a nextest run may rerun tests of it, as a rerun or a
    second recipe in the same log does.
    """
    used = set()
    runs = []
    for _ in range(rng.choice([1, 2])):
        shape = rng.choice(["libtest", "captured", "serial", "parallel"])
        binary = f"holon-{word(rng, used)}::{word(rng, used)}"
        earlier = []
        if runs and runs[-1].shape != "libtest" and shape != "libtest" and rng.random() < 0.5:
            binary = runs[-1].binary
            earlier = [t.name for t in runs[-1].tests]
        tests = []
        for _ in range(rng.randint(1, 5)):
            again = [n for n in earlier if n not in {t.name for t in tests}]
            tests.append(draw_test(rng, used, shape, rng.choice(again) if again and rng.random() < 0.6 else None))
        runs.append(Run(shape, binary, tests))
    # Only a run with a later one may be cut: a log that ends cut off states no
    # outcome, which the classifier reports as no verdict (exit 3).
    if len(runs) == 2 and runs[0].shape == "serial" and runs[0].tests[-1].panics:
        runs[0].cut = rng.random() < 0.7
    return runs


def panic_head(p, tid):
    return f"thread '{p.thread}' ({tid}) panicked at {p.site}:"


def verdict_line(t, verdict):
    return f"test {t.name}{' - should panic' if t.should_panic else ''} ... {verdict}"


def foreign(rng, out, indent=""):
    for _ in range(rng.choice([1, 1, 2])):
        line = rng.choice(FOREIGN)
        out.lines.append(indent + line if line else "")


def render_libtest(run, index, rng, interleave, out):
    scope = ("section", index)
    out.lines += [f"     Running tests/main.rs (target/debug/deps/{run.binary.replace('::', '-')})", "", f"running {len(run.tests)} tests"]
    queues = []
    for t in run.tests:
        chunks = []
        for p in t.panics:
            split = rng.choice(["whole", "after", "before"]) if interleave else "whole"
            if split == "whole":
                chunks.append(("whole", p))
            elif split == "after":
                chunks += [("head-after", p), ("cont-after", p)]
            else:
                chunks += [("head-before", rng.choice(SPLIT_AFTER_QUOTE)), ("cont-before", p, rng.choice(SPLIT_BEFORE_NAME))]
        chunks.append(("verdict", t))
        queues.append(chunks)
    open_heads = 0
    tid = 100
    while any(queues):
        chunk = rng.choice([q for q in queues if q]).pop(0)
        tid += 1
        if chunk[0] == "whole":
            p = chunk[1]
            out.lines += [panic_head(p, tid), p.msg]
            out.panics.append((scope, p.thread, p))
        elif chunk[0] == "head-after":
            out.lines.append(f"thread '{chunk[1].thread}")
            open_heads += 1
        elif chunk[0] == "cont-after":
            p = chunk[1]
            out.lines += [f"' ({tid}) panicked at {p.site}:", p.msg]
            # Only a single open head names the continuation.
            out.panics.append((scope, p.thread if open_heads == 1 else "", p))
            if open_heads == 1:
                open_heads = 0
        elif chunk[0] == "head-before":
            out.lines.append("thread '" + chunk[1])
            open_heads += 1
        elif chunk[0] == "cont-before":
            p = chunk[1]
            out.lines += [chunk[2] + f"{p.thread}' ({tid}) panicked at {p.site}:", p.msg]
            out.panics.append((scope, p.thread, p))
        else:
            t = chunk[1]
            out.lines.append(verdict_line(t, t.status))
            if t.should_panic and t.status == "ok":
                out.expects.append((scope, t.name))
        if interleave and rng.random() < 0.3:
            foreign(rng, out)
    failed = sum(t.status == "FAILED" for t in run.tests)
    out.failed |= failed > 0
    out.lines.append(f"test result: {'FAILED' if failed else 'ok'}. {len(run.tests) - failed} passed; {failed} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s")


def body(t, cut=False):
    """The libtest lines a nextest test prints: (line, panic or None)."""
    lines = [("", None), ("running 1 test", None)]
    for p in t.panics:
        lines += [(panic_head(p, 7), p), (p.msg, None)]
    if cut:
        pass
    elif t.status in BODY_OK:
        lines.append((verdict_line(t, "ok"), None))
    elif t.status in BODY_FAILED:
        lines.append((verdict_line(t, "FAILED"), None))
    return lines


def status_line(label, i, n, unit):
    return f"{label:>12} [   0.010s] ({i}/{n}) {unit}"


def render_body(t, scope, rng, interleave, out, indent="", cut=False, proven=False):
    """`proven`: the text proves `t` owns its panics, so each counts by `t`'s own status."""
    first = len(out.panics)
    for line, p in body(t, cut):
        out.lines.append(indent + line if line else "")
        if p is not None:
            out.panics.append((scope, p.thread, p))
        elif line.endswith(" - should panic ... ok"):
            out.expects.append((scope, t.name))
        # nextest captures a test's output whole: only --no-capture output interleaves.
        if interleave and not indent and p is None and rng.random() < 0.3:
            foreign(rng, out)
    if proven and not cut and t.status in PASS_LABELS:
        out.in_passing.update(range(first, len(out.panics)))


def render_nextest(run, index, rng, interleave, out):
    n = len(run.tests)
    units = [f"{run.binary} {t.name}" for t in run.tests]
    if run.shape == "serial":
        out.lines.append(SERIAL_MARKER)
    out.lines.append(f"    Starting {n} tests across 1 binary")
    if run.shape == "parallel":
        for i, unit in enumerate(units, 1):
            out.lines.append(status_line("START", i, n, unit))
        # With every test started, the text names only the last START as owner.
        for t in run.tests:
            render_body(t, ("inst", index, units[-1]), rng, interleave, out)
    for i, (t, unit) in enumerate(zip(run.tests, units), 1):
        if run.shape == "serial":
            out.lines.append(status_line("START", i, n, unit))
            cut = run.cut and i == n
            render_body(t, ("inst", index, unit), rng, interleave, out, cut=cut, proven=True)
            if cut:
                return
        out.lines.append(status_line(t.status, i, n, unit))
        out.statuses.append((("inst", index, unit), t.status))
        if run.shape == "captured" and (t.panics or t.should_panic):
            out.lines.append("  stderr ───")
            render_body(t, ("inst", index, unit), rng, interleave, out, indent="    ", proven=True)
    failing = [(i, t, unit) for i, (t, unit) in enumerate(zip(run.tests, units), 1) if t.status in FAIL_LABELS]
    out.lines.append(f"     Summary [   1.000s] {n} tests run: {n - len(failing)} passed, {len(failing)} failed, 0 skipped")
    for i, t, unit in failing:
        out.lines.append(status_line(t.status, i, n, unit))
    if failing:
        out.lines.append("error: test run failed")
        out.failed = True


def render(runs, rng, interleave):
    out = Rendered()
    for index, run in enumerate(runs):
        if run.shape == "libtest":
            render_libtest(run, index, rng, interleave, out)
        else:
            render_nextest(run, index, rng, interleave, out)
    return out


def verdict(r):
    """The classifier's verdict on `r`, from the owners and names its text states.

    A panic is inside a passing test, and dropped into the note, when the text
    proves its owner and the owner passed in that run, or when its scope holds as many
    `- should panic ... ok` lines for its thread name as panics on that name and no panic whose name
    the text loses. Every other panic is printed, except an injected one. A
    failing status is a signature unless its label is FAIL / FAIL + LEAK and
    its owner printed a panic or an injected one.
    """
    expected = Counter(r.expects)
    count = Counter((scope, name) for scope, name, _ in r.panics)
    unnamed = {scope for scope, name, _ in r.panics if name == ""}
    kept = [(scope, p) for i, (scope, name, p) in enumerate(r.panics)
            if i not in r.in_passing
            and (scope in unnamed or not (expected[(scope, name)] and expected[(scope, name)] == count[(scope, name)]))]
    note = len(r.panics) - len(kept)
    listed = Counter(p.msg for _, p in kept if p.kind == "novel")
    matched = sum(p.kind == "known" for _, p in kept)
    owners = {scope for scope, _ in kept}
    for inst, label in r.statuses:
        long = FAIL_LABELS.get(label)
        if long and not (long in ("FAIL", "FAIL + LEAK") and inst in owners):
            listed[f"{long}: {inst[2]}"] = 1
    novel = sum(listed.values())
    if novel + matched == 0 and r.failed:
        novel = 1  # "run failed but no panic signature was extracted"
    return Verdict(1 if novel else 0, novel, matched, listed, note)


NOTE = re.compile(r"^\[known-reds\] NOTE: (\d+) panic\(s\) inside passing tests \(ignored\)$", re.M)
FINAL = re.compile(r"^\[known-reds\] (?:FAIL: (\d+) novel panic\(s\), (\d+) known-red|PASS-WITH-NOTE: (\d+) known-red|PASS: \d+ green run)", re.M)


def parse(stdout):
    """The Verdict that classifier output `stdout` states, with exit code left 0."""
    found = FINAL.findall(stdout)
    assert len(found) == 1, f"want one final verdict line, got {found}:\n{stdout}"
    novel, matched, note = found[0]
    listed = Counter()
    if "\nNOVEL signatures (distinct, with occurrence counts):\n" in stdout:
        section = stdout.split("\nNOVEL signatures (distinct, with occurrence counts):\n", 1)[1].split("\n\n", 1)[0]
        for line in section.splitlines():
            m = re.match(r"^ +(\d+) (.*)$", line)
            assert m, f"unreadable NOVEL line {line!r}"
            listed[m.group(2)] = int(m.group(1))
    notes = NOTE.findall(stdout)
    assert len(notes) <= 1, f"want at most one NOTE line, got {notes}"
    return Verdict(0, int(novel or 0), int(matched or note or 0), listed, int(notes[0]) if notes else 0)


W123 = r'''     Running tests/main.rs (target/debug/build/holon-integration-tests/c316d237e1494d27/out/integration-c316d237e1494d27)

running 19 tests

thread 'hand_authored_regressions::empty_locks_are_refused' (1382932353) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:176:9:
hand-authored regression "<inline>":1 case "n": `locks` is empty; a case locks one invariant at least
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'hand_authored_regressions::mistyped_initial_state_key_is_loud' (1382932358) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:417:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state" — a mistyped key is SILENTLY DROPPED by serde, so this case would replay something other than what it names. Known keys: ["name", "description", "environment", "initial_state", "transitions", "locks", "also_check_after", "known_red"]
  line: {"name": "n", "transitions": [], "initail_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}}, "locks": ["inv-x"]}
thread 'hand_authored_regressions::nested_unknown_wiring_field_is_loud
Applying transition 4/4: ()
test hand_authored_regressions::replay_sweeps_around_the_last_transition_only ... ok
test hand_authored_regressions::recorded_env_flag_mismatch_is_reported ... ok
test hand_authored_regressions::replay_of_a_case_without_transitions_sweeps_once_after_boot ... ok
test hand_authored_regressions::replay_also_sweeps_around_each_named_transition ... ok
' (1382932359) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:427:5:
assertion `left == right` failed: hand-authored regression "<inline>":1: `initial_state` does not round-trip — the authored JSON and the parsed value differ, so replaying this case would start from a DIFFERENT state than the run it pins
  left: Object {"wiring": Object {"storage_adapters": Array [String("Turso")], "sync_adapters": Array [], "actors": Array [], "peers": Array [String("x")]}}
 right: Object {"wiring": Object {"storage_adapters": Array [String("Turso")], "sync_adapters": Array [], "actors": Array []}}

thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused' (1382932350) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:226:13:
hand-authored regression "<inline>":1 case "n": `also_check_after` names transition 4, out of range 1..3 (1-based; the last transition 4 is always checked)

test hand_authored_regressions::invalid_pinned_wiring_is_loud - should panic ... ok
test hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused - should panic ... ok
test hand_authored_regressions::also_check_after_the_last_transition_is_refused - should panic ... ok
test hand_authored_regressions::nested_unknown_wiring_field_is_loud - should panic ... ok
test hand_authored_regressions::unknown_initial_state_field_is_loud - should panic ... ok
test hand_authored_regressions::empty_locks_are_refused - should panic ... ok
test hand_authored_regressions::a_case_without_locks_is_refused_by_name - should panic ... ok
test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok
test hand_authored_regressions::malformed_also_check_after_is_refused - should panic ... ok
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 619 filtered out; finished in 630.01s
'''

GATE = r'''     Running tests/main.rs (target/debug/build/holon-integration-tests/ea3330c8b0dd9d83/out/integration-ea3330c8b0dd9d83)

running 19 tests

thread 'hand_authored_regressions::invalid_pinned_wiring_is_loud' (1713310698) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:61:13:
<inline>: initial_state carries an INVALID wiring manifest Wiring { storage_adapters: {}, sync_adapters: {}, actors: {}, editor_leg: Dispatch }: wiring has no storage adapter (nothing to display/persist) — `init_state` only ever draws valid manifests, so this case could not have come from a real run
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'hand_authored_regressions::a_case_without_locks_is_refused_by_name' (1713310690) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:168:13:
hand-authored regression "<inline>":1 case "no-locks" has no `locks` field: name the catalog invariant id(s) its bug breaks (docs/Testing/HandAuthoredRegressions.md)

thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused' (1713310693) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:226:13:
hand-authored regression "<inline>":1 case "n": `also_check_after` names transition 4, out of range 1..3 (1-based; the last transition 4 is always checked)

thread 'hand_authored_regressions::malformed_also_check_after_is_refused' (1713310700) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:222:17:
hand-authored regression "<inline>":1 case "n": `also_check_after` holds "2", not a transition number

thread 'test hand_authored_regressions::absent_initial_state_parses_as_none ... ok
test hand_authored_regressions::recorded_env_flag_mismatch_is_reported ... ok

Applying transition 1/4: hand_authored_regressions::mistyped_initial_state_key_is_loud' (1713310701) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:417:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state" — a mistyped key is SILENTLY DROPPED by serde, so this case would replay something other than what it names. Known keys: ["name", "description", "environment", "initial_state", "transitions", "also_check_after", "locks", "known_red"]
  line: {"name": "n", "transitions": [], "initail_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}}}

thread 'hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused' (1713310691) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:315:5:
locked invariant(s) ["inv-x"] were `Skipped` at every checkpoint: the case never reaches the state they judge, so it locks nothing (engaged: {"inv-no-errors"})
()

: 
thread '()
hand_authored_regressions::nested_unknown_wiring_field_is_loud' (1713310702) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:427:5:
assertion `left == right` failed: hand-authored regression "<inline>":1: `initial_state` does not round-trip — the authored JSON and the parsed value differ, so replaying this case would start from a DIFFERENT state than the run it pins
  left: Object {"wiring": Object {"storage_adapters": Array [String("Turso")], "sync_adapters": Array [], "actors": Array [], "peers": Array [String("x")]}}
 right: Object {"wiring": Object {"storage_adapters": Array [String("Turso")], "sync_adapters": Array [], "actors": Array []}}
[hand-authored regression] running case "create-block-smoke" (1 transitions, Checkpoints({1}), Locks({"inv-blocks-match-ref/block_raw", "inv-live-children-match-ref"})test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok
)
test hand_authored_regressions::a_case_without_locks_is_refused_by_name - should panic ... ok
test hand_authored_regressions::unknown_initial_state_field_is_loud - should panic ... ok
test hand_authored_regressions::invalid_pinned_wiring_is_loud - should panic ... ok
test hand_authored_regressions::malformed_also_check_after_is_refused - should panic ... ok
test hand_authored_regressions::empty_locks_are_refused - should panic ... ok
test hand_authored_regressions::also_check_after_the_last_transition_is_refused - should panic ... ok
test hand_authored_regressions::nested_unknown_wiring_field_is_loud - should panic ... ok
test hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused - should panic ... ok
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 623 filtered out; finished in 484.18s
'''

SPLIT_HEAD = "thread 'hand_authored_regressions::nested_unknown_wiring_field_is_loud\n"
ALSO_CHECK = "thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused' (1382932350) panicked at"
ALSO_CHECK_OK = "test hand_authored_regressions::also_check_after_the_last_transition_is_refused - should panic ... ok\n"
KNOWN_FAILED = f"thread 'hand_authored_keystone_regressions' (3) panicked at {KNOWN_SITE}:\n{KNOWN_MSG}\ntest hand_authored_keystone_regressions ... FAILED\n"


def also_check_split(verdict):
    """A passing (or failing) should_panic test whose head splits after the name, beside a listed red."""
    text = W123.replace(ALSO_CHECK, "thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused\nApplying transition 4/4: ()\n' (1382932350) panicked at")
    text = text.replace(ALSO_CHECK_OK, ALSO_CHECK_OK.replace("... ok", f"... {verdict}"))
    return text.replace("test result: ok. 19 passed; 0 failed", KNOWN_FAILED + "test result: FAILED. 18 passed; 1 failed")


CRASH_HEAD = "thread 'tokio-runtime-worker' (11) panicked at crates/holon-filesystem/src/crash_injection.rs:43:9:"
CRASH_MSG = "[crash-injection] the file-sync controller dies at after_write_back"


def no_capture_run(a_head, a_msg, a_status, marker=True):
    """A `--no-capture` run: test `a` prints the given panic and ends in `a_status`, test `b` fails by the known red."""
    a, b = "holon-filesystem::crash a_controller_dies", "holon-filesystem::crash b_diverges"
    verdict = "ok" if a_status == "PASS" else "FAILED"
    failing = [status_line("FAIL", 2, 2, b)] + ([status_line(a_status, 1, 2, a)] if a_status != "PASS" else [])
    return "\n".join(([SERIAL_MARKER] if marker else []) + [
        "    Starting 2 tests across 1 binary",
        status_line("START", 1, 2, a), "", "running 1 test", a_head, a_msg,
        "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
        f"test a_controller_dies ... {verdict}", status_line(a_status, 1, 2, a),
        status_line("START", 2, 2, b), "", "running 1 test", f"thread 'b_diverges' (12) panicked at {KNOWN_SITE}:", KNOWN_MSG,
        "test b_diverges ... FAILED", status_line("FAIL", 2, 2, b),
        f"     Summary [   1.000s] 2 tests run: {2 - len(failing)} passed, {len(failing)} failed, 0 skipped",
    ] + failing + ["error: test run failed"]) + "\n"


GREEN_SERIAL_RUN = "\n".join([
    SERIAL_MARKER, "    Starting 1 test across 1 binary",
    status_line("START", 1, 1, "holon-filesystem::scan scans"), status_line("PASS", 1, 1, "holon-filesystem::scan scans"),
    "     Summary [   1.000s] 1 test run: 1 passed, 0 skipped",
]) + "\n"
CAUGHT_HEAD = "thread 'a_controller_dies' (13) panicked at crates/holon-integration-tests/src/pbt/composed/slo_probe.rs:106:9:"
CAUGHT_MSG = 'slo probe: armed while clocks were pending on ["block:slo-probe-leftover"]. They belong to an earlier stretch of this process'

# Parentheses as bracket expressions: grep reads `\(` as a group.
NOTE_ONE = r"^\[known-reds\] NOTE: 1 panic[(]s[)] inside passing tests [(]ignored[)]$"


def examples():
    """(name, log text, want exit, a regex one output line must match), in grep's and Python's common syntax."""
    gate = GATE
    return [
        # A head split after the name, with one open head in its section.
        ("w123-split-after-name", W123, 0, r"^\[known-reds\] PASS: 1 green run"),
        # A second open head makes the split panic's name unknown: novel.
        ("w123-two-open-heads", W123.replace(SPLIT_HEAD, SPLIT_HEAD + "thread 'hand_authored_regressions::empty_locks_are_refused\n"), 1,
         r"^ *1 assertion .left == right. failed: hand-authored regression .*does not round-trip"),
        # Foreign text before the name on the head's own line, and an `ok` glued behind `thread '`.
        ("gate-overwritten-head", gate, 0, r"^\[known-reds\] PASS: 1 green run"),
        ("gate-overwritten-head-failed", gate.replace("mistyped_initial_state_key_is_loud - should panic ... ok", "mistyped_initial_state_key_is_loud - should panic ... FAILED")
         .replace("test result: ok. 19 passed; 0 failed", "test result: FAILED. 18 passed; 1 failed"), 1,
         r"^ *1 hand-authored regression .*unknown top-level key .initail_state."),
        # The passing should_panic test's message beside one listed red is no novel panic.
        ("should-panic-split-beside-a-red", also_check_split("ok"), 0, r"^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic"),
        # A failing test's panic on no list stays novel.
        ("should-panic-split-beside-a-red-failed", also_check_split("FAILED"), 1, r"^\[known-reds\] FAIL: 1 novel panic"),
        # Under the serial marker a worker panic between a passing test's START and status is that test's.
        ("serial-worker-panic-in-a-passing-test", no_capture_run(CRASH_HEAD, CRASH_MSG, "PASS"), 0, NOTE_ONE),
        ("serial-worker-panic-in-a-passing-test-beside-a-red", no_capture_run(CRASH_HEAD, CRASH_MSG, "PASS"), 0,
         r"^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic"),
        ("serial-worker-panic-in-a-failing-test", no_capture_run(CRASH_HEAD, CRASH_MSG, "FAIL"), 1, r"^\[known-reds\] FAIL: 1 novel panic"),
        # Without the marker tests may overlap, so the START does not prove the owner.
        ("unmarked-worker-panic-in-a-passing-test", no_capture_run(CRASH_HEAD, CRASH_MSG, "PASS", marker=False), 1,
         r"^\[known-reds\] FAIL: 1 novel panic"),
        # The marker holds until its run's Summary.
        ("worker-panic-after-a-serial-run", GREEN_SERIAL_RUN + no_capture_run(CRASH_HEAD, CRASH_MSG, "PASS", marker=False), 1,
         r"^\[known-reds\] FAIL: 1 novel panic"),
        # A panic a passing test catches, as the latency gate's serial run prints it.
        ("caught-panic-in-a-passing-test", no_capture_run(CAUGHT_HEAD, CAUGHT_MSG, "PASS"), 0, NOTE_ONE),
        ("caught-panic-in-a-failing-test", no_capture_run(CAUGHT_HEAD, CAUGHT_MSG, "FAIL"), 1,
         r"^\[known-reds\] FAIL: 1 novel panic"),
        # A test that fails and passes in a later run of the same log is still red.
        ("failed-test-that-passes-in-a-later-run", r'''    Starting 2 tests across 1 binary
        FAIL [   0.010s] (1/2) holon-x::main a::case_b
  stderr ───

    running 1 test
    thread 'a::case_b' (7) panicked at crates/x/tests/y.rs:1:1:
    novel defect zzz
        FAIL [   0.010s] (2/2) holon-x::main c::case_d
  stderr ───

    running 1 test
    thread 'c::case_d' (8) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
    reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  field deltas (1):\n    block:a: content: sut=\"x\" ref=\"y\"\n")]
     Summary [   1.000s] 2 tests run: 0 passed, 2 failed, 0 skipped
        FAIL [   0.010s] (1/2) holon-x::main a::case_b
        FAIL [   0.010s] (2/2) holon-x::main c::case_d
error: test run failed
    Starting 1 tests across 1 binary
        PASS [   0.010s] (1/1) holon-x::main a::case_b
     Summary [   1.000s] 1 tests run: 1 passed, 0 failed, 0 skipped
''', 1,
         r"^\[known-reds\] FAIL: 1 novel panic"),
        # A serial run cut off after a panic is red, though a later run of the same log passes the test.
        ("serial-panic-cut-off-then-passing-in-a-later-run", "\n".join([
            SERIAL_MARKER, "    Starting 2 tests across 1 binary",
            status_line("START", 1, 2, "holon-x::main a::case_b"), "", "running 1 test",
            "thread 'a::case_b' (7) panicked at crates/x/tests/y.rs:1:1:", "novel defect zzz",
        ]) + "\n" + no_capture_run(CAUGHT_HEAD, CAUGHT_MSG, "PASS").replace(
            "holon-filesystem::crash a_controller_dies", "holon-x::main a::case_b"), 1,
         r"^\[known-reds\] FAIL: 1 novel panic"),
        # The marker of a run cut off before its Summary does not hold for the next run.
        ("unmarked-worker-panic-after-a-cut-serial-run", "\n".join([
            SERIAL_MARKER, "    Starting 1 test across 1 binary",
            status_line("START", 1, 1, "holon-x::main a::case_b"), "", "running 1 test",
            "thread 'a::case_b' (7) panicked at crates/x/tests/y.rs:1:1:", "novel defect zzz",
        ]) + "\n" + no_capture_run(CRASH_HEAD, CRASH_MSG, "PASS", marker=False), 1,
         r"^\[known-reds\] FAIL: 2 novel panic"),
        # A test failing without a panic is red, though it panics while passing in a later run.
        ("failed-without-panic-then-passing-with-one-in-a-later-run", "\n".join([
            "    Starting 1 test across 1 binary", status_line("FL+LK", 1, 1, "holon-x::main a::case_b"),
            "     Summary [   1.000s] 1 test run: 0 passed, 1 failed, 0 skipped", status_line("FL+LK", 1, 1, "holon-x::main a::case_b"),
            "error: test run failed",
            SERIAL_MARKER, "    Starting 1 test across 1 binary",
            status_line("START", 1, 1, "holon-x::main a::case_b"), "", "running 1 test",
            "thread 'a::case_b' (7) panicked at crates/x/tests/y.rs:1:1:", "novel defect zzz",
            "test a::case_b ... ok", status_line("PASS", 1, 1, "holon-x::main a::case_b"),
            "     Summary [   1.000s] 1 test run: 1 passed, 0 skipped",
        ]) + "\n", 1, r"^ *1 FAIL [+] LEAK: holon-x::main a::case_b$"),
    ]


def main(argv):
    assert len(argv) == 3 and argv[1] == "write-examples", "usage: known_reds_logs.py write-examples <dir>"
    out = Path(argv[2])
    for name, text, want_exit, line in examples():
        (out / f"{name}.log").write_text(text)
        print(f"{name}\t{want_exit}\t{line}")


if __name__ == "__main__":
    main(sys.argv)
