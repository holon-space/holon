#!/usr/bin/env python3
"""Properties of archlint's test-only scoping (`cfg_test_spans`, `is_cfg_test_module_file`, `filter_test_mods`) over
drawn module trees.

Run: `scripts/python311 scripts/test-archlint-test-modules.py` (CASES=<n>, SEED=<s>). Needs Python >= 3.11 (archlint
reads TOML).
Each draw writes a parent module file (layout `x.rs` with `x/`, `x/mod.rs`, `lib.rs` or `main.rs`) and its child
files into a temp tree, and works with that tree as the working directory (archlint reads paths relative to it). The
generator records, per line, whether it is test code; that record is the oracle:
- `cfg_test_spans` of the parent equals the recorded `#[cfg(test)]` item ranges, in order;
- `is_cfg_test_module_file` is True exactly for the children declared inside a `#[cfg(test)]` item, and False for a
  file named `lib.rs`, `main.rs` or `mod.rs`;
- `filter_test_mods` drops a diag of a `skip_test_mods` rule iff the record marks its line test code (a child declared
  as a test module is test code as a whole), keeps every diag of another rule, and keeps the input order.
Parent segments: a production fn, `mod <child>;`, `#[cfg(test)] mod <child>;` with a drawn visibility and 0-2 extra
attribute lines, an inline `#[cfg(test)] mod` with 0-3 nested brace levels, a brace-less `#[cfg(test)] use`, and an
unterminated `#[cfg(test)]` item at the end of the file. The fixed examples run first through the same properties.
Known limits, not drawn: a brace inside a string literal or comment shifts a span; an attribute on the same line as
its item (rustfmt splits it; the gate runs `cargo fmt --check`); a directory name with a dot; a test module declared in
a parent of none of the four layouts.
"""
import contextlib
import importlib.util
import os
import random
import sys
import tempfile
from pathlib import Path

assert sys.version_info >= (3, 11), f"archlint needs Python >= 3.11, running {sys.version}"

ARCHLINT = Path(__file__).resolve().parent.parent / "archlint/archlint.py"
LAYOUTS = {"x.rs": ("src/x.rs", "src/x"), "x/mod.rs": ("src/x/mod.rs", "src/x"), "lib.rs": ("src/lib.rs", "src"),
           "main.rs": ("src/main.rs", "src")}
VISIBILITY = ["", "pub ", "pub(crate) "]
EXTRA_ATTRS = ["#[allow(dead_code)]", "#[rustfmt::skip]"]
SMELLS = [{"id": "scoped", "skip_test_mods": True}, {"id": "free"}]


def load_archlint():
    spec = importlib.util.spec_from_file_location("archlint_under_test", ARCHLINT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# A segment is (kind, args); `lines` turns it into (text, is_test) lines and the child it declares, if any.
def segment_lines(seg):
    kind, args = seg
    if kind == "fn":
        name, body = args
        return [(f"fn {name}() {{", False)] + [(f"    let v = {i};", False) for i in range(body)] + [("}", False)]
    if kind == "mod":
        child, vis = args
        return [(f"{vis}mod {child};", False)]
    if kind == "test_mod":
        child, vis, attrs = args
        return [(line, True) for line in ["#[cfg(test)]", *attrs, f"{vis}mod {child};"]]
    if kind == "inline":
        name, depth = args
        inner = [("    " * (d + 1) + "fn n() {" if d == 0 else "    " * (d + 1) + "{") for d in range(depth)]
        close = ["    " * (d + 1) + "}" for d in reversed(range(depth))]
        return [(line, True) for line in ["#[cfg(test)]", f"mod {name} {{", *inner, "    use super::*;", *close, "}"]]
    if kind == "use":
        return [("#[cfg(test)]", True), ("use std::fmt;", True)]
    if kind == "open":
        return [("#[cfg(test)]", True), ("fn unterminated()", True), ("    -> u8", True)]
    raise AssertionError(kind)


def write_case(root, layout, segments):
    """Writes the case; returns (parent path, per-line is_test of the parent, {child path: is_test})."""
    parent, child_dir = LAYOUTS[layout]
    lines = [line for seg in segments for line in segment_lines(seg)]
    (root / child_dir).mkdir(parents=True, exist_ok=True)
    (root / parent).write_text("".join(f"{text}\n" for text, _ in lines))
    children = {}
    for kind, args in segments:
        if kind in ("mod", "test_mod"):
            path = f"{child_dir}/{args[0]}.rs"
            (root / path).write_text("pub fn planted() {}\n")
            children[path] = kind == "test_mod"
    return parent, [is_test for _, is_test in lines], children


def spans_of(is_test, segments):
    """The recorded ranges: one per test segment, 1-based inclusive."""
    spans, line = [], 1
    for seg in segments:
        n = len(segment_lines(seg))
        if segment_lines(seg)[0][1]:
            spans.append((line, line + n - 1))
        line += n
    assert sum(hi - lo + 1 for lo, hi in spans) == sum(is_test)
    return spans


def check(arch, name, layout, segments, rng):
    failures = []
    with tempfile.TemporaryDirectory() as d, contextlib.chdir(d):
        parent, is_test, children = write_case(Path(d), layout, segments)
        want_spans = spans_of(is_test, segments)
        got_spans = arch.cfg_test_spans(arch.read_lines(parent))
        if got_spans != want_spans:
            failures.append(f"cfg_test_spans {got_spans}, recorded {want_spans}")
        for path, test in children.items():
            if arch.is_cfg_test_module_file(path) != test:
                failures.append(f"is_cfg_test_module_file({path}) is {not test}, recorded {test}")
        if Path(parent).name in ("lib.rs", "main.rs", "mod.rs") and arch.is_cfg_test_module_file(parent):
            failures.append(f"is_cfg_test_module_file({parent}) is True")
        planted = [(parent, i + 1, test) for i, test in enumerate(is_test)] + [(p, 1, t) for p, t in children.items()]
        diags = [{"id": rule, "file": path, "line": line, "message": "m", "test": test}
                 for path, line, test in planted for rule in ("scoped", "free")]
        rng.shuffle(diags)
        want = [diag for diag in diags if diag["id"] == "free" or not diag["test"]]
        got = arch.filter_test_mods(diags, SMELLS)
        if got != want:
            show = lambda ds: sorted((x["id"], x["file"], x["line"]) for x in ds)
            failures.append(f"filter_test_mods dropped {[x for x in show(diags) if x not in show(got)]}, "
                            f"want dropped {[x for x in show(diags) if x not in show(want)]}, order kept: "
                            f"{[x for x in got] == [x for x in diags if x in got]}")
    return [f"{name} ({layout}, {segments}): {f}" for f in failures]


def draw(rng):
    segments, n_child, n_inline = [], 0, 0
    for _ in range(rng.randrange(1, 9)):
        kind = rng.choice(["fn", "mod", "test_mod", "inline", "use"])
        if kind == "fn":
            segments.append(("fn", (f"p{len(segments)}", rng.randrange(4))))
        elif kind == "mod":
            segments.append(("mod", (f"c{n_child}", rng.choice(VISIBILITY))))
            n_child += 1
        elif kind == "test_mod":
            segments.append(("test_mod", (f"c{n_child}", rng.choice(VISIBILITY),
                                          [rng.choice(EXTRA_ATTRS) for _ in range(rng.randrange(3))])))
            n_child += 1
        elif kind == "inline":
            segments.append(("inline", (f"m{n_inline}", rng.randrange(4))))
            n_inline += 1
        else:
            segments.append(("use", ()))
    if rng.random() < 0.2:
        segments.append(("open", ()))
    return rng.choice(list(LAYOUTS)), segments


FIXED = {
    "a production and a test child of one parent": ("x.rs", [("mod", ("prod", "")), ("test_mod", ("t", "", []))]),
    "production code after an inline test module": ("lib.rs", [("inline", ("tests", 2)), ("fn", ("helper", 2))]),
    "a brace-less cfg(test) use before production": ("main.rs", [("use", ()), ("fn", ("after", 1))]),
    "an unterminated cfg(test) item at the end": ("x/mod.rs", [("fn", ("first", 1)), ("open", ())]),
}


def main():
    cases = int(os.environ.get("CASES", "300"))
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    print(f"seed {seed}, {cases} drawn cases")
    arch = load_archlint()
    failures = []
    for name, (layout, segments) in FIXED.items():
        failures += check(arch, name, layout, segments, rng)
    for i in range(cases):
        failures += check(arch, f"draw {i}", *draw(rng), rng)
    for failure in failures:
        print(f"FAIL: {failure}")
    print(f"test-archlint-test-modules: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
