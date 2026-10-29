#!/usr/bin/env python3
"""Check that the test logs a lane summary cites ran exactly the test steps its diff selects.

    scripts/flow-checks/tests-cover-diff.py --base <commit> --summary <path>

Run from the lane root. It runs `scripts/select-tests.py --base <commit>` and reads its `<step>: <args>` lines.
The cited logs are the `target/...` `.log` and `.txt` paths between the summary's `How tested` and `Confidence` lines;
a brace list (`target/x/{a,b}.log`) cites each of its paths, as the shell expands it.
Of a cited log it reads only line 1 and the last line: a log written by scripts/flow-checks/run-step.sh has the step
line as line 1 and `exit: <code>` as its last line, and the lines between are the step's own output.
A step ran when a cited log has the selected line as line 1 and `exit: 0` as its last line; the selector's disclosure
steps (NOT_RUN_STEPS of select-tests.py) need no log.
PASS (exit 0) when every selected step ran, every cited log whose line 1 names a selected step has the selected line
as line 1 and `exit: 0` as its last line, every backticked `<step>: <args>` span of the summary equals the selected
line, and the selector printed no step twice.
Lines compare with runs of whitespace collapsed. Every problem is one stderr line; it never runs tests.
"""
import argparse
import importlib.util
import re
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
    lines = summary_text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(rf"{HEADING}How tested", line)), None)
    if start is None:
        raise SystemExit("tests-cover-diff: the summary has no `How tested` line")
    end = next((i for i in range(start, len(lines)) if re.match(rf"{HEADING}Confidence", lines[i])), len(lines))
    paths = set()
    for line in lines[start:end]:
        for m in LOG_PATH.finditer(line):
            if m.start() and line[m.start() - 1] not in PATH_START:
                token = re.split(r"[\s`]", line[:m.start()])[-1] + m.group(0)
                raise SystemExit(f"tests-cover-diff: the summary cites `{token}`; cite the log by its relative, "
                                 "absolute or ~ path")
            paths.update(expand(m.group(0)))
    return sorted(paths)


def check(selected_text, summary_text, logs, not_run):
    """`logs` maps each cited path to its text; `not_run` names the steps no log runs. Returns the problems."""
    counts = Counter(m.group(1) for line in selected_text.splitlines() if (m := STEP_LINE.match(line)))
    bad = [f"the selector prints step `{step}` {n} times" for step, n in sorted(counts.items()) if n > 1]
    selected = step_lines(selected_text)
    ran = set()
    for path, text in sorted(logs.items()):
        lines = text.splitlines() or [""]
        first = (m := STEP_LINE.match(lines[0])) and m.group(1)
        if first not in selected:
            continue
        if {norm(lines[0])} != selected[first]:
            bad.append(f"{path} runs `{lines[0]}`, the selector prints {sorted(selected[first])}")
        elif lines[-1] == EXIT_OK:
            ran.add(first)
        else:
            bad.append(f"{path} runs `{first}` but its last line is `{lines[-1]}`, not `{EXIT_OK}`")
    bad += [f"no cited run-step log ran the selected step `{step}`" for step in sorted(set(selected) - not_run - ran)]
    for span in SPAN.findall(summary_text):
        if (m := STEP_LINE.match(span)) and m.group(1) in selected and {norm(span)} != selected[m.group(1)]:
            bad.append(f"the summary cites `{span}`, the selector prints {sorted(selected[m.group(1)])}")
    return bad


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base", required=True)
    p.add_argument("--summary", required=True, type=Path)
    a = p.parse_args()
    summary_text = a.summary.read_text()
    done = subprocess.run([sys.executable, SELECTOR, "--base", a.base], capture_output=True, text=True)
    if done.returncode:
        tail = " | ".join(done.stderr.strip().splitlines()[-3:])
        sys.exit(f"tests-cover-diff: {SELECTOR} --base {a.base} exited {done.returncode}: {tail}")
    logs = {}
    for path in cited_logs(summary_text):
        f = Path(path).expanduser()
        if not f.is_file():
            sys.exit(f"tests-cover-diff: cited log {path} does not exist")
        logs[path] = f.read_text(errors="replace")
    bad = check(done.stdout, summary_text, logs, not_run_steps())
    for problem in bad:
        print(f"tests-cover-diff: {problem}", file=sys.stderr)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
