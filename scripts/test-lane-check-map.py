#!/usr/bin/env python3
"""Property check of scripts/lane-check-map.py, which decides whether a lane diff runs archlint or deny's advisories.

Run: `scripts/python311 scripts/test-lane-check-map.py` (SEED=<n>, CASES=<n>, default 200). Needs Python >= 3.11
(archlint reads TOML). Properties:
- P1 every file `archlint --all` reads in this tree (archlint.py collect_all_files) and every file under archlint/
  selects archlint: a lane that skips archlint never skips a file archlint would judge;
- P2 a path drawn inside a check's scope selects the check, and a path drawn outside it does not;
- P3 a drawn diff (a list of inside and outside paths) gets `run` iff one of its paths is inside, and the CLI prints
  `<check> skipped: diff touches none of <globs>` with every glob of the scope otherwise;
- P4 the CLI refuses an unknown check with a non-zero exit;
- P5 a diff that renames a path out of a check's scope runs the check: the path list that `just lane-checks`
  computes from a jj repo still reaches the rename's source.
PBT fit (property-based-testing/reference/when-to-use-pbt.md): the input is a set of paths with a crisp oracle (the
scope's membership, and archlint's own file list for P1), so it fits. Fixed near misses run first through P2.
"""
import importlib.util
import os
import random
import re
import subprocess
import tempfile
import sys
from pathlib import Path

assert sys.version_info >= (3, 11), f"archlint needs Python >= 3.11, running {sys.version}"

REPO = Path(__file__).resolve().parent.parent
SCRIPT = REPO / "scripts/lane-check-map.py"


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


lcm = load("lane_check_map", SCRIPT)
archlint = load("archlint", REPO / "archlint/archlint.py")
select_tests = load("select_tests", REPO / "scripts/select-tests.py")
cover_diff = load("tests_cover_diff", REPO / "scripts/flow-checks/tests-cover-diff.py")
STATIC_RENAMES = [("crates/a b/src/l\u00fcge.rs", "docs/z \u00fc/l\u00fcge.rs"), ("deny.toml", "notes/deny.toml")]

# (check, path, selected)
STATIC = [
    ("archlint", "Cargo.toml", True),
    ("archlint", "README.md", True),
    ("archlint", "archlint/rules/no-unwrap.yml", True),
    ("archlint", "crates/a/src/lib.rs", True),
    ("archlint", "frontends/gpui/Cargo.toml", True),
    ("archlint", "Cargo.lock", False),
    ("archlint", "crates/a/README.md", False),
    ("archlint", "docs/Testing/TimingCritical.md", False),
    ("archlint", "crates/a/src/lib.rs.orig", False),
    ("archlint", "guests/cooklang/src/lib.rs", False),
    ("archlint", "scripts/lane-check-map.py", False),
    ("deny-advisories", "Cargo.lock", True),
    ("deny-advisories", "frontends/holon-worker/Cargo.lock", True),
    ("deny-advisories", "guests/testkit/Cargo.toml", True),
    ("deny-advisories", "deny.toml", True),
    ("deny-advisories", "deny.toml.bak", False),
    ("deny-advisories", "crates/a/src/lib.rs", False),
    ("deny-advisories", "Cargo.lockfile", False),
]

WORDS = ["a", "holon-core", "src", "tests", "x_y", "gpui", "lib", "v2"]
EXTS = [".rs", ".md", ".py", ".toml", ".lock", ".yml", ".txt", ""]
ROOTS = ["crates", "frontends", "docs", "scripts", "guests", "archlint", ".github", "assets"]


def draw_inside(rng, check):
    """A path that one glob of the scope matches by construction."""
    glob = rng.choice(lcm.SCOPES[check])
    out, parts = [], glob.split("/")
    for i, part in enumerate(parts):
        if part == "**":
            out += [rng.choice(WORDS) for _ in range(rng.randint(1 if i == len(parts) - 1 else 0, 3))]
        else:
            out.append(part.replace("*", rng.choice(WORDS)))
    return "/".join(out)


def in_scope(check, path):
    """The scope written as plain path tests, independent of the glob translation."""
    parts = path.split("/")
    if check == "archlint":
        return (parts[0] in ("crates", "frontends") and len(parts) > 1 and (path.endswith(".rs") or parts[-1] == "Cargo.toml")
                or len(parts) == 1 and (path == "Cargo.toml" or path.endswith(".md"))
                or parts[0] == "archlint" and len(parts) > 1
                or path == "scripts/python311")
    return parts[-1] in ("Cargo.toml", "Cargo.lock") or path == "deny.toml"


def draw_any(rng):
    depth = rng.randint(0, 4)
    dirs = ([rng.choice(ROOTS)] if depth else []) + [rng.choice(WORDS) for _ in range(depth - 1)]
    name = rng.choice(["Cargo.toml", "Cargo.lock", "deny.toml", "python311", rng.choice(WORDS) + rng.choice(EXTS)])
    return "/".join(dirs + [name])


def draw_rename(rng, check):
    """A rename from inside the scope to a path outside it."""
    dst = draw_any(rng)
    while in_scope(check, dst):
        dst = draw_any(rng)
    return draw_inside(rng, check), dst


def recipe_paths(src, dst):
    """The changed paths that the lane-checks recipe computes for a commit that renames src to dst."""
    shown = subprocess.run(["just", "--justfile", str(REPO / "justfile"), "--show", "lane-checks"],
                           capture_output=True, text=True, check=True).stdout
    command = re.sub(r"\{\{ *base *\}\}", "@-", re.search(r"changed=\$\((.*)\)", shown).group(1))
    command = command.replace("scripts/select-tests.py", str(REPO / "scripts/select-tests.py"))
    with tempfile.TemporaryDirectory() as repo:
        run = lambda *argv: subprocess.run(argv, cwd=repo, capture_output=True, text=True, check=True).stdout
        run("jj", "git", "init", ".")
        (Path(repo) / src).parent.mkdir(parents=True, exist_ok=True)
        (Path(repo) / src).write_text("fn moved() {}\n" * 8)
        run("jj", "commit", "-m", "base")
        (Path(repo) / dst).parent.mkdir(parents=True, exist_ok=True)
        (Path(repo) / src).rename(Path(repo) / dst)
        return run("bash", "-c", command).split()


def cli(check, paths):
    return subprocess.run([sys.executable, str(SCRIPT), check], input="\n".join(paths), capture_output=True, text=True)


def selector_paths(src, dst):
    """(jj_changed, chain_paths) of a scratch jj repo whose one change renames src to dst.

    P6: the two Python path selectors list both the source and the destination of a rename."""
    with tempfile.TemporaryDirectory() as repo:
        run = lambda *argv: subprocess.run(argv, cwd=repo, capture_output=True, text=True, check=True).stdout
        run("jj", "git", "init", ".")
        root = Path(repo)
        (root / src).parent.mkdir(parents=True, exist_ok=True)
        (root / src).write_text("fn moved() {}\n" * 8)
        run("jj", "commit", "-m", "base")
        (root / dst).parent.mkdir(parents=True, exist_ok=True)
        (root / src).rename(root / dst)
        run("jj", "commit", "-m", "rename")
        cwd = os.getcwd()
        os.chdir(repo)
        try:
            [(_, chain)] = cover_diff.chain_paths("@--", "@-")
            return select_tests.jj_changed("@--"), chain
        finally:
            os.chdir(cwd)


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    cases = int(os.environ.get("CASES", 200))
    rng = random.Random(seed)
    failures = []

    read = [f.relative_to(archlint.REPO_ROOT).as_posix() for f in archlint.collect_all_files()]
    read += [p.relative_to(REPO).as_posix() for p in (REPO / "archlint").rglob("*") if p.is_file()]
    assert len(read) > 100, f"archlint reads only {len(read)} files: the oracle has nothing to judge"
    failures += [f"P1 archlint reads {p} but the map skips it" for p in read if not lcm.selects("archlint", p)]

    for check, path, want in STATIC:
        if lcm.selects(check, path) != want:
            failures.append(f"P2 static {check} {path}: want selected={want}")
    for _ in range(cases):
        for check in lcm.SCOPES:
            inside = draw_inside(rng, check)
            if not in_scope(check, inside) or not lcm.selects(check, inside):
                failures.append(f"P2 {check} {inside}: drawn inside, oracle={in_scope(check, inside)}")
            path = draw_any(rng)
            if lcm.selects(check, path) != in_scope(check, path):
                failures.append(f"P2 {check} {path}: map={lcm.selects(check, path)} oracle={in_scope(check, path)}")

    for _ in range(max(1, cases // 20)):
        check = rng.choice(sorted(lcm.SCOPES))
        diff = [draw_any(rng) for _ in range(rng.randint(1, 6))]
        r = cli(check, diff)
        want = "run" if any(in_scope(check, p) for p in diff) else (
            f"{check} skipped: diff touches none of {' '.join(lcm.SCOPES[check])}")
        if r.returncode != 0 or r.stdout.strip() != want:
            failures.append(f"P3 {check} {diff}: got rc={r.returncode} {r.stdout.strip()!r}, want {want!r}")

    for _ in range(max(1, cases // 40)):
        check = rng.choice(sorted(lcm.SCOPES))
        src, dst = draw_rename(rng, check)
        got = lcm.verdict(check, recipe_paths(src, dst))
        if got != "run":
            failures.append(f"P5 {check} rename {src} -> {dst}: got {got!r}, want 'run'")

    r = cli("no-such-check", ["Cargo.lock"])
    if r.returncode == 0:
        failures.append("P4 an unknown check exits 0")

    drawn = [draw_rename(rng, rng.choice(sorted(lcm.SCOPES))) for _ in range(max(1, cases // 40))]
    for src, dst in STATIC_RENAMES + drawn:
        for name, got in zip(("jj_changed", "chain_paths"), selector_paths(src, dst)):
            if src not in got or dst not in got:
                failures.append(f"P6 {name} rename {src} -> {dst}: got {got}, want both paths")

    for f in failures[:20]:
        print(f)
    print(f"test-lane-check-map: {len(failures)} failure(s) (SEED={seed}, CASES={cases}, {len(read)} archlint files)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
