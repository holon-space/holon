#!/usr/bin/env python3
"""Self-check for scripts/gate-skip.py against this workspace's crate graph and justfile.

Run: `python3 scripts/test-gate-skip.py`. Exits non-zero on the first violation.
"""
import ast
import shutil
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCRIPT = REPO / "scripts" / "gate-skip.py"
SKIP = "skip: 3 4 5 6 11 12 13 14 15 16 18"


def gate_skip(*args, cwd=REPO):
    return subprocess.run([sys.executable, str(SCRIPT), *args], cwd=cwd, capture_output=True, text=True)


def on_paths(*paths):
    with tempfile.NamedTemporaryFile("w", suffix=".txt") as f:
        f.write("".join(p + "\n" for p in paths))
        f.flush()
        r = gate_skip("--paths-from", f.name)
    assert r.returncode == 0, f"{paths}: exit {r.returncode}: {r.stderr}"
    return r.stdout.strip(), r.stderr


def expect_full(paths, blocked, reason):
    out, err = on_paths(*paths)
    assert out == "full", f"{paths}: expected full, got {out!r}"
    line = f"full: {blocked}: {reason}"
    assert line in err, f"{paths}: expected {line!r} on stderr, got:\n{err}"


def load_module():
    spec = importlib.util.spec_from_file_location("gate_skip", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_scan():
    g = load_module()
    metadata = json.loads(subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1"],
                                         cwd=REPO, capture_output=True, text=True, check=True).stdout)
    ws = g.Workspace(metadata, "cargo nextest run -p holon-net\ncargo check -p holon-api", "")
    assert ws.named_pkgs == {"holon-net"}, f"a whole-package test run names its package, a check does not: {ws.named_pkgs}"
    ws = g.Workspace(metadata, "", 'filter = "test(=exec_support::x)"')
    assert ws.named_pkgs == {"holon-net"}, f"a bare test(=module::) row names the crate holding tests/module: {ws.named_pkgs}"


def check_summary_paths():
    g = load_module()
    got = g.summary_paths("R docs/Testing/{ => registry}/KeystoneKnownReds.md\nC {a => b}/x.rs\nM justfile\nD old.md")
    assert got == ["docs/Testing/KeystoneKnownReds.md", "docs/Testing/registry/KeystoneKnownReds.md",
                   "a/x.rs", "b/x.rs", "justfile", "old.md"], f"a rename or copy gives both paths, got {got}"


def check_baseline_readers():
    """timing-assertions-baseline.txt is allowed because only `assertions` (step 7) reads it."""
    timing_split = (REPO / "scripts" / "timing-split.py").read_text()
    g = load_module()
    other_steps = g.step_sources(REPO, (REPO / "justfile").read_text()).replace(timing_split, "")
    assert "timing-assertions-baseline" not in other_steps, "a skipped step outside timing-split.py names the baseline"
    tree = ast.parse(timing_split)
    readers = {"cmd_assertions", "read_baseline"}
    for node in tree.body:
        scope = node.name if isinstance(node, ast.FunctionDef) else None
        for n in ast.walk(node):
            if isinstance(n, ast.Name) and n.id in ("BASELINE", "read_baseline") and isinstance(n.ctx, ast.Load):
                assert scope in readers, f"timing-split.py:{n.lineno}: {n.id} used in {scope or 'module scope'}; " \
                    "a skipped step may now read the baseline, so put it back on gate-skip.py's TRIGGERS"


def check_renamed_trigger():
    """A trigger moved to an allowed path is still a changed trigger path."""
    with tempfile.TemporaryDirectory() as d:
        copy = Path(d)
        for f in subprocess.run(["jj", "file", "list", "--ignore-working-copy"], cwd=REPO,
                                capture_output=True, text=True, check=True).stdout.splitlines():
            if (REPO / f).is_file():
                (copy / f).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy(REPO / f, copy / f)
        jj = lambda *a: subprocess.run(["jj", *a], cwd=copy, capture_output=True, text=True, check=True)
        jj("git", "init", ".")
        jj("commit", "-m", "base")
        (copy / "docs/Testing/registry").mkdir()
        (copy / "docs/Testing/KeystoneKnownReds.md").rename(copy / "docs/Testing/registry/KeystoneKnownReds.md")
        jj("commit", "-m", "move")
        r = gate_skip("--base", "@--", "--tip", "@-", cwd=copy)
    assert r.stdout.strip() == "full", f"a moved trigger must be full, got {r.stdout!r}: {r.stderr}"


def main():
    check_scan()
    check_baseline_readers()
    check_renamed_trigger()
    out, err = on_paths("crates/holon-net/tests/net_load.rs")
    assert out == SKIP, f"a tests/-only diff of one binary must skip, got {out!r}: {err}"
    out, err = on_paths("docs/Plans/x.md", "crates/holon-net/tests/net_file.rs", "crates/holon-core/tests/shape_gate/claims.rs")
    assert out == SKIP, f"a docs and unnamed-test-binary diff must skip, got {out!r}: {err}"

    out, err = on_paths("docs/Architecture/FeatureMap.md", "docs/Architecture/featuremap.yaml",
                        "docs/Architecture/baseline/crates", "docs/Testing/known-red-evidence/x/y.md",
                        "crates/holon-net/tests/net_file.rs")
    assert out == SKIP, f"files only steps outside the skip set read must skip, got {out!r}: {err}"

    out, err = on_paths("scripts/timing-assertions-baseline.txt", "crates/holon-net/tests/net_file.rs")
    assert out == SKIP, f"the timing-assertions baseline plus one test binary must skip, got {out!r}: {err}"

    for trigger in ["Cargo.lock", "crates/holon-api/Cargo.toml", "justfile", ".config/nextest.toml",
                    "rust-toolchain.toml", ".cargo/config.toml", "docs/Testing/KeystoneKnownReds.md",
                    "docs/Testing/AdmissionFootprintCensus.md",
                    "docs/Testing/HomeProfileLookupLatency.txt"]:
        expect_full([trigger], trigger, "trigger")

    expect_full(["crates/holon-api/src/lib.rs"], "crates/holon-api/src/lib.rs",
                "lib src of holon-api, in the holon-integration-tests closure")

    expect_full(["crates/holon-net/tests/net_load.rs", "frobnicate/notes.txt", "docs/Plans/x.md"],
                "frobnicate/notes.txt", "unknown path")

    expect_full(["crates/holon-integration-tests/tests/hand_authored_regressions.rs"],
                "crates/holon-integration-tests/tests/hand_authored_regressions.rs",
                "test binary hand_authored_regressions, named by a gate step")
    expect_full(["crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs"],
                "crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs",
                "test binary general_e2e_composed_pbt, named by a gate step")
    expect_full(["crates/holon-orgmode/tests/round_trip_pbt.rs"], "crates/holon-orgmode/tests/round_trip_pbt.rs",
                "test binary round_trip_pbt, named by a gate step")
    expect_full(["frontends/gpui/tests/nested_page_chevron_gate.rs"], "frontends/gpui/tests/nested_page_chevron_gate.rs",
                "tests of holon-gpui, a package a gate step selects")
    expect_full(["crates/holon-net/tests/exec_support/twin_names.rs"], "crates/holon-net/tests/exec_support/twin_names.rs",
                "test path of holon-net that is not one test binary")

    out, err = on_paths()
    assert out == "full" and "full: empty diff" in err, f"an empty diff must be full, got {out!r}: {err}"

    r = gate_skip("--base", "no-such-revision-anywhere", "--tip", "@")
    assert r.returncode != 0 and "skip" not in r.stdout, f"a failed jj diff must fail: {r.returncode} {r.stdout!r}"
    assert "jj diff" in r.stderr, r.stderr

    with tempfile.TemporaryDirectory() as empty, tempfile.NamedTemporaryFile("w", suffix=".txt") as f:
        f.write("crates/holon-net/tests/net_load.rs\n")
        f.flush()
        r = gate_skip("--paths-from", f.name, cwd=empty)
    assert r.returncode != 0 and "skip" not in r.stdout, f"a failed cargo metadata must fail: {r.returncode} {r.stdout!r}"
    assert "cargo metadata" in r.stderr, r.stderr

    check_summary_paths()
    print("test-gate-skip: OK")


if __name__ == "__main__":
    main()
