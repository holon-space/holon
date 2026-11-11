#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/tests-cover-diff.py.

Run: `python3 scripts/flow-checks/test-tests-cover-diff.py [cases]`. Exits non-zero on the first violation.
Each fixture is a lane root in a temp dir whose `scripts/select-tests.py` prints a fixed selection.
fixtures/run-step-clippy.txt is a log scripts/flow-checks/run-step.sh wrote for a real lane.
Every lane root sits in a git repo, so a log's `TREE:` commit resolves to a real source tree; the checked tree is
`--tree` of the repo's child commit, whose tree is the base's.
fixtures/windowed-*.txt are red windowed gate step logs of real lanes (the ledger path's home folder shortened to ~).
"""
import hashlib
import importlib.util
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().parent / "tests-cover-diff.py"
REAL_LOG = Path(__file__).resolve().parent / "fixtures/run-step-clippy.txt"
# run() writes the repo's base commit for TREE_ID and its commit of another tree for OTHER_TREE_ID.
TREE_ID, OTHER_TREE_ID = "e5d2c227" + "a" * 32, "0bad" + "f" * 36
HEADER = f"BASE: 3936d83aba008d69cc09e73d1c5db883ed6b5ae4\nTREE: {TREE_ID}\n"
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
        base, child, other = git_repo(root)
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
        summary.write_text("# lane change summary\n\nHow tested:\n"
                           + "".join(f"- Green log: {p.replace('<root>', str(root))} (`Summary [ 1s]`)\n"
                                     for p in (cites if cites is not None else [*logs, *ran]))
                           + "".join(f"- selector line `{s}`\n" for s in spans)
                           + "\nConfidence (1-5) that the verifier will confirm: 4\n")
        return subprocess.run([sys.executable, str(CHECKER), "--base", "3936d83", "--summary", str(summary),
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
    spec = importlib.util.spec_from_file_location("tests_cover_diff", CHECKER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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
        assert check(stdout + selected[step] + "\n", spans, logs, not_run), f"case {case}: step printed twice passes"
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
# Text the shard's other output writes into the same line under --nocapture; the recipe reads the name after it.
INTERLEAVED = ["", "Applying transition 2/6: ", "[inv-sql-budget] UndoLastMutation: not judged] "]


def real_hand_log(tree, first_skip):
    """The step line, header, shard, case, failing-test and HAND-AUTHORED RED lines of a real hand-authored run
    whose cases ran red on the base too, with each red shard's libtest `failures:` list; `first_skip` is its shard 0
    skip line, which the lane run interleaved."""
    skip = ' via HOLON_HAND_AUTHORED_SKIP (known issue — see docs/Testing/BugFunnel.md)'
    shard = "target/gate-logs/pbt-hand-authored-shard-{}.log"
    keystone_failed = [f"test {KEYSTONE} ... FAILED", "", "failures:", "", "failures:", f"    {KEYSTONE}", ""]
    return [
        HAND, "BASE: d80bb20cbbabd282d19d78af6aafb5eb93d26c96", f"TREE: {tree}", "STARTED: 2026-11-09T15:44:25Z",
        f"== hand-authored shard 0 (exit 101): {shard.format(0)} ==", first_skip,
        f'[hand-authored regression] RED case "{WBS}"', *keystone_failed,
        f"== hand-authored shard 1 (exit 0): {shard.format(1)} ==",
        f'[hand-authored regression] SKIPPED case "reboot-orphans-the-previous-boots-watchers"{skip}',
        f"== hand-authored shard 2 (exit 101): {shard.format(2)} ==",
        f'[hand-authored regression] SKIPPED case "a-failed-create-cancels-its-keystrokes-with-one-disclosure-loro-dispatch"{skip}',
        f'[hand-authored regression] RED case "{HM4_EDIT}"', f'[hand-authored regression] RED case "{CRASH_COPY}"',
        *keystone_failed,
        f"== hand-authored shard 3 (exit 101): {shard.format(3)} ==",
        f'[hand-authored regression] SKIPPED case "split-then-two-backspaces-caret-follows-second-join"{skip}',
        f'[hand-authored regression] SKIPPED case "external-rewrite-keeps-a-promoted-pages-body"{skip}',
        f'[hand-authored regression] RED case "{HM4_IDS}"', *keystone_failed,
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
UNIT_CITES = ["ok"] * 5 + ["red", "none", "stale"]
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


def unit_judge(log):
    return f"target/r1/unit-{Path(log).name}-known-reds.log"


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
    leaves only the signal's exit. A sharded run's recipe may be cut while it waits for shard k >= 1, so shards k..
    and the `HAND-AUTHORED RED:` block never print, or after its block: `cut` is then (k, the step's exit), k = the
    shard count for a cut after the block."""
    shards = shards or rng.choice([4, 4, 4, 1])
    cases = rng.sample(cases, len(cases))
    spec = [{"cases": cases[k::shards], "tests": [KEYSTONE] if cases[k::shards] else []} for k in range(shards)]
    if rng.random() < 0.25:
        rng.choice(spec)["tests"].append(rng.choice([MODULE_TEST, KEYSTONE]))
    for shard in spec:
        if shard["tests"] and rng.random() < 0.15:
            sig = rng.choice(list(SIGNALS))
            shard["died"] = rng.choice([(101, sig), (sig, sig)])
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
        lines += skips if k == 0 else []
        lines += [f'{rng.choice(INTERLEAVED)}[hand-authored regression] RED case "{c}"' for c in shard["cases"]]
        # A shard process that dies leaves only the intact result lines of the tests that failed before; the test that
        # killed it writes none.
        died = "died" in shard
        for t in shard["tests"]:
            should_panic = t != KEYSTONE and rng.random() < 0.5
            lines += ([f"test {t}{' - should panic' if should_panic else ''} ... FAILED"] if died
                      else libtest_failure_lines(rng, t, should_panic)[:-4])
        if died and rc == 101:
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
    """Three commits: the base, an empty child with the base's tree, and a commit with another tree."""
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
    return base, child, git("rev-parse", "HEAD")


def lane_files(rng, spec, commits):
    """The cited files, the How-tested lines and the reference problems of one lane spec.

    `spec` maps each selected step line to its outcome. A reference problem is the tuple of needles its stderr line
    holds; it is derived from the outcome alone, not from the checker's reading of the logs."""
    base, child, other = commits
    files, cites, extra, want = {}, [], [], []

    def log(path, line, body, tail, tree=base):
        files[path] = f"{line}\nBASE: {base}\nTREE: {tree}\nSTARTED: 2026-11-09T05:15:49Z\n" + "".join(
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
        log(path, line, body, f"exit: {hand_exit(o['hand']) if 'hand' in o else rng.choice([1, 100, 101])}")
        judge = o.get("classifier")
        stale = judge == 0 and not judged_on_tree(o)
        if judge is not None:
            log(f"target/r1/{step}-known-reds.log", f"known-reds: scripts/keystone-known-reds.sh {path}", [],
                f"exit: {judge}", tree=other if stale else base)
        if judge == 0 and not stale:
            continue
        base_path = f"target/base/{step}.log"
        if "base" in o or "base_hand" in o:
            extra.append(f"- accepted-base-red: {step} {base_path} {o.get('acceptance', ACCEPT_URL)}")
            if "base_hand" in o:
                base_body = hand_lines(rng, o["base_hand"])
            elif o.get("base_nextest", True):
                base_body = results(o["base"])
            else:
                base_body = gate_lines(failing_units(o["base"], o.get("base_hung", ())))
            log(base_path, o.get("base_line", line), base_body, "exit: 100",
                tree={"base": base, "child": child, "other": other}[o.get("tree", "child")])
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
            want.append((f"target/r1/{step}-known-reds.log classifies {path} of step", f"`exit: {judge}`"))
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
            if cite != "none":
                log(unit_judge(path), f"known-reds: scripts/keystone-known-reds.sh {path}", [],
                    f"exit: {1 if cite == 'red' else 0}", tree=trees["other" if cite == "stale" else "base"])
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
    return sum(1 for o in spec.values() if o["kind"] == "red" and not judged_on_tree(o) and "base" in o
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
    summary.write_text("# s\n\nHow tested:\n" + prose + "".join(f"- log: {p} (`x`)\n" for p in cites)
                       + "\n".join(extra)
                       + "\n\nConfidence (1-5) that the verifier will confirm: 4\n")
    r = subprocess.run([sys.executable, str(CHECKER), "--base", commits[0], "--summary", str(summary),
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
    if (c := rng.choice([None, 0, 1, 3])) is not None:
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
    if (c := rng.choice([None, 0, 1, 3])) is not None:
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
        for i, (name, spec) in enumerate(drawn):
            r, want = run_lane(repo / f"lane{i}", spec, rng, commits)
            judge(name, r, want)
            notes = r.stdout.count("compared at module level")
            assert notes == module_notes(spec), f"{name}: {notes} module notes, want {module_notes(spec)}:\n{r.stdout}"
            if name in STATIC:
                print(f"[tests-cover-diff] {name}: {'PASS' if not want else 'FAIL'} as expected")
    print(f"[tests-cover-diff] outcomes: {cases} cases PASS")


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


def windowed_step(units, off=0, classifier=1):
    """A red windowed step whose step-log classifier finds no panic in it (exit 1), as for every real gate log."""
    o = {"kind": "red", "red": [], "nextest": False, "units": units, "off": off}
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
    "a unit classifier log run on another tree": windowed_step({UNDO: red_unit(UNDO, rerun=("present", "stale"))}),
    "an end line that counts one red unit more": windowed_step({UNDO: red_unit(UNDO)}, off=1),
    "a red unit without a rerun line, its one log a known red": windowed_step(
        {UNDO: {"verdict": "RED", "rerun": False, "logs": {unit_logs(UNDO, {"verdict": "RED", "rerun": False})[0]:
                                                             ("present", "ok")}}}),
}
# The words of each refusal of the windowed red path; a case with no refusal is accepted.
VERDICT_CLASSES = {("FAIL lines",): "count", ("is HUNG",): "hung", ("does not exist",): "missing",
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
    return windowed_step(units, rng.choice([0] * 8 + [1] + [-1] * red), rng.choice([None, 1, 1, 3]))


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
    print(f"[tests-cover-diff] windowed units: {cases} cases PASS; cases per verdict class {classes}")


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
        reruns = checker.gate_reruns(lines)
        assert all(reruns[unit][1] == final for _, unit, final in fails), (f.name, fails, reruns)
        print(f"[tests-cover-diff] real windowed log {f.name}: {len(fails)} FAIL lines = RED={red} HUNG={hung}")


if __name__ == "__main__":
    fixtures()
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 300
    generated(cases)
    citations(cases)
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
    hand_killed_steps(cases // 3)
    complete_runs_of_any_size(cases // 3)
