#!/usr/bin/env python3
"""A lane's nextest run that stops at the first red outside the known-reds list, and the rerun of its reds.

    scripts/lane-nextest.py run <nextest args>      `cargo nextest run --no-fail-fast <args>`, stopped at the first
                                                    red that scripts/keystone-known-reds.sh does not list
    scripts/lane-nextest.py rerun <nextest args>    the same args, `-E` narrowed to the tests of the red list

Both runs pass the nextest output through to stdout. `run` writes the red list to $LANE_NEXTEST_DIR/reds.txt
(default target/lane-nextest/), one `<binary id> <test name>` line per red test, known reds included, and prints
its path and the reason it stopped as `[fail-fast]` lines.
A red is judged when its output block is complete: by the classifier over the run's output so far, so a red is
known only when every red before it is too. `rerun` refuses an empty or missing red list.
"""
import os
import queue
import re
import signal
import subprocess
import sys
import threading
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
IDLE_SECONDS = 1.0


def out_dir():
    return Path(os.environ.get("LANE_NEXTEST_DIR", "target/lane-nextest"))


def red_of(line):
    """`(binary id, test name)` when `line` is a nextest status line of a red test, else None."""
    m = STATUS.match(line)
    return None if m is None or NOT_RED.match(m.group(1)) else (m.group(2), m.group(3))


def classify(stream_log, verdict_log):
    """True when every red in `stream_log` so far is on the known-reds list."""
    with open(verdict_log, "w") as f:
        rc = subprocess.run([str(CLASSIFIER), str(stream_log)], stdout=f, stderr=subprocess.STDOUT).returncode
    return rc == 0


def stop(proc):
    """SIGTERM nextest's process group, unless nextest has already exited."""
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except (PermissionError, ProcessLookupError):
        # macOS answers EPERM for a group whose leader exited and is not yet reaped.
        if proc.poll() is None:
            raise


def run(args):
    d = out_dir()
    d.mkdir(parents=True, exist_ok=True)
    reds_file, stream_log, verdict_log = d / "reds.txt", d / "stream.log", d / "verdict.log"
    reds_file.unlink(missing_ok=True)
    proc = subprocess.Popen([*CARGO, "nextest", "run", "--no-fail-fast", *args], stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, text=True, bufsize=1, start_new_session=True)
    lines = queue.Queue()
    threading.Thread(target=lambda: [lines.put(l) for l in proc.stdout] + [lines.put(None)], daemon=True).start()
    reds, pending, stopped, summary = [], None, None, False
    with open(stream_log, "w") as log:
        def judge():
            log.flush()
            if not classify(stream_log, verdict_log):
                stop(proc)
                return pending
            print(f"[fail-fast] {pending[0]} {pending[1]}: on the known-reds list, the run goes on", flush=True)
            return None

        while True:
            try:
                line = lines.get(timeout=IDLE_SECONDS)
            except queue.Empty:
                if pending and not stopped:
                    stopped, pending = judge(), None
                continue
            if line is None:
                break
            text = line.rstrip("\n")
            summary = summary or bool(END_OF_RUN.match(text))
            red = None if summary else red_of(text)
            # The judged red's block ends before this line: the classifier must not see the next red's status line.
            if pending and not stopped and not summary and (red or STATUS.match(text)):
                stopped, pending = judge(), None
            sys.stdout.write(line)
            log.write(line)
            if red:
                reds.append(red)
                pending = red
        if pending and not stopped:
            stopped = judge()
    rc = proc.wait()
    reds_file.write_text("".join(f"{b} {t}\n" for b, t in dict.fromkeys(reds)))
    if stopped:
        print(f"[fail-fast] stopped at {stopped[0]} {stopped[1]}: a red outside the known-reds list ({verdict_log})")
    print(f"[fail-fast] red list: {reds_file} ({len(dict.fromkeys(reds))} tests)", flush=True)
    return 1 if stopped else rc


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
