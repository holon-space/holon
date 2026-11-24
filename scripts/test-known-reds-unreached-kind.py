#!/usr/bin/env python3
"""A case of `kinds` in test-known-reds-classifier.py that no draw reaches fails
the run: the floor check refuses a generator that lost a draw kind.

Run: `python3 scripts/test-known-reds-unreached-kind.py` (SEED=<s>, CASES=<n> random cases, default 20).
The fixed kind name runs first, through every check of the classifier driver. Then CASES random kind names, each
with its own draw seed, run only the floor check: the driver's other checks do not read the kind.
"""
import contextlib
import importlib.util
import io
import os
import random
import string
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
KIND_FREE = ["stale_only_failures", "broken_table_failures", "stale_location_failures", "path_spelling_location_failures", "twin_failures"]
sys.path.insert(0, str(HERE))


def run_without(kind, seed, floor_only):
    spec = importlib.util.spec_from_file_location("classifier_property", HERE / "test-known-reds-classifier.py")
    prop = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(prop)
    prop.ATTEMPTS = 40
    if floor_only:
        for owner, name, stub in [(prop, n, lambda *a: []) for n in KIND_FREE] + [(prop, "check_draw", lambda *a: ([], "")), (prop.logs, "examples", lambda: [])]:
            assert hasattr(owner, name), f"{owner.__name__} has no {name}"
            setattr(owner, name, stub)
    reach = prop.kinds
    def kinds(runs, clean, mixed):
        k = reach(runs, clean, mixed)
        k[kind] += 0
        return k

    prop.kinds = kinds
    os.environ.update(SEED=str(seed), CASES="0")
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        try:
            prop.main()
        except SystemExit as e:
            return e.code, out.getvalue()
    return 0, out.getvalue()


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    cases = int(os.environ.get("CASES", 20))
    rng = random.Random(seed)
    drawn = [(" ".join("".join(rng.choice(string.ascii_lowercase) for _ in range(rng.randint(2, 7))) for _ in range(3)), rng.randrange(1 << 30)) for _ in range(cases)]
    failed = 0
    for kind, draw_seed, floor_only in [("a kind no draw reaches", seed, False)] + [(k, s, True) for k, s in drawn]:
        code, out = run_without(kind, draw_seed, floor_only)
        if code != 1 or f"no draw of 40 reaches {kind}\n" not in out:
            failed += 1
            print(f"kind {kind!r}: want exit 1 and `no draw of 40 reaches {kind}`, got exit {code}")
    print(f"[unreached-kind] {'FAIL' if failed else 'PASS'}: {failed} of {1 + cases} kind(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
