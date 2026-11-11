#!/usr/bin/env python3
"""Check that the test logs a lane summary cites ran exactly the test steps its diff selects.

    scripts/flow-checks/tests-cover-diff.py --base <commit> --summary <path> [--tree <commit>]

Run from the lane root. It runs `scripts/select-tests.py --base <commit>` and reads its `<step>: <args>` lines.
The cited logs are the `target/...` `.log` and `.txt` paths between the summary's `How tested` and `Confidence` lines;
a brace list (`target/x/{a,b}.log`) cites each of its paths, as the shell expands it.
Of a cited log it reads line 1 and the last line: a log written by scripts/flow-checks/run-step.sh has the step line
as line 1 and `exit: <code>` as its last line, and the lines between are the step's own output. It reads a
classifier log's `TREE:` line, a red `windowed:` step log's gate lines and the unit logs they name, and, for an
`accepted-base-red:` line, the `TREE:` line and the nextest result lines between.
A step ran when a cited log has the selected line as line 1, and one of these holds:
  - its last line is `exit: 0`;
  - its last line is `exit: <non-zero>` AND the same log is cited together with a classifier log:
    `scripts/keystone-known-reds.sh <step log>` run on the checked tree, whose last line is `exit: 0` (every signature
    matched a known-red row, every keystone sample verdict=OK);
  - a `windowed:` step log whose last line is `exit: <non-zero>` and whose failing units are all known reds in their
    own unit logs: the gate's end line counts RED and HUNG units, as many as its `FAIL: RED|HUNG <unit> (log <path>)`
    lines and at least one; no unit is HUNG; every log of every RED unit (the two of its rerun line, else the one of
    its FAIL line) exists, has the sha256 its `unit-log: <unit> <path> sha256=<hex>` line records, and is cited
    together with a classifier log of it that ends `exit: 0` on the checked tree;
  - its last line is `exit: <non-zero>`, its classifier log is missing or red, AND the How-tested block has a line
    `accepted-base-red: <step> <base log> <acceptance>`: the base log is cited, has the selected line as line 1, and its
    `TREE:` commit has the source tree of `--base`; every test that fails in the step log fails in the base log too.
    Failing tests are the nextest result lines (`FAIL [ 1.2s] <binary> <test>`, any status but a pass). When either
    log has none, both compare at module level: the units of the windowed gate's `FAIL:` lines, or the
    module of each failing test; stdout notes each such compare. A lane log with neither line that `just hand-authored`
    wrote (a `] RED case "<name>"` or `HAND-AUTHORED RED:` line) compares its red case names and failing tests (the
    indented names of each libtest `failures:` list, and each intact `test <name> ... FAILED` result line), whatever
    shard ran them; it is refused when a red shard names no case, when the run is not shown complete (every process
    ended with libtest's exit 101 or 0 and no cargo `process didn't exit successfully ... (signal: N` line; a sharded
    run ends with a `HAND-AUTHORED RED:` line for every red shard head and `exit: 1`, a single process with
    `exit: 101`), or when it skipped a case (`] SKIPPED case "<name>" via HOLON_HAND_AUTHORED_SKIP`) the base log did
    not skip. A red log with none of these lines is refused, so a red of another step kind cannot be compared. A lane
    log must show the end of a complete run, else it is refused too, as its reds prove nothing about the tests it never
    ran: a log with nextest failing-test lines needs a `Summary [...] N tests run` line (`k/n tests run` with k < n is a
    cut run, in any log), and a log with windowed gate verdicts needs the gate's `windowed-gate: wall=` end line.
A step also ran by a ruled route: the How-tested block has a line `ruled-route: <step> <ruling> <evidence log>` and the
evidence log is cited and exists. The check does not read the ruling ledger; the verifier checks the ruling id.
An `<acceptance>` or `<ruling>` is a ruling id (a letter or digit, then letters, digits and `_./-`, with a digit) or an
https issue-comment URL. Each such line is the whole line, after an optional list marker and backticks.
The logs these lines name are evidence, not runs of their step.
Every other outcome of the classifier (exit 1 regression, exit 3 indeterminate) is a FAIL of tests-cover-diff that
names the step and the classifier exit.
The check never runs the classifier itself and never reads the registry: one copy of each rule (the registry and the
classifier own the matching).
A classifier log is a cited run-step log of the line `known-reds: scripts/keystone-known-reds.sh <log>`; it
classifies the log whose path resolves, from the lane root, to the same file. It counts only when its `TREE:` commit
has the source tree of the checked commit: `--tree`, else jj's `@`.
The selector's disclosure steps (NOT_RUN_STEPS of select-tests.py) need no log.
PASS (exit 0) when every selected step ran, every cited log exists, every cited log whose line 1 names a selected step
has the selected line as line 1, every backticked `<step>: <args>` span of the summary equals the selected line, and
the selector printed no step twice.
Every problem is one stderr line and the check prints them all; only a failing selector stops it early.
Lines compare with runs of whitespace collapsed. It never runs tests.
"""
import argparse
import hashlib
import importlib.util
import re
import shlex
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

SELECTOR = "scripts/select-tests.py"
STEP_LINE = re.compile(r"([a-z][a-z0-9-]*): \S")
BRACE = r"\{[\w./~-]*(?:,[\w./~-]*)+\}"
LOG_PATH = re.compile(rf"(?<![\w./~{{}}-])(?:[\w./~-]|{BRACE}){{0,256}}target/(?:[\w./-]|{BRACE}){{0,256}}\.(?:log|txt)(?!\w)")
# A path starts after one of these; any other character before it belongs to a prefix the check cannot resolve.
PATH_START = set(" \t`\"'([<=:,*|")
SPAN = re.compile(r"`([^`\n]+)`")
HEADING = r"^(?:#+\s*)?(?:\*\*)?"
EXIT_OK = "exit: 0"
EXIT_RED = re.compile(r"exit: [1-9][0-9]*")
CLASSIFIER = "known-reds: scripts/keystone-known-reds.sh"
ACCEPTED = "accepted-base-red"
ROUTE = "ruled-route"
# The whole line, after a list marker: prose that mentions the forms is no such line.
DIRECTIVE = re.compile(rf"\s*(?:[-*] +)?({ACCEPTED}|{ROUTE}): ([a-z][a-z0-9-]*) (\S+) (\S+)\s*")
TOKEN = re.compile(r"[A-Za-z0-9][A-Za-z0-9_./-]*(?<=[0-9])[A-Za-z0-9_./-]*"
                   r"|https://github\.com/[\w.-]+/[\w.-]+/issues/[0-9]+#issuecomment-[0-9]+")
# A gate job log prefixes each line with its UTC time.
RESULT = re.compile(r"(?:\d{4}-\d\d-\d\dT[\d:]+Z)?\s+([A-Z][A-Z0-9 -]*?) \[\s*[\d.]+s\] "
                    r"(?:\(\s*\d+/\d+\) )?(\S+) (\S+)\s*")
# shortcut: a retried test that later passes still counts as failing; read retries when a lane needs it.
PASSING = re.compile(r"(?:TRY \d+ )?(?:PASS|LEAK|START)")
# The lines of scripts/windowed-gate.py `main` that name a unit's verdict and logs: the FAIL line of each RED or HUNG
# unit, the rerun line of each re-run unit, the `unit-log:` line of each log of a RED or HUNG unit, and the end line.
# A unit is a binary id, or a binary id and a module of the `main` binary.
# shortcut: the windowed gate names its reds per unit only; compare test names once it prints them.
GATE_FAIL = re.compile(r"\s*FAIL: (RED|HUNG) (.+?) \(log ([^)]+)\)(?: .*)?")
GATE_RERUN = re.compile(r"[A-Z-]+ +(.+?)  rerun wall=\S+  logs=(\S+) (\S+)\s*")
GATE_UNIT_LOG = re.compile(r"\s*unit-log: (.+) (\S+) sha256=([0-9a-f]{64})\s*")
GATE_COUNTS = re.compile(r"windowed-gate: wall=.* RED=(\d+) HUNG=(\d+)\s*")
WINDOWED = "windowed"
# The grammar of the hand-authored recipe's `sed` that lists a shard's red cases (justfile).
HAND_CASE = re.compile(r'\] RED case "([^"]*)"')
HAND_SKIP = re.compile(r'\] SKIPPED case "([^"]*)" via HOLON_HAND_AUTHORED_SKIP')
HAND_SHARD = re.compile(r"HAND-AUTHORED RED: shard (\d+) exit (\d+), red cases: (.*), log: ")
HAND_SHARD_HEAD = re.compile(r"== hand-authored shard (\d+) \(exit (\d+)\): ")
# libtest's exit when tests failed; any other exit means the process died and never ran its remaining tests.
LIBTEST_RED_EXIT = "101"
# The recipe ends a red sharded run with its `HAND-AUTHORED RED:` block and `exit 1`; a single process ends with
# cargo's exit.
SHARDED_RED_EXIT = "exit: 1"
CARGO_SIGNAL = re.compile(r"process didn't exit successfully.*\(signal: \d+")
HAND_UNNAMED = "none, see the log"
# Other threads' output under --nocapture can surround a libtest result line or split FAILED off it; the
# `failures:` list that ends the run names each failing test on a line of its own.
LIBTEST_LISTED = re.compile(r" {4}(\S+)")
LIBTEST_FAILED = re.compile(r"test (\S+)(?: - should panic)? \.\.\. FAILED")
# nextest's end of run: `k/n tests run` with k < n is a run cut short; `[fail-fast] stopped at` of
# scripts/lane-nextest.py also follows a full run.
NEXTEST_SUMMARY = re.compile(r"Summary \[[^]]*\] (?:(\d+)/)?(\d+) tests? run: ")
# The windowed gate prints this line once every unit has its verdict.
GATE_END = "windowed-gate: wall="


def not_run_steps():
    selector = Path(__file__).resolve().parents[1] / "select-tests.py"
    spec = importlib.util.spec_from_file_location("select_tests", selector)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return set(module.NOT_RUN_STEPS)


def norm(line):
    return " ".join(line.split())


def step_lines(text):
    """{step: {line}} of the lines of `text` that start with `<step>: `."""
    found = defaultdict(set)
    for line in text.splitlines():
        if m := STEP_LINE.match(line):
            found[m.group(1)].add(norm(line))
    return found


def expand(path):
    if not (m := re.search(BRACE, path)):
        return [path]
    return [p for alt in m.group(0)[1:-1].split(",") for p in expand(path[:m.start()] + alt + path[m.end():])]


def cited_logs(summary_text):
    """The cited paths, and one problem per citation the check cannot resolve."""
    paths, bad = set(), []
    block = how_tested(summary_text)
    if block is None:
        return [], ["the summary has no `How tested` line"]
    for line in block:
        for m in LOG_PATH.finditer(line):
            if m.start() and line[m.start() - 1] not in PATH_START:
                token = re.split(r"[\s`]", line[:m.start()])[-1] + m.group(0)
                bad.append(f"the summary cites `{token}`; cite the log by its relative, absolute or ~ path")
                continue
            paths.update(expand(m.group(0)))
    return sorted(paths), bad


def how_tested(summary_text):
    """The lines from the `How tested` line up to the `Confidence` line; None when there is no `How tested` line."""
    lines = summary_text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(rf"{HEADING}How tested", line)), None)
    if start is None:
        return None
    end = next((i for i in range(start, len(lines)) if re.match(rf"{HEADING}Confidence", lines[i])), len(lines))
    return lines[start:end]


def same_file(a, b):
    return Path(a).expanduser().resolve() == Path(b).expanduser().resolve()


def classified(lines):
    """The args of a classifier log's line 1, or None when line 1 is no classifier line."""
    if not norm(lines[0]).startswith(CLASSIFIER + " "):
        return None
    return shlex.split(lines[0])


def failing_tests(lines):
    """`<binary> <test>` of every nextest result line that is no pass."""
    return {f"{m.group(2)} {m.group(3)}" for line in lines
            if (m := RESULT.fullmatch(line)) and not PASSING.fullmatch(m.group(1))}


def gate_failures(lines):
    """(verdict, unit, final log) of every failing verdict line (RED, HUNG) of the windowed gate."""
    return [m.groups() for line in lines if (m := GATE_FAIL.fullmatch(line))]


def gate_red_modules(lines):
    return {unit for _, unit, _ in gate_failures(lines)}


def gate_reruns(lines):
    """{unit: (first log, rerun log)} of the windowed gate's rerun lines."""
    return {m.group(1): (m.group(2), m.group(3)) for line in lines if (m := GATE_RERUN.fullmatch(line))}


def gate_counts(lines):
    """(RED, HUNG) of the windowed gate's end line, or None when the log has none."""
    return next(((int(m.group(1)), int(m.group(2))) for line in lines if (m := GATE_COUNTS.search(line))), None)


def unit_problems(step, path, lines, classifiers, split, on_tree):
    """Why the windowed gate log does not show every failing unit a known red in its own unit logs; [] when it does.
    Every log of every RED unit (its first run and its rerun) must still hold what the gate hashed into its
    `unit-log:` line, and each classifier log of it must end `exit: 0` on the checked tree."""
    head = f"{path} of step `{step}`"
    if (counts := gate_counts(lines)) is None:
        return [f"{head}: no `{GATE_END}...` end line, so its units have no verdict"]
    fails = gate_failures(lines)
    if not fails or len(fails) != sum(counts):
        return [f"{head}: {len(fails)} FAIL lines but the end line says RED={counts[0]} HUNG={counts[1]}"]
    reruns = gate_reruns(lines)
    hashes = [(m.group(2), m.group(3)) for line in lines if (m := GATE_UNIT_LOG.fullmatch(line))]
    bad = []
    for verdict, unit, final in sorted(fails):
        if verdict == "HUNG":
            bad.append(f"{head}: unit {unit} is HUNG; a hang has no classifier verdict")
            continue
        logs = list(reruns.get(unit, ()))
        logs += [] if any(same_file(final, log) for log in logs) else [final]
        for log in logs:
            f = Path(log).expanduser()
            if not f.is_file():
                bad.append(f"{head}: unit log {log} does not exist")
                continue
            now = hashlib.sha256(f.read_bytes()).hexdigest()
            recorded = next((h for p, h in hashes if same_file(p, log)), None)
            if recorded is None:
                bad.append(f"unit log {log} has no unit-log line in {path}")
            elif recorded != now:
                bad.append(f"unit log {log} changed after the gate ran (sha256 {now} != {recorded})")
            judges = [c for c, target in sorted(classifiers.items()) if same_file(target, log)]
            if not judges:
                bad.append(f"{head}: unit {unit} log {log} has no cited classifier log; cite a run-step log of "
                           f"`{CLASSIFIER} {log}`")
            bad += [problem for c in judges for problem in judged(c, split[c], f"{log} of unit {unit}", on_tree)]
    return bad


def judged(classifier, lines, subject, on_tree):
    """Why the classifier log does not show `subject` all known reds on the checked tree; [] when it does."""
    if lines[-1] != EXIT_OK:
        return [f"{classifier} classifies {subject}; its last line is `{lines[-1]}`, not `{EXIT_OK}`"]
    if not on_tree(lines):
        return [f"{classifier} classifies {subject} on TREE {tree_line(lines)}, whose source tree is not the checked "
                f"tree"]
    return []


def libtest_failures(lines):
    """The test names of every libtest `failures:` list (the indented lines right after a `failures:` line) and of
    every intact result line, which a shard that died before its list leaves."""
    found = {m.group(1) for line in lines if (m := LIBTEST_FAILED.search(line))}
    listing = False
    for line in lines:
        m = LIBTEST_LISTED.fullmatch(line)
        if listing and m:
            found.add(m.group(1))
        else:
            listing = line == "failures:"
    return found


def incomplete(lines, heads, shards):
    """Why a red `just hand-authored` log does not show a run that ended normally; [] when it does. Complete means
    every process ended with libtest's exit and, when sharded, the recipe reached its end: its `HAND-AUTHORED RED:`
    block names every red shard head and the step ends `exit: 1`."""
    why = [line.strip() for line in lines if CARGO_SIGNAL.search(line)]
    why += [f"shard {k} exit {rc}" for k, rc in sorted({**heads, **{k: rc for k, rc, _, _ in shards}}.items())
            if rc not in ("0", LIBTEST_RED_EXIT)]
    if not heads:
        return why + ([f"step `{lines[-1]}`"] if lines[-1] != f"exit: {LIBTEST_RED_EXIT}" else [])
    if lines[-1] != SHARDED_RED_EXIT:
        why.append(f"step `{lines[-1]}`")
    blocked = {k for k, _, _, _ in shards}
    if missing := sorted(k for k, rc in heads.items() if rc != "0" and k not in blocked):
        why.append(f"no `HAND-AUTHORED RED:` line for red shard {', '.join(missing)}")
    return why


def hand_authored(lines):
    """The red case names and failing libtest tests, the red shard lines that name no case, why the run is not
    complete, and the skipped cases of a `just hand-authored` log; None for the red set when the log has no RED case
    and no HAND-AUTHORED RED line."""
    cases = {m.group(1) for line in lines if (m := HAND_CASE.search(line))}
    shards = [(m.group(1), m.group(2), m.group(3), line.strip()) for line in lines if (m := HAND_SHARD.search(line))]
    heads = {m.group(1): m.group(2) for line in lines if (m := HAND_SHARD_HEAD.search(line))}
    red = cases | libtest_failures(lines)
    return (red if cases or shards else None, [line for _, _, names, line in shards if names == HAND_UNNAMED],
            incomplete(lines, heads, shards), {m.group(1) for line in lines if (m := HAND_SKIP.search(line))})


def hand_problems(step, lane_path, lane_red, unnamed, cut, skipped, base_path, base_lines):
    """Why the hand-authored `base_lines` do not prove every red of the lane red on the base; [] when they do."""
    base_red, _, _, base_skipped = hand_authored(base_lines)
    bad = [f"{lane_path} of step `{step}`: a red shard names no red case ({'; '.join(unnamed)}), so {ACCEPTED} "
           f"cannot compare it with {base_path}"] if unnamed else []
    if cut:
        bad.append(f"{lane_path} of step `{step}`: the run is incomplete ({'; '.join(cut)}), so its reds prove "
                   f"nothing about the tests it never ran; {ACCEPTED} needs a complete run")
    if only := sorted(skipped - base_skipped):
        bad.append(f"{lane_path} of step `{step}`: {', '.join(only)} skipped only in the lane, not in base log "
                   f"{base_path}")
    if only := sorted(lane_red - (base_red or set())):
        bad.append(f"{lane_path} of step `{step}`: {', '.join(only)} red only in the lane, not in base log {base_path}")
    return bad


def stop_line(lines):
    """Why a nextest or windowed gate log does not show the end of a complete run, or None when it does: a nextest
    log needs a `Summary [...] N tests run` line, not `k/n` with k < n; a gate log needs its end line."""
    ends = [(line.strip(), m) for line in lines if (m := NEXTEST_SUMMARY.search(line))]
    if cut := next((line for line, m in ends if m.group(1) and int(m.group(1)) < int(m.group(2))), None):
        return f"`{cut}`"
    if failing_tests(lines) and not ends:
        return "no nextest `Summary [...] N tests run` line"
    if gate_red_modules(lines) and not any(GATE_END in line for line in lines):
        return f"no `{GATE_END}...` end line"
    return None


def module(name):
    binary, test = name.split(" ")
    return f"{binary} {test.split('::')[0]}"


def red_names(lane_lines, base_lines):
    """The lane's and the base's red names at the finest level both logs carry, and whether that is the module."""
    lane, base = failing_tests(lane_lines), failing_tests(base_lines)
    if lane and base:
        return lane, base, False
    lane_modules = {module(t) for t in lane} or gate_red_modules(lane_lines)
    return lane_modules, {module(t) for t in base} or gate_red_modules(base_lines), True


def tree_line(lines):
    return next((line.split()[1] for line in lines[1:] if line.startswith("TREE: ") and line.split()[1:]), None)


def git_tree(commit):
    """The source tree id of `commit`, or None when git cannot resolve it."""
    done = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"{commit}^{{tree}}"],
                          capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


def directives(summary_text):
    """(kind, step, log path, ruling or acceptance) of each `accepted-base-red:` / `ruled-route:` line of the
    How-tested block."""
    found = [m.groups() for line in how_tested(summary_text) or []
             if (m := DIRECTIVE.fullmatch(line.replace("`", "")))]
    return [(kind, step, a, b) if kind == ACCEPTED else (kind, step, b, a) for kind, step, a, b in found]


def cited(path, logs):
    return next((p for p in logs if same_file(p, path)), None)


def base_red_problems(step, lane_path, lane_lines, base_path, logs, selected_line, base_tree, tree_of, notes):
    """Why `base_path` does not prove every red of `lane_path` red on the base; [] when it does."""
    if not (key := cited(base_path, logs)):
        return [f"{ACCEPTED} of step `{step}` names base log {base_path}, which is no cited existing log"]
    base = logs[key].splitlines() or [""]
    if norm(base[0]) != selected_line:
        return [f"{ACCEPTED} base log {base_path} runs `{base[0]}`, the selector prints `{selected_line}`"]
    tree = tree_line(base)
    if tree is None:
        return [f"{ACCEPTED} base log {base_path} has no `TREE:` line"]
    if base_tree is None or tree_of(tree) != base_tree:
        return [f"{ACCEPTED} base log {base_path} ran on {tree}, whose tree is not the tree of --base"]
    if stop := stop_line(lane_lines):
        return [f"{lane_path} of step `{step}` stopped before all its tests ran ({stop}), so its reds prove "
                f"nothing about the tests it never ran; {ACCEPTED} needs a full run"]
    lane_red, base_red, by_module = red_names(lane_lines, base)
    hand_red, unnamed, cut, skipped = hand_authored(lane_lines)
    if not lane_red and hand_red is not None:
        return hand_problems(step, lane_path, hand_red, unnamed, cut, skipped, base_path, base)
    if not lane_red:
        return [f"{lane_path} of step `{step}` has no nextest failing-test line, no windowed gate `FAIL:` verdict "
                f"line and no hand-authored `RED case` line, so {ACCEPTED} cannot compare it with {base_path}"]
    if by_module:
        notes.append(f"{lane_path} of step `{step}` and base log {base_path} compared at module level")
    if only := sorted(lane_red - base_red):
        return [f"{lane_path} of step `{step}`: {', '.join(only)} red only in the lane, not in base log {base_path}"]
    return []


def check(selected_text, summary_text, logs, not_run, base=None, tree_of=git_tree, notes=None, head=None):
    """`logs` maps each cited existing path to its text; `not_run` names the steps no log runs; `tree_of` maps a
    commit to its source tree id; `notes` collects the module-level compares; `head` is the source tree id of the
    checked tree, which every classifier log must have run on. Returns the problems."""
    notes = [] if notes is None else notes

    def on_tree(lines):
        return head is not None and (tree := tree_line(lines)) is not None and tree_of(tree) == head

    counts = Counter(m.group(1) for line in selected_text.splitlines() if (m := STEP_LINE.match(line)))
    bad = [f"the selector prints step `{step}` {n} times" for step, n in sorted(counts.items()) if n > 1]
    selected = step_lines(selected_text)
    ran = set()
    split = {path: text.splitlines() or [""] for path, text in logs.items()}
    classifiers = {}
    for path, lines in split.items():
        if (args := classified(lines)) is None:
            continue
        if len(args) != 3:
            bad.append(f"{path} has line 1 `{lines[0]}`; a classifier line names exactly one step log")
            continue
        classifiers[path] = args[2]
    found = directives(summary_text)
    evidence = [path for _, _, path, _ in found]
    accepts = defaultdict(list)
    for kind, step, path, token in found:
        if not TOKEN.fullmatch(token):
            bad.append(f"{kind} of step `{step}` names `{token}`, which is no ruling id or issue-comment URL")
        elif step not in selected:
            bad.append(f"{kind} names step `{step}`, which the selector does not print")
        elif kind == ACCEPTED:
            accepts[step].append(path)
        elif cited(path, logs):
            ran.add(step)
        else:
            bad.append(f"{ROUTE} of step `{step}` names evidence {path}, which is no cited existing log")
    base_tree = base and tree_of(base)
    for path, lines in sorted(split.items()):
        first = (m := STEP_LINE.match(lines[0])) and m.group(1)
        if first not in selected or any(same_file(path, e) for e in evidence):
            continue
        if {norm(lines[0])} != selected[first]:
            bad.append(f"{path} runs `{lines[0]}`, the selector prints {sorted(selected[first])}")
            continue
        if lines[-1] == EXIT_OK:
            ran.add(first)
            continue
        if not EXIT_RED.fullmatch(lines[-1]):
            bad.append(f"{path} runs `{first}` but its last line is `{lines[-1]}`, not `exit: <code>`")
            continue
        judges = [c for c, target in sorted(classifiers.items()) if same_file(target, path)]
        verdicts = [problem for c in judges for problem in judged(c, split[c], f"{path} of step `{first}`", on_tree)]
        if judges and not verdicts:
            ran.add(first)
            continue
        line = next(iter(selected[first]))
        refusals = [base_red_problems(first, path, lines, b, logs, line, base_tree, tree_of, notes)
                    for b in accepts[first]]
        if [] in refusals:
            ran.add(first)
            continue
        units = unit_problems(first, path, lines, classifiers, split, on_tree) if first == WINDOWED else None
        if units == []:
            ran.add(first)
            continue
        if not judges and units is None:
            others = "".join(f"; {c} classifies {t}" for c, t in sorted(classifiers.items()))
            bad.append(f"{path} runs `{first}` but its last line is `{lines[-1]}`, not `{EXIT_OK}`, and no cited "
                       f"classifier log has line 1 `{CLASSIFIER} {path}`{others}")
        bad += verdicts + [problem for r in refusals for problem in r] + (units or [])
    bad += [f"no cited run-step log ran the selected step `{step}`" for step in sorted(set(selected) - not_run - ran)]
    for span in SPAN.findall(summary_text):
        if (m := STEP_LINE.match(span)) and m.group(1) in selected and {norm(span)} != selected[m.group(1)]:
            bad.append(f"the summary cites `{span}`, the selector prints {sorted(selected[m.group(1)])}")
    return bad


def jj_head():
    done = subprocess.run(["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"], capture_output=True, text=True)
    if done.returncode:
        sys.exit(f"tests-cover-diff: `jj log -r @` exited {done.returncode}: {done.stderr.strip()}")
    return done.stdout.strip()


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base", required=True)
    p.add_argument("--summary", required=True, type=Path)
    p.add_argument("--tree", help="the commit whose source tree every classifier log must have run on (default: "
                                  "jj's @)")
    a = p.parse_args()
    summary_text = a.summary.read_text()
    done = subprocess.run([sys.executable, SELECTOR, "--base", a.base], capture_output=True, text=True)
    if done.returncode:
        tail = " | ".join(done.stderr.strip().splitlines()[-3:])
        sys.exit(f"tests-cover-diff: {SELECTOR} --base {a.base} exited {done.returncode}: {tail}")
    paths, bad = cited_logs(summary_text)
    logs = {}
    for path in paths:
        f = Path(path).expanduser()
        if f.is_file():
            logs[path] = f.read_text(errors="replace")
        else:
            bad.append(f"cited log {path} does not exist")
    commit = a.tree or jj_head()
    if (head := git_tree(commit)) is None:
        sys.exit(f"tests-cover-diff: git cannot resolve the source tree of the checked commit {commit}")
    notes = []
    bad += check(done.stdout, summary_text, logs, not_run_steps(), a.base, notes=notes, head=head)
    for note in notes:
        print(f"tests-cover-diff: note: {note}")
    for problem in bad:
        print(f"tests-cover-diff: {problem}", file=sys.stderr)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
