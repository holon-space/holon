#!/usr/bin/env python3
"""An integrate run under the real build slot keeps its job's TMPDIR from the job's start to its end, whatever
the order and the way its tracks end.

Each case runs scripts/gate-integrate.sh inside the real with-build-slot.sh with FLOW_JOB_TMPDIR set, as
`flow job run` starts it, with FLOW_JOB_TMPDIR_OWNER drawn as the job's id, unset or another id, under a
scratch HOME (its own slot semaphore, hold marker and job registry) with `just` and `cargo` stubbed. The last step of every track sleeps a drawn time, records whether the job's TMPDIR
exists, and ends its track by exit 0, exit 1 or a TERM of the track. A drawn slot count of 2 makes one track
wait for its seat until the other one ends. The job removes its TMPDIR at its end only when the owner is its id.

Run: `python3 scripts/test-gate-integrate-tmpdir.py` (CASES=<n>, SEED=<s>, JOBS=<parallel cases>, KEEP=1 keeps the
scratch folder with each case's log). The fixed examples run first.
"""
import atexit
import os
import random
import re
import shutil
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
INTEGRATE = HERE / "gate-integrate.sh"
ORCH = Path.home() / ".claude/skills/orchestrator/scripts"
KINDS = ["exit0", "exit1", "term"]
_block = re.search(r"^TRACKS=\(\n(.*?)^\)", INTEGRATE.read_text(), re.M | re.S).group(1)
TRACKS = [[e.split(":")[1] for e in line.strip().strip('"').split()] for line in _block.splitlines()]
SCRATCH = Path(tempfile.mkdtemp(prefix="test-gate-integrate-tmpdir-"))
if not os.environ.get("KEEP"):
    atexit.register(shutil.rmtree, SCRATCH, ignore_errors=True)

STUB = r"""#!/usr/bin/env bash
case " $* " in *" --evaluate "*) exec "$REAL_JUST" "$@" ;; esac
[ "$(basename "$0")" = just ] || exit 0
for t in 0 1 2; do
  last=LAST_STEP_$t end=PROBE_END_$t
  [ "${1:-}" = "${!last}" ] || continue
  read -r delay kind <<<"${!end}"
  sleep "$delay"
  if [ -d "$PROBE_DIR" ]; then s=exists; else s=gone; fi
  echo "end $t $s" >>"$PROBE"
  case $kind in
    exit0) exit 0 ;;
    exit1) exit 1 ;;
    term)
      p=$PPID
      until ps -o args= -p "$p" | grep -q -- "--track $t\$"; do
        p=$(ps -o ppid= -p "$p" | tr -d ' ')
        [ "$p" -gt 1 ] || { echo "stub: no ancestor runs track $t" >&2; exit 3; }
      done
      kill -TERM "$p"; sleep 1; exit 1 ;;
  esac
done
"""


def make_repo(repo):
    """A gate keeps state in its repo's jj working copy and target/, and production runs one gate per workspace."""
    repo.mkdir()
    run = lambda *a: subprocess.run(a, cwd=repo, check=True, capture_output=True, text=True).stdout
    run("jj", "git", "init", "--quiet")
    (repo / ".gitignore").write_text("target/\n")
    (repo / "f").write_text("base\n")
    leftovers = repo / "scripts/check-target-leftovers.sh"
    leftovers.parent.mkdir()
    leftovers.write_text("#!/bin/sh\n")
    leftovers.chmod(0o755)
    run("jj", "commit", "--quiet", "-m", "base")
    return run("jj", "log", "-r", "@-", "--no-graph", "-T", "commit_id").strip()


def make_stubs():
    stubs = SCRATCH / "stubs"
    stubs.mkdir()
    for tool in ("just", "cargo"):
        (stubs / tool).write_text(STUB)
        (stubs / tool).chmod(0o755)
    return stubs


def case_env(home, stubs, job_tmp, probe, case):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("FLOW_", "GATE_", "HOLON_BUILD_", "HOLON_PBT_", "HOLON_GATE_", "HOLON_CARGO_"))}
    env.update(
        HOME=str(home), RUSTUP_HOME=os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
        CARGO_HOME=os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
        PATH=f"{stubs}:{env['PATH']}", REAL_JUST=shutil.which("just"),
        # An explicit empty wrapper skips the slot script's sccache probe compile; nothing here compiles.
        RUSTC_WRAPPER="",
        FLOW_RUNTIME_DIR=str(home / "flow-runtime"), FLOW_JOB_ID=f"tmpdir-prop.{home.name}",
        FLOW_JOB_TMPDIR=str(job_tmp), TMPDIR=str(job_tmp), PROBE_DIR=str(job_tmp), PROBE=str(probe),
        HOLON_BUILD_SLOTS=str(case["slots"]), GATE_DISK_FLOOR_GIB="0", GATE_STEP_POLL_S="0.1",
    )
    if case["owner"] != "unset":
        env["FLOW_JOB_TMPDIR_OWNER"] = env["FLOW_JOB_ID"] if case["owner"] == "own" else "another-job"
    for t, steps in enumerate(TRACKS):
        env[f"LAST_STEP_{t}"] = steps[-1]
        env[f"PROBE_END_{t}"] = f"{case['delay'][t]} {case['kind'][t]}"
    if case["selected"] != [0, 1, 2]:
        selection = home / "selected"
        selection.write_text("".join(f"{name} drawn\n" for t in case["selected"] for name in TRACKS[t]))
        env.update(GATE_SELECT="1", GATE_SELECTED=str(selection))
    return env


def run_case(stubs, n, case):
    home = SCRATCH / f"case{n}"
    repo = home / "repo"
    (home / ".claude/skills/orchestrator").mkdir(parents=True)
    (home / ".claude/skills/orchestrator/scripts").symlink_to(ORCH)
    # The slot script reads flow's job index from the gh-kanban skill beside it.
    (home / ".claude/skills/gh-kanban").symlink_to(ORCH.parent.parent / "gh-kanban")
    (home / ".cache/holon-build").mkdir(parents=True)
    tip = make_repo(repo)
    job_tmp = home / "fj-job"
    job_tmp.mkdir()
    probe = home / "probe"
    probe.touch()
    gate = ('"$0" "$@"; rc=$?; if [ -d "$PROBE_DIR" ]; then s=exists; else s=gone; fi; '
            'echo "gate-end exit=$rc $s" >>"$PROBE"; exit $rc')
    argv = [str(ORCH / "with-build-slot.sh"), "bash", "-c", gate, str(INTEGRATE), f"gate{n}", "1", tip]
    out = home / "out.log"
    with out.open("w") as f:
        subprocess.run(argv, cwd=repo, env=case_env(home, stubs, job_tmp, probe, case), stdout=f,
                       stderr=subprocess.STDOUT, timeout=300)
    return probe.read_text().split("\n")[:-1], job_tmp.exists(), out


def check(stubs, n, case):
    lines, left, out = run_case(stubs, n, case)
    where = f"case {n} {case}: probe {lines}, log {out}"
    ends = sorted(int(line.split()[1]) for line in lines if line.startswith("end "))
    assert ends == case["selected"], f"the probe did not see every track's end: {where}"
    assert all(line.endswith(" exists") for line in lines), f"the job's TMPDIR was gone before the job ended: {where}"
    assert lines[-1].startswith("gate-end "), f"no probe after the gate's last track: {where}"
    assert left == (case["owner"] != "own"), \
        f"the job's TMPDIR {'outlived' if left else 'did not outlive'} the job: {where}"


def draw(rng):
    return {"delay": [rng.choice([0, 0.5, 1, 2, 3]) for _ in range(3)], "kind": [rng.choice(KINDS) for _ in range(3)],
            "slots": rng.choice([2, 6]), "owner": rng.choice(["own", "own", "unset", "other"]),
            "selected": rng.choice([[0, 1, 2], [0, 1, 2], [0, 1], [0, 2], [1, 2], [1], [2]])}


# The evidence run (track 2 ends first while track 1 still runs), the same with one track waiting for its seat,
# a TERM-ed and a red track, and a folder that the job does not own (owner unset, another id).
EXAMPLES = [
    {"delay": [1, 3, 0], "kind": ["exit0", "exit0", "exit0"], "slots": 6, "selected": [0, 1, 2], "owner": "own"},
    {"delay": [0, 3, 0], "kind": ["exit0", "exit0", "exit0"], "slots": 2, "selected": [0, 1, 2], "owner": "own"},
    {"delay": [3, 0, 1], "kind": ["exit0", "term", "exit1"], "slots": 6, "selected": [0, 1, 2], "owner": "own"},
    {"delay": [1, 3, 0], "kind": ["exit0", "exit0", "exit0"], "slots": 6, "selected": [0, 1, 2], "owner": "unset"},
    {"delay": [0, 1, 0], "kind": ["exit0", "exit1", "exit0"], "slots": 2, "selected": [0, 1, 2], "owner": "other"},
]


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "50"))
    stubs = make_stubs()
    for n, case in enumerate(EXAMPLES):
        check(stubs, 1000 + n, case)
    rng = random.Random(seed)
    drawn = [draw(rng) for _ in range(cases)]
    with ThreadPoolExecutor(max_workers=int(os.environ.get("JOBS", "8"))) as pool:
        for f in [pool.submit(check, stubs, n, case) for n, case in enumerate(drawn)]:
            f.result()
    print(f"ok: {len(EXAMPLES)} examples and {cases} cases (SEED={seed})")


if __name__ == "__main__":
    main()
