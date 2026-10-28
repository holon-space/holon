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


def check_summary_paths():
    g = load_module()
    got = g.summary_paths("R docs/Testing/{ => registry}/KeystoneKnownReds.md\nC {a => b}/x.rs\nM justfile\nD old.md")
    assert got == ["docs/Testing/KeystoneKnownReds.md", "docs/Testing/registry/KeystoneKnownReds.md",
                   "a/x.rs", "b/x.rs", "justfile", "old.md"], f"a rename or copy gives both paths, got {got}"


BASELINE_READERS = {"cmd_assertions", "read_baseline"}


def baseline_violations(source):
    """Lines of a timing-split.py source where something other than `assertions` (step 7) may reach the baseline."""
    found = []
    for node in ast.parse(source).body:
        scope = node.name if isinstance(node, ast.FunctionDef) else None
        for n in ast.walk(node):
            if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Load):
                if n.id in ("BASELINE", "read_baseline") and scope not in BASELINE_READERS:
                    found.append((n.lineno, n.id))
                if n.id == "cmd_assertions" and scope != "main":
                    found.append((n.lineno, n.id))
            if isinstance(n, ast.Constant) and isinstance(n.value, str) and "timing-assertions-baseline" in n.value:
                assigns_baseline = isinstance(node, ast.Assign) and [t.id for t in node.targets] == ["BASELINE"]
                if not assigns_baseline and scope != "cmd_assertions":
                    found.append((n.lineno, "literal baseline path"))
    return found


BASELINE_PROBES = {
    "cmd_record calls cmd_assertions()": "def cmd_record():\n    cmd_assertions()\n",
    "literal baseline path in cmd_record":
        'def cmd_record():\n    open(REPO / "scripts/timing-assertions-baseline.txt")\n',
    "literal baseline path assigned to another name": 'PATH = "scripts/timing-assertions-baseline.txt"\n',
    "BASELINE read in cmd_record": "def cmd_record():\n    BASELINE.read_text()\n",
}

BASELINE_ALLOWED = {
    "BASELINE assignment alone": 'BASELINE = REPO / "scripts/timing-assertions-baseline.txt"\n',
    "cmd_assertions called from main alone": "def main(argv):\n    return cmd_assertions()\n",
    "baseline named inside cmd_assertions": 'def cmd_assertions():\n    print("see scripts/timing-assertions-baseline.txt")\n    BASELINE.read_text()\n',
}


def check_baseline_readers():
    """timing-assertions-baseline.txt is allowed because only `assertions` (step 7) reads it."""
    timing_split = (REPO / "scripts" / "timing-split.py").read_text()
    g = load_module()
    other_steps = g.step_sources(REPO, (REPO / "justfile").read_text()).replace(timing_split, "")
    assert "timing-assertions-baseline" not in other_steps, "a skipped step outside timing-split.py names the baseline"
    for name, source in BASELINE_PROBES.items():
        assert baseline_violations(source), f"the guard must report: {name}"
    for name, source in BASELINE_ALLOWED.items():
        assert not baseline_violations(source), f"the guard must not report: {name}"
    found = baseline_violations(timing_split)
    assert not found, f"timing-split.py: {found}; a skipped step may now read the baseline, " \
        "so put it back on gate-skip.py's TRIGGERS"


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


def check_helper_reach_forms():
    g = load_module()
    pulls = {"crate": "use crate::helper as _h;\n", "super": "use super::helper as _h;\n", "grouped": "use crate::{helper as _h};\n",
             "path": '#[path = "helper.rs"]\nmod h;\n', "include": 'mod h { include!("helper.rs"); }\n'}
    for form, pull in pulls.items():
        with tempfile.TemporaryDirectory() as root:
            tests = Path(root) / "crates/fake/tests"
            tests.mkdir(parents=True)
            (tests / "main.rs").write_text("mod helper;\nmod leaf;\n")
            (tests / "helper.rs").write_text("pub fn shared() {}\n")
            (tests / "leaf.rs").write_text(pull)
            metadata = {"workspace_root": root, "workspace_members": ["fake", "it"], "packages": [{
                "id": "fake", "name": "fake", "manifest_path": f"{root}/crates/fake/Cargo.toml", "dependencies": [],
                "targets": [{"kind": ["test"], "src_path": f"{root}/crates/fake/tests/main.rs"}]},
                {"id": "it", "name": "holon-integration-tests", "manifest_path": f"{root}/it/Cargo.toml",
                 "dependencies": [], "targets": []}]}
            ws = g.Workspace(metadata, "", "")
            got = ws.test_binary("fake", g.P("crates/fake/tests/helper.rs"))
            assert got is None, f"a file another test pulls in by {form} is a shared helper, got binary {got!r}"


def check_glob_macro_and_attr_reach_forms():
    g = load_module()
    forms = {
        "glob super": ("mod helper;\nmod leaf;\n", "use super::*;\nfn g() { helper::shared() }\n"),
        "glob crate": ("mod helper;\nmod leaf;\n", "use crate::*;\nfn g() { helper::shared() }\n"),
        "re-export in main.rs": ("mod helper;\nuse helper::shared;\nmod leaf;\n", "fn g() { crate::shared() }\n"),
        "macro_use in main.rs": ("#[macro_use]\nmod helper;\nmod leaf;\n", "fn g() { shared_mac!() }\n"),
        "include of concat": ("mod leaf;\n", 'mod h { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/helper.rs")); }\n'),
        "cfg_attr path": ("mod leaf;\n", '#[cfg_attr(all(), path = "helper.rs")]\nmod h;\n'),
    }
    for form, (main_rs, leaf) in forms.items():
        with tempfile.TemporaryDirectory() as root:
            tests = Path(root) / "crates/fake/tests"
            tests.mkdir(parents=True)
            (tests / "main.rs").write_text(main_rs)
            (tests / "helper.rs").write_text("pub fn shared() {}\nmacro_rules! shared_mac { () => {} }\n")
            (tests / "leaf.rs").write_text(leaf)
            metadata = {"workspace_root": root, "workspace_members": ["fake", "it"], "packages": [{
                "id": "fake", "name": "fake", "manifest_path": f"{root}/crates/fake/Cargo.toml", "dependencies": [],
                "targets": [{"kind": ["test"], "src_path": f"{root}/crates/fake/tests/main.rs"}]},
                {"id": "it", "name": "holon-integration-tests", "manifest_path": f"{root}/it/Cargo.toml",
                 "dependencies": [], "targets": []}]}
            ws = g.Workspace(metadata, "", "")
            got = ws.test_binary("fake", g.P("crates/fake/tests/helper.rs"))
            assert got is None, f"a file another test reaches by {form} is a shared helper, got binary {got!r}"


def main():
    check_helper_reach_forms()
    check_glob_macro_and_attr_reach_forms()
    check_baseline_readers()
    check_renamed_trigger()
    out, err = on_paths("docs/Plans/x.md")
    assert out == SKIP, f"a docs-only diff must skip, got {out!r}: {err}"

    out, err = on_paths("docs/Architecture/FeatureMap.md", "docs/Architecture/featuremap.yaml",
                        "docs/Architecture/baseline/crates", "docs/Testing/known-red-evidence/x/y.md")
    assert out == SKIP, f"files only steps outside the skip set read must skip, got {out!r}: {err}"

    out, err = on_paths("scripts/timing-assertions-baseline.txt", "docs/Plans/x.md")
    assert out == SKIP, f"the timing-assertions baseline plus a doc must skip, got {out!r}: {err}"

    for trigger in ["Cargo.lock", "crates/holon-api/Cargo.toml", "justfile", ".config/nextest.toml",
                    "rust-toolchain.toml", ".cargo/config.toml", "docs/Testing/KeystoneKnownReds.md",
                    "docs/Testing/AdmissionFootprintCensus.md",
                    "docs/Testing/HomeProfileLookupLatency.txt"]:
        expect_full([trigger], trigger, "trigger")

    expect_full(["crates/holon-api/src/lib.rs"], "crates/holon-api/src/lib.rs",
                "lib src of holon-api, in the holon-integration-tests closure")

    expect_full(["crates/holon-net/tests/net_load.rs", "frobnicate/notes.txt", "docs/Plans/x.md"],
                "frobnicate/notes.txt", "unknown path")

    for member, test_path in [
        ("holon-net", "crates/holon-net/tests/exec_answer.rs"), ("holon-net", "crates/holon-net/tests/exec_support/mod.rs"),
        ("holon-net", "crates/holon-net/tests/net_load.rs"), ("holon-core", "crates/holon-core/tests/shape_gate/claims.rs"),
        ("holon-profiles", "crates/holon-profiles/tests/common/mod.rs"),
        ("holon-integration-tests", "crates/holon-integration-tests/tests/hand_authored_regressions.rs"),
        ("holon-gpui", "frontends/gpui/tests/nested_page_chevron_gate.rs"),
    ]:
        expect_full([test_path], test_path, f"test path of {member} that is not one test binary")

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
