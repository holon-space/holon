"""Stub Python interpreters for property checks of scripts/python311, the one Python >= 3.11 picker.

A stub answers the picker's `-c` version probe with its drawn `major.minor`. Asked to run a script, it appends its
own name to a record file, then a stub >= 3.11 runs the script under the real interpreter and an older one exits 1,
as an old interpreter fails on `import tomllib`.
"""
import os
import shutil
import sys
from pathlib import Path

CANDIDATES = ["python3.14", "python3.13", "python3.12", "python3.11", "python3"]
# The tools the picker and the stubs call, so the stub PATH needs no host Python directory.
TOOLS = ["bash", "sh", "env", "dirname"]


def picked(versions):
    """The model of the picker: the first candidate present with version >= 3.11, else None."""
    return next((c for c in CANDIDATES if c in versions and versions[c] >= (3, 11)), None)


def write_stubs(bin_dir, versions, record):
    """One stub per entry of `versions` (candidate name -> (major, minor)) in `bin_dir`."""
    assert sys.version_info >= (3, 11), f"a stub >= 3.11 runs scripts under this interpreter, running {sys.version}"
    bin_dir.mkdir(parents=True, exist_ok=True)
    for name, (major, minor) in versions.items():
        assert name in CANDIDATES, name
        run = f'exec "{sys.executable}" "$@"' if (major, minor) >= (3, 11) else "exit 1"
        stub = bin_dir / name
        stub.write_text(f'#!/bin/sh\nif [ "$1" = "-c" ]; then echo "{major}.{minor}"; exit 0; fi\n'
                        f'echo "{name}" >> "{record}"\n{run}\n')
        stub.chmod(0o755)


def tool_dir(dest):
    """A directory holding links to TOOLS only, for a PATH that hides every host Python."""
    dest.mkdir(parents=True)
    for tool in TOOLS:
        found = shutil.which(tool)
        assert found, f"{tool} is not on PATH"
        os.symlink(found, dest / tool)
    return dest
