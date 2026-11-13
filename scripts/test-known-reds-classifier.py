#!/usr/bin/env python3
"""Property check of scripts/keystone-known-reds.sh on logs drawn by scripts/known_reds_logs.py.

Run: `python3 scripts/test-known-reds-classifier.py` (CASES=<n> draws n logs,
default 100; SEED=<s> replays a draw). The fixed examples run first. Properties:
- P1: the classifier's verdict, with the tests its NOTE line names and the
  location of each signature it prints, equals the model's, on the clean and
  the interleaved rendering of each draw;
- P2: LC_ALL, TMPDIR and the working directory do not change the output;
- P3: interleaving never moves a printed panic out of the output, except under
  the message a cut location leaves it;
- P4: without the `--no-capture` marker no panic leaves the output.
Before the CASES draws it checks base draws, drawn until each case of `kinds` below is reached at least once, so every case
is checked in every run; it fails when ATTEMPTS draws leave a case unreached. Exits non-zero when a property fails; each failing
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

ATTEMPTS = 5000
CLASSIFIER = Path(os.environ.get("CLASSIFIER", Path(__file__).resolve().parent / "keystone-known-reds.sh"))


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


def kept_within(small, big, restated=()):
    """True when `big` prints every panic `small` prints, each of `restated` under any message.

    Exit codes are not compared: a printed known red masks the "no panic
    signature" exit of a failed run, which is no panic.
    """
    moved = Counter(p.msg for p in restated if p.kind == "novel")
    return (all(big.listed[k] + moved[k] >= n for k, n in small.listed.items())
            and big.matched + sum(p.kind == "known" for p in restated) >= small.matched)


def show(v):
    return f"exit={v.exit} novel={v.novel} matched={v.matched} note={v.note} dropped={sorted(v.dropped)} listed={dict(v.listed)} sites={dict(v.sites)}"


def check(work, text, want, label):
    """Failures of P1 and P2 on `text`, whose model verdict is `want`; returns (failures, stated verdict)."""
    log = work / "case.log"
    log.write_text(text)
    got = stated(log)
    failures = []
    if (got.exit, got.novel, got.matched, got.listed, got.note, got.dropped, got.sites) != (want.exit, want.novel, want.matched, want.listed, want.note, want.dropped, want.sites):
        failures.append(f"P1 {label}: classifier {show(got)}\n   model {show(want)}")
    env = dict(os.environ, LC_ALL="en_US.UTF-8", TMPDIR=str(work / "tmp"))
    (work / "tmp").mkdir(exist_ok=True)
    if classify(log, env=env, cwd=work)[1] != classify(log)[1]:
        failures.append(f"P2 {label}: LC_ALL / TMPDIR / cwd changed the output")
    return failures, got


def report(text, failures):
    print("\n".join(failures))
    print(f"  as an EXAMPLES entry:\n    (\"drawn\", r'''{text}''', <exit>, r\"<line>\"),")


def kinds(runs, clean, mixed):
    """How often the draw `runs`, rendered as `clean` and `mixed`, reaches each case it must reach; every case is a key."""
    k = Counter()
    k["a panic without its thread name"] += any(name == "" for _, name, _ in mixed.panics)
    k["the --no-capture marker"] += logs.SERIAL_MARKER in clean.text
    k["a panic inside a passing test whose owner the text proves"] += bool(clean.in_passing)
    k["a panic inside a passing test read by its thread name"] += bool(logs.verdict(clean).dropped - {s[2] for s, _, _ in clean.panics if s[0] == "inst"})
    k["a verdict behind another thread's text on its line"] += any(line.endswith(tuple(f"{x}{w}" for x in logs.SPLIT_BEFORE_VERDICT for w in ("ok", "FAILED"))) for line in mixed.lines)
    k["another thread's line that ends in a verdict word"] += any(line.endswith(tuple(logs.VERDICT_ENDED)) for line in mixed.lines)
    k["another thread's line that starts with a verdict word"] += any(line in logs.VERDICT_LED[:2] + logs.VERDICT_LED[3:4] for line in mixed.lines)
    k["a verdict glued behind another thread's word"] += any(re.fullmatch(r"step\d(ok|FAILED)", line) for line in mixed.lines)
    k["another run's `test result:` line inside a run"] += any(line.endswith(logs.CHILD_RESULT) for line in mixed.lines)
    k["a failing test's panic in a section beside a passing test's"] += any(
        r.shape == "libtest" and {t.status for t in r.tests} == {"ok", "FAILED"} and any(t.panics for t in r.tests if t.status == "ok") for r in runs)
    k["a head split after its ' ("] += any(re.match(r"thread '[^']+' \((?!\d+\) panicked)", line) for line in mixed.lines)
    for what in logs.DRAWN:
        k[what] += mixed.drawn[what]
    k["one binary's libtest sections twice in one log"] += sum(r.shape == "libtest" for r in runs) == 2 and runs[0].binary == runs[1].binary
    k["a test run twice in one log"] += any(n > 1 for n in Counter(inst[2] for inst, _ in clean.statuses).values())
    k["a serial run cut off after a panic"] += any(r.cut for r in runs)
    k["a run without the marker after a serial run"] += any(
        a.shape == "serial" and b.shape == "parallel" for a, b in zip(runs, runs[1:]))
    return k


def check_draw(work, clean, mixed):
    """Failures of P1 to P4 on one draw rendered as `clean` and `mixed`; returns (failures, the log to report)."""
    f1, got_clean = check(work, clean.text, logs.verdict(clean), "clean")
    f2, got_mixed = check(work, mixed.text, logs.verdict(mixed), "interleaved")
    failures = f1 + f2
    if not kept_within(got_clean, got_mixed, mixed.restated):
        failures.append(f"P3: interleaving dropped a panic: clean {show(got_clean)}\n   interleaved {show(got_mixed)}")
    if logs.SERIAL_MARKER in clean.text:
        unmarked = work / "unmarked.log"
        unmarked.write_text(clean.text.replace(logs.SERIAL_MARKER + "\n", ""))
        got_unmarked = stated(unmarked)
        if not kept_within(got_clean, got_unmarked):
            failures.append(f"P4: without the marker a panic left the output: {show(got_unmarked)}\n   with it {show(got_clean)}")
    return failures, clean.text if f1 or failures[-1:] and failures[-1].startswith("P4") else mixed.text


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

        def draw():
            runs = logs.draw(rng)
            render_seed = rng.randrange(1 << 30)
            clean = logs.render(runs, random.Random(render_seed), interleave=False)
            mixed = logs.render(runs, random.Random(render_seed), interleave=True)
            return clean, mixed, kinds(runs, clean, mixed)

        # One base draw per case first, so every case is checked in every run, whatever CASES and the seed.
        draws, missing = [], None
        for _ in range(ATTEMPTS):
            clean, mixed, k = draw()
            missing = set(k) if missing is None else missing
            if missing & {w for w, n in k.items() if n}:
                missing -= {w for w, n in k.items() if n}
                draws.append((clean, mixed, k))
            if not missing:
                break
        for what in sorted(missing):
            failed += 1
            print(f"no draw of {ATTEMPTS} reaches {what}")
        print(f"{len(draws)} base draw(s) reach every case")
        draws += [draw() for _ in range(cases)]
        for clean, mixed, k in draws:
            reached.update(k)
            failures, text = check_draw(work, clean, mixed)
            if failures:
                failed += 1
                report(text, failures)
        for what in reached:
            print(f"draws reaching {what}: {reached[what]}")
    print(f"[known-reds-classifier] {'FAIL' if failed else 'PASS'}: {failed} failing case(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
