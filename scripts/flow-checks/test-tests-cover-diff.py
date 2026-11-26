#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/tests-cover-diff.py.

Run: `python3 scripts/flow-checks/test-tests-cover-diff.py [cases]`. Exits non-zero on the first violation.
Each fixture is a lane root in a temp dir whose `scripts/select-tests.py` prints a fixed selection.
fixtures/run-step-clippy.txt is a log scripts/flow-checks/run-step.sh wrote for a real lane.
Every lane root sits in a colocated jj repo, so a log's `TREE:` commit resolves to a real source tree; the checked tree
is `--tree` of the repo's child commit, whose tree is the base's, and the summary's `Commit:` is that child.
fixtures/windowed-*.txt are red windowed gate step logs of real lanes (the ledger path's home folder shortened to ~).
"""
import contextlib
import functools
import hashlib
import importlib.util
import io
import itertools
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

CHECKER = Path(__file__).resolve().parent / "tests-cover-diff.py"
REPO = Path(__file__).resolve().parents[2]
REAL_LOG = Path(__file__).resolve().parent / "fixtures/run-step-clippy.txt"
# run() writes the repo's base commit for TREE_ID and its commit of another tree for OTHER_TREE_ID.
TREE_ID, OTHER_TREE_ID = "e5d2c227" + "a" * 32, "0bad" + "f" * 36
HEADER = f"BASE: 3936d83aba008d69cc09e73d1c5db883ed6b5ae4\nTREE: {TREE_ID}\nHOLON_PBT_TIMING=measure\n"
CLIPPY = "clippy: --workspace --features pbt"
NEXTEST = "nextest: --workspace --features pbt -E 'package(holon) | package(holon-api)' --profile load-tolerant"
NARROW = "nextest: --workspace --features pbt -E 'package(holon)' --profile load-tolerant"
WINDOWED = "windowed: just windowed-gate"
SELECTED = [CLIPPY, NEXTEST, WINDOWED]
TIMING = "timing: just timing-gate"
UNOWNED = "unowned: package(leaf) is excluded by the load-tolerant profile and has no recipe in scripts/timing-split.py"


def step_log(line, code=0):
    return f"{line}\n{HEADER}STARTED: 2026-10-31T17:00:00Z\nrunning\nSummary [ 1s]\n\nexit: {code}\n"


def classifier_log(step_log_path, code=0, tree=TREE_ID):
    return step_log(f"known-reds: scripts/keystone-known-reds.sh {step_log_path}", code).replace(TREE_ID, tree)


def run(selected, logs, spans=(), selector_exit=0, ran={}, cites=None):
    """Run the checker in a fresh lane root; `logs` maps a path under target/ to the step lines of a log shaped like
    the selector output, `ran` maps a path under target/ to the text of a run-step log.
    `cites` replaces the cited paths of the summary; `<root>` in it is the lane root."""
    with tempfile.TemporaryDirectory() as root:
        root = Path(root)
        base, child, other, moved = git_repo(root)
        (root / "scripts").mkdir()
        out = HEADER + "".join(line + "\n" for line in selected)
        (root / "scripts/select-tests.py").write_text(
            f"import sys\nsys.stdout.write({out!r})\nprint('selector failed', file=sys.stderr)\n"
            f"sys.exit({selector_exit})\n")
        for path, lines in logs.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(HEADER + "running\n" + "".join(line + "\n" for line in lines) + "Summary [ 1s]\n")
        for path, text in ran.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(text.replace(TREE_ID, base).replace(OTHER_TREE_ID, other))
        summary = root / "target/summary.md"
        summary.parent.mkdir(exist_ok=True)
        summary.write_text(f"# lane change summary\n\nCommit: {change_of(root, child)}\n\nHow tested:\n"
                           + "".join(f"- Green log: {p.replace('<root>', str(root))} (`Summary [ 1s]`)\n"
                                     for p in (cites if cites is not None else [*logs, *ran]))
                           + "".join(f"- selector line `{s}`\n" for s in spans)
                           + "\nConfidence (1-5) that the verifier will confirm: 4\n")
        return subprocess.run([sys.executable, str(CHECKER), "--base", moved, "--summary", str(summary),
                               "--tree", child], cwd=root, capture_output=True, text=True)


def expect(name, r, code, *needles):
    assert (r.returncode == 0) == (code == 0), f"{name}: exit {r.returncode}, stderr:\n{r.stderr}"
    for n in needles:
        assert n in r.stderr, f"{name}: {n!r} not on stderr:\n{r.stderr}"
    print(f"[tests-cover-diff] {name}: {'PASS' if code == 0 else 'FAIL'} as expected")


def fixtures():
    ran = {f"target/r1/{line.split(':')[0]}.log": step_log(line) for line in SELECTED}
    expect("pass", run(SELECTED, {"target/r1/select.log": SELECTED}, spans=[NEXTEST, "nextest:"], ran=ran), 0)
    real = REAL_LOG.read_text()
    real_line = real.splitlines()[0]
    expect("real run-step log", run([real_line], {}, ran={"target/r2/clippy.log": real}), 0)
    expect("real run-step log ending exit: 1",
           run([real_line], {}, ran={"target/r2/clippy.log": real.replace("exit: 0", "exit: 1")}),
           1, "target/r2/clippy.log", "exit: 1")
    expect("disclosure lines need no log",
           run([real_line, TIMING, UNOWNED], {}, spans=[TIMING], ran={"target/r2/clippy.log": real}), 0)
    expect("non-run steps still need their logs",
           run([CLIPPY, NEXTEST, TIMING, UNOWNED], {}, ran={"target/r2/clippy.log": step_log(CLIPPY)}), 1, "`nextest`")
    expect("non-run steps still need their logs",
           run([CLIPPY, NEXTEST, TIMING, UNOWNED], {}, ran={"target/r2/nextest.log": step_log(NEXTEST)}), 1, "`clippy`")
    expect("brace list", run(SELECTED, {}, ran=ran, cites=["target/r1/{clippy,nextest,windowed}.log"]), 0)
    expect("brace list with one log missing",
           run(SELECTED, {}, ran=ran, cites=["target/r1/{clippy,windowed}.log"]), 1, "`nextest`")
    expect("absolute path under the lane root", run(SELECTED, {}, ran=ran, cites=[f"<root>/{p}" for p in ran]), 0)
    expect("path behind a placeholder", run(SELECTED, {}, ran=ran, cites=[*ran, "<ws>/target/r1/clippy.log"]),
           1, "`<ws>/target/r1/clippy.log`")
    # An owner recipe may print a line that starts with its own step name.
    odt = "org-deep-text-timing: just org-deep-text-timing"
    odt_log = f"{odt}\n{HEADER}org-deep-text-timing: load before {{ 45.47 69.68 96.03 }}\nSummary [ 1s]\n\nexit: 0\n"
    expect("owner recipe that echoes its step name",
           run([real_line, odt], {}, ran={"target/r2/clippy.log": real, "target/r2/org-deep-text-timing.log": odt_log}), 0)
    expect("run-step log without exit trailer",
           run(SELECTED, {}, ran={**ran, "target/r1/nextest.log": step_log(NEXTEST).replace("exit: 0\n", "")}),
           1, "target/r1/nextest.log", "`nextest`")
    expect("narrower set", run(SELECTED, {}, ran={**ran, "target/r1/nextest.log": step_log(NARROW)}),
           1, "target/r1/nextest.log", NARROW, NEXTEST)
    expect("also a narrower line", run(SELECTED, {}, ran={**ran, "target/r1/narrow.log": step_log(NARROW)}),
           1, "target/r1/narrow.log", NARROW)
    body = step_log(CLIPPY).replace("running\n", f"running\n{NEXTEST}\n")
    expect("body line of another selected step",
           run(SELECTED, {}, ran={"target/r1/clippy.log": body, "target/r1/windowed.log": step_log(WINDOWED)}),
           1, "`nextest`")
    red = {**ran, "target/r1/nextest.log": step_log(NEXTEST, 100)}
    expect("red step judged known reds only",
           run(SELECTED, {}, ran={**red, "target/r1/nextest-known-reds.log": classifier_log("target/r1/nextest.log")}), 0)
    expect("red step judged by a classifier cited by its absolute path",
           run(SELECTED, {}, ran={**red, "target/r1/nextest-known-reds.log": classifier_log("target/r1/nextest.log")},
               cites=[*ran, "<root>/target/r1/nextest-known-reds.log"]), 0)
    expect("green step without a classifier log", run(SELECTED, {}, ran=ran), 0)
    for code in (1, 3):
        expect(f"classifier exit {code}",
               run(SELECTED, {}, ran={**red, "target/r1/nextest-known-reds.log":
                                      classifier_log("target/r1/nextest.log", code)}),
               1, "target/r1/nextest-known-reds.log", "`nextest`", f"`exit: {code}`")
    expect("red step without a classifier log", run(SELECTED, {}, ran=red),
           1, "target/r1/nextest.log", "`exit: 100`", "known-reds: scripts/keystone-known-reds.sh target/r1/nextest.log")
    expect("classifier run on another step log",
           run(SELECTED, {}, ran={**red, "target/r0/nextest.log": step_log(NEXTEST, 100),
                                  "target/r1/nextest-known-reds.log": classifier_log("target/r0/nextest.log")},
               cites=[*red, "target/r1/nextest-known-reds.log"]),
           1, "target/r1/nextest.log", "target/r1/nextest-known-reds.log classifies target/r0/nextest.log")
    expect("red step judged known reds on another tree",
           run(SELECTED, {}, ran={**red, "target/r1/nextest-known-reds.log":
                                  classifier_log("target/r1/nextest.log", tree=OTHER_TREE_ID)}),
           1, "target/r1/nextest-known-reds.log classifies target/r1/nextest.log", "not the checked tree", "`nextest`")
    unit_logs = {p: f"{p}: the oracle output of one run\n" for p in UNDO_LOGS}
    sha = {p: hashlib.sha256(t.encode()).hexdigest() for p, t in unit_logs.items()}
    gate = step_log(WINDOWED, 1).replace("running\n", "".join(line + "\n" for line in windowed_red_lines(sha)))
    judged_units = {f"target/r1/windowed-unit-{i}-known-reds.log": classifier_log(p) for i, p in enumerate(UNDO_LOGS)}
    windowed_red = {**ran, "target/r1/windowed.log": gate,
                    "target/r1/windowed-known-reds.log": classifier_log("target/r1/windowed.log", 1), **judged_units}
    expect("windowed red unit whose two logs are known reds",
           run(SELECTED, {}, ran={**windowed_red, **unit_logs}, cites=[*windowed_red]), 0)
    expect("step log without exit trailer judged known reds only",
           run(SELECTED, {}, ran={**ran, "target/r1/nextest.log": step_log(NEXTEST, 100).rsplit("exit: 100", 1)[0],
                                  "target/r1/nextest-known-reds.log": classifier_log("target/r1/nextest.log")}),
           1, "target/r1/nextest.log", "`nextest`")
    expect("red step of a narrower line judged known reds only",
           run(SELECTED, {}, ran={**ran, "target/r1/nextest.log": step_log(NARROW, 100),
                                  "target/r1/nextest-known-reds.log": classifier_log("target/r1/nextest.log")}),
           1, "target/r1/nextest.log", NARROW)
    # The selector's own output names the steps but is no evidence that any of them ran.
    expect("selector output only", run(SELECTED, {"target/r1/select.log": SELECTED}), 1)
    expect("step not covered", run(SELECTED, {"target/r1/select.log": [CLIPPY, WINDOWED]}), 1, "`nextest`")
    expect("changed span", run(SELECTED, {"target/r1/select.log": SELECTED}, spans=[NARROW]), 1, f"`{NARROW}`")
    expect("step printed twice", run(SELECTED + [NARROW], {"target/r1/select.log": SELECTED + [NARROW]}),
           1, "step `nextest` 2 times")
    expect("selector fails", run(SELECTED, {"target/r1/select.log": SELECTED}, selector_exit=3),
           1, "exited 3", "selector failed")
    r = run(SELECTED, {}, ran={k: v for k, v in ran.items() if "nextest" not in k},
            cites=[*[k for k in ran if "nextest" not in k], "target/r1/gone.log"])
    expect("a missing log and an unselected step in one run", r, 1, "target/r1/gone.log does not exist", "`nextest`")


def load_checker():
    return load_module("tests_cover_diff", CHECKER)


def generated(cases):
    """Random selections with one run-step log per run step: the exact set passes, each single mutation fails."""
    checker = load_checker()
    not_run = checker.not_run_steps()

    def check(*args):
        return checker.check(*args, tree_of=lambda c: "t", head="t")
    rng = random.Random(0)
    names = ["clippy", "nextest", "windowed", "keystone-smoke", "timing", "unowned", "a", "b-2"]
    tokens = ["--workspace", "--features", "pbt", "-E", "'package(x)'", "just", "gate", "binary(y)",
              "a:", "b-2: x", "\\t", "  ", "é"]
    # No noise line is exactly `exit: 0`: run-step.sh writes that line, cargo and just never print it.
    noise = ["", "error: unrelated", "Summary [ 1s] 3 tests run", "nextest:", "UPPER: x", "exit: 1", "  exit: 0"]
    for case in range(cases):
        steps = rng.sample(names, rng.randint(1, len(names)))
        # The selector's own output contract (scripts/select-tests.py STEP_LINE) starts the args with a non-blank.
        selected = {s: f"{s}: --workspace " + " ".join(rng.choices(tokens, k=rng.randint(0, 6))) for s in steps}
        stdout = HEADER + "".join(line + "\n" for line in selected.values())
        # A step's output may echo any step line, its own name with other text or another step's exact line.
        echoes = [f"{s}: load before 1.5" for s in steps] + list(selected.values())
        body = noise + echoes
        logs = {f"target/r1/{s}.log": "\n".join([line, *HEADER.splitlines(), *rng.sample(body, rng.randint(0, 9)),
                                                 "", "exit: 0"]) for s, line in selected.items() if s not in not_run}
        if rng.random() < 0.5:
            logs["target/r1/select.log"] = stdout
        spans = "".join(f"`{line}`\n" for line in selected.values())
        bad = check(stdout, spans, logs, not_run)
        assert bad == [], f"case {case}: exact set fails: {bad}"

        step = rng.choice(steps)
        mutated = selected[step] + " extra"
        assert check(stdout, spans.replace(selected[step], mutated), logs, not_run), f"case {case}: changed span passes"
        assert checker.repeated_steps(stdout + selected[step] + "\n"), f"case {case}: step printed twice passes"
        assert checker.repeated_steps(stdout) == [], f"case {case}: steps printed once each fail"
        if step in not_run:
            continue
        path = f"target/r1/{step}.log"
        for name, text in [("changed line 1", logs[path].replace(selected[step], mutated, 1)),
                           ("exit 1", logs[path].rsplit("exit: 0", 1)[0] + "exit: 1"),
                           ("no exit trailer", logs[path].rsplit("exit: 0", 1)[0])]:
            assert any(path in b for b in check(stdout, spans, {**logs, path: text}, not_run)), \
                f"case {case}: {name} in {path} passes"
        dropped = {p: t for p, t in logs.items() if p != path}
        assert any(f"`{step}`" in b for b in check(stdout, spans, dropped, not_run)), \
            f"case {case}: dropped {step} passes"

        red_code = rng.choice([1, 100, 101, 2])
        red = {**logs, path: logs[path].rsplit("exit: 0", 1)[0] + f"exit: {red_code}"}
        judge = f"target/r1/{step}-known-reds.log"
        judged = {**red, judge: classifier_log(rng.choice([path, f"./{path}"]))}
        assert check(stdout, spans, judged, not_run) == [], f"case {case}: red {step} judged known reds only fails"
        for name, text in [("classifier exit 1", classifier_log(path, 1)), ("classifier exit 3", classifier_log(path, 3)),
                           ("classifier without exit trailer", classifier_log(path).rsplit("exit: 0", 1)[0]),
                           ("classifier on another log", classifier_log(f"target/r0/{step}.log"))]:
            assert any(f"`{step}`" in b for b in check(stdout, spans, {**red, judge: text}, not_run)), \
                f"case {case}: {name} for red {step} passes"
        assert any(path in b for b in check(stdout, spans, red, not_run)), f"case {case}: red {step} unjudged passes"
        cut = rng.choice(["", "killed", f"exit: {red_code} (signal)"])
        cut_judged = {**judged, path: logs[path].rsplit("exit: 0", 1)[0] + cut}
        assert any(path in b and f"`{step}`" in b for b in check(stdout, spans, cut_judged, not_run)), \
            f"case {case}: {step} log ending {cut!r} judged known reds only passes"
    print(f"[tests-cover-diff] generated: {cases} cases PASS")


def timing_modes(cases):
    """Step kind x header mode: the check fails exactly the nextest and keystone-smoke logs without `measure` in the
    header, and names the log and the value; a test's own output line `HOLON_PBT_TIMING=measure` never counts."""
    checker = load_checker()
    rng = random.Random(21)
    kinds = ["nextest", "keystone-smoke", "clippy", "windowed", "echo"]
    modes = ["measure", "enforce", "", "unset", None]
    for case in range(cases):
        kind, mode = rng.choice(kinds), rng.choice(modes)
        line = f"{kind}: --workspace {rng.choice(['a', 'b'])}"
        header = ["BASE: b", "TREE: t", "STARTED: 2026-10-31T17:00:00Z",
                  *([] if mode is None else [f"HOLON_PBT_TIMING={mode}"])]
        body = ["running", *(["HOLON_PBT_TIMING=measure"] if rng.random() < 0.5 else [])]
        log = "\n".join([line, *header, *body, "", "exit: 0"])
        path = f"target/r1/{kind}.log"
        bad = checker.check(line + "\n", "", {path: log}, set(), tree_of=lambda c: "t", head="t")
        want = kind in ("nextest", "keystone-smoke") and mode != "measure"
        assert bool(bad) == want, f"case {case}: {kind} header {header} body {body}: {bad}"
        if want:
            found = "no HOLON_PBT_TIMING= line" if mode is None else f"HOLON_PBT_TIMING={mode}"
            assert any(path in b and found in b for b in bad), f"case {case}: {bad}"
    print(f"[tests-cover-diff] timing modes: {cases} cases PASS")


def citation(rng):
    """One cited path in a form lanes write, and the paths it names."""
    root = rng.choice(["", "/Users/m/w/lane-12-a-b/", "/var/folders/x1/T/tmpq_7z/", "~/w/lane-3/"])
    dirs = rng.choice(["target/", "target/r2/", "target/lane-x.y/r1/"])
    stem = rng.choice(["clippy", "nextest", "gate-alone", "gate.r1"])
    ext = rng.choice([".log", ".txt"])
    if rng.random() < 0.4:
        alts = rng.sample(["1", "2", "3", "a", "b-2", ""], rng.randint(2, 4))
        cut = rng.choice([len(dirs), len(dirs) + len(stem)])
        head, tail = (dirs + stem)[:cut], (dirs + stem)[cut:]
        if cut == len(dirs):
            alts = [a or "x" for a in alts]
        return f"{root}{head}{{{','.join(alts)}}}{tail}{ext}", {f"{root}{head}{a}{tail}{ext}" for a in alts}
    return f"{root}{dirs}{stem}{ext}", {f"{root}{dirs}{stem}{ext}"}


def citations(cases):
    """Summaries citing paths in the forms lanes write: the check reads exactly the paths the generator drew."""
    cited_logs = load_checker().cited_logs
    rng = random.Random(1)
    wraps = ["{}", "`{}`", "({})", "{}.", "{},", "{};", "`{}`.", "**{}**", "[log]({})", "<{}>", "log={}", "{}:12"]
    lead = ["- ", "  - ", "* ", "1. ", "Green log: ", "- Red log: ", "| clippy | ", ""]
    for case in range(cases):
        want, body = set(), []
        for _ in range(rng.randint(0, 6)):
            parts = []
            for _ in range(rng.randint(1, 3)):
                text, paths = citation(rng)
                want |= paths
                parts.append(rng.choice(wraps).format(text))
            body.append(rng.choice(lead) + rng.choice([" ", ", ", " and "]).join(parts)
                        + rng.choice(["", " (`Summary [ 1s] 3 tests run: 3 passed`)", " -> exit: 0",
                                      " copy of target/s-summary.md", " see src/a.log", " target/x.logs"]))
        placeholder = None
        if rng.random() < 0.15:
            text, _ = citation(rng)
            placeholder = rng.choice(["<ws>/", "<archive>/", "$WS/"]) + text.lstrip("/~")
            body.insert(rng.randint(0, len(body)), f"- `{placeholder}`")
        outside = citation(rng)[0]
        summary = (f"# s\nClaim: see {outside}\n## How tested:\n" + "\n".join(body)
                   + f"\n\nConfidence (1-5) that the verifier will confirm: 4\n{outside}\n")
        got, bad = cited_logs(summary)
        assert got == sorted(want), f"case {case}: read {got}, cited {sorted(want)} in:\n{summary}"
        assert [placeholder] * len(bad) == [p for p in [placeholder] if p and any(f"`{p}`" in b for b in bad)], \
            f"case {case}: placeholder {placeholder}, problems {bad}"
    print(f"[tests-cover-diff] citations: {cases} cases PASS")


# Result lines copied from real nextest logs: a lane nextest run, an integrate gate job log (UTC prefix) and a
# windowed paired run.
REAL_RESULTS = [
    "        FAIL [   5.383s] (3689/8378) holon-integration-tests::integration advice_live_mcp_gate::advice_live_mcp_gate",
    "     TIMEOUT [ 120.011s] (3384/8380) holon-architecture-tests::architecture_rules archlint_all_passes",
    "2026-11-09T08:41:07Z         FAIL [   1.704s] (1/1) holon-gpui::main "
    "breadcrumb_resolves_from_view_root_windowed::the_breadcrumb_follows_the_view_root_and_then_the_focus",
    "        FAIL [   5.039s] ( 5/18) holon-gpui::main action_bar_windowed::tapping_a_global_op_button_dispatches_it",
]
REAL_PASSES = [
    "        PASS [   0.054s] (   1/8378) holon api::accepted_rules::tests::a_refusal_is_published_as_a_source_and_a_delete_forgets_it",
    "        LEAK [   0.241s] (  46/8379) holon api::history_store::tests::batch_shares_one_op_group_and_groups_are_distinct",
    "     Summary [1263.904s] 8378 tests run: 8371 passed (29 slow), 7 failed, 156 skipped",
]
WINDOWED_GATE_RED = ["RED       holon-gpui::main action_bar_windowed  tests=9  wall=173.7s",
                     "  FAIL: RED holon-gpui::main action_bar_windowed (log target/gate-logs/windowed/a.rerun.log)",
                     "windowed-gate: wall=2333.6s serial-sum=3379.3s PASS=107 KNOWN-RED=7 FLAKY=2 RED=4 HUNG=0"]
UNDO = "holon-gpui::main undo_survives_blur_windowed"
UNDO_LOGS = ("target/gate-logs/windowed/undo.log", "target/gate-logs/windowed/undo.rerun.log")


def windowed_red_lines(sha):
    """The rerun, FAIL and end lines of a real red windowed gate run with its paths shortened, and the `unit-log:` line
    of each log of its red unit that `sha` maps to its hash."""
    return ["FLAKY     holon-gpui::main chrome_one_row_windowed  rerun wall=46.1s  "
            "logs=target/gate-logs/windowed/chrome.log target/gate-logs/windowed/chrome.rerun.log",
            f"RED       {UNDO}  rerun wall=15.3s  logs={UNDO_LOGS[0]} {UNDO_LOGS[1]}",
            f"  FAIL: RED {UNDO} (log {UNDO_LOGS[1]})",
            "windowed-gate: wall=281.0s serial-sum=829.6s PASS=107 KNOWN-RED=11 FLAKY=1 RED=1 HUNG=0",
            *(f"  unit-log: {UNDO} {p} sha256={h}" for p, h in sha.items())]


ACTION_BAR = [f"holon-gpui::main action_bar_windowed::{t}" for t in (
    "tapping_a_global_op_button_dispatches_it", "the_dock_stays_one_row_below_the_main_panel",
    "with_the_keyboard_down_the_bar_is_not_painted", "entity_ops_come_before_global_ops_in_the_bar")]
LOOP = [f"holon-gpui::main gpui_composed_windowed_loop::{t}" for t in (
    "benchmark_windowed_per_case_boot_cost", "focused_task_block_shows_its_keyword_surface",
    "general_e2e_composed_pbt_windowed")]
ACCEPT_URL = "https://github.com/holon-space/holon-work/issues/1049#issuecomment-6732769520"
STATUSES = ["FAIL", "TIMEOUT", "SIGSEGV", "LEAK-FAIL", "ABORT SIG 6"]
HAND = "hand-authored: just hand-authored"
KEYSTONE = "hand_authored_regressions::hand_authored_keystone_regressions"
MODULE_TEST = "hand_authored_regressions::empty_locks_are_refused"
WBS, HM4_EDIT, CRASH_COPY, HM4_IDS = (
    "wbs-an-empty-org-block-survives-write-back-and-a-crash-restart",
    "hm4-a-crash-over-a-file-only-behind-its-store-keeps-the-stores-edit",
    "crash-restore-a-file-equal-to-its-moved-merge-base-with-a-write-owed-is-disclosed-with-a-copy",
    "hm4-a-crash-over-an-id-less-file-re-stamps-its-ids-at-boot")
HAND_CASES = [WBS, HM4_EDIT, CRASH_COPY, HM4_IDS, "create-block-smoke", "lane-only-case"]
SKIPS = ["reboot-orphans-the-previous-boots-watchers", "external-rewrite-keeps-a-promoted-pages-body",
         "split-then-two-backspaces-caret-follows-second-join"]
RUNNING = ("     Running tests/main.rs (target/debug/build/holon-integration-tests/7df5d042db08f7bc/out/"
           "integration-7df5d042db08f7bc)")
EARLIER_BINARY = ["     Running unittests src/lib.rs (target/debug/deps/holon_integration_tests-1c2d3e4f)", "",
                  "running 0 tests", "", "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; "
                  "finished in 0.00s", ""]
GUARD = "[inv-no-database-stuck] No harness timeout ended the run within 2940 s, so the guard ends it."
# Text the shard's other output writes into the same line under --nocapture; the recipe reads the name after it.
INTERLEAVED = ["", "Applying transition 2/6: ", "[inv-sql-budget] UndoLastMutation: not judged] "]


def real_hand_log(tree, first_skip):
    """The step line, header, shard, case, failing-test and HAND-AUTHORED RED lines of a real hand-authored run
    whose cases ran red on the base too, with each shard's cargo `Running` line and libtest end: a red shard's
    `failures:` list and `test result: FAILED` line; `first_skip` is its shard 0 skip line, which the lane run
    interleaved."""
    skip = ' via HOLON_HAND_AUTHORED_SKIP (known issue — see docs/Testing/BugFunnel.md)'
    shard = "target/gate-logs/pbt-hand-authored-shard-{}.log"
    keystone_failed = [f"test {KEYSTONE} ... FAILED", "", "failures:", "", "failures:", f"    {KEYSTONE}", ""]
    def ended(passed):
        return [f"test result: FAILED. {passed} passed; 1 failed; 0 ignored; 0 measured; 644 filtered out; finished in "
                "235.31s", "", "error: test failed, to rerun pass `-p holon-integration-tests --test integration`"]
    return [
        HAND, "BASE: d80bb20cbbabd282d19d78af6aafb5eb93d26c96", f"TREE: {tree}", "STARTED: 2026-11-09T15:44:25Z",
        f"== hand-authored shard 0 (exit 101): {shard.format(0)} ==", RUNNING, first_skip,
        f'[hand-authored regression] RED case "{WBS}"', *keystone_failed, *ended(18),
        f"== hand-authored shard 1 (exit 0): {shard.format(1)} ==", RUNNING,
        f'[hand-authored regression] SKIPPED case "reboot-orphans-the-previous-boots-watchers"{skip}',
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 662 filtered out; finished in 222.63s", "",
        f"== hand-authored shard 2 (exit 101): {shard.format(2)} ==", RUNNING,
        f'[hand-authored regression] SKIPPED case "a-failed-create-cancels-its-keystrokes-with-one-disclosure-loro-dispatch"{skip}',
        f'[hand-authored regression] RED case "{HM4_EDIT}"', f'[hand-authored regression] RED case "{CRASH_COPY}"',
        *keystone_failed, *ended(0),
        f"== hand-authored shard 3 (exit 101): {shard.format(3)} ==", RUNNING,
        f'[hand-authored regression] SKIPPED case "split-then-two-backspaces-caret-follows-second-join"{skip}',
        f'[hand-authored regression] SKIPPED case "external-rewrite-keeps-a-promoted-pages-body"{skip}',
        f'[hand-authored regression] RED case "{HM4_IDS}"', *keystone_failed, *ended(0),
        f"HAND-AUTHORED RED: shard 0 exit 101, red cases: {WBS}, log: {shard.format(0)}",
        f"HAND-AUTHORED RED: shard 2 exit 101, red cases: {HM4_EDIT},{CRASH_COPY}, log: {shard.format(2)}",
        f"HAND-AUTHORED RED: shard 3 exit 101, red cases: {HM4_IDS}, log: {shard.format(3)}",
        "exit: 1"]


REAL_HAND_LANE = real_hand_log(
    "c2828c70b3aa05f4558294c582f17069acca4edd",
    "[hand-authored regression] SKIPPED case test hand_authored_regressions::a_case_without_locks_is_refused_by_name"
    " - should panic ... ok")
REAL_HAND_BASE = real_hand_log(
    "9512c8b501b576f584b3db0ea381737116c4da52",
    '[hand-authored regression] SKIPPED case "a-failed-create-cancels-its-keystrokes-with-one-disclosure" via '
    'HOLON_HAND_AUTHORED_SKIP (known issue — see docs/Testing/BugFunnel.md)')


def module(test):
    binary, name = test.split(" ")
    return f"{binary} {name.split('::')[0]}"


def unit_logs(unit, spec):
    """The logs scripts/windowed-gate.py writes for a unit: a second one when the unit was re-run."""
    stem = "target/gate-logs/windowed/" + unit.replace("::", "__").replace(" ", "__")
    v = spec["verdict"]
    rerun = v in ("FLAKY", "HUNG") or (v in ("RED", "KNOWN-RED") and spec.get("rerun", True))
    return [f"{stem}.log", f"{stem}.rerun.log"] if rerun else [f"{stem}.log"]


def log_states(unit, spec):
    """{log: (file state, citation state)} of a RED unit; a unit drawn without states left its logs unwritten."""
    return spec.get("logs") or {log: ("missing", "none") for log in unit_logs(unit, spec)}


def gate_lines(units, off=0, sha={}):
    """The run-step body scripts/windowed-gate.py writes for `units` ({unit: {"verdict": <final verdict>, "rerun":
    <bool>}}): a first verdict line per unit, a rerun line per re-run unit, the end line (its RED count off by `off`),
    a FAIL line per RED or HUNG unit and a `unit-log:` line per log of those units that `sha` maps to its hash."""
    lines = [f"windowed-gate: {len(units)} units, {3 * len(units)} tests, jobs=4, per-unit timeout=900s"]
    for u, s in sorted(units.items()):
        logs = unit_logs(u, s)
        lines.append(f"{s['verdict'] if len(logs) == 1 else 'RED':<9} {u}  tests=3  wall=1.0s  log={logs[0]}")
    lines.append("windowed-gate: re-running the red/hung binaries alone, once")
    lines += [f"{s['verdict']:<9} {u}  rerun wall=2.0s  logs={' '.join(unit_logs(u, s))}"
              for u, s in sorted(units.items()) if len(unit_logs(u, s)) == 2]
    n = {v: sum(s["verdict"] == v for s in units.values()) for v in ("PASS", "KNOWN-RED", "FLAKY", "RED", "HUNG")}
    n["RED"] += off
    lines.append("windowed-gate: wall=9.0s serial-sum=9.0s " + " ".join(f"{k}={v}" for k, v in n.items()))
    bad = [(u, s) for u, s in sorted(units.items()) if s["verdict"] in ("RED", "HUNG")]
    lines += [f"  FAIL: {s['verdict']} {u} (log {unit_logs(u, s)[-1]})" for u, s in bad]
    return lines + [f"  unit-log: {u} {log} sha256={sha[log]}" for u, s in bad for log in unit_logs(u, s) if log in sha]


def failing_units(red, hung=()):
    """The units of a gate run whose failing verdicts name the modules of `red`; a module of `hung` is HUNG."""
    return {m: {"verdict": "HUNG" if m in hung else "RED"} for m in {module(t) for t in red}}


UNIT_FILES = ["present"] * 5 + ["missing", "changed", "unlisted"]
UNIT_CITES = ["ok"] * 5 + ["red", "none", "stale", "ok+red"]
# The exits of the classifier logs of one log, in the sorted order of their paths: `ok+red` is a green one first.
JUDGE_EXITS = {0: [0], 1: [1], 3: [3], "ok+red": [0, 1]}
# Failing units of every binary shape: a module of the `main` binary, a `harness = false` binary, the lib.
UNIT_POOL = [UNDO, "holon-gpui::main chrome_one_row_windowed", "holon-gpui::gpui_gherkin_replay", "holon-gpui"]


def gate_units(rng, failing, hung=()):
    """A windowed gate run: each unit of `failing` RED (HUNG when in `hung`), beside up to 3 units that pass, are
    known reds or are flaky. Each log of a RED unit draws its file state (present, missing, changed after the gate
    hashed it, or without its `unit-log:` line) and its citation (a classifier log exit 0, exit 1, on another tree, or
    none); a unit drawn all good has every log present and classified exit 0."""
    units = {}
    for m in failing:
        if m in hung:
            units[m] = {"verdict": "HUNG"}
            continue
        s = {"verdict": "RED", "rerun": rng.random() < 0.8}
        good = rng.random() < 0.6
        s["logs"] = {log: ("present", "ok") if good else (rng.choice(UNIT_FILES), rng.choice(UNIT_CITES))
                     for log in unit_logs(m, s)}
        units[m] = s
    for i in range(rng.randint(0, 3)):
        name = rng.choice(["holon-gpui::main other_{}_windowed", "holon-gpui::other_{}_replay"]).format(i)
        units[name] = {"verdict": rng.choice(["PASS", "KNOWN-RED", "FLAKY"]), "rerun": rng.random() < 0.5}
    return units


def judge_path(stem, i):
    """The path of the i-th classifier log of one log; the paths sort in the order of `i`."""
    path = f"{stem}-known-reds{i + 1 if i else ''}.log"
    assert i == 0 or f"{stem}-known-reds{i if i > 1 else ''}.log" < path
    return path


def unit_judge(log, i=0):
    return judge_path(f"target/r1/unit-{Path(log).name}", i)


def windowed_mode(o):
    """The header mode of a windowed lane log: a drawn `mode` (None is no header line), else measure.
    A red windowed log with another mode is refused for that alone, whatever else its logs show."""
    return o.get("mode", "measure")


def mode_refusal(path, mode):
    found = "no HOLON_PBT_TIMING= line" if mode is None else f"HOLON_PBT_TIMING={mode}"
    return [(path, "without the header line `HOLON_PBT_TIMING=measure`", found)] if mode != "measure" else []


def unit_refusals(units, off, path):
    """The needles of each refusal of the windowed red path, from the drawn state; [] when the step counts as run.
    `units` None is a step log without gate verdicts."""
    if units is None:
        return [(path, "no `windowed-gate: wall=...` end line")]
    bad = sorted(u for u, s in units.items() if s["verdict"] in ("RED", "HUNG"))
    hung = sum(units[u]["verdict"] == "HUNG" for u in bad)
    red = len(bad) - hung + off
    if not bad or len(bad) != red + hung:
        return [(path, f"{len(bad)} FAIL lines", f"RED={red} HUNG={hung}")]
    want = []
    for u in bad:
        if units[u]["verdict"] == "HUNG":
            want.append((path, f"unit {u} is HUNG"))
            continue
        for log, (file, cite) in log_states(u, units[u]).items():
            if file == "missing":
                want.append((path, f"unit log {log} does not exist"))
                continue
            if file == "changed":
                want.append((f"unit log {log} changed after the gate ran",))
            if file == "unlisted":
                want.append((f"unit log {log} has no unit-log line in {path}",))
            if cite == "none":
                want.append((path, f"unit {u} log {log} has no cited classifier log"))
            if cite == "red":
                want.append((unit_judge(log), f"classifies {log} of unit {u}", "`exit: 1`"))
            if cite == "stale":
                want.append((unit_judge(log), f"classifies {log} of unit {u}", "not the checked tree"))
            if cite == "ok+red":
                want.append((unit_judge(log, 1), f"classifies {log} of unit {u}", "`exit: 1`"))
    return want


def gate_spec(o):
    """The units of a red outcome's gate step log; None when its log carries nextest lines instead."""
    if o.get("nextest", True) and "units" not in o:
        return None
    return o["units"] if "units" in o else failing_units(o["red"], o.get("hung", ()))


CANCEL = "  Cancelling due to signal: 3 tests still running"
STOPPED_AT = "[fail-fast] stopped at holon-orgmode::main writeback::a: a red outside the known-reds list (target/v.log)"


def hand_spec(rng, cases, skips, shards=None):
    """A hand-authored run: its cases dealt round-robin in a random order to 4 shards, or one process when `shards`
    is 1; a shard with red cases fails its keystone test, and any shard may fail a test that names no case. A red
    shard's process may die by a signal after its failing tests: `died` is (the shard's exit, the signal's exit). A
    test binary's death leaves cargo's exit 101 and its `process didn't exit successfully` line; cargo's own death
    leaves only the signal's exit. A test process that ends itself (the stuck-database guard) is `died` (101, None):
    exit 101 and no signal line, but no libtest end either. `earlier`: a test binary ran before the shard's one. A sharded run's recipe may be cut while it waits for shard k >= 1, so shards k..
    and the `HAND-AUTHORED RED:` block never print, or after its block: `cut` is then (k, the step's exit), k = the
    shard count for a cut after the block."""
    shards = shards or rng.choice([4, 4, 4, 1])
    cases = rng.sample(cases, len(cases))
    spec = [{"cases": cases[k::shards], "tests": [KEYSTONE] if cases[k::shards] else []} for k in range(shards)]
    if rng.random() < 0.25:
        rng.choice(spec)["tests"].append(rng.choice([MODULE_TEST, KEYSTONE]))
    for shard in spec:
        shard["earlier"] = rng.random() < 0.3
        if shard["tests"] and rng.random() < 0.15:
            sig = rng.choice(list(SIGNALS))
            shard["died"] = rng.choice([(101, sig), (sig, sig), (101, None)])
    h = {"shards": spec, "skip": skips}
    # A cut before shard 0's head leaves no hand-authored line: the generic refusal, drawn by the static examples.
    if shards > 1 and rng.random() < 0.15:
        k = rng.randrange(1, shards + 1)
        h["cut"] = (k, rng.choice([1, 130, 137, 143] if k < shards else [130, 137, 143]))
    return h


SIGNALS = {134: "signal: 6, SIGABRT: process abort signal", 139: "signal: 11, SIGSEGV: invalid memory reference",
           137: "signal: 9, SIGKILL: kill"}


def hand_shown(h):
    """The shards whose output the log shows."""
    return h["shards"][:h["cut"][0]] if "cut" in h else h["shards"]


def hand_red(h):
    """The reference red set: case names and failing tests, whatever shard they ran in."""
    return {name for shard in hand_shown(h) for name in shard["cases"] + shard["tests"]}


def hand_unnamed(h):
    """A red shard of a sharded run that names no red case, in the `HAND-AUTHORED RED:` block."""
    return len(h["shards"]) > 1 and hand_shown(h) == h["shards"] and any(s["tests"] and not s["cases"]
                                                                          for s in h["shards"])


def hand_incomplete(h):
    return "cut" in h or any("died" in s for s in hand_shown(h))


def hand_exit(h):
    """The recipe's exit: 1 for a red sharded run, the step's exit of a cut one, else the one cargo process's exit."""
    if "cut" in h:
        return h["cut"][1]
    if len(h["shards"]) > 1:
        return 1
    shard = h["shards"][0]
    return shard["died"][0] if "died" in shard else 101


def hand_lines(rng, h):
    """The run-step body `just hand-authored` writes for `h`."""
    sharded = len(h["shards"]) > 1
    lines, reds, head_stamp = [], [], stamp(rng)
    skips = [f'[hand-authored regression] SKIPPED case "{c}" via HOLON_HAND_AUTHORED_SKIP (known issue)'
             for c in h["skip"]]
    for k, shard in enumerate(hand_shown(h)):
        red = bool(shard["cases"] or shard["tests"])
        rc = shard["died"][0] if "died" in shard else 101 if red else 0
        log = f"target/gate-logs/pbt-hand-authored-shard-{k}.log"
        if sharded:
            lines.append(f"{head_stamp}== hand-authored shard {k} (exit {rc}): {log} ==")
        lines += EARLIER_BINARY * shard.get("earlier", False) + [RUNNING]
        lines += skips if k == 0 else []
        lines += [f'{rng.choice(INTERLEAVED)}[hand-authored regression] RED case "{c}"' for c in shard["cases"]]
        # A shard process that dies leaves only the intact result lines of the tests that failed before; the test that
        # killed it writes none.
        died = "died" in shard
        for t in shard["tests"]:
            should_panic = t != KEYSTONE and rng.random() < 0.5
            lines += ([f"test {t}{' - should panic' if should_panic else ''} ... FAILED"] if died
                      else libtest_failure_lines(rng, t, should_panic)[:-4])
        if died and shard["died"][1] is None:
            lines += [GUARD] * (rng.random() < 0.5) + [
                "error: test failed, to rerun pass `-p holon-integration-tests --test integration`"]
        elif died and rc == 101:
            lines += ["error: test failed, to rerun pass `-p holon-integration-tests --test integration`", "",
                      "Caused by:", "  process didn't exit successfully: `target/debug/deps/integration-0a1b "
                      f"--nocapture` ({SIGNALS[shard['died'][1]]})"]
        elif shard["tests"]:
            lines += ["", "failures:", "", "failures:", *(f"    {t}" for t in shard["tests"]), ""]
        if not died:
            lines.append(f"test result: {'FAILED' if red else 'ok'}. 1 passed; 0 failed; 0 ignored; 0 measured")
        if red:
            reds.append(f"HAND-AUTHORED RED: shard {k} exit {rc}, red cases: "
                        f"{','.join(shard['cases']) or 'none, see the log'}, log: {log}")
    return lines + (reds if sharded and hand_shown(h) == h["shards"] else [])


def stamp(rng):
    """A gate job's time stamp in front of a log line, or none."""
    return rng.choice(["", "2026-11-09T22:44:13Z "])


def nextest_end_line(rng, passed, failed, total=None):
    """A nextest `Summary` line in each shape real logs carry: an optional gate-job time stamp, `1 test run` or
    `N tests run`, `k/n tests run` for a run cut at k of n (`total`), passes with `(N slow)` or `(N slow, N leaky)`,
    and the failed and timed-out counts only when not 0."""
    ran = passed + failed
    timed_out = rng.choice([0, 0, rng.randint(0, failed)])
    count = f"{ran}/{total} test{'s' * (total != 1)}" if total else f"{ran} test{'s' * (ran != 1)}"
    slow = rng.choice(["", f" ({rng.randint(1, 30)} slow)", f" ({rng.randint(1, 30)} slow, {rng.randint(1, 9)} leaky)"])
    rest = f", {failed - timed_out} failed" * (failed > timed_out) + f", {timed_out} timed out" * (timed_out > 0)
    return (f"{stamp(rng)}{' ' * rng.randint(1, 6)}Summary [{rng.uniform(0, 2000):>8.3f}s] {count} run: {passed} passed"
            f"{slow}{rest}, {rng.randint(0, 9000)} skipped")


def run_tail(rng, cut):
    """The end of a lane-nextest.py run: only a `k/n` summary with k < n shows a cut run; the Cancelling and
    `[fail-fast] stopped at` lines may come with a cut run and with a full one."""
    passed, failed = rng.randint(0, 5000), rng.randint(1, 9)
    total = passed + failed + rng.randint(1, 900) if cut else rng.choice([None, passed + failed])
    summary = nextest_end_line(rng, passed, failed, total)
    return [*[CANCEL] * (rng.random() < 0.5), summary, *[STOPPED_AT] * (rng.random() < 0.5)]


def result_line(rng, test, status):
    binary, name = test.split(" ")
    stamp = rng.choice(["", "2026-11-09T08:41:07Z "])
    counter = rng.choice(["", f"({rng.randint(1, 99):>4}/8378) "])
    return f"{stamp}{' ' * rng.randint(1, 8)}{status} [{rng.uniform(0, 300):>8.3f}s] {counter}{binary} {name}"


def git_repo(root):
    """The base, an empty child with the base's tree, a commit with another tree, and `moved`: a landing on the
    base that the lane's working copy does not contain. The checker takes `moved` as --base, so each run also
    asserts that it reads the lane's own changes and base tree from the fork point."""
    def git(*args):
        return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout.strip()
    git("init", "-q")
    git("config", "user.email", "t@t")
    git("config", "user.name", "t")
    (root / "f").write_text("a\n")
    git("add", "f")
    git("commit", "-qm", "base")
    base = git("rev-parse", "HEAD")
    git("commit", "-q", "--allow-empty", "-m", "child")
    child = git("rev-parse", "HEAD")
    (root / "f").write_text("b\n")
    git("commit", "-qam", "other")
    other = git("rev-parse", "HEAD")
    git("checkout", "-q", "-b", "landed", base)
    (root / "landed").write_text("landed\n")
    git("add", "landed")
    git("commit", "-qm", "landed")
    moved = git("rev-parse", "HEAD")
    git("checkout", "-q", "--detach", other)
    subprocess.run(["jj", "git", "init", "--colocate"], cwd=root, check=True, capture_output=True)
    return base, child, other, moved


def change_of(root, commit):
    return subprocess.run(["jj", "log", "--no-graph", "-r", commit, "-T", "change_id"], cwd=root, check=True,
                          capture_output=True, text=True).stdout


def lane_files(rng, spec, commits):
    """The cited files, the How-tested lines and the reference problems of one lane spec.

    `spec` maps each selected step line to its outcome. A reference problem is the tuple of needles its stderr line
    holds; it is derived from the outcome alone, not from the checker's reading of the logs."""
    base, child, other, _ = commits
    files, cites, extra, want = {}, [], [], []

    def log(path, line, body, tail, tree=base, mode="measure"):
        timing = "" if mode is None else f"HOLON_PBT_TIMING={mode}\n"
        files[path] = f"{line}\nBASE: {base}\nTREE: {tree}\n{timing}STARTED: 2026-11-09T05:15:49Z\n" + "".join(
            b + "\n" for b in body) + f"\n{tail}\n"
        cites.append(path)

    def results(red, green=()):
        lines = [result_line(rng, t, rng.choice(STATUSES)) for t in red] + [
            result_line(rng, t, "PASS") for t in green] + rng.sample(REAL_PASSES, rng.randint(0, 3))
        rng.shuffle(lines)
        return lines

    for line, o in spec.items():
        step = line.split(":")[0]
        path = f"target/r1/{step}.log"
        kind = o["kind"]
        not_run = (f"`{step}`", "no cited run-step log")
        if kind == "green":
            log(path, line, results([], o.get("green", ["b t::ok"])), "exit: 0")
            continue
        if kind == "missing":
            cites.append(path)
            want += [(path, "does not exist"), not_run]
            continue
        if kind == "malformed":
            log(path, line, results(o.get("red", [])), rng.choice(["", "killed", "exit: 100 (signal)"]))
            want += [(path, "not `exit: <code>`"), not_run]
            continue
        if kind in ("route", "route-missing"):
            evidence = f"target/r1/{step}-excl.log"
            extra.append(f"- `ruled-route: {step} {o.get('ruling', '1011/ROUTE-C')} {evidence}`")
            if kind == "route":
                log(evidence, line + " -E '!test(=t::slow)'", results(o.get("red", [])), "exit: 100")
            else:
                cites.append(evidence)
                want += [(evidence, "does not exist"), (evidence, "no cited existing log"), not_run]
            continue
        red = o["red"]
        units = gate_spec(o)
        if "hand" in o:
            body = hand_lines(rng, o["hand"])
        elif units is None:
            body = results(red, o.get("green", [])) + run_tail(rng, o.get("stopped", False))
        elif step == "windowed":
            body = gate_lines(units, o.get("off", 0), unit_files(units, log, files, {"base": base, "other": other}))
        else:
            body = gate_lines(units)
        code = hand_exit(o["hand"]) if "hand" in o else rng.choice([1, 100, 101])
        mode = windowed_mode(o) if step == "windowed" else "measure"
        log(path, line, body, f"exit: {code}", mode=mode)
        judge = o.get("classifier")
        stale = judge == 0 and not judged_on_tree(o)
        exits = JUDGE_EXITS[judge] if judge is not None else []
        for i, judge_code in enumerate(exits):
            log(judge_path(f"target/r1/{step}", i), f"known-reds: scripts/keystone-known-reds.sh {path}", [],
                f"exit: {judge_code}", tree=other if stale else base)
        base_path = f"target/base/{step}.log"
        has_base = "base" in o or "base_hand" in o
        if has_base and not (judge == 0 and not stale):
            extra.append(f"- accepted-base-red: {step} {base_path} {o.get('acceptance', ACCEPT_URL)}")
            if "base_hand" in o:
                base_body = hand_lines(rng, o["base_hand"])
            elif o.get("base_nextest", True):
                base_body = results(o["base"])
            else:
                base_body = gate_lines(failing_units(o["base"], o.get("base_hung", ())))
            log(base_path, o.get("base_line", line), base_body, "exit: 100",
                tree={"base": base, "child": child, "other": other}[o.get("tree", "child")])
        if code and mode != "measure":
            want += mode_refusal(path, mode) + [not_run]
            continue
        if judge == 0 and not stale:
            continue
        if has_base:
            refusals = base_refusal(o, path, base_path, red)
            if not refusals:
                continue
        else:
            refusals = []
        windowed = unit_refusals(units, o.get("off", 0), path) if step == "windowed" else None
        if windowed == []:
            continue
        if stale:
            want.append((f"target/r1/{step}-known-reds.log classifies {path} of step", "not the checked tree"))
        elif judge is not None:
            want += [(f"{judge_path(f'target/r1/{step}', i)} classifies {path} of step", f"`exit: {code}`")
                     for i, code in enumerate(exits) if code]
        elif windowed is None:
            want.append((f"{path} runs `{step}`", "no cited classifier log"))
        want += refusals + (windowed or [])
        want.append(not_run)
    return files, cites, extra, want


def unit_files(units, log, files, trees):
    """Writes each log of the RED and HUNG `units` in its drawn file state and the classifier log of its drawn
    citation (`trees` maps "base" and "other" to their commits); returns the hash each `unit-log:` line records."""
    sha = {}
    for u, s in units.items():
        if s["verdict"] not in ("RED", "HUNG"):
            continue
        states = log_states(u, s) if s["verdict"] == "RED" else {p: ("present", "none") for p in unit_logs(u, s)}
        for path, (file, cite) in states.items():
            text = f"{path}: the oracle output of one run\n"
            if file != "missing":
                files[path] = text
            if file != "unlisted":
                sha[path] = hashlib.sha256((text + "an earlier run\n" if file == "changed" else text).encode()).hexdigest()
            exits = {"none": [], "ok": [0], "stale": [0], "red": [1], "ok+red": [0, 1]}[cite]
            for i, code in enumerate(exits):
                log(unit_judge(path, i), f"known-reds: scripts/keystone-known-reds.sh {path}", [],
                    f"exit: {code}", tree=trees["other" if cite == "stale" else "base"])
    return sha


def base_refusal(o, path, base_path, red):
    """The needles of each accepted-base-red refusal; [] when the base log proves every lane red."""
    if o.get("base_line"):
        return [(base_path, "the selector prints")]
    if o.get("tree") == "other":
        return [(base_path, "is not the tree of --base")]
    if "hand" in o:
        return hand_refusals(o["hand"], o["base_hand"], path, base_path)
    if o.get("stopped") and o.get("nextest", True):
        return [(path, "stopped before all its tests ran")]
    if not red:
        return [(path, "no nextest failing-test line")]
    if by_module(o):
        only = sorted({module(t) for t in red} - {module(t) for t in o["base"]})
    else:
        only = sorted(set(red) - set(o["base"]))
    return [(path, "red only in the lane", base_path, *only)] if only else []


def hand_refusals(lane, base, path, base_path):
    if not any(s["cases"] for s in hand_shown(lane)) and not hand_unnamed(lane):
        return [(path, "no nextest failing-test line", "no hand-authored")]
    want = [(path, "names no red case", base_path)] if hand_unnamed(lane) else []
    if hand_incomplete(lane):
        want.append((path, "the run is incomplete", "needs a complete run"))
    if skipped := sorted(set(lane["skip"]) - set(base["skip"])):
        want.append((path, "skipped only in the lane", base_path, *skipped))
    if only := sorted(hand_red(lane) - hand_red(base)):
        want.append((path, "red only in the lane", base_path, *only))
    return want


def by_module(o):
    return not (o.get("nextest", True) and o.get("base_nextest", True) and o["base"])


def judged_on_tree(o):
    return o.get("classifier") == 0 and o.get("classifier_tree") != "other"


def module_notes(spec):
    """How many accepted-base-red compares run at module level."""
    return sum(1 for line, o in spec.items() if o["kind"] == "red" and not judged_on_tree(o) and "base" in o
               and (line != WINDOWED or windowed_mode(o) == "measure")
               and not o.get("base_line") and o.get("tree") != "other" and o["red"] and by_module(o) and "hand" not in o
               and not (o.get("stopped") and o.get("nextest", True)))


def run_lane(root, spec, rng, commits):
    files, cites, extra, want = lane_files(rng, spec, commits)
    (root / "scripts").mkdir(parents=True)
    out = "".join(line + "\n" for line in spec)
    (root / "scripts/select-tests.py").write_text(f"import sys\nsys.stdout.write({out!r})\n")
    for path, text in files.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    summary = root / "summary.md"
    prose = "- The `accepted-base-red:` line names a log and a ruling; ruled-route: with an id and a log.\n"
    summary.write_text(f"# s\n\nCommit: {change_of(root, commits[1])}\n\nHow tested:\n" + prose + "".join(f"- log: {p} (`x`)\n" for p in cites)
                       + "\n".join(extra)
                       + "\n\nConfidence (1-5) that the verifier will confirm: 4\n")
    r = subprocess.run([sys.executable, str(CHECKER), "--base", commits[3], "--summary", str(summary),
                        "--tree", commits[1]], cwd=root, capture_output=True, text=True)
    return r, want


def judge(name, r, want):
    """Property 1: each stderr line is one reference problem. Property 2: exit 0 iff none.
    Property 3: as many lines as reference problems."""
    lines = [line for line in r.stderr.splitlines() if line.startswith("tests-cover-diff: ")]
    assert (r.returncode == 0) == (not want), f"{name}: exit {r.returncode}, want {want}, stderr:\n{r.stderr}"
    assert len(lines) == len(want), f"{name}: {len(lines)} lines for {len(want)} problems {want}:\n{r.stderr}"
    rest = list(lines)
    for needles in want:
        hit = next((line for line in rest if all(n in line for n in needles)), None)
        assert hit, f"{name}: no line holds {needles}:\n{r.stderr}"
        rest.remove(hit)


STATIC = {
    "six windowed reds, all red on the base (gate verdicts against a paired nextest base leg)": {
        "windowed: just windowed-gate": {"kind": "red", "red": ACTION_BAR[:3] + LOOP, "nextest": False,
                                         "base": ACTION_BAR + LOOP, "classifier": 1, "tree": "child"}},
    "one windowed red module, red on the base": {
        "windowed: just windowed-gate": {"kind": "red", "red": ACTION_BAR[:1], "nextest": False, "tree": "base",
                                         "base": ACTION_BAR[:1], "base_nextest": False, "classifier": 1}},
    "a windowed red module run in enforce mode, red on the base": {
        "windowed: just windowed-gate": {"kind": "red", "red": ACTION_BAR[:1], "nextest": False, "tree": "base",
                                         "base": ACTION_BAR[:1], "base_nextest": False, "classifier": 1,
                                         "mode": "enforce"}},
    "a windowed red module only in the lane": {
        "windowed: just windowed-gate": {"kind": "red", "red": ACTION_BAR[:1] + LOOP[:1], "nextest": False,
                                         "base": ACTION_BAR, "base_nextest": False}},
    "six nextest reds, all red on the base": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "red", "red": ACTION_BAR[:3] + LOOP,
                                                     "base": ACTION_BAR + LOOP, "classifier": 1}},
    "one red, red on the base": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "red", "red": ACTION_BAR[:1], "base": ACTION_BAR[:1],
                                                     "acceptance": "w142/IROH"}},
    "a lane-only red hidden among base reds": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "red", "red": [*ACTION_BAR, "holon api::t::lane_only"],
                                                     "base": ACTION_BAR + LOOP}},
    "a base log run on another tree": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "red", "red": LOOP, "base": LOOP, "tree": "other"}},
    "a base log of another step line": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "red", "red": LOOP, "base": LOOP,
                                                     "base_line": "nextest: --workspace -E 'package(x)'"}},
    "a red log of another step kind cannot be compared": {
        "clippy: --workspace": {"kind": "red", "red": [], "nextest": False, "base": LOOP}},
    "a red step with no failing-test line": {
        "nextest: --workspace": {"kind": "red", "red": [], "base": LOOP}},
    "a red step classified twice, exit 0 then exit 1": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "red", "red": LOOP, "classifier": "ok+red"}},
    "four hand-authored red cases in shards 0, 2 and 3, red on the base in the same shards": {
        HAND: {"kind": "red", "red": [], "classifier": 1, "tree": "child",
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                   {"cases": [HM4_EDIT, CRASH_COPY], "tests": [KEYSTONE]},
                                   {"cases": [HM4_IDS], "tests": [KEYSTONE]}], "skip": SKIPS},
               "base_hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                        {"cases": [HM4_EDIT, CRASH_COPY], "tests": [KEYSTONE]},
                                        {"cases": [HM4_IDS], "tests": [KEYSTONE]}], "skip": SKIPS}}},
    "a hand-authored red case only in the lane, hidden among base reds": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS, "lane-only-case"], "tests": [KEYSTONE]},
                                   {"cases": [HM4_IDS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                   {"cases": [], "tests": []}], "skip": []},
               "base_hand": {"shards": [{"cases": [WBS, HM4_EDIT], "tests": [KEYSTONE]},
                                        {"cases": [HM4_IDS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                        {"cases": [], "tests": []}], "skip": []}}},
    "the same hand-authored red cases in other shards": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [HM4_IDS], "tests": [KEYSTONE]},
                                   {"cases": [], "tests": []}, {"cases": [], "tests": []}], "skip": []},
               "base_hand": {"shards": [{"cases": [], "tests": []}, {"cases": [], "tests": []},
                                        {"cases": [HM4_IDS, WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []}],
                             "skip": []}}},
    "a red hand-authored shard that names no case": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": [MODULE_TEST]},
                                   {"cases": [], "tests": []}, {"cases": [], "tests": []}], "skip": []},
               "base_hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE, MODULE_TEST]},
                                        {"cases": [], "tests": []}, {"cases": [], "tests": []},
                                        {"cases": [], "tests": []}], "skip": []}}},
    "a hand-authored shard that ended itself with exit 101 before libtest's end line": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                   {"cases": [HM4_IDS], "tests": [KEYSTONE], "died": (101, None)},
                                   {"cases": [], "tests": []}], "skip": []},
               "base_hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                        {"cases": [HM4_IDS], "tests": [KEYSTONE]}, {"cases": [], "tests": []}],
                             "skip": []}}},
    "a hand-authored shard that ended itself with exit 101 after an earlier test binary's end line": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]},
                                   {"cases": [HM4_IDS], "tests": [KEYSTONE], "died": (101, None), "earlier": True}],
                        "skip": []},
               "base_hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [HM4_IDS], "tests": [KEYSTONE]}],
                             "skip": []}}},
    "a hand-authored run killed after its HAND-AUTHORED RED block": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                   {"cases": [], "tests": []}, {"cases": [], "tests": []}], "skip": [],
                        "cut": (4, 143)},
               "base_hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                        {"cases": [], "tests": []}, {"cases": [], "tests": []}], "skip": []}}},
    "a hand-authored case skipped only in the lane": {
        HAND: {"kind": "red", "red": [],
               "hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                   {"cases": [], "tests": []}, {"cases": [], "tests": []}], "skip": SKIPS[:2]},
               "base_hand": {"shards": [{"cases": [WBS], "tests": [KEYSTONE]}, {"cases": [], "tests": []},
                                        {"cases": [], "tests": []}, {"cases": [], "tests": []}], "skip": SKIPS[:1]}}},
    "the route of a ruling": {
        "nextest: --workspace -E 'package(holon)'": {"kind": "route", "red": LOOP}},
    "two missing steps and a missing log": {
        "clippy: --workspace": {"kind": "missing"}, "nextest: --workspace": {"kind": "missing"},
        "windowed: just windowed-gate": {"kind": "route-missing"}},
}


def outcome(rng):
    kind = rng.choice(["green", "red", "red", "red", "route", "route-missing", "missing", "malformed"])
    if kind != "red":
        return {"kind": kind, "red": rng.sample(LOOP, rng.randint(0, 2))}
    if rng.random() < 0.3:
        return hand_outcome(rng)
    pool = ACTION_BAR + LOOP + ["holon api::t::lane_only", "holon-tui::main a::b"]
    red = rng.sample(pool, rng.randint(0, 4))
    o = {"kind": "red", "red": red, "green": rng.sample(pool, rng.randint(0, 2))}
    if (c := rng.choice([None, 0, 1, 3, "ok+red"])) is not None:
        o["classifier"] = c
        o["classifier_tree"] = rng.choice(["base", "other"])
    if rng.random() < 0.7:
        base = set(red) | set(rng.sample(pool, rng.randint(0, 3)))
        if red and rng.random() < 0.3:
            base.discard(rng.choice(red))
        o["base"] = sorted(base)
        o["tree"] = rng.choice(["base", "child", "child", "other"])
        if rng.random() < 0.15:
            o["base_line"] = "nextest: --workspace -E 'package(other)'"
        if rng.random() < 0.3:
            o["nextest"] = False
        if rng.random() < 0.3:
            o["base_nextest"] = False
        modules = sorted({module(t) for t in pool})
        o["hung"] = rng.sample(modules, rng.randint(0, 2))
        o["base_hung"] = rng.sample(modules, rng.randint(0, 2))
    if rng.random() < 0.2:
        o["stopped"] = True
    o["mode"] = rng.choice(["measure"] * 9 + ["enforce", "", None])
    if not o.get("nextest", True):
        o["units"] = gate_units(rng, sorted({module(t) for t in red}), o.get("hung", ()))
        o["off"] = rng.choice([0] * 8 + [1] + [-1] * any(s["verdict"] == "RED" for s in o["units"].values()))
    return o


def hand_outcome(rng):
    """A red hand-authored step whose base run deals its cases to shards independently of the lane run."""
    lane = rng.sample(HAND_CASES, rng.randint(0, 4))
    base = set(lane) | set(rng.sample(HAND_CASES, rng.randint(0, 2)))
    if lane and rng.random() < 0.3:
        base.discard(rng.choice(lane))
    skip = rng.sample(SKIPS, rng.randint(0, 3))
    base_skip = set(skip) | set(rng.sample(SKIPS, rng.randint(0, 1)))
    if skip and rng.random() < 0.2:
        base_skip.discard(rng.choice(skip))
    o = {"kind": "red", "hand": hand_spec(rng, lane, skip)}
    o["red"] = sorted(hand_red(o["hand"]))
    if (c := rng.choice([None, 0, 1, 3, "ok+red"])) is not None:
        o["classifier"] = c
    if rng.random() < 0.8:
        o["base_hand"] = hand_spec(rng, sorted(base), sorted(base_skip))
        o["base"] = sorted(hand_red(o["base_hand"]))
        o["tree"] = rng.choice(["base", "child", "child", "other"])
        if rng.random() < 0.1:
            o["base_line"] = "hand-authored: just hand-authored x"
    return o


def outcomes(cases):
    """Every selected step draws one outcome; the checker's problem lines equal the reference problems."""
    rng = random.Random(2)
    lines = ["clippy: --workspace", "nextest: --workspace -E 'package(holon)'", "windowed: just windowed-gate",
             "a: just a", "b-2: just b", HAND]
    with tempfile.TemporaryDirectory() as repo:
        repo = Path(repo)
        commits = git_repo(repo)
        drawn = [(name, spec) for name, spec in STATIC.items()]
        drawn += [(f"case {i}", {line: outcome(rng) for line in rng.sample(lines, rng.randint(1, len(lines)))})
                  for i in range(cases)]
        judges = Counter(str(o.get("classifier")) for _, spec in drawn for o in spec.values() if o["kind"] == "red")
        assert all(judges[str(c)] for c in [None, *JUDGE_EXITS]), f"a classifier state no red step drew: {judges}"
        ends = Counter(("ended" if "died" not in shard else "guard" if shard["died"][1] is None else "signal")
                       + (" after an earlier binary" if shard.get("earlier") else "")
                       for _, spec in drawn for o in spec.values() if "hand" in o for shard in hand_shown(o["hand"])
                       if shard["tests"])
        assert all(ends[e + a] for e in ("ended", "guard", "signal") for a in ("", " after an earlier binary")), \
            f"a red shard end no hand step drew: {ends}"
        for i, (name, spec) in enumerate(drawn):
            r, want = run_lane(repo / f"lane{i}", spec, rng, commits)
            judge(name, r, want)
            notes = r.stdout.count("compared at module level")
            assert notes == module_notes(spec), f"{name}: {notes} module notes, want {module_notes(spec)}:\n{r.stdout}"
            if name in STATIC:
                print(f"[tests-cover-diff] {name}: {'PASS' if not want else 'FAIL'} as expected")
    print(f"[tests-cover-diff] outcomes: {cases} cases PASS; red steps per classifier state {dict(judges)}, "
          f"red hand shards per end {dict(ends)}")


def result_pattern():
    """Every real result line reads as its test; no pass and no summary line reads as a failing test; only the
    windowed gate's verdict line reads as a red module."""
    failing = load_checker().failing_tests
    got = failing(REAL_RESULTS + REAL_PASSES + WINDOWED_GATE_RED)
    assert got == {" ".join(re.split(r"\s+", line.strip())[-2:]) for line in REAL_RESULTS}, got
    modules = load_checker().gate_red_modules(REAL_RESULTS + REAL_PASSES + WINDOWED_GATE_RED)
    assert modules == {"holon-gpui::main action_bar_windowed"}, modules
    print("[tests-cover-diff] real result lines: PASS")


def real_hand_lines():
    """The lane and base hand-authored runs of one diff, whose four cases ran red on both: the checker reads the
    recipe's case names, interleaved skip line included, and accepts the lane red."""
    checker = load_checker()
    lane, unnamed, cut, skipped = checker.hand_authored(REAL_HAND_LANE)
    assert lane == {WBS, HM4_EDIT, CRASH_COPY, HM4_IDS, KEYSTONE} and not unnamed and not cut, (lane, unnamed, cut)
    assert checker.hand_authored(REAL_HAND_BASE)[0] == lane
    assert len(skipped) == 4, skipped
    summary = ("How tested:\n- target/r1/hand-authored.log\n- target/base/hand-authored.log\n"
               f"- accepted-base-red: hand-authored target/base/hand-authored.log {ACCEPT_URL}\n")
    logs = {"target/r1/hand-authored.log": "\n".join(REAL_HAND_LANE),
            "target/base/hand-authored.log": "\n".join(REAL_HAND_BASE)}
    bad = checker.check(HAND + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
    assert bad == [], bad
    print("[tests-cover-diff] real hand-authored lines: PASS")


def libtest_failure_lines(rng, test, should_panic):
    """The lines a failing libtest test writes into a --nocapture shard log: its result line, which other threads'
    output may split or extend, and the `failures:` list that ends the run."""
    head = f"test {test}{' - should panic' if should_panic else ''} ... "
    result = rng.choice([[head + "FAILED"], [head + "FAILED[inv-sql-budget] x: not judged"],
                         [head + "\x1b[2m WARN\x1b[0m holon_profiles::type_registry: computed fields", "FAILED"],
                         ["Applying transition 5/" + head + "6: FAILED"]])
    return result + ["", "failures:", f"    {test}", ""]


def hand_lane_only_tests(cases):
    """A module test that fails only in the lane, among case reds that the base shares, is never accepted, whatever
    form its libtest result line takes."""
    checker = load_checker()
    rng = random.Random(7)
    summary = ("How tested:\n- target/r1/hand-authored.log\n- target/base/hand-authored.log\n"
               f"- accepted-base-red: hand-authored target/base/hand-authored.log {ACCEPT_URL}\n")
    shard0 = REAL_HAND_LANE.index(f"test {KEYSTONE} ... FAILED")
    for case in range(cases):
        test = rng.choice([MODULE_TEST, "hand_authored_regressions::a_lane_only_module_test"])
        extra = libtest_failure_lines(rng, test, should_panic=case == 0 or rng.random() < 0.5)
        lane = REAL_HAND_LANE[:shard0] + extra + REAL_HAND_LANE[shard0:]
        logs = {"target/r1/hand-authored.log": "\n".join(lane),
                "target/base/hand-authored.log": "\n".join(REAL_HAND_BASE)}
        bad = checker.check(HAND + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert any("red only in the lane" in b and test in b for b in bad), (case, extra, bad)
    print(f"[tests-cover-diff] hand-authored lane-only module tests: {cases} cases PASS")


def died_shard_lines(rng):
    """A lane run of the real lines in which one shard's process died (an abort, a crash or a kill) where its base
    shard did not: no `failures:` list and no `test result:` line, the shard exits with the signal, and the test that
    killed it wrote no result line."""
    k = rng.choice([0, 1, 2, 3])
    rc, signal = rng.choice([(134, "signal: 6, SIGABRT: process abort signal"),
                             (139, "signal: 11, SIGSEGV: invalid memory reference"), (137, "signal: 9, SIGKILL: kill")])
    keystone_done = k == 0 and rng.random() < 0.5
    log = f"target/gate-logs/pbt-hand-authored-shard-{k}.log"
    lines, shard = [], None
    for line in REAL_HAND_LANE:
        if line.startswith("== hand-authored shard "):
            if shard == k:
                lines.append(f"  process didn't exit successfully ({signal})")
            shard = int(line.split()[3])
            if shard == k:
                line = re.sub(r"\(exit \d+\)", f"(exit {rc})", line)
        elif line.startswith("HAND-AUTHORED RED:") and shard is not None:
            if shard == k:
                lines.append(f"  process didn't exit successfully ({signal})")
            shard = None
        if shard == k and (line in ("failures:", "", f"    {KEYSTONE}")
                           or (line == f"test {KEYSTONE} ... FAILED" and not keystone_done)):
            continue
        if line.startswith(f"HAND-AUTHORED RED: shard {k} "):
            line = re.sub(r"exit \d+", f"exit {rc}", line)
        lines.append(line)
    if not any(line.startswith(f"HAND-AUTHORED RED: shard {k} ") for line in lines):
        lines.insert(lines.index("exit: 1"), f"HAND-AUTHORED RED: shard {k} exit {rc}, red cases: none, see the log, log: {log}")
    return k, rc, lines


def hand_died_shards(cases):
    """A lane shard whose process died where its base shard did not hides the red of the test that killed it and of
    every test it never ran: the check never accepts it, whatever case reds the base shares."""
    checker = load_checker()
    rng = random.Random(11)
    summary = ("How tested:\n- target/r1/hand-authored.log\n- target/base/hand-authored.log\n"
               f"- accepted-base-red: hand-authored target/base/hand-authored.log {ACCEPT_URL}\n")
    for case in range(cases):
        k, rc, lane = died_shard_lines(rng)
        logs = {"target/r1/hand-authored.log": "\n".join(lane),
                "target/base/hand-authored.log": "\n".join(REAL_HAND_BASE)}
        bad = checker.check(HAND + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert any("target/r1/hand-authored.log" in b for b in bad), (case, k, rc, "\n".join(lane), bad)
    print(f"[tests-cover-diff] hand-authored died shards: {cases} cases PASS")


def exited_shard_lines(rng):
    """A lane run of the real lines in which one shard's test process ended itself with libtest's red exit 101
    (`std::process::exit(101)`, as the database-stuck guard does) after its last RED case line: cargo writes the
    same `error: test failed` line as for a red shard, but libtest never wrote its `test result:` line nor its
    `failures:` list, and the tests after the exit never ran."""
    k = rng.choice([0, 1, 2, 3])
    guard = rng.random() < 0.5
    log = f"target/gate-logs/pbt-hand-authored-shard-{k}.log"
    lines, shard = [], None
    for line in REAL_HAND_LANE:
        if line.startswith("== hand-authored shard ") or line.startswith("HAND-AUTHORED RED:"):
            if shard == k:
                if guard:
                    lines.append("[inv-no-database-stuck] No harness timeout ended the run within 2940 s, so the "
                                 "guard ends it.")
                lines.append("error: test failed, to rerun pass `-p holon-integration-tests --test integration`")
            shard = int(line.split()[3]) if line.startswith("== ") else None
            if shard == k:
                line = re.sub(r"\(exit \d+\)", "(exit 101)", line)
        if shard == k and (line in ("failures:", "", f"    {KEYSTONE}") or line == f"test {KEYSTONE} ... FAILED"):
            continue
        lines.append(line)
    if not any(line.startswith(f"HAND-AUTHORED RED: shard {k} ") for line in lines):
        lines.insert(lines.index("exit: 1"), f"HAND-AUTHORED RED: shard {k} exit 101, red cases: none, see the log, log: {log}")
    return k, guard, lines


def hand_exited_shards(cases):
    """A lane shard whose test process exited with code 101 before libtest's `test result:` line ran only part of
    its tests: the check never accepts it, whatever case reds the base shares."""
    checker = load_checker()
    rng = random.Random(17)
    summary = ("How tested:\n- target/r1/hand-authored.log\n- target/base/hand-authored.log\n"
               f"- accepted-base-red: hand-authored target/base/hand-authored.log {ACCEPT_URL}\n")
    for case in range(cases):
        k, guard, lane = exited_shard_lines(rng)
        logs = {"target/r1/hand-authored.log": "\n".join(lane),
                "target/base/hand-authored.log": "\n".join(REAL_HAND_BASE)}
        bad = checker.check(HAND + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert any("target/r1/hand-authored.log" in b for b in bad), (case, k, guard, bad)
    print(f"[tests-cover-diff] hand-authored exited shards: {cases} cases PASS")


STATIC_STEPS = frozenset({"gate-arch", "fmt"})
# The end of a red `gate-arch` log that names no red check and no failing test.
NO_RED_TAIL = [
    "test-select-tests-registry-data: 0 failure(s)", "Traceback (most recent call last):",
    '  File "./scripts/test-gate-integrate-tmpdir.py", line 169, in <module>', "    main()",
    "AssertionError: the job's TMPDIR outlived the job", "error: recipe `gate-arch` failed with exit code 1", ""]
# The scripts of `just cargo-free-checks` come first: their verdict lines precede the recipe's own.
SCRIPTS = ["admission-sites-count", "known-reds-fixture", "fmt-check"]
AGG, RUN_HEADER, REGISTRY, NEXTEST_CHECK = "cargo-free-checks", "run-step-header", "select-tests-registry-data", "architecture_rules"
CHECKS = [*SCRIPTS, AGG, "gate-arch-verdicts", RUN_HEADER, REGISTRY, NEXTEST_CHECK]
BUILD_ERROR_TAIL = ["error[E0425]: cannot find value `x` in this scope",
                    "error: could not compile `holon-architecture-tests` (test \"architecture_rules\") due to 1 previous error"]


def units(checks):
    """The checks that can be red on their own: the recipe's verdict only sums up its scripts."""
    return set(checks) - {AGG}


def check_verdicts(failed, noise=()):
    """The lines of the gate-arch recipe: each check's output (`noise` lines go to the first script that is no
    nextest check), then its verdict `FAIL <name> (exit N)` or `PASS <name>`. The recipe's verdict is red when
    one of its scripts is."""
    red = set(failed) | ({AGG} if set(SCRIPTS) & set(failed) else set())
    return [line for c in CHECKS if c != NEXTEST_CHECK
            for line in (*([f"stub {c} ran", *noise] if c == SCRIPTS[0] else []),
                         f"FAIL {c} (exit 1)" if c in red else f"PASS {c}")]

ARCH_TESTS = [f"holon-architecture-tests::architecture_rules {t}" for t in (
    "archlint_all_passes", "holon_frontend_does_not_depend_on_holon_loro",
    "latency_events_are_emitted_above_the_release_level_ceiling")]
STATIC_FIXED = {
    "a gate-arch citation with a valid base log": dict(kind="static", base_line=True, tree=True, lane=[ARCH_TESTS[0]],
                                                      base=ARCH_TESTS[:2], comparable=True, cited=True),
    "a step name neither the selector nor a gate prints": dict(kind="unknown", base_line=True, tree=True,
                                                               lane=[ARCH_TESTS[0]], base=ARCH_TESTS[:1],
                                                               comparable=True, cited=True),
    "a gate-arch base log on another tree": dict(kind="static", base_line=True, tree=False, lane=[ARCH_TESTS[0]],
                                                 base=ARCH_TESTS, comparable=True, cited=True),
    "a gate-arch red only in the lane": dict(kind="static", base_line=True, tree=True, lane=ARCH_TESTS[:2],
                                             base=ARCH_TESTS[:1], comparable=True, cited=True),
    "a gate-arch log with no red check and no failing test": dict(kind="static", base_line=True, tree=True, lane=[],
                                                                  base=[], comparable=False, cited=True),
    "a gate-arch check red only in the lane, nextest reds equal to base": dict(
        kind="static", base_line=True, tree=True, lane=ARCH_TESTS[:1], base=ARCH_TESTS[:1], lane_checks=[REGISTRY],
        base_checks=[], comparable=True, cited=True),
    "a gate-arch script check red in the lane and in base, nextest green": dict(
        kind="static", base_line=True, tree=True, lane=[], base=[], lane_checks=[REGISTRY], base_checks=[RUN_HEADER, REGISTRY, NEXTEST_CHECK],
        comparable=True, cited=True),
    "a gate-arch nextest check red with no test run, base red on a test": dict(
        kind="static", base_line=True, tree=True, lane=[], base=ARCH_TESTS[:1], lane_checks=[NEXTEST_CHECK],
        base_checks=[NEXTEST_CHECK], no_run=True, comparable=True, cited=True),
    "an indented verdict-shaped line of a test's output, no check red in the lane": dict(
        kind="static", base_line=True, tree=True, lane=ARCH_TESTS[:1], base=ARCH_TESTS[:1], lane_checks=[REGISTRY],
        base_checks=[REGISTRY], noise=AGG, comparable=True, cited=True),
    "a script red only in the lane next to a base red script of the same recipe": dict(
        kind="static", base_line=True, tree=True, lane=[], base=[], lane_checks=[SCRIPTS[0], SCRIPTS[1]],
        base_checks=[SCRIPTS[0]], comparable=True, cited=True),
    "the same script red in the lane and in base": dict(
        kind="static", base_line=True, tree=True, lane=[], base=[], lane_checks=[SCRIPTS[2]],
        base_checks=SCRIPTS[1:], comparable=True, cited=True),
    "the recipe's verdict red in the lane and in base, no script named": dict(
        kind="static", base_line=True, tree=True, lane=[], base=[], lane_checks=[AGG], base_checks=[AGG, SCRIPTS[0]],
        comparable=True, cited=True),
    "a Summary-shaped line in a script's output, the nextest check ran no test": dict(
        kind="static", base_line=True, tree=True, lane=[], base=ARCH_TESTS[:1], lane_checks=[SCRIPTS[0], NEXTEST_CHECK],
        base_checks=[SCRIPTS[0], NEXTEST_CHECK], no_run=True, summary_noise=True, comparable=True, cited=True),
    "a gate-arch base log of another step line": dict(kind="static", base_line=False, tree=True, lane=[ARCH_TESTS[0]],
                                                      base=ARCH_TESTS, comparable=True, cited=True),
    "a gate-arch citation without a cited lane log": dict(kind="static", base_line=True, tree=True,
                                                          lane=[ARCH_TESTS[0]], base=ARCH_TESTS, comparable=True,
                                                          cited=False),
}


def static_citation(rng):
    lane = rng.sample(ARCH_TESTS, rng.randint(1, 3))
    base = sorted(set(lane) - ({rng.choice(lane)} if rng.random() < 0.3 else set())
                  | set(rng.sample(ARCH_TESTS, rng.randint(0, 2))))
    lane_checks = rng.sample(CHECKS, rng.randint(0, 2))
    base_checks = sorted(set(lane_checks) - ({rng.choice(lane_checks)} if lane_checks and rng.random() < 0.3 else set())
                         | set(rng.sample(CHECKS, rng.randint(0, 2))))
    if lane_checks and rng.random() < 0.5:
        lane = []
    no_run = NEXTEST_CHECK in lane_checks and rng.random() < 0.5
    if no_run:
        lane = []
    noise = rng.choice([c for c in CHECKS if c not in lane_checks]) if rng.random() < 0.3 else None
    return dict(kind=rng.choice(["selected", "static", "unknown"]), base_line=rng.random() < 0.75,
                tree=rng.random() < 0.75, lane=lane, base=base, lane_checks=lane_checks, base_checks=base_checks,
                no_run=no_run, noise=noise, summary_noise=no_run and rng.random() < 0.5,
                comparable=rng.random() < 0.8, cited=rng.random() < 0.85)


def static_citation_logs(rng, c):
    """(selected text, summary, logs) of one `accepted-base-red:` citation of a lane red log."""
    step = {"selected": "nextest", "static": rng.choice(sorted(STATIC_STEPS)), "unknown": "gate-archive"}[c["kind"]]
    line = "nextest: --workspace -E 'package(holon-architecture-tests)'" if step == "nextest" else f"{step}: just {step}"
    selected = line + "\n" if c["kind"] == "selected" else "clippy: --workspace\n"
    def body(tests, checks, lane):
        if not c["comparable"]:
            return NO_RED_TAIL
        noise = [f"    FAIL {c['noise']} (exit 1)"] if lane and c.get("noise") else []
        noise += [nextest_end_line(rng, 14, 0)] if lane and c.get("summary_noise") else []
        verdict = lambda: f"FAIL {NEXTEST_CHECK} (exit 1)" if NEXTEST_CHECK in checks else f"PASS {NEXTEST_CHECK}"
        if lane and c.get("no_run"):
            return check_verdicts(checks, noise) + BUILD_ERROR_TAIL + [verdict()]
        return (check_verdicts(checks, noise) + [result_line(rng, t, rng.choice(STATUSES)) for t in tests]
                + [nextest_end_line(rng, 14, len(tests)), verdict()])
    def log(first, commit, tests, checks=(), lane=False):
        return "\n".join([first, "BASE: b", f"TREE: {commit}", "HOLON_PBT_TIMING=measure", "STARTED: 2026-11-14T08:03:10Z", *body(tests, checks, lane),
                          "exit: 1"])
    logs = {"target/r1/clippy.log": log("clippy: --workspace", "lane", []).replace("exit: 1", "exit: 0"),
            "target/base/step.log": log(line if c["base_line"] else line + " --profile other",
                                        "bc" if c["tree"] else "oc", c["base"], c.get("base_checks", []))}
    if c["cited"]:
        logs["target/r1/step.log"] = log(line, "lane", c["lane"], c.get("lane_checks", []), lane=True)
    summary = ("How tested:\n" + "".join(f"- {p}\n" for p in logs)
               + f"- accepted-base-red: {step} target/base/step.log {ACCEPT_URL}\nConfidence (1-5): 4\n")
    return selected, summary, logs


def static_step_citations(cases):
    """An `accepted-base-red:` line of a static gate step is accepted exactly when it would be for a selected step:
    the base log runs the lane log's line 1 on the base tree and every lane red is red there; a step name that
    neither the selector nor a gate prints is refused."""
    checker = load_checker()
    rng = random.Random(19)
    drawn = list(STATIC_FIXED.items()) + [(f"case {i}", static_citation(rng)) for i in range(cases)]
    kinds = Counter()
    wrong = []
    for name, c in drawn:
        selected, summary, logs = static_citation_logs(rng, c)
        bad = checker.check(selected, summary, logs, set(), "b", tree_of={"b": "T", "bc": "T", "oc": "U"}.get,
                            static=STATIC_STEPS)
        accepted = (c["kind"] != "unknown" and c["base_line"] and c["tree"] and c["comparable"] and c["cited"]
                    and set(c["lane"]) <= set(c["base"])
                    and units(c.get("lane_checks", [])) <= units(c.get("base_checks", []))
                    and bool(c["lane"] or units(c.get("lane_checks", []))) and not c.get("no_run"))
        if (bad == []) != accepted:
            wrong.append(f"{name}: {'accepted' if accepted else 'refused'} expected, got {bad or 'accepted'}")
            continue
        kinds[f"{c['kind']} {'accepted' if accepted else 'refused'}"] += 1
        assert c["kind"] != "unknown" or any("which the selector does not print" in b for b in bad), (name, bad)
        compared = c["kind"] != "unknown" and c["cited"] and c["base_line"] and c["tree"]
        assert c["comparable"] or not compared or any("cannot compare" in b for b in bad), (name, bad)
        extra = units(c.get("lane_checks", [])) - units(c.get("base_checks", []))
        assert not (compared and c["comparable"] and extra and not c.get("no_run")) or all(
            e in " ".join(bad) for e in extra), (name, extra, bad)
        if name in STATIC_FIXED:
            print(f"[tests-cover-diff] {name}: {'accepted' if accepted else 'refused'} as expected")
    assert not wrong, "static step citations judged wrongly:\n" + "\n".join(f"FAIL case {w}" for w in wrong)
    assert all(kinds[k] for k in ("selected accepted", "selected refused", "static accepted", "static refused",
                                  "unknown refused")), kinds
    print(f"[tests-cover-diff] static step citations: {cases} cases PASS; per step kind {dict(kinds)}")


def static_steps_fail_loud():
    """A gate script whose step list cannot be read stops the check instead of leaving every static step unknown."""
    checker = load_checker()
    def no_steps(root, gate):
        raise ValueError(f"{gate} --steps printed no step name")
    selector = checker.selector_module()
    real, selector.gate_step_names = selector.gate_step_names, no_steps
    try:
        checker.static_steps()
    except ValueError:
        print("[tests-cover-diff] an unreadable gate step list: raises as expected")
        return
    finally:
        selector.gate_step_names = real
    raise AssertionError("static_steps returned although no gate step list could be read")


def killed_step_lines(rng):
    """A sharded lane run of the real lines whose recipe was cut while it waited for shard k: the earlier shards'
    output stands, the later shards and the `HAND-AUTHORED RED:` block never print, and the step exits with a signal
    or a failed write."""
    heads = [i for i, line in enumerate(REAL_HAND_LANE) if line.startswith("== hand-authored shard ")]
    end = next(i for i, line in enumerate(REAL_HAND_LANE) if line.startswith("HAND-AUTHORED RED:"))
    cut = rng.randrange(heads[1], end)
    rc = rng.choice([1, 130, 137, 143])
    return cut, rc, REAL_HAND_LANE[:cut] + ["", f"exit: {rc}"]


def hand_killed_steps(cases):
    """A sharded hand-authored run cut before all its shards reported hides the reds of the shards it never read:
    the check never accepts it, whatever case reds the base shares."""
    checker = load_checker()
    rng = random.Random(13)
    summary = ("How tested:\n- target/r1/hand-authored.log\n- target/base/hand-authored.log\n"
               f"- accepted-base-red: hand-authored target/base/hand-authored.log {ACCEPT_URL}\n")
    for case in range(cases):
        cut, rc, lane = killed_step_lines(rng)
        logs = {"target/r1/hand-authored.log": "\n".join(lane),
                "target/base/hand-authored.log": "\n".join(REAL_HAND_BASE)}
        bad = checker.check(HAND + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert any("target/r1/hand-authored.log" in b for b in bad), (case, cut, rc, bad)
    print(f"[tests-cover-diff] hand-authored killed steps: {cases} cases PASS")


def windowed_verdicts(cases):
    """Lane and base windowed gate logs whose final verdict lines carry any failing verdict of
    scripts/windowed-gate.py (RED, HUNG): a lane failing module the base log does not fail is never accepted, nor
    a lane gate killed before its `windowed-gate: wall=` end line, whose remaining units have no verdict; a
    complete lane gate whose failing modules all fail on the base is accepted."""
    checker = load_checker()
    rng = random.Random(3)
    modules = sorted({module(t) for t in ACTION_BAR + LOOP} | {"holon-gpui::main undo_survives_blur_windowed"})
    for case in range(cases):
        lane = {m: rng.choice(["RED", "HUNG"]) for m in rng.sample(modules, rng.randint(1, len(modules)))}
        base = {m: rng.choice(["RED", "HUNG"]) for m in rng.sample(modules, rng.randint(0, len(modules)))}
        killed = rng.random() < 0.3
        def gate_log(verdicts, killed=False):
            return "\n".join([WINDOWED, *HEADER.splitlines(), WINDOWED_GATE_RED[0],
                              *(f"  FAIL: {v} {m} (log target/gate-logs/windowed/m.rerun.log)"
                                for m, v in sorted(verdicts.items())), *[stamp(rng) + WINDOWED_GATE_RED[-1]] * (not killed), "",
                              f"exit: {rng.choice([137, 143]) if killed else 1}"])
        logs = {"target/r1/windowed.log": gate_log(lane, killed), "target/base/windowed.log": gate_log(base)}
        summary = ("How tested:\n- target/r1/windowed.log\n- target/base/windowed.log\n"
                   f"- accepted-base-red: windowed target/base/windowed.log {ACCEPT_URL}\n")
        bad = checker.check(WINDOWED + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        if only := sorted(set(lane) - set(base)):
            assert bad, f"case {case}: lane modules {only} fail only in the lane ({lane}), base {base}: accepted"
        assert bad or not killed, f"case {case}: a gate killed before its end line is accepted ({lane}, {base})"
        assert not bad or only or killed, (f"case {case}: a complete gate run, every lane module red on the base, "
                                           f"is refused: {bad}")
    print(f"[tests-cover-diff] windowed verdicts: {cases} cases PASS")


def stopped_runs(cases):
    """A lane nextest log that scripts/lane-nextest.py stopped at its first unlisted red ran only part of its
    tests, as did a run killed before nextest printed its summary: its red set says nothing about the tests it
    never ran, so a base red set never makes it a run."""
    checker = load_checker()
    rng = random.Random(4)
    for case in range(cases):
        red = rng.sample(ACTION_BAR + LOOP, rng.randint(1, 4))
        base = sorted(set(red) | set(rng.sample(ACTION_BAR + LOOP, rng.randint(0, 3))))
        total = rng.randint(100, 9000)
        ran = rng.randint(len(red), total - 1)
        killed = rng.random() < 0.3
        def nextest_log(tests, stop):
            lines = [result_line(rng, t, rng.choice(STATUSES)) for t in tests]
            if stop and killed:
                lines += [f"  Cancelling due to signal: {rng.randint(1, 8)} tests still running"] * (rng.random() < 0.5)
            elif stop:
                lines += [f"  Cancelling due to signal: {rng.randint(1, 8)} tests still running",
                          nextest_end_line(rng, ran - len(tests), len(tests), total),
                          f"[fail-fast] stopped at {tests[-1]}: a red outside the known-reds list "
                          "(target/lane-nextest/verdict.log)"]
            return "\n".join([NARROW, *HEADER.splitlines(), *lines, "", "exit: 1"])
        logs = {"target/r1/nextest.log": nextest_log(red, True),
                "target/base/nextest.log": nextest_log(base, rng.random() < 0.5)}
        summary = ("How tested:\n- target/r1/nextest.log\n- target/base/nextest.log\n"
                   f"- accepted-base-red: nextest target/base/nextest.log {ACCEPT_URL}\n")
        bad = checker.check(NARROW + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert bad, f"case {case}: a lane run stopped after {ran}/{total} tests is accepted as run"
    print(f"[tests-cover-diff] stopped runs: {cases} cases PASS")


def complete_runs(cases):
    """scripts/lane-nextest.py prints `[fail-fast] stopped at <red>` also after a run in which every test ran, when
    its last unlisted red reports just before the summary: that log is a full run, so a base log that fails every
    lane red makes the step a run."""
    checker = load_checker()
    rng = random.Random(5)
    for case in range(cases):
        red = rng.sample(ACTION_BAR + LOOP, rng.randint(1, 4))
        base = sorted(set(red) | set(rng.sample(ACTION_BAR + LOOP, rng.randint(0, 3))))
        passed = rng.randint(0, 5000)
        lines = [result_line(rng, t, rng.choice(STATUSES)) for t in red]
        lines += [nextest_end_line(rng, passed, len(red)),
                  f"[fail-fast] stopped at {red[-1]}: a red outside the known-reds list "
                  "(target/lane-nextest/verdict.log)",
                  f"[fail-fast] red list: target/lane-nextest/reds.txt ({len(red)} tests)"]
        logs = {"target/r1/nextest.log": "\n".join([NARROW, *HEADER.splitlines(), *lines, "", "exit: 1"]),
                "target/base/nextest.log": "\n".join([NARROW, *HEADER.splitlines(),
                                                      *(result_line(rng, t, "FAIL") for t in base), "", "exit: 100"])}
        summary = ("How tested:\n- target/r1/nextest.log\n- target/base/nextest.log\n"
                   f"- accepted-base-red: nextest target/base/nextest.log {ACCEPT_URL}\n")
        bad = checker.check(NARROW + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert not bad, f"case {case}: a complete lane run of {passed + len(red)} tests, every red also red on the base, is refused: {bad}"
    print(f"[tests-cover-diff] complete runs: {cases} cases PASS")


def nextest_summary_line(rng, passed, failed):
    """nextest's end-of-run line in its own grammar: `1 test run` for one test, `N tests run` otherwise."""
    ran = passed + failed
    return (f"{' ' * rng.randint(1, 8)}Summary [{rng.uniform(0, 2000):>8.3f}s] {ran} test{'s' * (ran != 1)} run: "
            f"{passed} passed, {failed} failed, {rng.randint(0, 9000)} skipped")


def complete_runs_of_any_size(cases):
    """A complete lane nextest run of any size, every red also red on the base, is accepted: the end line counts
    one test as nextest prints it."""
    checker = load_checker()
    rng = random.Random(14)
    summary = ("How tested:\n- target/r1/nextest.log\n- target/base/nextest.log\n"
               f"- accepted-base-red: nextest target/base/nextest.log {ACCEPT_URL}\n")
    for case in range(cases):
        red = rng.sample(ACTION_BAR + LOOP, rng.randint(1, 3))
        passed = rng.choice([0, 0, 1, rng.randint(2, 5000)])
        def log(tests, ok):
            return "\n".join([NARROW, *HEADER.splitlines(), *(result_line(rng, t, "FAIL") for t in tests),
                              nextest_summary_line(rng, ok, len(tests)), "", "exit: 100"])
        logs = {"target/r1/nextest.log": log(red, passed), "target/base/nextest.log": log(red, passed)}
        bad = checker.check(NARROW + "\n", summary, logs, set(), "b", tree_of=lambda c: "t")
        assert not bad, (case, len(red), passed, bad)
    print(f"[tests-cover-diff] complete runs of any size: {cases} cases PASS")


CHROME = "holon-gpui::main chrome_one_row_windowed"
GHERKIN = "holon-gpui::gpui_gherkin_replay"


def red_unit(unit, first=("present", "ok"), rerun=("present", "ok")):
    logs = unit_logs(unit, {"verdict": "RED"})
    return {"verdict": "RED", "logs": {logs[0]: first, logs[1]: rerun}}


def windowed_step(units, off=0, classifier=1, mode="measure"):
    """A red windowed step whose step-log classifier finds no panic in it (exit 1), as for every real gate log."""
    o = {"kind": "red", "red": [], "nextest": False, "units": units, "off": off, "mode": mode}
    return {**o, "classifier": classifier} if classifier is not None else o


WINDOWED_FIXED = {
    "one red unit with a rerun, both logs known reds": windowed_step(
        {UNDO: red_unit(UNDO), CHROME: {"verdict": "FLAKY"}}),
    "one red unit with a rerun, its first-run log not classified": windowed_step(
        {UNDO: red_unit(UNDO, first=("present", "none"))}),
    "a hung unit beside a red unit that is a known red": windowed_step(
        {UNDO: red_unit(UNDO), CHROME: {"verdict": "HUNG"}}),
    "a novel red: the rerun log classified exit 1": windowed_step({UNDO: red_unit(UNDO, rerun=("present", "red"))}),
    "a missing unit log": windowed_step({UNDO: red_unit(UNDO, first=("missing", "ok"))}),
    "a red unit of a binary that is no main module": windowed_step({GHERKIN: red_unit(GHERKIN)}, classifier=None),
    "a red step with an end line but no FAIL line": windowed_step({CHROME: {"verdict": "FLAKY"}}),
    "a red module unit only, no flaky one": windowed_step({CHROME: red_unit(CHROME)}),
    "a classifier log of another unit only": windowed_step(
        {UNDO: red_unit(UNDO), CHROME: red_unit(CHROME, first=("present", "none"), rerun=("present", "none"))}),
    "a unit log changed after the gate hashed it": windowed_step({UNDO: red_unit(UNDO, rerun=("changed", "ok"))}),
    "a unit log without its unit-log line": windowed_step({UNDO: red_unit(UNDO, first=("unlisted", "ok"))}),
    "a unit log classified twice, exit 0 then exit 1": windowed_step({UNDO: red_unit(UNDO, rerun=("present", "ok+red"))}),
    "a unit classifier log run on another tree": windowed_step({UNDO: red_unit(UNDO, rerun=("present", "stale"))}),
    "an end line that counts one red unit more": windowed_step({UNDO: red_unit(UNDO)}, off=1),
    "a red windowed step run in enforce mode, every unit a known red": windowed_step(
        {UNDO: red_unit(UNDO)}, mode="enforce"),
    "a red windowed step with no mode line, every unit a known red": windowed_step({UNDO: red_unit(UNDO)}, mode=None),
    "a red windowed step run in enforce mode, its step log classified exit 0": windowed_step(
        {UNDO: red_unit(UNDO)}, classifier=0, mode="enforce"),
    "a red unit without a rerun line, its one log a known red": windowed_step(
        {UNDO: {"verdict": "RED", "rerun": False, "logs": {unit_logs(UNDO, {"verdict": "RED", "rerun": False})[0]:
                                                             ("present", "ok")}}}),
}
# The words of each refusal of the windowed red path; a case with no refusal is accepted.
VERDICT_CLASSES = {("without the header line",): "mode", ("FAIL lines",): "count", ("is HUNG",): "hung", ("does not exist",): "missing",
                   ("changed after",): "changed", ("no unit-log line",): "unlisted",
                   ("no cited classifier",): "uncited",
                   ("of unit", "`exit: 1`"): "novel", ("of unit", "checked tree"): "stale"}


def windowed_outcome(rng):
    failing = rng.sample(UNIT_POOL, rng.randint(0, 3))
    hung = set(rng.sample(failing, rng.randint(0, len(failing)))) if rng.random() < 0.2 else ()
    units = gate_units(rng, failing, hung)
    if not units:
        units = {CHROME: {"verdict": "PASS"}}
    red = any(s["verdict"] == "RED" for s in units.values())
    return windowed_step(units, rng.choice([0] * 8 + [1] + [-1] * red), rng.choice([None, 0, 1, 1, 3]),
                         rng.choice(["measure"] * 9 + ["enforce", "", None]))


def windowed_units(cases):
    """A red windowed step counts as run iff the end line's RED and HUNG counts equal its FAIL lines (at least one),
    no unit is HUNG, and every log of every RED unit exists, matches its `unit-log:` hash and is classified exit 0 on
    the checked tree; the reference refusals come from the drawn state."""
    rng = random.Random(15)
    classes = {name: 0 for name in ["accept", *VERDICT_CLASSES.values()]}
    with tempfile.TemporaryDirectory() as repo:
        repo = Path(repo)
        commits = git_repo(repo)
        drawn = list(WINDOWED_FIXED.items()) + [(f"windowed case {i}", windowed_outcome(rng)) for i in range(cases)]
        cites = Counter(cite for _, o in drawn for s in o["units"].values() for _, cite in s.get("logs", {}).values())
        assert all(cites[c] for c in UNIT_CITES), f"a unit citation state no RED unit log drew: {cites}"
        for i, (name, o) in enumerate(drawn):
            r, want = run_lane(repo / f"lane{i}", {WINDOWED: o}, rng, commits)
            try:
                judge(name, r, want)
            except AssertionError:
                print(f"[tests-cover-diff] {name}: drawn state {o}", file=sys.stderr)
                raise
            hit = {c for words, c in VERDICT_CLASSES.items() for needles in want
                   if all(w in " ".join(needles) for w in words)}
            for c in hit or {"accept"}:
                classes[c] += 1
            if name in WINDOWED_FIXED:
                print(f"[tests-cover-diff] {name}: {'PASS' if not want else 'FAIL'} as expected")
    assert all(classes.values()), f"a verdict class no case reached: {classes}"
    print(f"[tests-cover-diff] windowed units: {cases} cases PASS; cases per verdict class {classes}; unit logs per "
          f"citation state {dict(cites)}")


REAL_WINDOWED = sorted(Path(__file__).resolve().parent.glob("fixtures/windowed-*.txt"))


def real_windowed_logs():
    """The red windowed gate step logs of real lanes: the FAIL lines read as many units as the end line's RED and
    HUNG counts, and each red unit's rerun line names its two logs, the second the one its FAIL line names."""
    checker = load_checker()
    assert len(REAL_WINDOWED) == 3, REAL_WINDOWED
    for f in REAL_WINDOWED:
        lines = f.read_text().splitlines()
        fails, (red, hung) = checker.gate_failures(lines), checker.gate_counts(lines)
        assert len(fails) == red + hung and fails, (f.name, fails, red, hung)
        assert checker.timing_mode(lines) == "measure", f.name
        reruns = checker.gate_reruns(lines)
        assert all(reruns[unit][1] == final for _, unit, final in fails), (f.name, fails, reruns)
        print(f"[tests-cover-diff] real windowed log {f.name}: {len(fails)} FAIL lines = RED={red} HUNG={hung}")


REGISTRY = "docs/Testing/KeystoneKnownReds.md"
EVIDENCE = "docs/Testing/known-red-evidence/a-row/keystone-smoke-gate.log.zst"
FEATUREMAP = "docs/Architecture/FeatureMap.md"
DATA = (REGISTRY, EVIDENCE, FEATUREMAP)
CRATE_SRC, CRATE_TESTS, BUILD = ("crates/holon-toon/src/lib.rs", "frontends/gpui/tests/accordion_bounded_pbt.rs",
                                 "crates/holon-toon/Cargo.toml")
CODE = (CRATE_SRC, CRATE_TESTS, BUILD)
# The stub prints the real selection of this tree for the paths of its `--paths-from` file, else of its jj diff.
CHAIN_STUB = """import json, subprocess, sys
a = sys.argv[1:]
if "--paths-from" in a:
    paths = open(a[a.index("--paths-from") + 1]).read().splitlines()
else:
    paths = subprocess.run(["jj", "diff", "--name-only", "--from", a[a.index("--base") + 1], "--to", "@"],
                           capture_output=True, text=True, check=True).stdout.splitlines()
table = json.load(open("selections.json"))
key = "\\n".join(sorted(set(paths)))
if key not in table:
    sys.exit(f"stub selector: no selection for {key!r}")
sys.stdout.write(%r + "".join(line + "\\n" for line in table[key]))
""" % HEADER
# (name, chain), run before the drawn chains.
CHAINS = [
    ("code in gpui tests, then a rows change", [[CRATE_TESTS], [REGISTRY, EVIDENCE]]),
    ("crate code, then a rows change: two nextest lines", [[CRATE_SRC], [REGISTRY, EVIDENCE, FEATUREMAP]]),
    ("crate code, then a change of the registry and gpui tests", [[CRATE_SRC], [REGISTRY, CRATE_TESTS]]),
    ("a rows change, then a change of the registry and gpui tests", [[REGISTRY, EVIDENCE], [REGISTRY, CRATE_TESTS]]),
    ("evidence without the registry, then a rows change", [[EVIDENCE], [REGISTRY]]),
    ("rows changes only", [[REGISTRY], [REGISTRY, FEATUREMAP]]),
]


@functools.cache
def real_selection():
    """{sorted paths joined by newlines: the selection of this tree} for every set of pool paths."""
    st = load_module("select_tests_real", REPO / "scripts/select-tests.py")
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO,
                                     check=True, capture_output=True, text=True).stdout)
    pool = sorted({*DATA, *CODE})
    table = {}
    with contextlib.redirect_stderr(io.StringIO()):
        for k in range(len(pool) + 1):
            for paths in itertools.combinations(pool, k):
                table["\n".join(paths)] = st.selection(REPO, meta, list(paths))
    return table


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def selected_for(paths):
    return set(real_selection()["\n".join(sorted(set(paths)))])


def rows_only(change):
    return REGISTRY in change and all(p in DATA for p in change)


def required(chain):
    """The reference: the lines of the code changes' paths plus those of the rows-only changes' paths, one
    selection of all paths when either side is empty."""
    rows = {p for c in chain if rows_only(c) for p in c}
    code = {p for c in chain if not rows_only(c) for p in c}
    if rows and code:
        return selected_for(code) | selected_for(rows)
    return selected_for(rows | code)


def chain_repo(root, chain):
    """A colocated repo whose base holds the stub selector and whose changes after it are `chain`; every change
    writes new text, so the diff from the base to @ is the union of the chain's paths. Returns the base and the
    change id and commit id of the last change. The returned base is a landing on the base that the chain does not
    contain, so the checker must read the chain's own changes from the fork point."""
    env = {**os.environ, "JJ_USER": "t", "JJ_EMAIL": "t@t"}

    def run(*args):
        return subprocess.run(args, cwd=root, env=env, check=True, capture_output=True, text=True).stdout.strip()
    (root / "scripts").mkdir(parents=True)
    (root / "scripts/select-tests.py").write_text(CHAIN_STUB)
    (root / "selections.json").write_text(json.dumps(real_selection()))
    (root / ".gitignore").write_text("target/\n")
    run("git", "init", "-q")
    run("git", "-c", "user.email=t@t", "-c", "user.name=t", "add", ".")
    run("git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base")
    base = run("git", "rev-parse", "HEAD")
    run("jj", "git", "init", "--colocate")
    for i, change in enumerate(chain):
        for path in change:
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(f"change {i}\n")
        run("jj", "commit", "-m", f"change {i}")
    at = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id")
    run("jj", "new", base, "-m", "landed")
    (root / "landed.txt").write_text("landed\n")
    moved = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id")
    run("jj", "edit", at)
    return moved, *run("jj", "log", "--no-graph", "-r", "@-", "-T", 'change_id ++ " " ++ commit_id').split()


def cover(root, base, change, commit, lines):
    """Run the checker with one exit-0 run-step log per line of `lines`, each also cited as a span."""
    if (root / "target").exists():
        shutil.rmtree(root / "target")
    logs = []
    for i, line in enumerate(sorted(lines)):
        path = f"target/c/{i}.log"
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(step_log(line))
        logs.append(path)
    summary = root / "target/summary.md"
    summary.write_text(f"# s\n\nCommit: {change}\n\nHow tested:\n" + "".join(f"- log: {p}\n" for p in logs)
                       + "".join(f"- line `{line}`\n" for line in sorted(lines))
                       + "\nConfidence (1-5) that the verifier will confirm: 4\n")
    return subprocess.run([sys.executable, str(CHECKER), "--base", base, "--summary", str(summary), "--tree", commit],
                          cwd=root, capture_output=True, text=True)


def draw_change(rng):
    kind = rng.choice(["rows", "mixed", "code"])
    data = [REGISTRY, *rng.sample(DATA[1:], rng.randint(0, 2))]
    if kind == "rows":
        return data
    if kind == "mixed":
        return data + rng.sample(CODE, rng.randint(1, 2))
    return rng.sample([*CODE, *DATA[1:]], rng.randint(1, 3))


def regrouped(rng, chain):
    """`chain` reordered, with one change split in two parts of its own class: the same paths per class."""
    chain = [list(c) for c in chain]
    i = rng.randrange(len(chain))
    change = chain[i]
    if len(change) > 1:
        cut = rng.randint(1, len(change) - 1)
        parts = [change[:cut], change[cut:]]
        if rows_only(change):
            parts = [sorted({REGISTRY, *part}) for part in parts]
        if all(rows_only(part) == rows_only(change) for part in parts):
            chain[i:i + 1] = parts
    rng.shuffle(chain)
    return chain


def per_change(cases):
    """The lines the checker requires for a lane chain equal the reference `required`: a run citing exactly those
    lines passes, a run missing any one of them fails naming its step; a reordered chain with a split change, which
    keeps the paths of each class, requires the same lines."""
    rng = random.Random(16)
    not_run = load_checker().not_run_steps()
    drawn = list(CHAINS) + [(f"chain {i}", [draw_change(rng) for _ in range(rng.randint(1, 5))])
                            for i in range(cases)]
    both = two_lines = 0
    for n, (name, chain) in enumerate(drawn):
        want = required(chain)
        rows = [c for c in chain if rows_only(c)]
        both += bool(rows) and len(rows) < len(chain)
        if rows and len(rows) < len(chain):
            registry_lines = selected_for({p for c in rows for p in c})
            assert not any(line.startswith(("hand-authored:", "timing:")) for line in registry_lines), \
                f"{name}: the rows changes {rows} select {sorted(registry_lines)}"
        steps = Counter(line.split(":")[0] for line in want)
        two_lines += any(k > 1 for k in steps.values())
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            base, change, commit = chain_repo(root, chain)
            r = cover(root, base, change, commit, want)
            assert r.returncode == 0, f"{name} {chain}: the reference lines {sorted(want)} fail:\n{r.stderr}"
            runnable = sorted(line for line in want if line.split(":")[0] not in not_run)
            dropped = runnable if n < len(CHAINS) else [rng.choice(
                [line for line in runnable if steps[line.split(":")[0]] > 1] or runnable)]
            for line in dropped:
                step = line.split(":")[0]
                r = cover(root, base, change, commit, want - {line})
                assert r.returncode == 1 and any(f"no cited run-step log ran the selected step `{step}`" in b
                                                 for b in r.stderr.splitlines()), \
                    f"{name} {chain}: without the log of `{line}`: exit {r.returncode}:\n{r.stderr}"
        other = regrouped(rng, chain)
        assert required(other) == want, f"{name}: the reference differs for {chain} and {other}"
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            base, change, commit = chain_repo(root, other)
            r = cover(root, base, change, commit, want)
            assert r.returncode == 0, f"{name}: {other}, regrouped from {chain}, fails:\n{r.stderr}"
        print(f"[tests-cover-diff] per change: {name}: PASS")
    assert both >= len(drawn) // 4, f"only {both} of {len(drawn)} chains hold both rows-only and other changes"
    assert two_lines >= len(drawn) // 8, f"only {two_lines} of {len(drawn)} chains require one step on two lines"
    print(f"[tests-cover-diff] per change: {len(drawn)} chains PASS; {both} with rows-only and other changes, "
          f"{two_lines} with a step on two lines")


if __name__ == "__main__":
    fixtures()
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    generated(cases)
    citations(cases)
    timing_modes(cases)
    result_pattern()
    stopped_runs(cases)
    complete_runs(cases)
    windowed_verdicts(cases)
    real_windowed_logs()
    windowed_units(60)
    outcomes(cases // 3)
    real_hand_lines()
    hand_lane_only_tests(cases // 3)
    hand_died_shards(cases // 3)
    hand_exited_shards(cases // 3)
    hand_killed_steps(cases // 3)
    static_step_citations(cases)
    static_steps_fail_loud()
    complete_runs_of_any_size(cases // 3)
    per_change(cases // 10)
