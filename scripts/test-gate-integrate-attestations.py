#!/usr/bin/env python3
"""scripts/gate-integrate.sh records and judges attestations across a first gate run and a second run on the same
tip that reuses the first run's steps.

Each case runs the real gate script inside the real with-build-slot.sh under a scratch HOME with `just` and `cargo`
stubbed. The stubs stand in for the Rust side: `keystone-smoke` writes the claim records (or not),
`projector-lag-lock` and `windowed-gate` write their attestation (or not) into $GATE_ATTEST_DIR, and every stub call
is logged. Drawn per case: which of the three records exist. Oracle, computed here: run 1 is green iff the claims
exist and every claimed kind has its attester; run 2 reuses every step (no recipe runs again) and its verdict equals
run 1's, so the records survive the reuse; a selected run 3 is green and says it did not judge.

Run: `python3 scripts/test-gate-integrate-attestations.py` (CASES=<n>, SEED=<s>).
"""
import atexit
import itertools
import os
import random
import shutil
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
INTEGRATE = HERE / "gate-integrate.sh"
ORCH = Path.home() / ".claude/skills/orchestrator/scripts"
SCRATCH = Path(tempfile.mkdtemp(prefix="test-gate-integrate-attestations-"))
if not os.environ.get("KEEP"):
    atexit.register(shutil.rmtree, SCRATCH, ignore_errors=True)

ATTESTER = {"projector-lag-lock": "StructuralKeyBurst", "windowed-gate": "DeliverBlockContent"}

STUB = r"""#!/usr/bin/env bash
case " $* " in *" --evaluate "*) exec "$REAL_JUST" "$@" ;; esac
[ "$(basename "$0")" = just ] || exit 0
echo "$1" >>"$CALLS"
write() { mkdir -p "$GATE_ATTEST_DIR/$1"; echo "$2" >"$GATE_ATTEST_DIR/$1/$2"; }
case $1 in
  keystone-smoke) [ "$STUB_CLAIMS" != 1 ] || { write claimed StructuralKeyBurst; write claimed DeliverBlockContent; } ;;
  projector-lag-lock) case " $STUB_ATTESTERS " in *" $1 "*) write attested StructuralKeyBurst ;; esac ;;
  windowed-gate) case " $STUB_ATTESTERS " in *" $1 "*) write attested DeliverBlockContent ;; esac ;;
esac
exit 0
"""


def sh(repo, *a):
    return subprocess.run(a, cwd=repo, check=True, capture_output=True, text=True).stdout


def make_repo(repo):
    repo.mkdir()
    sh(repo, "jj", "git", "init", "--quiet")
    (repo / ".gitignore").write_text("target/\n")
    (repo / "f").write_text("base\n")
    (repo / "scripts").mkdir()
    for name in ("check-target-leftovers.sh", "keystone-known-reds.sh"):
        (repo / "scripts" / name).write_text("#!/bin/sh\n")
        (repo / "scripts" / name).chmod(0o755)
    sh(repo, "jj", "commit", "--quiet", "-m", "base")
    return sh(repo, "jj", "log", "-r", "@-", "--no-graph", "-T", "commit_id").strip()


def run_gate(home, repo, stubs, tip, job, case, full, select=False):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("FLOW_", "GATE_", "HOLON_BUILD_", "HOLON_PBT_", "HOLON_GATE_", "HOLON_CARGO_", "HOLON_JUST_"))}
    shims = (HERE / "shims").resolve()
    env["PATH"] = os.pathsep.join(d for d in env["PATH"].split(os.pathsep) if Path(d).resolve() != shims)
    jobs = home / "jobs"
    jobs.mkdir(exist_ok=True)
    (jobs / f"{job}.log").touch()
    calls = home / f"calls-{job}"
    calls.touch()
    env.update(
        HOME=str(home), RUSTUP_HOME=os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
        CARGO_HOME=os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
        PATH=f"{stubs}:{env['PATH']}", REAL_JUST=shutil.which("just", path=env["PATH"]), RUSTC_WRAPPER="",
        FLOW_RUNTIME_DIR=str(home / "flow-runtime"), FLOW_JOB_ID=job, FLOW_JOB_LOG=str(jobs / f"{job}.log"),
        HOLON_BUILD_SLOTS="3", GATE_DISK_FLOOR_GIB="0", GATE_STEP_POLL_S="0.1", CALLS=str(calls),
        STUB_CLAIMS="1" if case["claims"] else "0", STUB_ATTESTERS=" ".join(case["attesters"]),
    )
    if full:
        env["GATE_FULL"] = "1"
    if select:
        (home / "selected").write_text("windowed-gate drawn\n")
        env.update(GATE_SELECT="1", GATE_SELECTED=str(home / "selected"))
    argv = [str(ORCH / "with-build-slot.sh"), "bash", str(INTEGRATE), f"gate-{job}", "1", tip]
    out = home / f"out-{job}.log"
    with out.open("w") as f:
        rc = subprocess.run(argv, cwd=repo, env=env, stdout=f, stderr=subprocess.STDOUT, timeout=600).returncode
    return rc, calls.read_text().split(), out


def check(stubs, n, case):
    home = SCRATCH / f"case{n}"
    (home / ".claude/skills/orchestrator").mkdir(parents=True)
    (home / ".claude/skills/orchestrator/scripts").symlink_to(ORCH)
    (home / ".claude/skills/gh-kanban").symlink_to(ORCH.parent.parent / "gh-kanban")
    (home / ".cache/holon-build").mkdir(parents=True)
    repo = home / "repo"
    tip = make_repo(repo)
    green = case["claims"] and all(k in case["attesters"] for k in ATTESTER)
    where = f"case {n} {case}: logs in {home}"

    rc1, calls1, out1 = run_gate(home, repo, stubs, tip, f"first{n}", case, full=True)
    text1 = out1.read_text()
    assert set(ATTESTER) <= set(calls1), f"run 1 did not run the attesting recipes {calls1}: {where}"
    assert (rc1 == 0) == green, f"run 1 exit {rc1}, expected green={green}: {where}"
    assert ("INTEGRATE GREEN" in text1) == green, where

    rc2, calls2, out2 = run_gate(home, repo, stubs, tip, f"second{n}", case, full=False)
    assert "step reuse: on" in out2.read_text(), where
    assert not (set(ATTESTER) | {"keystone-smoke"}) & set(calls2), f"run 2 ran steps it should reuse {calls2}: {where}"
    assert (rc2 == 0) == green, f"run 2 (reuse) exit {rc2}, run 1 exit {rc1}: {where}"

    rc3, calls3, out3 = run_gate(home, repo, stubs, tip, f"third{n}", case, full=False, select=True)
    text3 = out3.read_text()
    assert "windowed-gate" in calls3, f"the selected step did not run {calls3}: {where}"
    assert rc3 == 0 and text3.count("attestations not judged: selected run") == 1, f"selected run exit {rc3}: {where}"


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print(f"SEED={seed}")
    rng = random.Random(seed)
    stubs = SCRATCH / "stubs"
    stubs.mkdir()
    for tool in ("just", "cargo"):
        (stubs / tool).write_text(STUB)
        (stubs / tool).chmod(0o755)
    every = [{"claims": c, "attesters": list(a)} for c in (True, False) for r in (0, 1, 2)
             for a in itertools.combinations(ATTESTER, r)]
    fixed = [{"claims": True, "attesters": list(ATTESTER)},
             {"claims": True, "attesters": ["windowed-gate"]},
             {"claims": False, "attesters": list(ATTESTER)}]
    cases = fixed + [rng.choice(every) for _ in range(int(os.environ.get("CASES", 3)))]
    for n, case in enumerate(cases):
        check(stubs, n, case)
    print(f"[gate-integrate-attestations] PASS: {len(cases)} cases, seed {seed}")


if __name__ == "__main__":
    main()
