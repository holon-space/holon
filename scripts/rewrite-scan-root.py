#!/usr/bin/env python3
"""Rewrite an archidoc IR file's absolute `scan_root` to a repo-relative one.

archidoc canonicalizes the directory it scanned, so a committed baseline records
the absolute path of whichever workspace last regenerated it. Called from
`just arch-baseline` with (file, relative-root) pairs.
"""
import pathlib
import re
import sys

args = sys.argv[1:]
assert args and len(args) % 2 == 0, "usage: rewrite-scan-root.py <file> <rel-root> ..."

for path_str, rel_root in zip(args[::2], args[1::2]):
    path = pathlib.Path(path_str)
    text = path.read_text()
    new_text, n = re.subn(
        r'("scan_root":\s*)"[^"]*"', lambda m: m.group(1) + '"' + rel_root + '"', text, count=1
    )
    assert n == 1, f"no scan_root field in {path}"
    if new_text != text:
        path.write_text(new_text)
        print(f"{path}: scan_root → {rel_root}")
