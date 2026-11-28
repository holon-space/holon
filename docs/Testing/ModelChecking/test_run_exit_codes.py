"""run.py keeps its exit codes apart: 2 for every crash of the script itself,
never the 1 that means an outcome differs. Its --if-touched gate runs the specs
for a modelled file, skips otherwise, and never reads a broken anchor, a missing
tool or an empty path list as "nothing touched"."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

RUN = Path(__file__).resolve().parent / "run.py"
ROOT = RUN.parents[3]
MODELLED = "crates/holon-orgmode/src/di.rs"
SPEC_ANCHOR = "crates/holon-api/src/live_data/home_by.rs:home_diffs_with_progress"


def load_run():
    spec = importlib.util.spec_from_file_location("run", RUN)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def gate(root: Path, paths: str | None, env_extra=None):
    """run.py --if-touched with a stub `java` on PATH and a missing TLC jar, from the repo root:
    a run that starts TLC ends with exit 2 naming the jar, so it is told apart from a skip."""
    bin_dir = root / "bin"
    bin_dir.mkdir(exist_ok=True)
    java = bin_dir / "java"
    java.write_text("#!/bin/sh\nexit 0\n")
    java.chmod(0o755)
    env = dict(os.environ, PATH=f"{bin_dir}:{os.environ['PATH']}", TLA2TOOLS_JAR=str(root / "no-jar"), **(env_extra or {}))
    changed = root / "changed.txt"
    if paths is not None:
        changed.write_text(paths)
    return subprocess.run([sys.executable, str(RUN), "--out", str(root / "out"), "--if-touched", str(changed)],
                          capture_output=True, text=True, env=env, cwd=ROOT)


def crash_shapes(root: Path):
    """Every --out that the script cannot create before any tool runs."""
    a_file = root / "a-file"
    a_file.write_text("")
    read_only = root / "read-only"
    read_only.mkdir()
    read_only.chmod(0o500)
    yield "out below a file", a_file / "out"
    yield "out is a file", a_file
    yield "out below a read-only folder", read_only / "out"


def test_a_crash_before_any_tool_exits_2():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for name, out in crash_shapes(root):
            env = dict(os.environ, TLA2TOOLS_JAR=str(root / "no-jar"))
            done = subprocess.run([sys.executable, str(RUN), "--out", str(out)],
                                  capture_output=True, text=True, env=env, cwd=root)
            assert done.returncode == 2, f"{name}: exit {done.returncode}\n{done.stderr}"
        (root / "read-only").chmod(0o700)


def test_a_modelled_file_runs_the_specs():
    with tempfile.TemporaryDirectory() as tmp:
        done = gate(Path(tmp), f"README.md\n{MODELLED}\n")
        assert done.returncode == 2 and f"{tmp}/no-jar" in done.stderr, (done.returncode, done.stdout, done.stderr)
        assert done.stdout.startswith(f"RUN: {MODELLED} is modelled by Writeback.tla:"), done.stdout


def test_a_change_in_the_spec_folder_runs_the_specs():
    with tempfile.TemporaryDirectory() as tmp:
        done = gate(Path(tmp), "docs/Testing/ModelChecking/run.py\n")
        assert done.returncode == 2 and done.stdout.startswith("RUN: "), (done.returncode, done.stdout, done.stderr)


def test_a_diff_touching_nothing_modelled_skips():
    with tempfile.TemporaryDirectory() as tmp:
        done = gate(Path(tmp), "docs/other.md\ncrates/holon-orgmode/src/lib.rs\n")
        assert (done.returncode, done.stdout.strip()) == (0, "SKIP: no modelled file touched"), (done.stdout, done.stderr)


def test_a_clean_tree_skips_naming_its_reason():
    with tempfile.TemporaryDirectory() as tmp:
        for name, paths in (("empty", ""), ("blank lines", "\n\n")):
            done = gate(Path(tmp), paths)
            assert (done.returncode, done.stdout.strip()) == (0, "SKIP: no changed path"), (name, done.stdout, done.stderr)


def test_a_missing_or_unreadable_path_list_is_an_error_not_a_skip():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        done = gate(root, None)
        assert done.returncode == 2 and "SKIP" not in done.stdout, (done.returncode, done.stdout, done.stderr)
        (root / "changed.txt").mkdir()
        done = gate(root, None)
        assert done.returncode == 2 and "SKIP" not in done.stdout, (done.returncode, done.stdout, done.stderr)


def test_missing_java_exits_2_naming_the_search_path():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "changed.txt").write_text(f"{MODELLED}\n")
        env = dict(os.environ, PATH=str(root), TLA2TOOLS_JAR=str(root / "no-jar"))
        done = subprocess.run([sys.executable, str(RUN), "--out", str(root / "out"), "--if-touched", str(root / "changed.txt")],
                              capture_output=True, text=True, env=env, cwd=ROOT)
        assert done.returncode == 2 and f"PATH={root}" in done.stderr, (done.returncode, done.stderr)


def test_the_anchors_of_the_repo_specs_resolve_and_cover_continuation_lines():
    os.chdir(ROOT)
    anchors = load_run().modelled_files(RUN.parent)
    assert {a.path for a in anchors} >= {MODELLED, "crates/holon-filesystem/src/file_sync_controller/writeback.rs",
                                         "crates/holon/src/api/ui_watcher.rs"}, sorted(anchors)


def test_an_anchor_with_a_renamed_function_or_missing_file_names_its_spec_line():
    os.chdir(ROOT)
    run = load_run()
    with tempfile.TemporaryDirectory() as tmp:
        specs = Path(tmp)
        for anchor, text in ((SPEC_ANCHOR.replace("home_diffs_with_progress", "renamed_fn"), "no `fn renamed_fn`"),
                             ("crates/gone.rs:f", "does not exist")):
            (specs / "A.tla").write_text(f"---- MODULE A ----\n\\* Models: {anchor} (x)\nAct == TRUE\n")
            try:
                run.modelled_files(specs)
            except run.ToolError as e:
                assert str(e).startswith(f"A.tla:2: {anchor}") and text in str(e), str(e)
            else:
                raise AssertionError(f"{anchor}: no error")


def unparsable_models_lines():
    """`Models:` texts that name no <file>.rs:<function>: prose, a non-Rust file, a Rust file without a function."""
    return ["the apply actor", "crates/holon-api/src/live_data.py:subscribe",
            "crates/holon-api/src/live_data.rs (the apply)", "live_data.rs: subscribe"]


def test_a_models_line_naming_no_anchor_names_its_spec_line():
    os.chdir(ROOT)
    run = load_run()
    with tempfile.TemporaryDirectory() as tmp:
        specs = Path(tmp)
        for text in unparsable_models_lines():
            (specs / "A.tla").write_text(f"---- MODULE A ----\n\\* Models: {text}\nAct == TRUE\n")
            try:
                run.modelled_files(specs)
            except run.ToolError as e:
                assert str(e).startswith("A.tla:2:"), str(e)
            else:
                raise AssertionError(f"{text}: no error")


def long_words():
    """64 KiB words of the characters that the path part of a `Models:` anchor repeats."""
    for unit in ("a", ".rs", "a/", "-", "_."):
        yield unit * (65536 // len(unit))


def test_a_long_word_in_a_models_comment_is_read_in_linear_time():
    os.chdir(ROOT)
    run = load_run()
    with tempfile.TemporaryDirectory() as tmp:
        specs = Path(tmp)
        for word in long_words():
            (specs / "A.tla").write_text(f"---- MODULE A ----\n\\* Models: {SPEC_ANCHOR} {word}\nAct == TRUE\n")
            start = time.monotonic()
            run.modelled_files(specs)
            took = time.monotonic() - start
            assert took < 0.3, (word[:6], len(word), took)


if __name__ == "__main__":
    test_a_crash_before_any_tool_exits_2()
    for name, test in sorted(globals().items()):
        if name.startswith("test_") and name != "test_a_crash_before_any_tool_exits_2":
            test()
    print("PASS")
