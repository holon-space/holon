#!/usr/bin/env python3
"""Property test of `bugfunnel.py check` and `counts` over the `model` and `spec` fields.

Run: uv run --no-project --with pyyaml --with hypothesis python scripts/test_bugfunnel.py
"""

import contextlib
import io
import itertools
import sys
import tempfile
from pathlib import Path

from hypothesis import example, given, settings, strategies as st

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bugfunnel

VALID_MODELS = ("none", "UNMODELLED", "SPEC-COVERAGE", "SPEC-ORACLE", "CONFORMANCE")
NEEDS_SPEC = ("SPEC-COVERAGE", "SPEC-ORACLE", "CONFORMANCE")
ABSENT = "absent"
MODEL_DRAWS = (ABSENT, "null") + VALID_MODELS + ("not-a-class",)
SPEC_DRAWS = (ABSENT, "existing", "missing")
VALID_MODEL_DRAWS = (ABSENT,) + VALID_MODELS


def expected_failures(model, spec):
    """Fields that must be reported for one entry, by the rules of the sheet."""
    fields = set()
    if model not in (ABSENT,) + VALID_MODELS:
        fields.add("model")
    if spec != ABSENT and model == ABSENT:
        fields.add("spec")
    if model in NEEDS_SPEC and spec != "existing":
        fields.add("spec")
    return fields


def write_entries(root, draws):
    existing = root / "ExistingSpec.tla"
    existing.write_text("")
    entries = root / "bugfunnel" / "entries"
    entries.mkdir(parents=True)
    names = []
    for i, (model, spec) in enumerate(draws):
        name = f"2026-01-01-entry-{i}"
        lines = ["---", f"id: {name}", "date: 2026-01-01", "gap: ORACLE", "status: FIXED", "summary: s"]
        if model == "null":
            lines.append("model:")
        elif model != ABSENT:
            lines.append(f"model: {model}")
        if spec == "existing":
            lines.append(f"spec: {existing}")
        elif spec == "missing":
            lines.append(f"spec: {root}/Missing.tla")
        lines += ["---", ""]
        (entries / f"{name}.md").write_text("\n".join(lines))
        names.append(f"{name}.md")
    return root / "bugfunnel", names


def run(*argv):
    out = io.StringIO()
    old = sys.argv
    sys.argv = ["bugfunnel.py", *argv]
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rc = bugfunnel.main()
    finally:
        sys.argv = old
    return rc, out.getvalue()


ALL_PAIRS = list(itertools.product(MODEL_DRAWS, SPEC_DRAWS))


def with_every_pair(test):
    for pair in ALL_PAIRS:
        test = example(draws=[pair])(test)
    return test


@with_every_pair
@settings(max_examples=60, deadline=None)
@given(draws=st.lists(st.sampled_from(ALL_PAIRS), min_size=1, max_size=8))
def test_check_enforces_model_and_spec_rules(draws):
    with tempfile.TemporaryDirectory() as tmp:
        funnel, names = write_entries(Path(tmp), draws)
        rc, out = run("check", "--dir", str(funnel))
    lines = out.splitlines()
    any_bad = False
    for name, (model, spec) in zip(names, draws):
        bad = expected_failures(model, spec)
        reported = [line for line in lines if line.startswith(f"{name}:")]
        for field in ("model", "spec"):
            hits = [line for line in reported if line.startswith(f"{name}: `{field}`")]
            assert bool(hits) == (field in bad), (name, model, spec, field, lines)
        if "model" in bad:
            value = None if model == "null" else model
            assert any(repr(value) in line for line in reported if line.startswith(f"{name}: `model`")), (name, model, lines)
        any_bad |= bool(bad)
    assert (rc != 0) == any_bad, (draws, rc, lines)
    assert lines[-1] == f"{len(draws)} entries, {len(lines) - 1} problems"


@with_every_pair
@settings(max_examples=60, deadline=None)
@given(draws=st.lists(st.sampled_from(ALL_PAIRS), min_size=1, max_size=8))
def test_counts_model_line_sums_to_entries(draws):
    draws = [d for d in draws if not expected_failures(*d)]
    if not draws:
        return
    with tempfile.TemporaryDirectory() as tmp:
        funnel, _ = write_entries(Path(tmp), draws)
        rc, out = run("counts", "--dir", str(funnel))
    assert rc == 0, out
    line = [l for l in out.splitlines() if l.startswith("MODEL CLASSES:")]
    assert len(line) == 1, out
    counts = dict(part.rsplit(" ", 1) for part in line[0].split(":", 1)[1].split(","))
    counts = {k.strip(): int(v) for k, v in counts.items()}
    assert sum(counts.values()) == len(draws), (line, draws)
    assert counts["not asked"] == sum(1 for m, _ in draws if m == ABSENT), (line, draws)
    for cls in VALID_MODELS:
        assert counts[cls] == sum(1 for m, _ in draws if m == cls), (line, draws)


INVALID_PAIRS = [pair for pair in ALL_PAIRS if expected_failures(*pair)]


def sets_with_an_invalid_entry():
    return st.tuples(
        st.lists(st.sampled_from(ALL_PAIRS), max_size=7),
        st.sampled_from(INVALID_PAIRS),
        st.integers(min_value=0, max_value=7),
    ).map(lambda t: t[0][: t[2]] + [t[1]] + t[0][t[2]:])


def with_every_invalid_pair(test):
    for pair in INVALID_PAIRS:
        test = example(draws=[pair])(test)
    return test


@with_every_invalid_pair
@settings(max_examples=60, deadline=None)
@given(draws=sets_with_an_invalid_entry())
def test_counts_refuses_a_set_with_an_invalid_entry(draws):
    with tempfile.TemporaryDirectory() as tmp:
        funnel, _ = write_entries(Path(tmp), draws)
        rc, out = run("counts", "--dir", str(funnel))
    assert rc != 0, (draws, out)
    assert not any(l.startswith("MODEL CLASSES:") for l in out.splitlines()), (draws, out)


if __name__ == "__main__":
    test_check_enforces_model_and_spec_rules()
    test_counts_model_line_sums_to_entries()
    test_counts_refuses_a_set_with_an_invalid_entry()
    print("OK")
