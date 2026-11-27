"""run.py keeps its exit codes apart: 2 for every crash of the script itself,
never the 1 that means an outcome differs."""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

RUN = Path(__file__).resolve().parent / "run.py"


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


if __name__ == "__main__":
    test_a_crash_before_any_tool_exits_2()
    print("PASS")
