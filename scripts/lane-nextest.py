#!/usr/bin/env python3
"""A lane's nextest run that runs to its end and judges its reds against the known-reds list, and the rerun of its reds.

    scripts/lane-nextest.py run <nextest args>      `cargo nextest run --no-fail-fast <args>`, then the reds of the
                                                    whole run judged by scripts/keystone-known-reds.sh
    scripts/lane-nextest.py rerun <nextest args>    the same args, `-E` narrowed to the tests of the red list

Both runs pass the nextest output through to stdout. `run` writes the red list to $LANE_NEXTEST_DIR/reds.txt
(default target/lane-nextest/), one `<binary id> <test name>` line per red test, known reds included, and prints
its path. It exits 0 when nextest passed or every red is on the known-reds list, else 1 with one `[lane-nextest]
novel:` line per novel signature of the classifier's verdict. A classifier error is an error of `run` too.
`rerun` refuses an empty or missing red list.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CLASSIFIER = REPO / "scripts" / "keystone-known-reds.sh"
# `cargo` under the shape env of scripts/cargo-shape-env.sh, so a run and its rerun build one shape.
CARGO = ["bash", "-c", 'source "$0" lane && exec cargo "$@"', str(REPO / "scripts" / "cargo-shape-env.sh")]
# nextest right-aligns its status label in 12 columns: `        FAIL [   0.004s] (2/5) crate::bin some::test`.
STATUS = re.compile(r"^ *([A-Z][A-Z0-9 +/-]*?) +\[[^]]*\] (?:\[[0-9]+/[0-9]+\] )?(?:\([^)]*\) )?(\S+) (\S+)$")
NOT_RED = re.compile(r"^(?:TRY [0-9]+ )?(?:START|SLOW|TRMNTG|TERMINATING|PASS|LEAK|TMPASS|TIMEOUT-PASS|SLOW \+ LEAK"
                     r"|SLOW\+TMPASS|SKIP|FLAKY [0-9]+/[0-9]+|SETUP|SETUP SLOW|SETUP PASS|SETUP LEAK|SETUP TMPASS)$")
END_OF_RUN = re.compile(r"^ *(?:Summary|Cancelling|Canceling) ")
# The classifier's line of a signature that no known-reds row covers: the run's first signature, or a later one.
NOVEL = re.compile(r"^(?:PRIMARY: |  )(\[(?:novel|cross-family:).*)")


def out_dir():
    return Path(os.environ.get("LANE_NEXTEST_DIR", "target/lane-nextest"))


def red_of(line):
    """`(binary id, test name)` when `line` is a nextest status line of a red test, else None."""
    m = STATUS.match(line)
    return None if m is None or NOT_RED.match(m.group(1)) else (m.group(2), m.group(3))


def run(args):
    d = out_dir()
    d.mkdir(parents=True, exist_ok=True)
    reds_file, stream_log, verdict_log = d / "reds.txt", d / "stream.log", d / "verdict.log"
    reds_file.unlink(missing_ok=True)
    verdict_log.unlink(missing_ok=True)
    proc = subprocess.Popen([*CARGO, "nextest", "run", "--no-fail-fast", *args], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, bufsize=1)
    reds, summary = [], False
    with open(stream_log, "w") as log:
        for line in proc.stdout:
            sys.stdout.write(line)
            log.write(line)
            text = line.rstrip("\n")
            summary = summary or bool(END_OF_RUN.match(text))
            red = None if summary else red_of(text)
            if red:
                reds.append(red)
    rc = proc.wait()
    reds = list(dict.fromkeys(reds))
    reds_file.write_text("".join(f"{b} {t}\n" for b, t in reds))
    print(f"[lane-nextest] red list: {reds_file} ({len(reds)} tests)", flush=True)
    if rc == 0:
        return 0
    with open(verdict_log, "w") as f:
        verdict = subprocess.run([str(CLASSIFIER), str(stream_log)], stdout=f, stderr=subprocess.STDOUT).returncode
    if verdict not in (0, 1):
        raise SystemExit(f"run: {CLASSIFIER.name} exited {verdict}, the reds are not judged ({verdict_log})")
    if verdict == 1:
        for line in verdict_log.read_text().splitlines():
            if m := NOVEL.match(line):
                print(f"[lane-nextest] novel: {m.group(1)}")
        print(f"[lane-nextest] a red outside the known-reds list: {verdict_log}", flush=True)
    return verdict


def rerun(args):
    reds_file = out_dir() / "reds.txt"
    reds = [l.split(" ") for l in reds_file.read_text().splitlines()] if reds_file.exists() else []
    if not reds:
        raise SystemExit(f"rerun: no red tests in {reds_file}; run `scripts/lane-nextest.py run` first")
    only = " | ".join(f"(binary_id({b}) & test(={t}))" for b, t in reds)
    args = list(args)
    if "-E" in args:
        i = args.index("-E") + 1
        args[i] = f"({args[i]}) & ({only})"
    else:
        args += ["-E", only]
    return subprocess.run([*CARGO, "nextest", "run", "--no-fail-fast", *args],
                          stderr=subprocess.STDOUT).returncode


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in ("run", "rerun"):
        raise SystemExit(__doc__)
    sys.exit((run if sys.argv[1] == "run" else rerun)(sys.argv[2:]))
