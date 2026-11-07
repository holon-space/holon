#!/usr/bin/env python3
"""Property check of scripts/keystone-known-reds.sh on logs drawn by scripts/known_reds_logs.py.

Run: `python3 scripts/test-known-reds-classifier.py` (CASES=<n> draws n logs,
default 100; SEED=<s> replays a draw). The fixed examples run first. Properties:
- P1: the classifier's verdict equals the model's, on the clean and the
  interleaved rendering of each draw;
- P2: LC_ALL, TMPDIR and the working directory do not change the output;
- P3: interleaving never moves a printed panic out of the output;
- P4: without the `--no-capture` marker no panic leaves the output.
It fails as well when no draw reaches a panic whose thread name the text loses,
or a `--no-capture` marker. Exits non-zero when a property fails; each failing
log is printed as an EXAMPLES entry.
"""
import os
import random
import re
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

import known_reds_logs as logs

CLASSIFIER = Path(__file__).resolve().parent / "keystone-known-reds.sh"


def classify(log, env=None, cwd=None):
    """(exit code, stdout) of the classifier on file `log`; any other exit than 0 or 1 is no verdict."""
    r = subprocess.run([str(CLASSIFIER), str(log)], env=env, cwd=cwd, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"classifier exit {r.returncode}:\n{r.stdout}{r.stderr}"
    return r.returncode, r.stdout


def stated(log, **kw):
    code, out = classify(log, **kw)
    v = logs.parse(out)
    v.exit = code
    return v


def kept_within(small, big):
    """True when `big` prints every panic `small` prints."""
    return all(big.listed[k] >= n for k, n in small.listed.items()) and big.matched >= small.matched and big.exit >= small.exit


def show(v):
    return f"exit={v.exit} novel={v.novel} matched={v.matched} listed={dict(v.listed)}"


def check(work, text, want, label):
    """Failures of P1 and P2 on `text`, whose model verdict is `want`; returns (failures, stated verdict)."""
    log = work / "case.log"
    log.write_text(text)
    got = stated(log)
    failures = []
    if (got.exit, got.novel, got.matched, got.listed) != (want.exit, want.novel, want.matched, want.listed):
        failures.append(f"P1 {label}: classifier {show(got)}\n   model {show(want)}")
    env = dict(os.environ, LC_ALL="en_US.UTF-8", TMPDIR=str(work / "tmp"))
    (work / "tmp").mkdir(exist_ok=True)
    if classify(log.resolve(), env=env, cwd=work)[1] != classify(log)[1]:
        failures.append(f"P2 {label}: LC_ALL / TMPDIR / cwd changed the output")
    return failures, got


def report(text, failures):
    print("\n".join(failures))
    print(f"  as an EXAMPLES entry:\n    (\"drawn\", r'''{text}''', <exit>, r\"<line>\"),")


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    cases = int(os.environ.get("CASES", 100))
    rng = random.Random(seed)
    failed = 0
    reached = Counter()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for name, text, want_exit, line in logs.examples():
            log = work / f"{name}.log"
            log.write_text(text)
            code, out = classify(log)
            if code != want_exit or not re.search(line, out, re.M):
                failed += 1
                print(f"example {name}: want exit {want_exit} and a line matching {line!r}, got exit {code}:\n{out}")
        print(f"drawn with seed {seed} ({cases} cases)")
        for _ in range(cases):
            runs = logs.draw(rng)
            render_seed = rng.randrange(1 << 30)
            clean = logs.render(runs, random.Random(render_seed), interleave=False)
            mixed = logs.render(runs, random.Random(render_seed), interleave=True)
            f1, got_clean = check(work, clean.text, logs.verdict(clean), "clean")
            f2, got_mixed = check(work, mixed.text, logs.verdict(mixed), "interleaved")
            failures = f1 + f2
            reached["a panic without its thread name"] += any(name == "" for _, name, _ in mixed.panics)
            reached["the --no-capture marker"] += logs.SERIAL_MARKER in clean.text
            if not kept_within(got_clean, got_mixed):
                failures.append(f"P3: interleaving dropped a panic: clean {show(got_clean)}\n   interleaved {show(got_mixed)}")
            if logs.SERIAL_MARKER in clean.text:
                unmarked = work / "unmarked.log"
                unmarked.write_text(clean.text.replace(logs.SERIAL_MARKER + "\n", ""))
                got_unmarked = stated(unmarked)
                if not kept_within(got_clean, got_unmarked):
                    failures.append(f"P4: without the marker a panic left the output: {show(got_unmarked)}\n   with it {show(got_clean)}")
            if failures:
                failed += 1
                report(clean.text if f1 or failures[-1].startswith("P4") else mixed.text, failures)
        for what in ("a panic without its thread name", "the --no-capture marker"):
            print(f"draws reaching {what}: {reached[what]}")
            if not reached[what]:
                failed += 1
    print(f"[known-reds-classifier] {'FAIL' if failed else 'PASS'}: {failed} failing case(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
