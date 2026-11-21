#!/usr/bin/env python3
"""Property check of scripts/light-paths-gate.py, the refusal of a light card's move into `verify`.

Run from the workspace root: `python3 scripts/test-light-paths-gate.py` (SEED=<n>, CASES=<n>, default 30).
A stub `gh` on PATH answers the REST pull-request calls with the drawn file list of each PR head (a rename carries
`previous_filename`, the list is paged), so no network is used. Change sets are drawn from the tracked files of this tree
plus near-miss and deleted paths and renames, in 1 to 3 heads. Properties:
- P1 the verdict is "refuse" iff `gate_selection` of the union of all old and new paths is a full run or holds a step
  outside TREE_STEPS;
- P2 with several heads it is "refuse" iff one head alone is refused;
- P3 adding a path to a refused set keeps it refused;
- P4 every path the refusal names selects a relevant step on its own;
- P5 from a passing run, each missing or malformed input makes the script exit non-zero, never 0, also under
  PYTHONOPTIMIZE=1.
Fixed cases run first. Each failing case is printed as its explicit path lists.
"""
import contextlib
import importlib.util
import io
import json
import os
import random
import stat
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path, PurePosixPath

from jj_read import tree_files

REPO = Path(__file__).resolve().parent.parent
SCRIPT = REPO / "scripts/light-paths-gate.py"

spec = importlib.util.spec_from_file_location("light_paths_gate", SCRIPT)
lpg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lpg)
ST = lpg.ST
META = ST.load_metadata()
lpg.ST.load_metadata = lambda: META

# The changed files of a merged PR that changed a crate's tests, a manifest and a gate script.
CRATE_TEST_PR = [
    "Cargo.lock",
    "crates/holon-orgmode/tests/empty_file_is_not_a_document.rs",
    "crates/holon-orgmode/tests/vault_file_emptied_disclosure.rs",
    "deny.toml",
    "scripts/keystone-known-reds.sh",
]
def joined(*parts):
    return "/".join(parts)


# Built by joining: a path written whole in a script a gate step runs counts as named by that step (names_path),
# which would make these paths gate-relevant.
DOC = joined("docs", "Plans", "new-plan.md")
INERT_NEAR_MISS = [DOC, joined("docs", "Plans", "gone.md"), "README.md", joined("assets", "logo.png")]
NEAR_MISS = INERT_NEAR_MISS + ["scripts/never-existed.py", "crates/no-such-crate/src/lib.rs", "justfile.orig", "Cargo.toml.bak"]
STUB_GH = """#!/bin/sh
# stub gh: `gh api [--paginate --slurp] repos/<o>/<r>/pulls/<N>[/files]` answers from $GH_STUB_DIR; no file means failure.
for last; do :; done
number=${last#repos/*/*/pulls/}
number=${number%/files}
case "$last" in
  */files) file="$GH_STUB_DIR/$number.files.json" ;;
  *) file="$GH_STUB_DIR/$number.json" ;;
esac
[ -f "$file" ] || { echo "stub gh: no PR $number" >&2; exit 1; }
cat "$file"
"""
FLOW = {"FLOW_ISSUE": "7", "FLOW_TO": "verify"}


class World:
    """A temp dir holding the stub `gh` and one answer file per PR number."""

    def __init__(self):
        self.dir = Path(tempfile.mkdtemp(prefix="light-paths-gate-"))
        gh = self.dir / "bin/gh"
        gh.parent.mkdir()
        gh.write_text(STUB_GH)
        gh.chmod(gh.stat().st_mode | stat.S_IXUSR)

    def pr(self, number, entries, changed=None):
        """`entries`: a path, or an `(old, new)` pair for a rename. The file list is served in pages of two."""
        files = [{"filename": e, "status": "modified"} if isinstance(e, str)
                 else {"filename": e[1], "previous_filename": e[0], "status": "renamed"} for e in entries]
        pages = [files[i:i + 2] for i in range(0, len(files), 2)]
        (self.dir / f"{number}.files.json").write_text(json.dumps(pages))
        (self.dir / f"{number}.json").write_text(json.dumps({"changed_files": len(files) if changed is None else changed}))

    def env(self, numbers, **override):
        heads = " ".join(f"o/r#{n}@{n:040x}" for n in numbers)
        env = {**os.environ, **FLOW, "FLOW_PR_HEADS": heads, "GH_STUB_DIR": str(self.dir),
               "PATH": f"{self.dir / 'bin'}:{os.environ['PATH']}", "HOLON_CARGO_RECORD": str(self.dir / "cargo-calls.jsonl")}
        env.update(override)
        return {k: v for k, v in env.items() if v is not None}


WORLD = World()
os.environ["HOLON_CARGO_RECORD"] = str(WORLD.dir / "cargo-calls.jsonl")


def old_and_new(sets):
    """Every old and new path of the heads in `sets`."""
    return sorted({p for s in sets for e in s for p in ([e] if isinstance(e, str) else e)})


def verdict(sets, **override):
    """(exit code, stderr) of the script in this process, one PR head per entry list in `sets`."""
    for n, paths in enumerate(sets, 1):
        WORLD.pr(n, paths)
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        code = lpg.run(WORLD.env(range(1, len(sets) + 1), **override))
    return code, err.getvalue()


def subprocess_verdict(env):
    """(exit code, stderr) of the script as flow runs it."""
    done = subprocess.run([sys.executable, str(SCRIPT)], cwd=REPO, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    return done.returncode, done.stderr


def oracle_refuses(paths):
    with contextlib.redirect_stderr(io.StringIO()):
        selected = ST.gate_selection(REPO, META, lpg.GATE, paths)
    return any(step == "*" or step not in ST.TREE_STEPS for step, _, _ in selected)


def refused_paths(stderr):
    lines = stderr.splitlines()
    return [l.split(": ", 1)[0] for l in lines[lines.index(next(l for l in lines if "gate-relevant" in l)) + 1:] if ": " in l and not l.startswith("repair: ")]


def problems(sets):
    found = []
    code, err = verdict(sets)
    assert "TREE: " in err, f"the script did not run to its verdict: {err!r}"
    want = oracle_refuses(old_and_new(sets))
    if (code != 0) != want:
        found.append(f"P1: exit {code}, but gate_selection of the union refuses={want}")
    if len(sets) > 1:
        alone = [verdict([s])[0] != 0 for s in sets]
        if (code != 0) != any(alone):
            found.append(f"P2: exit {code}, heads alone refuse {alone}")
    if code != 0:
        if f"repair: flow set-work-type {FLOW['FLOW_ISSUE']} code" not in err:
            found.append(f"refusal lacks the repair command: {err!r}")
        found += [f"P4: {p} is named but selects no relevant step alone" for p in refused_paths(err) if not oracle_refuses([p])]
    return found


def monotone_problems(rng, sets, pool):
    if verdict(sets)[0] == 0:
        return []
    grown = [sets[0] + [rng.choice(pool)], *sets[1:]]
    return [] if verdict(grown)[0] != 0 else [f"P3: grown to {grown} the verdict is a pass"]


def missing_input_problems(paths):
    """P5, starting from the passing state: each input then breaks on its own, with and without PYTHONOPTIMIZE."""
    found = []
    WORLD.pr(1, paths)
    ok = WORLD.env([1])
    for optimize in (None, "1"):
        code, err = subprocess_verdict({**ok, **({"PYTHONOPTIMIZE": optimize} if optimize else {})})
        if code != 0 or "TREE: " not in err:
            return [f"P5: the passing start state exits {code} (PYTHONOPTIMIZE={optimize}): {err!r}"]
    WORLD.pr(3, [])
    WORLD.pr(4, paths, changed=len(paths) + 1)
    for name, env in [
        ("FLOW_PR_HEADS unset", WORLD.env([1], FLOW_PR_HEADS=None)),
        ("FLOW_PR_HEADS empty", WORLD.env([1], FLOW_PR_HEADS="")),
        ("FLOW_PR_HEADS blank", WORLD.env([1], FLOW_PR_HEADS="  ")),
        ("head without sha", WORLD.env([1], FLOW_PR_HEADS="o/r#1")),
        ("head with a bad number", WORLD.env([1], FLOW_PR_HEADS="o/r#x@abc")),
        ("FLOW_TO landed", WORLD.env([1], FLOW_TO="landed")),
        ("FLOW_TO build", WORLD.env([1], FLOW_TO="build")),
        ("FLOW_TO empty", WORLD.env([1], FLOW_TO="")),
        ("FLOW_TO unset", WORLD.env([1], FLOW_TO=None)),
        ("gh fails", WORLD.env([2])),
        ("one head fails, one passes", WORLD.env([1, 2])),
        ("a PR with no file", WORLD.env([3])),
        ("a file list shorter than the PR's changed-file count", WORLD.env([4])),
    ]:
        for optimize in (None, "1"):
            code, err = subprocess_verdict({**env, **({"PYTHONOPTIMIZE": optimize} if optimize else {})})
            if code == 0:
                found.append(f"P5: {name} (PYTHONOPTIMIZE={optimize}): exit 0, stderr {err!r}")
    return found


def explicit_input_problems(paths):
    """Each broken input, alone and beside a passing head, with and without PYTHONOPTIMIZE: exit non-zero with its own message."""
    found = []
    WORLD.pr(1, paths)
    WORLD.pr(3, [])
    WORLD.pr(4, paths, changed=len(paths) + 1)
    good = f"o/r#1@{1:040x}"
    for name, env, message in [
        ("head without sha", WORLD.env([1], FLOW_PR_HEADS=f"{good} o/r#1"), "is not owner/name#N@sha"),
        ("FLOW_PR_HEADS blank", WORLD.env([1], FLOW_PR_HEADS="  "), "no linked PR head"),
        ("a PR with no file", WORLD.env([1, 3]), "lists no changed file"),
        ("a short file list", WORLD.env([1, 4]), "changed files"),
        ("FLOW_TO landed", WORLD.env([1], FLOW_TO="landed"), "'landed': this gate belongs on the move into verify"),
    ]:
        for optimize in (None, "1"):
            code, err = subprocess_verdict({**env, **({"PYTHONOPTIMIZE": optimize} if optimize else {})})
            if code == 0 or message not in err:
                found.append(f"P5: {name} (PYTHONOPTIMIZE={optimize}): exit {code} without {message!r}, stderr {err!r}")
    return found


def check_line_commands():
    """The `./` words of the justfile's `check <name> <log> <command...>` lines: scripts run directly, also behind a wrapper."""
    for line in (REPO / "justfile").read_text().splitlines():
        words = line.split()
        if len(words) >= 4 and words[0] == "check":
            yield from (w for w in words[3:] if w.startswith("./"))


def runnable_problems():
    return [f"{cmd} is run by a justfile check line but is not executable" for cmd in check_line_commands()
            if not os.access(REPO / cmd, os.X_OK)]


FLOW_SKILL = Path.home() / ".claude/skills/gh-kanban"
GATE_COMMAND = f"""
[gates.commands.light-paths]
run = ["python3", "{SCRIPT}"]
cwd = "{REPO}"
timeout = "120s"
"""


def flow_config_error(stage):
    """The ConfigError text of flow's own config loader (the one `flow check` uses) for a light card gated on `stage`;
    None when it loads. A fixed pair of configs: the stage is the only input."""
    sys.path.insert(0, str(FLOW_SKILL))
    from flow import model

    base = (FLOW_SKILL / "flow/fixtures/holon-flow.toml").read_text()
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "flow.toml"
        path.write_text(f'{base}{GATE_COMMAND}\n[child.light.gates]\n{stage} = ["command:light-paths"]\n')
        try:
            model.load_config(path)
        except model.ConfigError as e:
            return str(e)
    return None


def config_load_problems():
    """The gate line on `verify` loads; the same line on the terminal stage `landed` is refused by flow."""
    found = []
    if (err := flow_config_error("verify")) is not None:
        found.append(f"flow refuses the light-paths gate on verify: {err}")
    err = flow_config_error("landed")
    if err is None:
        found.append("flow accepts the light-paths gate on the terminal stage landed")
    elif "terminal stage" not in err:
        found.append(f"flow refuses the gate on landed for another reason: {err}")
    return found


RELEVANT_POOLS = ["scripts", "crate src", "crate tests", "root", "machinery", "near miss", "docs testing"]


def pools():
    files = sorted(p for p in tree_files(REPO) if (REPO / p).is_file())
    dirs, _ = ST.workspace_graph(META)
    crate_src, crate_tests = [], []
    for p in files:
        o = ST.owner(PurePosixPath(p), dirs)
        if o and p.endswith(".rs"):
            (crate_tests if dirs[o] / "tests" in PurePosixPath(p).parents else crate_src).append(p)
    found = {
        "docs": [p for p in files if p.startswith("docs/") and not p.startswith("docs/Testing/")],
        "docs testing": [p for p in files if p.startswith("docs/Testing/")],
        "scripts": [p for p in files if p.startswith("scripts/")],
        "crate src": crate_src,
        "crate tests": crate_tests,
        "root": [p for p in files if "/" not in p or p.startswith(".cargo/")],
        "machinery": sorted(ST.GATE_MACHINERY | {"justfile"}),
        "near miss": NEAR_MISS,
        "inert near miss": INERT_NEAR_MISS,
    }
    assert not [k for k, v in found.items() if not v], f"empty pools in {found}"
    return found


def draw_sets(rng, pools_):
    """1 to 3 heads; each draws up to 4 paths from the inert pools, now and then a path from a pool that selects work, and
    now and then a rename from such a pool to an inert docs path."""
    heads = []
    for _ in range(rng.randint(1, 3)):
        paths = rng.sample(pools_["docs"], rng.randint(0, 2)) + rng.sample(pools_["inert near miss"], rng.randint(0, 2))
        if rng.random() < 0.3:
            paths += rng.sample(pools_[rng.choice(RELEVANT_POOLS)], 1)
        renames = [(rng.choice(pools_[rng.choice(RELEVANT_POOLS)]), rng.choice(pools_["docs"]))] if rng.random() < 0.3 else []
        heads.append(sorted(set(paths)) + renames or [rng.choice(pools_["docs"])])
    return heads


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    pools_ = pools()
    flat = [p for v in pools_.values() for p in v]
    fixed = [[CRATE_TEST_PR], [[DOC]], [["justfile"]], [[DOC], ["justfile"]], [[(CRATE_TEST_PR[2], DOC)]], [[(DOC, CRATE_TEST_PR[2])]]]
    drawn = [draw_sets(rng, pools_) for _ in range(int(os.environ.get("CASES", "30")))]
    print(f"drawn with seed {seed}; pool sizes {({k: len(v) for k, v in pools_.items()})}")
    failures = []
    for sets in fixed + drawn:
        failures += [f"{sets}: {f}" for f in problems(sets) + monotone_problems(rng, sets, flat)]
    for sets, refuses in zip(fixed, [True, False, True, True, True, True]):
        if (verdict(sets)[0] != 0) != refuses:
            failures.append(f"fixed case {sets}: expected refuse={refuses}")
    code, err = verdict([CRATE_TEST_PR])
    if "crates/holon-orgmode/tests/vault_file_emptied_disclosure.rs" not in err:
        failures.append(f"the crate-test PR refusal does not name its test path: {err!r}")
    failures += missing_input_problems([DOC])
    failures += runnable_problems()
    failures += config_load_problems()
    failures += explicit_input_problems([DOC])
    print(f"refused draws: {Counter(verdict(s)[0] != 0 for s in drawn)}")
    for f in failures:
        print(f"FAIL: {f}")
    print(f"test-light-paths-gate: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
