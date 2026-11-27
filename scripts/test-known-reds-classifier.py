#!/usr/bin/env python3
"""Property check of scripts/keystone-known-reds.sh on logs drawn by scripts/known_reds_logs.py.

Run: `python3 scripts/test-known-reds-classifier.py` (CASES=<n> draws n logs,
default 100; SEED=<s> replays a draw). The fixed examples run first. Properties:
- P1: the classifier's verdict, with the tests its NOTE line names and the
  location of each signature it prints, equals the model's, on the clean and
  the interleaved rendering of each draw;
  a TIMEOUT counts as collateral of a row only when all panics of its own test match that one row;
- P2: LC_ALL, TMPDIR and the working directory do not change the output;
- P3: interleaving never moves a printed panic out of the output, except under
  the message a cut location leaves it;
- P4: without the `--no-capture` marker no panic leaves the output;
- P6: when several registry rows match one panic, the first row that accepts its site is named, else the first row is cross-family (synthetic registry in a copied tree);
- P5: a flow job capture of the log (a timestamp before every line) gets the same verdict word and exit code as the raw capture;
- P7: when the output lists any entry, exactly one of them is the `PRIMARY: ` line.
Before the CASES draws it checks base draws, drawn until each case of `kinds` below is reached at least once, so every case
is checked in every run; it fails when ATTEMPTS draws leave a case unreached. Exits non-zero when a property fails; each failing
log is printed as an EXAMPLES entry.
Fixed cases also check `--stale` on green runs and that a broken `Test | Registry key` table is a broken-registry exit.
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
    moved = Counter(p.msg for p in restated if p.kind in ("novel", "collateral"))
    return (all(big.listed[k] + moved[k] >= n for k, n in small.listed.items())
            and big.matched + sum(p.kind in ("known", "row") for p in restated) >= small.matched)


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
    out = classify(log)[1]
    if classify(log, env=env, cwd=work)[1] != out:
        failures.append(f"P2 {label}: LC_ALL / TMPDIR / cwd changed the output")
    entries = len(re.findall(r"^(?:PRIMARY: |  \[)", out, re.M))
    primaries = len(re.findall(r"^PRIMARY: ", out, re.M))
    if entries and primaries != 1:
        failures.append(f"P7 {label}: {primaries} PRIMARY lines among {entries} entries:\n{out}")
    return failures, got


def one_primary_examples():
    """Static examples of P7: a timed-out test attributed to a row right after the first printed signature."""
    cycle = (logs.CYCLE_SITE, logs.CYCLE_MSG)
    unit = logs.STABLE_ID_UNIT
    def rendered(*runs):
        return logs.render([logs.Run(shape, unit[0], [logs.Test(unit[1], status, False, [logs.Panic("known", unit[1], *pm) for pm in ps])]) for shape, status, ps in runs],
                           random.Random(0), interleave=False)
    return [rendered(("serial", "TIMEOUT", [cycle])), rendered(("captured", "TIMEOUT", [cycle] * 3)),
            rendered(("captured", "TIMEOUT", []), ("captured", "TIMEOUT", [cycle] * 3))]


# Logs a random draw once made the classifier and its model answer differently on.
DRAWN_CUT_OFF_LOGS = [
    ("cut-off-instance-then-passing-tests-and-a-failing-one", r'''warning: ignoring --test-threads because --no-capture is specified
    Starting 3 tests across 1 binary
       START [   0.010s] (1/3) holon-bgpjwzg::acia nemaenv::case_gxwxjo

running 1 test
thread 'nemaenv::case_gxwxjo' (7) panicked at crates/kbygkg/tests/ollepcz.rs:484:1:
novel defect vkns iqinsbev wknw
thread 'nemaenv::case_gxwxjo' (7) panicked at crates/mbnpotu/tests/kszhc.rs:734:38:
novel defect rvfd fajmtwd zwcfm
test nemaenv::case_gxwxjo - should panic ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
        PASS [   0.010s] (1/3) holon-bgpjwzg::acia nemaenv::case_gxwxjo
       START [   0.010s] (2/3) holon-bgpjwzg::acia vzgagepu::case_vzyvsvuj

running 1 test
thread 'vzgagepu::case_vzyvsvuj' (7) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  field deltas (1):\n    block:a: content: sut=\"x\" ref=\"y\"\n")]
test vzgagepu::case_vzyvsvuj - should panic ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
TIMEOUT-PASS [   0.010s] (2/3) holon-bgpjwzg::acia vzgagepu::case_vzyvsvuj
       START [   0.010s] (3/3) holon-bgpjwzg::acia clboq::case_kastw

running 1 test
thread 'clboq::case_kastw' (7) panicked at crates/holon-loro/tests/compacted_peer_receives_concurrent_edits.rs:789:5:
generated (shrunk): Test failed: after Restart { peer: 0 }: ["peer 0 trimmed below VersionVector({1000: 2}), which peer 1 at VersionVector({1000: 1}) lacks"]
thread 'clboq::case_kastw' (7) panicked at /Users/u/.cargo/git/checkouts/loro-e88d0f38e94f7134/d302b15/crates/loro-internal/src/sync.rs:34:31:
poisoned LoroMutex
     Running tests/main.rs (target/debug/deps/holon-fmes-urijwixl)

running 1 tests
thread 'ghgoaz::case_edxbbx' (101) panicked at crates/jvxlfqqt/tests/hygu.rs:1:1:
novel defect dngecm
test ghgoaz::case_edxbbx ... FAILED

failures:
    ghgoaz::case_edxbbx

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
''', 1, r"^\[known-reds\] FAIL: 1 novel panic\(s\), 1 known-red panic\(s\), 1 collateral \(ignored\)\.$"),
    ("cut-off-instance-then-a-run-with-its-own-collateral", r'''warning: ignoring --test-threads because --no-capture is specified
    Starting 1 tests across 1 binary
       START [   0.010s] (1/1) holon-ytqjz::azja xfpga::case_yejyq

running 1 test
thread 'tokio-runtime-worker' (7) panicked at crates/holon-loro/tests/compacted_peer_receives_concurrent_edits.rs:789:5:
generated (shrunk): Test failed: after Restart { peer: 0 }: ["peer 0 trimmed below VersionVector({1000: 2}), which peer 1 at VersionVector({1000: 1}) lacks"]
thread 'tokio-runtime-worker' (7) panicked at /Users/u/.cargo/git/checkouts/loro-e88d0f38e94f7134/d302b15/crates/loro-internal/src/oplog/pending_changes.rs:338:62:
called `Result::unwrap()` on an `Err` value: ()
     Running tests/main.rs (target/debug/deps/holon-qwyeahra-vwxskqv)

running 2 tests
thread 'rkgu::case_wddu' (101) panicked at crates/pztazgu/tests/nmmhmux.rs:1:1:
novel defect jpfqcuhf
thread 'rkgu::case_wddu' (102) panicked at crates/holon-integration-tests/src/pbt/composed/boot_fault.rs:22:5:
keystone boot fault: injected task panic
test rkgu::case_wddu ... FAILED
thread 'rkgu::case_wdduFAILED' (104) panicked at /Users/u/.cargo/git/checkouts/loro-e88d0f38e94f7134/d302b15/crates/loro-internal/src/oplog/pending_changes.rs:338:62:
called `Result::unwrap()` on an `Err` value: ()
thread 'rkgu::case_wdduFAILED' (105) panicked at crates/lmqo/tests/bozcewym.rs:1:1:
novel defect ejtjzr
thread 'rkgu::case_wdduFAILED' (106) panicked at crates/holon-integration-tests/src/pbt/composed/harness.rs:1574:13:
reconciled composed sequence diverged from the oracle: [("inv-blocks-match-ref/org", "[inv-blocks-match-ref/org] fields diverge from reference\n  field deltas (1):\n    block:a: content: sut=\"x\" ref=\"y\"\n")]
test rkgu::case_wdduFAILED ... FAILED

failures:
    rkgu::case_wddu
    rkgu::case_wdduFAILED

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
''', 1, r"^\[known-reds\] FAIL: 3 novel panic\(s\), 2 known-red panic\(s\), 1 collateral \(ignored\)\.$"),
]


def cut_off_examples():
    """Static examples of a serial instance cut off after its row's panic and collateral, followed by a run of its own.

    The later run's panics belong to no instance the text proves, so they neither prove the cut instance
    unclean nor take its collateral with them: the cut instance's collateral is attributed to its row, the
    later run's collateral stays novel.
    """
    cut = "warning: ignoring --test-threads because --no-capture is specified\n    Starting 1 tests across 1 binary\n       START [   0.010s] (1/1) holon-x::main a::case_cut\n\nrunning 1 test\n"
    row = "thread 'a::case_cut' (7) panicked at crates/holon-loro/tests/compacted_peer_receives_concurrent_edits.rs:789:5:\n" \
          "generated (shrunk): Test failed: after Restart { peer: 0 }: [\"peer 0 trimmed below VersionVector({1000: 2}), which peer 1 at VersionVector({1000: 1}) lacks\"]\n"
    poisoned = "thread 'a::case_cut' (7) panicked at /Users/u/.cargo/git/checkouts/loro-e88d0f38e94f7134/d302b15/crates/loro-internal/src/sync.rs:34:31:\npoisoned LoroMutex\n"
    later = "     Running tests/main.rs (target/debug/deps/holon-x)\n\nrunning 1 tests\n" \
            "thread 'b::case_later' (101) panicked at crates/pztazgu/tests/nmmhmux.rs:1:1:\nnovel defect jpfqcuhf\n" \
            "test b::case_later ... FAILED\n\nfailures:\n    b::case_later\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"
    later_poisoned = later.replace("novel defect jpfqcuhf", "poisoned LoroMutex").replace("crates/pztazgu/tests/nmmhmux.rs:1:1", "/Users/u/.cargo/git/checkouts/loro-e88d0f38e94f7134/d302b15/crates/loro-internal/src/sync.rs:34:31")
    verdict = r"^\[known-reds\] FAIL: %d novel panic\(s\), 1 known-red panic\(s\), %d collateral \(ignored\)\.$"
    return DRAWN_CUT_OFF_LOGS + [
        ("cut-off-instance-collateral-then-later-novel-run", cut + row + poisoned + later, 1, verdict % (1, 1)),
        ("cut-off-instance-collateral-then-later-collateral-run", cut + row + poisoned + later_poisoned, 1, verdict % (1, 1)),
        ("cut-off-instance-without-its-row-then-later-run", cut + poisoned + later, 1, r"^\[known-reds\] FAIL: 2 novel panic\(s\), 0 known-red panic\(s\), 0 collateral \(ignored\)\.$"),
    ]


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
    for what, n in logs.collateral_placements(clean).items():
        k[what] += n
    for what, n in logs.timeout_placements(clean).items():
        k[what] += n
    for what in logs.DRAWN:
        k[what] += mixed.drawn[what]
    k["one binary's libtest sections twice in one log"] += sum(r.shape == "libtest" for r in runs) == 2 and runs[0].binary == runs[1].binary
    k["a test run twice in one log"] += any(n > 1 for n in Counter(inst[2] for inst, _ in clean.statuses).values())
    k["a serial run cut off after a panic"] += any(r.cut for r in runs)
    for what, has_row in (("a cut-off serial instance with a collateral panic beside its row's panic, then a later run with a panic", True),
                          ("a cut-off serial instance with a collateral panic and no row's panic, then a later run with a panic", False)):
        k[what] += any(
            a.cut and any(p.kind == "collateral" for p in a.tests[-1].panics)
            and any(p.kind == "row" for p in a.tests[-1].panics) == has_row
            and any(t.panics for t in b.tests)
            for a, b in zip(runs, runs[1:]))
    k["a run without the marker after a serial run"] += any(
        a.shape == "serial" and b.shape == "parallel" for a, b in zip(runs, runs[1:]))
    return k


def class_word(log):
    r = subprocess.run([str(CLASSIFIER), "--class", str(log)], capture_output=True, text=True)
    return r.returncode, r.stdout


def capture_failures(work, text, rng):
    """P5: the verdict word and exit code do not depend on how the log was captured."""
    raw, job = work / "raw.log", work / "job.log"
    raw.write_text(text)
    job.write_text(logs.job_capture(text, rng))
    want, got = class_word(raw), class_word(job)
    return [f"P5: raw capture {want}, job capture {got}"] if want != got else []


def check_draw(work, clean, mixed, rng):
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
    for text in (clean.text, mixed.text):
        failures += capture_failures(work, text, rng)
    return failures, clean.text if f1 or failures[-1:] and failures[-1].startswith("P4") else mixed.text


def stale_only_failures(work):
    """`--stale` prints nothing for a green run without a stale row, and the STALE report for one with it."""
    pinned, key = logs.pinned_test()
    failures = []
    for name, units, want in (("no-stale", ("holon-x::main a::case_b",), ""),
                              ("stale", ("holon-x::main a::case_b", pinned), f"[known-reds] STALE: {key}: its test {pinned} passed at ")):
        log = work / f"stale-only-{name}.log"
        log.write_text(logs.green_run(*units))
        out = subprocess.run([CLASSIFIER, "--stale", log], capture_output=True, text=True)
        if out.returncode != 0 or not out.stdout.startswith(want) or bool(want) != bool(out.stdout):
            failures.append(f"--stale {name}: want exit 0 and stdout starting {want!r}, got exit {out.returncode}:\n{out.stdout}{out.stderr}")
    return failures


def stale_location_failures(work, rng):
    """STALE and UNKNOWN-label reports locate the line in the log they were handed, under either capture."""
    pinned, key = logs.pinned_test()
    failures = []
    for i in range(8):
        units = [f"holon-x::main a::case_{c}" for c in rng.sample("bcdefg", rng.randrange(1, 4))]
        units.insert(rng.randrange(len(units) + 1), pinned)
        text = logs.green_run(*units)
        for capture, body in (("raw", text), ("job", logs.job_capture(text, rng))):
            log = work / f"stale-location-{i}-{capture}.log"
            log.write_text(body)
            out = subprocess.run([CLASSIFIER, log], capture_output=True, text=True).stdout
            want = f"[known-reds] STALE: {key}: its test {pinned} passed at {log}:{units.index(pinned) + 2}."
            if want not in out.splitlines():
                failures.append(f"stale location, {capture} capture: want the line {want!r}, got:\n{out}")

            bad = logs.green_run(*units).splitlines()
            at = rng.randrange(1, len(bad) - 1)
            bad.insert(at, logs.status_line("BOGUS", 1, len(units), units[0]))
            body = "\n".join(bad) + "\n"
            if capture == "job":
                body = logs.job_capture(body, rng)
            log = work / f"unknown-location-{i}-{capture}.log"
            log.write_text(body)
            err = subprocess.run([CLASSIFIER, log], capture_output=True, text=True).stderr
            want = f'UNKNOWN nextest status label "BOGUS" at {log}:{at + 1} '
            if want not in err:
                failures.append(f"unknown label location, {capture} capture: want {want!r}, got:\n{err}")
    return failures


def path_spelling_location_failures(work, rng):
    """A STALE report names the input path byte for byte, whatever characters the path holds."""
    pinned, key = logs.pinned_test()
    parts = ["plain", "sp ace", "q'uote", "back\\tslash", "two\\\\back", "nl\\nesc", "dollar$x"]
    names = ["back\\tslash"] + ["-".join(rng.sample(parts, rng.randrange(1, 3))) for _ in range(6)]
    failures = []
    for i, name in enumerate(names):
        units = [f"holon-x::main a::case_{c}" for c in rng.sample("bcdefg", rng.randrange(1, 4))]
        units.insert(rng.randrange(len(units) + 1), pinned)
        text = logs.green_run(*units)
        for capture, body in (("raw", text), ("job", logs.job_capture(text, rng))):
            log = work / f"spelling-{i}" / name / f"{capture}.log"
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text(body)
            out = subprocess.run([CLASSIFIER, log], capture_output=True, text=True).stdout
            want = f"[known-reds] STALE: {key}: its test {pinned} passed at {log}:{units.index(pinned) + 2}."
            if want not in out.splitlines():
                failures.append(f"path spelling {name!r}, {capture} capture: want the line {want!r}, got:\n{out}")
    return failures


def broken_table_failures(work):
    """A table row that names no known-red row, or a test listed twice, stops the classifier with exit 2."""
    repo = CLASSIFIER.resolve().parent.parent
    registry = (repo / "docs/Testing/KeystoneKnownReds.md").read_text()
    header = "| Test | Registry key |\n| --- | --- |\n"
    pinned, key = logs.pinned_test()
    failures = []
    for name, row, want in (("unknown key", "| `holon-x::main a::case_b` | `no-such-row` |", "no-such-row"),
                            ("test twice", f"| `{pinned}` | `{key}` |", "listed twice")):
        tree = work / name.replace(" ", "-")
        (tree / "docs/Testing").mkdir(parents=True)
        (tree / "scripts").symlink_to(CLASSIFIER.resolve().parent)
        (tree / "docs/Testing/known-red-evidence").symlink_to(repo / "docs/Testing/known-red-evidence")
        assert registry.count(header) == 1
        (tree / "docs/Testing/KeystoneKnownReds.md").write_text(registry.replace(header, header + row + "\n"))
        log = work / "broken-table.log"
        log.write_text(logs.green_run("holon-x::main a::case_b", "holon-x::main a::case_c"))
        out = subprocess.run([tree / "scripts/keystone-known-reds.sh", log], capture_output=True, text=True)
        if out.returncode != 2 or want not in out.stderr:
            failures.append(f"broken table, {name}: want exit 2 and stderr naming {want!r}, got exit {out.returncode}:\n{out.stdout}{out.stderr}")
    return failures


def twin_failures(work, rng, cases):
    """P6: when several rows match one panic, the verdict is the first row that accepts its site, else cross-family on the first row.

    The real registry has no such pair, so each case runs the classifier on a copy of the tree whose registry
    holds the case's two rows ahead of all others.
    """
    repo = CLASSIFIER.resolve().parent.parent
    registry = (repo / "docs/Testing/KeystoneKnownReds.md").read_text()
    failures = []
    for i, (name, rows, text, want_exit, line) in enumerate(logs.twin_examples() + [logs.twin_case(rng) for _ in range(cases)]):
        tree = work / f"twin-{i}"
        (tree / "docs/Testing").mkdir(parents=True)
        (tree / "scripts").symlink_to(repo / "scripts")
        (tree / "docs/Testing/known-red-evidence").symlink_to(repo / "docs/Testing/known-red-evidence")
        (tree / "docs/Testing/KeystoneKnownReds.md").write_text(logs.registry_with(registry, rows))
        log = tree / "case.log"
        log.write_text(text)
        out = subprocess.run([tree / "scripts/keystone-known-reds.sh", log], capture_output=True, text=True)
        if out.returncode != want_exit or not re.search(line, out.stdout, re.M):
            failures.append(f"P6 {name}: want exit {want_exit} and a line matching {line!r}, rows {rows}, got exit {out.returncode}:\n{out.stdout}{out.stderr}\n  log:\n{text}")
    return failures


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    cases = int(os.environ.get("CASES", 100))
    rng = random.Random(seed)
    failed = 0
    reached = Counter()
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for name, text, want_exit, line in logs.examples() + cut_off_examples():
            log = work / f"{name}.log"
            log.write_text(text)
            code, out = classify(log)
            if code != want_exit or not re.search(line, out, re.M):
                failed += 1
                print(f"example {name}: want exit {want_exit} and a line matching {line!r}, got exit {code}:\n{out}")
        for failure in stale_only_failures(work) + broken_table_failures(work) + stale_location_failures(work, rng) + path_spelling_location_failures(work, rng) + twin_failures(work, rng, cases):
            failed += 1
            print(failure)
        for r in one_primary_examples():
            failures, _ = check(work, r.text, logs.verdict(r), "static")
            if failures:
                failed += 1
                report(r.text, failures)
        print(f"drawn with seed {seed} ({cases} cases)")

        def draw():
            runs = logs.draw(rng)
            for r in runs:
                assert len({t.name for t in r.tests}) == len(r.tests), f"a run draws one test name twice: {[t.name for t in r.tests]}"
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
            failures, text = check_draw(work, clean, mixed, rng)
            if failures:
                failed += 1
                report(text, failures)
        for what in reached:
            print(f"draws reaching {what}: {reached[what]}")
    print(f"[known-reds-classifier] {'FAIL' if failed else 'PASS'}: {failed} failing case(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
