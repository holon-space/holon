#!/usr/bin/env python3
"""scripts/check-gate-attestations.py over a temp attest dir: the exit code and the printed kinds equal the
set difference the test computes itself; selected gates do not judge; an unreadable record names its path.

Run: `python3 scripts/test-check-gate-attestations.py` (CASES=<n>, SEED=<s>).
"""
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

SUT = Path(__file__).resolve().parent / "check-gate-attestations.py"
KINDS = ["StructuralKeyBurst", "DeliverBlockContent", "Third", "Fourth"]
CLEAN_ENV = {k: v for k, v in os.environ.items() if k not in ("GATE_SELECT", "GATE_FULL")}
EXAMPLES = [
    (["StructuralKeyBurst", "DeliverBlockContent"], ["DeliverBlockContent"]),
    (["StructuralKeyBurst", "DeliverBlockContent"], []),
    (["StructuralKeyBurst"], ["StructuralKeyBurst", "DeliverBlockContent"]),
    (["StructuralKeyBurst", "DeliverBlockContent"], ["StructuralKeyBurst", "DeliverBlockContent"]),
    ([], ["StructuralKeyBurst"]),
    ([], []),
]


def run(claimed, attested, env=CLEAN_ENV):
    with tempfile.TemporaryDirectory() as d:
        root = Path(d)
        for sub, kinds in (("claimed", claimed), ("attested", attested)):
            if sub == "claimed" and not claimed:
                continue
            (root / sub).mkdir()
            for k in kinds:
                (root / sub / k).write_text(f"site of {k}\n")
        p = subprocess.run([sys.executable, str(SUT), str(root)], env=env, capture_output=True, text=True)
    return p.returncode, p.stdout


def check(claimed, attested):
    code, out = run(claimed, attested)
    gone = sorted(set(claimed) - set(attested))
    if not claimed:
        assert code == 2, (claimed, attested, code, out)
        return
    assert code == (1 if gone else 0), (claimed, attested, code, out)
    lines = sorted(out.splitlines())
    want = sorted(f"claimed kind {k} was attested by no gate step (reported at: site of {k})" for k in gone)
    assert lines == want, (claimed, attested, lines, want)


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print(f"SEED={seed}")
    rng = random.Random(seed)
    cases = list(EXAMPLES)
    for _ in range(int(os.environ.get("CASES", 40))):
        cases.append((rng.sample(KINDS, rng.randint(0, len(KINDS))), rng.sample(KINDS, rng.randint(0, len(KINDS)))))
    for claimed, attested in cases:
        check(claimed, attested)

    # A selected gate does not judge, however much is missing; a full gate (GATE_FULL=1) judges.
    code, out = run(["StructuralKeyBurst"], [], dict(CLEAN_ENV, GATE_SELECT="1"))
    assert (code, out) == (0, "attestations not judged: selected run\n"), (code, out)
    code, _ = run(["StructuralKeyBurst"], [], dict(CLEAN_ENV, GATE_SELECT="1", GATE_FULL="1"))
    assert code == 1, code

    with tempfile.TemporaryDirectory() as d:
        root = Path(d)
        (root / "claimed").mkdir()
        rec = root / "claimed" / "StructuralKeyBurst"
        rec.write_text("x\n")
        rec.chmod(0)
        p = subprocess.run([sys.executable, str(SUT), str(root)], env=CLEAN_ENV, capture_output=True, text=True)
        rec.chmod(0o644)
    assert p.returncode == 3 and str(rec) in p.stdout, (p.returncode, p.stdout)
    print(f"[check-gate-attestations] PASS: {len(cases)} cases, seed {seed}")


if __name__ == "__main__":
    main()
