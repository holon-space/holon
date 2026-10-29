#!/usr/bin/env python3
"""Check that the test logs a lane summary cites ran exactly the test steps its diff selects.

    scripts/flow-checks/tests-cover-diff.py --base <commit> --summary <path>

Run from the lane root. It runs `scripts/select-tests.py --base <commit>` and reads its `<step>: <args>` lines.
The cited logs are the `target/...` `.log` and `.txt` paths between the summary's `How tested` and `Confidence` lines.
A step ran when a cited log written by scripts/flow-checks/run-step.sh has the selected line as line 1 and `exit: 0`
as its last line; the selector's disclosure steps (NOT_RUN_STEPS of select-tests.py) need no log.
PASS (exit 0) when every selected step ran, each cited log holds for every step it names exactly the selected line,
every backticked `<step>: <args>` span of the summary equals the selected line, and the selector printed no step twice.
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
LOG_PATH = re.compile(r"(?<![\w./~-])[\w./~-]{0,256}target/[\w./-]{0,256}\.(?:log|txt)(?!\w)")
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


def cited_logs(summary_text):
    lines = summary_text.splitlines()
    start = next((i for i, line in enumerate(lines) if re.match(rf"{HEADING}How tested", line)), None)
    if start is None:
        raise SystemExit("tests-cover-diff: the summary has no `How tested` line")
    end = next((i for i in range(start, len(lines)) if re.match(rf"{HEADING}Confidence", lines[i])), len(lines))
    return sorted({m.group(0) for line in lines[start:end] for m in LOG_PATH.finditer(line)})


def check(selected_text, summary_text, logs, not_run):
    """`logs` maps each cited path to its text; `not_run` names the steps no log runs. Returns the problems."""
    counts = Counter(m.group(1) for line in selected_text.splitlines() if (m := STEP_LINE.match(line)))
    bad = [f"the selector prints step `{step}` {n} times" for step, n in sorted(counts.items()) if n > 1]
    selected = step_lines(selected_text)
    ran = set()
    for path, text in sorted(logs.items()):
        for step, held in sorted(step_lines(text).items()):
            if step in selected and held != selected[step]:
                bad.append(f"{path} holds `{step}` lines {sorted(held)}, the selector prints {sorted(selected[step])}")
        lines = text.splitlines() or [""]
        first = (m := STEP_LINE.match(lines[0])) and m.group(1)
        if first in selected and {norm(lines[0])} == selected[first]:
            if lines[-1] == EXIT_OK:
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
