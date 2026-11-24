#!/usr/bin/env python3
"""Each one-token mutant of scripts/keystone-known-reds.sh turns scripts/known-reds-property-checks.py red.

Run: `python3 scripts/test-known-reds-mutants.py` (SEED=<s> replays a draw). Assures what the checks' own PASS cannot:
that they can go red. Every MUTANT is applied once to a copy of the classifier inside a tree that shares the
repository's other scripts and docs; the property checks run with CLASSIFIER=<copy> on the printed seed and must exit
non-zero, and must exit zero on the unmutated classifier. A mutant that survives is a failure, so a survivor is
removed from MUTANTS only with a note here.
It runs by hand only, after a change to the classifier or to a property-check driver, never in `just cargo-free-checks`:
each mutant is a whole property-checks run, and it cannot be a driver of the runner it mutates.
"""
import os
import random
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
CLASSIFIER = "keystone-known-reds.sh"
# (name, text in the classifier that occurs exactly once, its replacement)
MUTANTS = [
    ("cross-family-exit-flipped", 'if [ "$cross" -ne 0 ]; then', 'if [ "$cross" -eq 0 ]; then'),
    ("novel-exit-flipped", 'if [ "$novel" -ne 0 ]; then', 'if [ "$novel" -eq 0 ]; then'),
    ("no-verdict-exit-flipped", 'if [ "$no_verdict" -ne 0 ]; then\n    echo "[known-reds] NO VERDICT', 'if [ "$no_verdict" -eq 0 ]; then\n    echo "[known-reds] NO VERDICT'),
    ("stale-flag-dropped", "|| stale_only=1", "|| stale_only=0"),
    ("matcher-keeps-last-accepting-row", "{ hit = i; break }", "{ hit = i; continue }"),
    ("site-anchor-inverted", 'if (site[i] == "" || loc ~ site[i])', 'if (site[i] == "" || loc !~ site[i])'),
    ("name-boundary-inverted", 'substr(t, p, 1) !~ /[A-Za-z0-9_:-]/ || substr(t, p, 5) == "test "', 'substr(t, p, 1) ~ /[A-Za-z0-9_:-]/ || substr(t, p, 5) == "test "'),
    ("note-line-lost", 'echo "[known-reds] PASS-WITH-NOTE:', 'echo "[known-reds] PASS:'),
]


def mutant_tree(work, name, old, new):
    text = (SCRIPTS / CLASSIFIER).read_text()
    assert text.count(old) == 1, f"{name}: want one occurrence of {old!r}, found {text.count(old)}"
    tree = work / name
    (tree / "scripts").mkdir(parents=True)
    for f in SCRIPTS.iterdir():
        if f.name != CLASSIFIER:
            (tree / "scripts" / f.name).symlink_to(f)
    (tree / "docs").symlink_to(SCRIPTS.parent / "docs")
    copy = tree / "scripts" / CLASSIFIER
    copy.write_text(text.replace(old, new))
    copy.chmod(0o755)
    return copy


def checks(seed, classifier=None):
    env = {**os.environ, "SEED": str(seed)}
    if classifier:
        env["CLASSIFIER"] = str(classifier)
    return subprocess.run(["python3", str(SCRIPTS / "known-reds-property-checks.py")], env=env, capture_output=True, text=True)


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print(f"drawn with seed {seed} ({len(MUTANTS)} mutants)", flush=True)
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        copies = [(name, mutant_tree(work, name, old, new)) for name, old, new in MUTANTS]
        with ThreadPoolExecutor(max_workers=len(copies) + 1) as pool:
            base = pool.submit(checks, seed)
            runs = [(name, pool.submit(checks, seed, copy)) for name, copy in copies]
            if base.result().returncode != 0:
                failed += 1
                print(f"the unmutated classifier fails the checks:\n{base.result().stdout}")
            for name, run in runs:
                r = run.result()
                if r.returncode == 0:
                    failed += 1
                    print(f"SURVIVED {name}")
                else:
                    reds = [l for l in r.stdout.splitlines() if l.startswith("FAIL ")]
                    print(f"killed {name}: {', '.join(reds)}")
    print(f"[known-reds-mutants] {'FAIL' if failed else 'PASS'}: {failed} failing case(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
