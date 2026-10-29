#!/usr/bin/env python3
"""Check that the test logs a lane summary cites ran exactly the test steps its diff selects.

    scripts/flow-checks/tests-cover-diff.py --base <commit> --summary <path>

Run from the lane root. It runs `scripts/select-tests.py --base <commit>` and reads its `<step>: <args>` lines.
The cited logs are the `target/...` `.log` and `.txt` paths between the summary's `How tested` and `Confidence` lines.
PASS (exit 0) when each selected step appears in some cited log, each cited log holds for every step it names exactly
the selected line, and every backticked `<step>: <args>` span of the summary equals the selected line.
Lines compare with runs of whitespace collapsed. Every problem is one stderr line; it never runs tests.
"""
import argparse
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


def check(selected_text, summary_text, logs):
    """`logs` maps each cited path to its text; returns the problems."""
    counts = Counter(m.group(1) for line in selected_text.splitlines() if (m := STEP_LINE.match(line)))
    bad = [f"the selector prints step `{step}` {n} times" for step, n in sorted(counts.items()) if n > 1]
    selected = step_lines(selected_text)
    covered = set()
    for path, text in sorted(logs.items()):
        for step, held in sorted(step_lines(text).items()):
            if step not in selected:
                continue
            covered.add(step)
            if held != selected[step]:
                bad.append(f"{path} holds `{step}` lines {sorted(held)}, the selector prints {sorted(selected[step])}")
    bad += [f"no cited log holds the selected step `{step}`" for step in sorted(set(selected) - covered)]
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
    bad = check(done.stdout, summary_text, logs)
    for problem in bad:
        print(f"tests-cover-diff: {problem}", file=sys.stderr)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
