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
between lines (some start or end with a verdict word, some are another run's
`test result:` line), a libtest panic head split before or after the thread
name or after its `' (`, text glued before or behind the name, the name on a
line of its own between `thread '` and `panicked at`, a panic location cut
before its `<line>:<col>:`, a libtest verdict pushed onto a later line or glued
to another thread's word, and another thread's text in front of a verdict word
on its line. libtest's main thread writes the `failures:` list and the
`test result:` line after every test has ended, so nothing interleaves there.
While it prints, it records for each panic the owner, thread name, location
and message the text still states, and whether the panic's own test passed;
`verdict` reads only that record.

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
    "Applying transition 1486:5:",
]
# Foreign lines that never end the way a cut location ends.
BETWEEN = [line for line in FOREIGN if not re.search(r"[0-9]:$", line)]
# Text another thread prints right after `thread '` or right before the name.
SPLIT_AFTER_QUOTE = ["", "()"]
# The classifier strips `"`, `(`, `[` and `=` glued before a closed name, and keeps `x`.
SPLIT_BEFORE_NAME = ["", "Applying transition 1/4: ", ": ", '"', 'case "', "(", "[", "=", "x"]
# Text glued before an open head's name; a trailing name character hides the name.
OPEN_LEAD = ["", "()", ": ()", "Applying transition 1"]
# A verdict word another thread glues behind an open head's name.
GLUED_WORD = ["", "ok", "FAILED"]
# Text before a name that stands on a line of its own, `<text><name>' (<id>`.
NAME_LINE_LEAD = ["", "Applying transition 1/", ": ", "step"]
# Text another thread prints right before the rest of a cut location.
LOC_LEAD = ["", "()", "Applying transition 1"]
NAME_CHAR = re.compile(r"[A-Za-z0-9_:-]")
# Text another thread prints between `... ` and the verdict word.
SPLIT_BEFORE_VERDICT = ["6: ", "Applying transition 5/6: ", "[inv-sql-budget] not judged "]
# Another thread's text that ends or starts with a verdict word.
VERDICT_ENDED = ["[sync] peer handshake: ok", "status=ok", "[inv-sql-budget] check FAILED", "health FAILED", "health: ignored"]
VERDICT_LED = ["okay, retrying the handshake", "ok: ()", "ok", "FAILED to bind 127.0.0.1:0, retrying", "FAILED", "ignored"]
# A child run's result line that a test prints.
CHILD_RESULT = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"
FOREIGN_RESULT = [CHILD_RESULT, "[child] " + CHILD_RESULT]
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
    # Indexes into `panics` whose stated thread name is their own test's, in a
    # libtest run that completed with that test passing.
    by_name: set = field(default_factory=set)
    # (("inst", run, unit), label) of each nextest status line.
    statuses: list = field(default_factory=list)
    # Indexes into `panics` whose owner the text proves (a captured block, a
    # serial run's START to status) and whose status in that run passes.
    in_passing: set = field(default_factory=set)
    failed: bool = False
    # Panics whose stated message is not their own, as drawn.
    restated: list = field(default_factory=list)
    # How often each split of DRAWN was drawn.
    drawn: Counter = field(default_factory=Counter)

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
    dropped: set = field(default_factory=set)  # the tests the NOTE line names
    sites: Counter = field(default_factory=Counter)  # the location of each printed signature


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

    A run after a run of the same runner may rerun tests of it, as a rerun, a
    second recipe or a second shard in the same log does.
    """
    used = set()
    runs = []
    for _ in range(rng.choice([1, 2])):
        shape = rng.choice(["libtest", "captured", "serial", "parallel"])
        binary = f"holon-{word(rng, used)}::{word(rng, used)}"
        earlier = []
        again = runs and rng.random() < 0.5
        if again and runs[-1].shape == "libtest":
            shape = "libtest"
        # Same-runner runs of one binary: a rerun, or a shard of a sharded recipe.
        if again and (runs[-1].shape == "libtest") == (shape == "libtest"):
            binary = runs[-1].binary
            earlier = [t.name for t in runs[-1].tests]
        tests = []
        for _ in range(rng.randint(1, 5)):
            again = [n for n in earlier if n not in {t.name for t in tests}]
            tests.append(draw_test(rng, used, shape, rng.choice(again) if again and rng.random() < 0.6 else None))
        # A name that is another's with a verdict word glued behind it.
        if shape == "libtest" and len(tests) > 1 and rng.random() < 0.5:
            tests[1] = draw_test(rng, used, shape, tests[0].name + rng.choice(GLUED_WORD[1:]))
            for t in tests[:2]:
                t.panics.append(draw_panic(rng, used, t.name))
        runs.append(Run(shape, binary, tests))
    # Only a run with a later one may be cut: a log that ends cut off states no
    # outcome, which the classifier reports as no verdict (exit 3).
    if len(runs) == 2 and runs[0].shape == "serial" and runs[0].tests[-1].panics:
        runs[0].cut = rng.random() < 0.7
    return runs


def verdict_line(t, verdict):
    return f"test {t.name}{' - should panic' if t.should_panic else ''} ... {verdict}"


def spliced_verdict(rng, status, interleave, tails=("",)):
    """A verdict word as libtest writes it, with another thread's text either
    after it, or (with `interleave`) in front of it on the same line."""
    tail = rng.choice(tails)
    if interleave and not tail and rng.random() < 0.3:
        return rng.choice(SPLIT_BEFORE_VERDICT) + status
    return status + tail


def foreign(rng, out, indent=""):
    """Appends another thread's lines."""
    for _ in range(rng.choice([1, 1, 2])):
        line = rng.choice(FOREIGN + VERDICT_ENDED + VERDICT_LED + FOREIGN_RESULT)
        out.lines.append(indent + line if line else "")


def add_panic(out, scope, stated, p, owner, passed, as_stated=None):
    """Records panic `p` of test `owner` under the thread name, and as the panic `as_stated`, the text states."""
    as_stated = as_stated or p
    if as_stated.msg != p.msg:
        out.restated.append(p)
    if stated and stated == owner and passed:
        out.by_name.add(len(out.panics))
    out.panics.append((scope, stated, as_stated))


DRAWN = [
    "a location cut after its file and colon", "a location cut inside its line number", "a location cut after its line number",
    "a location cut before its closing colon", "a location cut right after its file", "a location cut with a verdict line before its rest",
    "a verdict word glued behind an open head's name", "a name on a line of its own", "two test names that end an open head",
    "punctuation glued before a closed name",
]


def closed_name(text):
    """The name the classifier reads from `<text>' (<id>) panicked at`."""
    return text.split(" ")[-1].lstrip('"([=')


def recovered(text, names, out):
    """The one name of `names` that ends `text` behind a character no name holds, optionally followed by a glued verdict word; "" when none or several."""
    found = {n for n in names for w in GLUED_WORD
             if text.endswith(n + w) and (len(text) == len(n + w) or not NAME_CHAR.match(text[-len(n + w) - 1]))}
    out.drawn["two test names that end an open head"] += len(found) > 1
    return found.pop() if len(found) == 1 else ""


def restated(p, site, msg):
    """Panic `p` as a text that states `site` and `msg` reads."""
    kind = "novel" if msg != p.msg or (p.kind == "injected" and site != p.site) else p.kind
    return Panic(kind, p.thread, site, msg)


def located(rng, out, cut, before, p):
    """The lines of panic `p` from its head text `before` through its message, and the panic they state.

    With `cut` another thread's text may cut the location before its last colon;
    the classifier completes it from the first later line that ends in the rest.
    A cut right after the file, or one whose rest follows a verdict line, it
    keeps as written, with the next line as message.
    """
    if not cut or rng.random() < 0.6:
        return [f"{before}panicked at {p.site}:", p.msg], p
    where = rng.choice(DRAWN[:6])
    out.drawn[where] += 1
    path, line, col = p.site.rsplit(":", 2)
    head = [f"{path}:", f"{path}:{line[:-1]}" if len(line) > 1 else f"{path}:", f"{path}:{line}:", p.site, path, f"{path}:"][DRAWN.index(where)]
    tail = rng.choice(LOC_LEAD) + f"{p.site}:"[len(head):]
    first = f"{before}panicked at {head}"
    if where == DRAWN[4]:
        return [first, tail, p.msg], restated(p, head, tail)
    if where == DRAWN[5]:
        bound = rng.choice(FOREIGN_RESULT)
        return [first, bound, tail, p.msg], restated(p, path, bound)
    between = [rng.choice(BETWEEN) for _ in range(rng.choice([0, 0, 1, 2]))]
    return [first, *between, tail, p.msg], restated(p, (head + re.search(r"[0-9:]*$", tail).group())[:-1], p.msg)


def result_lines(passed, failing, indent=""):
    """libtest's closing lines: the `failures:` list and the `test result:` line."""
    lines = ["failures:", *(f"    {n}" for n in failing), ""] if failing else []
    lines.append(f"test result: {'FAILED' if failing else 'ok'}. {passed} passed; {len(failing)} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s")
    return [indent + line if line else "" for line in lines]


def render_libtest(run, index, rng, interleave, out):
    scope = ("section", index)
    names = [t.name for t in run.tests]
    out.lines += [f"     Running tests/main.rs (target/debug/deps/{run.binary.replace('::', '-')})", "", f"running {len(run.tests)} tests"]
    queues = []
    for t in run.tests:
        chunks = []
        for p in t.panics:
            split = rng.choice(["whole", "after", "before", "paren", "chain"]) if interleave else "whole"
            if split == "whole":
                chunks.append(("whole", p, t))
            elif split == "after":
                chunks += [("head-after", p, rng.choice(OPEN_LEAD), rng.choice(GLUED_WORD)), ("cont-after", p, t)]
            elif split == "before":
                chunks += [("head-before", rng.choice(SPLIT_AFTER_QUOTE)), ("cont-before", p, t, rng.choice(SPLIT_BEFORE_NAME))]
            elif split == "paren":
                chunks += [("head-paren", p, rng.choice(FOREIGN)), ("cont-paren", p, t, rng.choice(SPLIT_BEFORE_NAME))]
            else:
                chunks += [("head-chain", rng.choice(SPLIT_AFTER_QUOTE)), ("cont-chain", p, t, rng.choice(NAME_LINE_LEAD))]
        if interleave and rng.random() < 0.3:
            chunks += [("verdict-head", t), ("verdict-tail", t)]
        else:
            chunks.append(("verdict", t))
        queues.append(chunks)
    open_heads = 0
    # The text of the head that opened while none was open.
    head_text = ""
    tid = 100
    while any(queues):
        chunk = rng.choice([q for q in queues if q]).pop(0)
        tid += 1
        if chunk[0] in ("head-after", "head-paren", "head-before", "head-chain"):
            if chunk[0] == "head-after":
                text = chunk[2] + chunk[1].thread + chunk[3]
                out.drawn["a verdict word glued behind an open head's name"] += bool(chunk[3])
            else:
                text = chunk[1].thread if chunk[0] == "head-paren" else chunk[1]
            out.lines.append(f"thread '{text}" + (f"' ({chunk[2]}" if chunk[0] == "head-paren" else ""))
            if not open_heads:
                head_text = text
            open_heads += 1
            continue
        if chunk[0] == "verdict-head":
            out.lines.append(rng.choice(["", "Applying transition 4/4: "]) + verdict_line(chunk[1], rng.choice(["", "()", *VERDICT_ENDED, *VERDICT_LED])))
        elif chunk[0] == "verdict-tail":
            # The real word on a later line: alone, before other text, glued
            # behind another thread's word, or behind a child run's result line.
            word = chunk[1].status
            out.lines += rng.choice([[word], [word + ": ()"], [f"step{rng.randint(1, 9)}{word}"], [rng.choice(FOREIGN_RESULT), word]])
        elif chunk[0] == "verdict":
            out.lines.append(verdict_line(chunk[1], spliced_verdict(rng, chunk[1].status, interleave)))
        else:
            p, t = chunk[1], chunk[2]
            if chunk[0] == "whole":
                before, stated = f"thread '{p.thread}' ({tid}) ", p.thread
            elif chunk[0] in ("cont-after", "cont-paren"):
                before = (chunk[3] if chunk[0] == "cont-paren" else "' (") + f"{tid}) "
                stated = recovered(head_text, names, out) if open_heads == 1 else ""
            elif chunk[0] == "cont-before":
                before, stated = chunk[3] + f"{p.thread}' ({tid}) ", closed_name(chunk[3] + p.thread)
                out.drawn["punctuation glued before a closed name"] += chunk[3] in ('"', 'case "', "(", "[", "=")
            else:
                out.drawn["a name on a line of its own"] += 1
                out.lines.append(f"{chunk[3]}{p.thread}' ({tid}" + rng.choice(["", ": ()"]))
                if interleave and rng.random() < 0.5:
                    foreign(rng, out)
                before = rng.choice(["", "Applying transition 1/4: ()"]) + ") "
                stated = recovered((chunk[3] + p.thread).split(" ")[-1], names, out) if open_heads == 1 else ""
            # A head continued while exactly one was open closes it.
            if chunk[0] != "whole" and open_heads == 1:
                open_heads = 0
            lines, as_stated = located(rng, out, interleave, before, p)
            out.lines += lines
            add_panic(out, scope, stated, p, t.name, t.status == "ok", as_stated)
        if interleave and rng.random() < 0.3:
            foreign(rng, out)
    failing = [t.name for t in run.tests if t.status == "FAILED"]
    out.failed |= bool(failing)
    out.lines += ["", *result_lines(len(run.tests) - len(failing), failing)]


def body(t, cut=False):
    """The libtest lines a nextest test prints: (line, or None for the lines of panic, panic or None, whether another thread may follow it)."""
    lines = [("", None, True), ("running 1 test", None, True)]
    lines += [(None, p, True) for p in t.panics]
    if cut:
        pass
    elif t.status in BODY_OK:
        lines.append((verdict_line(t, "ok"), None, False))
        lines += [(line, None, False) for line in ["", *result_lines(1, [])]]
    elif t.status in BODY_FAILED:
        lines.append((verdict_line(t, "FAILED"), None, False))
        lines += [(line, None, False) for line in ["", *result_lines(0, [t.name])]]
    return lines


def status_line(label, i, n, unit):
    return f"{label:>12} [   0.010s] ({i}/{n}) {unit}"


def render_body(t, scope, rng, interleave, out, indent="", cut=False, proven=False):
    """`proven`: the text proves `t` owns its panics, so each counts by `t`'s own status."""
    first = len(out.panics)
    for line, p, open_ in body(t, cut):
        # nextest captures a test's output whole: only --no-capture output interleaves.
        if p is not None:
            lines, as_stated = located(rng, out, interleave and not indent, f"thread '{p.thread}' (7) ", p)
            out.lines += [indent + line if line else "" for line in lines]
            add_panic(out, scope, p.thread, p, t.name, not cut and t.status in BODY_OK, as_stated)
        else:
            out.lines.append(indent + line if line else "")
        if interleave and not indent and open_ and rng.random() < 0.3:
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
    proves its owner and the owner passed in that run, or when the text states
    its thread name, that name is its own test's, and that test passed in a
    libtest run that completed. Every other panic is printed, except an
    injected one. A failing status is a signature
    unless its label is FAIL / FAIL + LEAK and its owner printed a panic or an
    injected one.
    """
    kept, dropped = [], set()
    for i, (scope, name, p) in enumerate(r.panics):
        if i in r.in_passing:
            dropped.add(scope[2])
        elif i in r.by_name:
            dropped.add(name)
        else:
            kept.append((scope, p))
    note = len(r.panics) - len(kept)
    listed = Counter(p.msg for _, p in kept if p.kind == "novel")
    matched = sum(p.kind == "known" for _, p in kept)
    sites = Counter(p.site for _, p in kept if p.kind != "injected")
    owners = {scope for scope, _ in kept}
    for inst, label in r.statuses:
        long = FAIL_LABELS.get(label)
        if long and not (long in ("FAIL", "FAIL + LEAK") and inst in owners) and f"{long}: {inst[2]}" not in listed:
            listed[f"{long}: {inst[2]}"] = 1
            sites["nextest"] += 1
    novel = sum(listed.values())
    if novel + matched == 0 and r.failed:
        novel = 1  # "run failed but no panic signature was extracted"
    return Verdict(1 if novel else 0, novel, matched, listed, note, dropped, sites)


NOTE = re.compile(r"^\[known-reds\] NOTE: (\d+) panic\(s\) inside passing tests \(ignored\): (.+)$", re.M)
ENTRY = re.compile(r"^(?:PRIMARY: |  )\[[^]]+\] \S+ @ (\S+?): ", re.M)
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
    return Verdict(0, int(novel or 0), int(matched or note or 0), listed,
                   int(notes[0][0]) if notes else 0, set(notes[0][1].split(", ")) if notes else set(), Counter(ENTRY.findall(stdout)))


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


def failures_list(names):
    return "\n".join(["", *result_lines(0, names)[:-1]]) + "\n"


def also_check_split(verdict):
    """A passing (or failing) should_panic test whose head splits after the name, beside a listed red."""
    text = W123.replace(ALSO_CHECK, "thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused\nApplying transition 4/4: ()\n' (1382932350) panicked at")
    text = text.replace(ALSO_CHECK_OK, ALSO_CHECK_OK.replace("... ok", f"... {verdict}"))
    failing = ["hand_authored_keystone_regressions"] + (["hand_authored_regressions::also_check_after_the_last_transition_is_refused"] if verdict == "FAILED" else [])
    return text.replace("test result: ok. 19 passed; 0 failed", KNOWN_FAILED + failures_list(failing) + f"test result: FAILED. {19 - len(failing)} passed; {len(failing)} failed")


HA = "crates/holon-integration-tests/src/pbt/hand_authored.rs"
SHARD_RUNNING = "     Running tests/main.rs (target/debug/build/holon-integration-tests/bf76ca5e48b900b1/out/integration-bf76ca5e48b900b1)"
# The `just hand-authored` step: libtest shards of one binary in one log. Shard 0
# runs the passing should_panic tests, one head spliced after its `' (`, beside
# a listed red; shard 1 passes the test that shard 0 fails.
HAND_AUTHORED_SHARDS = f"""== hand-authored shard 0 (exit 101): target/gate-logs/pbt-hand-authored-shard-0.log ==
{SHARD_RUNNING}

running 19 tests

thread 'hand_authored_regressions::invalid_pinned_wiring_is_loud' (1934971516) panicked at {HA}:61:13:
<inline>: initial_state carries an INVALID wiring manifest Wiring {{ storage_adapters: {{}}, sync_adapters: {{}}, actors: {{}}, editor_leg: Dispatch }}: wiring has no storage adapter
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'hand_authored_regressions::empty_locks_are_refused' (1934971514) panicked at {HA}:176:9:
hand-authored regression "<inline>":1 case "n": `locks` is empty; a case locks one invariant at least

thread 'hand_authored_regressions::mistyped_initial_state_key_is_loud' (test hand_authored_regressions::drawn_run_sweeps_after_boot_and_every_transition ... ok
1934971519
Applying transition 1test hand_authored_regressions::absent_initial_state_parses_as_none ... /4) panicked at {HA}:419:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state" — a mistyped key is SILENTLY DROPPED by serde
  line: {{"name": "n", "transitions": [], "initail_state": {{"wiring": {{"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}}}}}}
ok: ()

thread 'hand_authored_regressions::malformed_also_check_after_is_refused' (1934971518) panicked at {HA}:222:17:
hand-authored regression "<inline>":1 case "n": `also_check_after` holds "2", not a transition number


Applying transition 2/4: ()

Applying transition 3/4: ()
Applying transition 4/4: test hand_authored_regressions::recorded_env_flag_mismatch_is_reported ... ()
thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused' (1934971511) panicked at {HA}:226:13:
hand-authored regression "<inline>":1 case "n": `also_check_after` names transition 4, out of range 1..3 (1-based; the last transition 4 is always checked)
test hand_authored_regressions::replay_of_a_case_without_transitions_sweeps_once_after_boot ... 
thread 'hand_authored_regressions::nested_unknown_wiring_field_is_loud' (1934971520) panicked at {HA}:429:5:
assertion `left == right` failed: hand-authored regression "<inline>":1: `initial_state` does not round-trip
ok
test hand_authored_regressions::replay_also_sweeps_around_each_named_transition ... ok
test hand_authored_regressions::replay_sweeps_around_the_last_transition_only ... ok

thread 'hand_authored_regressions::a_case_without_locks_is_refused_by_name' (1934971508) panicked at {HA}:168:13:
hand-authored regression "<inline>":1 case "no-locks" has no `locks` field

thread 'hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused' (1934971509) panicked at {HA}:315:5:
locked invariant(s) ["inv-x"] were `Skipped` at every checkpoint: the case never reaches the state they judge, so it locks nothing

thread 'hand_authored_regressions::unknown_initial_state_field_is_loud' (1934971528) panicked at {HA}:411:9:
hand-authored regression "<inline>":1 failed to parse: unknown field `seed`, expected `wiring` at line 1 column 135
test hand_authored_regressions::empty_locks_are_refused - should panic ... ok
test hand_authored_regressions::a_case_without_locks_is_refused_by_name - should panic ... ok
test hand_authored_regressions::invalid_pinned_wiring_is_loud - should panic ... ok
test hand_authored_regressions::unknown_initial_state_field_is_loud - should panic ... ok
test hand_authored_regressions::judged_windows_start_swept_and_only_pre_sweeps_do_not ... ok
test hand_authored_regressions::nested_unknown_wiring_field_is_loud - should panic ... ok
test hand_authored_regressions::also_check_after_the_last_transition_is_refused - should panic ... ok
test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok
test hand_authored_regressions::malformed_also_check_after_is_refused - should panic ... ok
test hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused - should panic ... ok
test hand_authored_regressions::pinned_initial_state_round_trips_and_rebuilds_the_draw ... ok
thread 'hand_authored_regressions::hand_authored_keystone_regressions' (1934971515) panicked at {KNOWN_SITE}:
{KNOWN_MSG}
test hand_authored_regressions::hand_authored_keystone_regressions ... FAILED

failures:
    hand_authored_regressions::hand_authored_keystone_regressions

test result: FAILED. 18 passed; 1 failed; 0 ignored; 0 measured; 628 filtered out; finished in 218.20s

== hand-authored shard 1 (exit 0): target/gate-logs/pbt-hand-authored-shard-1.log ==
{SHARD_RUNNING}

running 1 test
test hand_authored_regressions::hand_authored_keystone_regressions ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 646 filtered out; finished in 214.25s
"""
# The card's evidence shard: one verdict behind another thread's text on its
# line (`... 6: ok`), then one pushed onto a later line.
SPLICED_VERDICTS = f"""{SHARD_RUNNING}

running 2 tests

thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused' (1934308419) panicked at {HA}:226:13:
hand-authored regression "<inline>":1 case "n": `also_check_after` names transition 4, out of range 1..3 (1-based; the last transition 4 is always checked)

thread 'hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused' (1934308417) panicked at {HA}:315:5:
locked invariant(s) ["inv-x"] were `Skipped` at every checkpoint: the case never reaches the state they judge, so it locks nothing

Applying transition 5/test hand_authored_regressions::also_check_after_the_last_transition_is_refused - should panic ... 6: ok
()
test hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused - should panic ... 2026-11-09T15:44:30.451795Z  WARN holon_profiles::type_registry: computed fields are outside the SQL-compilable subset
ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 628 filtered out; finished in 206.82s
"""
# A passing test that catches the refusal it provokes, beside a listed red.
CAUGHT_IN_LIBTEST = f"""     Running tests/main.rs (target/debug/deps/integration-0000000000000001)

running 3 tests

thread 'refusals::empty_locks_are_refused' (2) panicked at {HA}:176:9:
hand-authored regression "<inline>":1 case "n": `locks` is empty; a case locks one invariant at least
test refusals::empty_locks_are_refused ... ok
thread 'refusals::empty_locks_are_refused_twice' (3) panicked at crates/y/tests/b.rs:12:5:
REAL PRODUCT BUG: the second refusal was never raised
test refusals::empty_locks_are_refused_twice ... FAILED
{KNOWN_FAILED}{failures_list(["refusals::empty_locks_are_refused_twice", "hand_authored_keystone_regressions"])}test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"""

# A failing and a passing test whose verdict words other threads' text hides
# or imitates: a line that starts with a verdict word, a verdict glued behind
# another word, a child run's `test result:` line before the real verdict.
HIDDEN_VERDICTS = f"""     Running tests/main.rs (target/debug/deps/x-0123)

running 3 tests

thread 'refusals::real_bug' (99) panicked at crates/x/src/bug.rs:1:5:
REAL PRODUCT BUG
thread 'refusals::refuses_twice' (98) panicked at crates/x/src/refusal.rs:2:5:
refusal raised
test refusals::real_bug ... 6: FAILED
okay, retrying the handshake
test refusals::refuses_twice - should panic ... 
FAILED to bind 127.0.0.1:0, retrying
[sync] peer handshake: ok
{CHILD_RESULT}
step6ok
test refusals::plain_pass ... ok: ()

{failures_list(["refusals::real_bug"])}test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"""

# The `just hand-authored` gate log of a passing should_panic test whose head
# spans four lines: foreign text before the name and an `ok` glued behind it,
# the id, and a location cut after the file and continued behind another
# thread's `Applying transition 1`.
W183 = (SHARD_RUNNING + "\n\nrunning 19 tests\n" + '''Applying transition 2/4: ()

Applying transition 3/4: ()
test hand_authored_regressions::absent_initial_state_parses_as_none ... ok

Applying transition 4/
thread 'hand_authored_regressions::a_case_without_locks_is_refused_by_name' (61104203) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:195:13:
hand-authored regression "<inline>":1 case "no-locks" has no `locks` field: name the catalog invariant id(s) its bug breaks (docs/Testing/HandAuthoredRegressions.md)
4
thread 'test hand_authored_regressions::replay_of_a_case_without_transitions_sweeps_once_after_boot ... : ()hand_authored_regressions::nested_unknown_wiring_field_is_loudok
' (61104215
) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:
Applying transition 1486:5:
assertion `left == right` failed: hand-authored regression "<inline>":1: `initial_state` does not round-trip — the authored JSON and the parsed value differ, so replaying this case would start from a DIFFERENT state than the run it pins
  left: Object {"wiring": Object {"storage_adapters": Array [String("Turso")], "sync_adapters": Array [], "actors": Array [], "peers": Array [String("x")]}}
 right: Object {"wiring": Object {"storage_adapters": Array [String("Turso")], "sync_adapters": Array [], "actors": Array []}}
test hand_authored_regressions::replay_also_sweeps_around_each_named_transition ... ok
/4: ()

Applying transition 2/4: ()

Applying transition 3/4: ()

Applying transition 4/4: ()

thread 'hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused' (61104204) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:370:5:
locked invariant(s) ["inv-x"] were `Skipped` at every checkpoint: the case never reaches the state they judge, so it locks nothing (engaged: {"inv-no-errors"})

Applying transition 1/6: ()
test hand_authored_regressions::replay_sweeps_around_the_last_transition_only ... ok

thread 'hand_authored_regressions::unknown_initial_state_field_is_loud' (61104221) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:468:9:
hand-authored regression "<inline>":1 failed to parse: unknown field `seed`, expected `wiring` at line 1 column 135
  line: {"name": "n", "transitions": [], "initial_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}, "seed": 7}}
\x1b[2m2026-11-12T00:56:18.134235Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mholon_profiles::type_registry\x1b[0m\x1b[2m:\x1b[0m computed fields are outside the SQL-compilable subset and are served by Rhai alone; they cannot be planted as matview columns \x1b[3mentity\x1b[0m\x1b[2m=\x1b[0m"block" \x1b[3mfields\x1b[0m\x1b[2m=\x1b[0mhas_query_source, is_holon_source, is_page_row, is_program, is_rule_head, is_widget_only
[inv-sql-budget] UndoLastMutation: not judged — started from an unswept state

Applying transition 2/6: ()
[hand-authored regression] SKIPPED case "a-failed-create-cancels-its-keystrokes-with-one-disclosure" via HOLON_HAND_AUTHORED_SKIP (known issue — see docs/Testing/BugFunnel.md)
[hand-authored regression] 1 case(s) SKIPPED via HOLON_HAND_AUTHORED_SKIP — coverage is NOT full (see docs/Testing/BugFunnel.md)
[hand-authored regression] running case "create-block-smoke" (1 transitions, Checkpoints({1}), Locks({"inv-blocks-match-ref/block_raw", "inv-live-children-match-ref"}))

Applying transition 3/6: ()

Applying transition 4/6: ()

Applying transition 5/6: ()
test hand_authored_regressions::unknown_initial_state_field_is_loud - should panic ... ok
test hand_authored_regressions::malformed_also_check_after_is_refused - should panic ... ok
test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok
test hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused - should panic ... ok
[inv-sql-budget] UndoLastMutation: not judged — started from an unswept state

Applying transition 6/6: ()
test hand_authored_regressions::judged_windows_start_swept_and_only_pre_sweeps_do_not ... ok
test hand_authored_regressions::nested_unknown_wiring_field_is_loud - should panic ... ok
\x1b[2m2026-11-12T00:56:18.146239Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mholon_profiles::type_registry\x1b[0m\x1b[2m:\x1b[0m computed fields are outside the SQL-compilable subset and are served by Rhai alone; they cannot be planted as matview columns \x1b[3mentity\x1b[0m\x1b[2m=\x1b[0m"block" \x1b[3mfields\x1b[0m\x1b[2m=\x1b[0mhas_query_source, is_holon_source, is_page_row, is_program, is_rule_head, is_widget_only
test hand_authored_regressions::invalid_pinned_wiring_is_loud - should panic ... ok
test hand_authored_regressions::empty_locks_are_refused - should panic ... ok
test hand_authored_regressions::a_case_without_locks_is_refused_by_name - should panic ... ok
\x1b[2m2026-11-12T00:56:18.147601Z\x1b[0m \x1b[33m WARN\x1b[0m \x1b[2mholon_profiles::type_registry\x1b[0m\x1b[2m:\x1b[0m computed fields are outside the SQL-compilable subset and are served by Rhai alone; they cannot be planted as matview columns \x1b[3mentity\x1b[0m\x1b[2m=\x1b[0m"block" \x1b[3mfields\x1b[0m\x1b[2m=\x1b[0mhas_query_source, is_holon_source, is_page_row, is_program, is_rule_head, is_widget_only
'''
        + "\ntest result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 644 filtered out; finished in 142.89s\n")
W183_OK = "test hand_authored_regressions::nested_unknown_wiring_field_is_loud - should panic ... ok\n"
W183_MSG = r"^ *1 assertion .left == right. failed: hand-authored regression .*does not round-trip"
# A head whose name stands on a later line behind foreign text, with the id cut
# and `panicked at` on a third line.
W973 = (SHARD_RUNNING + "\n\nrunning 19 tests\n" + '''thread 'test hand_authored_regressions::recorded_env_flag_mismatch_is_reported ... ok

Applying transition 1/hand_authored_regressions::mistyped_initial_state_key_is_loud' (18737672264: ()
test hand_authored_regressions::replay_of_a_case_without_transitions_sweeps_once_after_boot ... 
Applying transition 1/4: ()) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:417:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state" — a mistyped key is SILENTLY DROPPED by serde, so this case would replay something other than what it names. Known keys: ["name", "description", "environment", "initial_state", "transitions", "also_check_after", "locks", "known_red"]
  line: {"name": "n", "transitions": [], "initail_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}}}
ok
test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok
'''
        + "\ntest result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 628 filtered out; finished in 218.20s\n")
W973_OK = "test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok\n"
W973_MSG = r"^ *1 hand-authored regression .*unknown top-level key .initail_state."
# A closed head behind a replay line's `case "`.
W169 = SHARD_RUNNING + """

running 2 tests
thread ' case(s) SKIPPED via HOLON_HAND_AUTHORED_SKIP — coverage is NOT full (see docs/Testing/BugFunnel.md)
case "hand_authored_regressions::mistyped_initial_state_key_is_loud' (1969029362) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:476:9:
hand-authored regression "<inline>":1: unknown top-level key "initail_state" — a mistyped key is SILENTLY DROPPED by serde
test hand_authored_regressions::recorded_env_flag_mismatch_is_reported ... ok
test hand_authored_regressions::mistyped_initial_state_key_is_loud - should panic ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 628 filtered out; finished in 218.20s
"""
# The ok line of a should_panic test glued behind another thread's `thread '`.
SPLIT_OK_LINE = """     Running tests/main.rs (target/debug/deps/x-0123)

running 2 tests

thread 'refusals::refuses' (98) panicked at crates/x.rs:1:1:
expected refusal
thread 'test refusals::refuses - should panic ... ok
test refusals::plain_pass ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"""


def failed_instead(text, ok_line, name, n):
    """`text` with the should_panic test `name` failing instead of passing, in a run of `n` tests."""
    return text.replace(ok_line, ok_line.replace("... ok", "... FAILED")).replace(
        f"test result: ok. {n} passed; 0 failed", failures_list([name]) + f"test result: FAILED. {n - 1} passed; 1 failed")


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
NOTE_ONE = r"^\[known-reds\] NOTE: 1 panic[(]s[)] inside passing tests [(]ignored[)]: "


def examples():
    """(name, log text, want exit, a regex one output line must match), in grep's and Python's common syntax."""
    gate = GATE
    nested, mistyped = "hand_authored_regressions::nested_unknown_wiring_field_is_loud", "hand_authored_regressions::mistyped_initial_state_key_is_loud"
    twin = W183.replace("running 19 tests", "running 20 tests").replace(W183_OK, W183_OK + W183_OK.replace("_loud - should panic", "_loudok")).replace("19 passed", "20 passed")
    return [
        # A passing should_panic test's head over four lines, its location cut after the file.
        ("w183-split-location", W183, 0, NOTE_ONE.replace("1 panic", "4 panic") + r".*" + nested),
        ("w183-split-location-failed", failed_instead(W183, W183_OK, nested, 19), 1, W183_MSG),
        ("w183-split-location-failed-site", failed_instead(W183, W183_OK, nested, 19), 1, r"^PRIMARY: [[]novel[]] \S+ @ " + HA + r":1486:5: assertion"),
        # Without its ok line nothing proves the test passed.
        ("w183-split-location-unpaired", W183.replace(W183_OK, ""), 1, W183_MSG),
        # The glued `ok` makes the head's text end in two test names.
        ("w183-split-location-two-candidates", twin, 1, W183_MSG),
        # A name behind a replay line's `case "`.
        ("w169-glued-quote-name", W169, 0, NOTE_ONE + mistyped + "$"),
        ("w169-glued-quote-name-failed", failed_instead(W169, W973_OK, mistyped, 2), 1, W973_MSG),
        # The name on a later line than `thread '`, `panicked at` on a third.
        ("w973-split-ok-header", W973, 0, NOTE_ONE + mistyped + "$"),
        ("w973-split-ok-header-failed", failed_instead(W973, W973_OK, mistyped, 19), 1, W973_MSG),
        ("w973-split-ok-header-two-open-heads", W973.replace("running 19 tests\n", "running 19 tests\nthread 'hand_authored_regressions::empty_locks_are_refused\n"), 1, W973_MSG),
        ("should-panic-split-ok-line", SPLIT_OK_LINE, 0, NOTE_ONE + "refusals::refuses$"),
        ("should-panic-split-ok-line-failed", failed_instead(SPLIT_OK_LINE, "test refusals::refuses - should panic ... ok\n", "refusals::refuses", 2), 1, r"^ *1 expected refusal$"),
        # The hand-authored step's shards: every should_panic test passes, one
        # head spliced after its `' (`; the listed red alone is left.
        ("hand-authored-shards", HAND_AUTHORED_SHARDS, 0, r"^\[known-reds\] PASS-WITH-NOTE: 1 known-red panic"),
        ("hand-authored-shards-spliced-head-named", HAND_AUTHORED_SHARDS, 0,
         r"^\[known-reds\] NOTE: 9 panic[(]s[)] inside passing tests [(]ignored[)]: .*hand_authored_regressions::mistyped_initial_state_key_is_loud"),
        # A caught panic of a passing test is dropped; a test whose name only
        # starts with that name keeps its own.
        ("caught-panic-in-a-passing-libtest-test", CAUGHT_IN_LIBTEST, 1, r"^\[known-reds\] FAIL: 1 novel panic[(]s[)], 1 known-red"),
        ("spliced-verdict-then-pushed-verdict", SPLICED_VERDICTS, 0,
         r"^\[known-reds\] NOTE: 2 panic[(]s[)] inside passing tests [(]ignored[)]: hand_authored_regressions::a_lock_skipped_at_every_checkpoint_is_refused, hand_authored_regressions::also_check_after_the_last_transition_is_refused$"),
        ("caught-panic-in-a-passing-libtest-test-named", CAUGHT_IN_LIBTEST, 1,
         r"^\[known-reds\] NOTE: 1 panic[(]s[)] inside passing tests [(]ignored[)]: refusals::empty_locks_are_refused$"),
        ("hidden-verdicts-failing-panic-stays", HIDDEN_VERDICTS, 1, r"^ *1 REAL PRODUCT BUG$"),
        ("hidden-verdicts-passing-panic-dropped", HIDDEN_VERDICTS, 1, r"^\[known-reds\] NOTE: 1 panic[(]s[)] inside passing tests [(]ignored[)]: refusals::refuses_twice$"),
        # Two binaries of one unmarked run share a test name: the failing one keeps its panic.
        ("same-name-in-two-binaries-of-one-run", "\n".join([
            "    Starting 2 tests across 2 binaries",
            status_line("START", 1, 2, "holon-a::main tests::refuses"), status_line("START", 2, 2, "holon-b::main tests::refuses"),
            "", "running 1 test", "test tests::refuses - should panic ... ok",
            "", "running 1 test", "thread 'tests::refuses' (2) panicked at crates/y/tests/b.rs:12:5:",
            "REAL PRODUCT BUG: the refusal was never raised", "test tests::refuses ... FAILED",
            status_line("PASS", 1, 2, "holon-a::main tests::refuses"), status_line("FAIL", 2, 2, "holon-b::main tests::refuses"),
            "     Summary [   1.000s] 2 tests run: 1 passed, 1 failed, 0 skipped",
            status_line("FAIL", 2, 2, "holon-b::main tests::refuses"), "error: test run failed",
        ]) + "\n", 1, r"^ *1 REAL PRODUCT BUG: the refusal was never raised$"),
        # A head split after the name, with one open head in its section.
        ("w123-split-after-name", W123, 0, r"^\[known-reds\] PASS: 1 green run"),
        # A second open head makes the split panic's name unknown: novel.
        ("w123-two-open-heads", W123.replace(SPLIT_HEAD, SPLIT_HEAD + "thread 'hand_authored_regressions::empty_locks_are_refused\n"), 1,
         r"^ *1 assertion .left == right. failed: hand-authored regression .*does not round-trip"),
        # Foreign text before the name on the head's own line, and an `ok` glued behind `thread '`.
        ("gate-overwritten-head", gate, 0, r"^\[known-reds\] PASS: 1 green run"),
        ("gate-overwritten-head-failed", gate.replace("mistyped_initial_state_key_is_loud - should panic ... ok", "mistyped_initial_state_key_is_loud - should panic ... FAILED")
         .replace("test result: ok. 19 passed; 0 failed", failures_list(["hand_authored_regressions::mistyped_initial_state_key_is_loud"]) + "test result: FAILED. 18 passed; 1 failed"), 1,
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
