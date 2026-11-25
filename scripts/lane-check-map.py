#!/usr/bin/env python3
"""Whether a lane diff reaches a path-scoped lane check.

    jj diff --from <base> --to @ --name-only | scripts/lane-check-map.py <check>

Prints `run` when a changed path is in the check's scope, else
`<check> skipped: diff touches none of <globs>`. A scope holds every path whose
change can turn the check red. Only the lane maps checks; the gate runs them all.
"""
import re
import sys

SCOPES = {
    # What `archlint --all` reads (archlint.py collect_all_files), its rules and baseline, and its interpreter.
    "archlint": ("crates/**/*.rs", "frontends/**/*.rs", "crates/**/Cargo.toml", "frontends/**/Cargo.toml",
                 "Cargo.toml", "*.md", "archlint/**", "scripts/python311"),
    # The advisories match the locked dependency set and deny.toml's ignore list.
    "deny-advisories": ("**/Cargo.toml", "**/Cargo.lock", "deny.toml"),
}


def glob_regex(glob):
    """`**/` matches zero or more directories, `**` anything, `*` anything but `/`."""
    out, i = "", 0
    while i < len(glob):
        if glob.startswith("**/", i):
            out, i = out + "(?:.*/)?", i + 3
        elif glob.startswith("**", i):
            out, i = out + ".*", i + 2
        elif glob[i] == "*":
            out, i = out + "[^/]*", i + 1
        else:
            out, i = out + re.escape(glob[i]), i + 1
    return re.compile(out + r"\Z")


REGEXES = {check: [glob_regex(g) for g in globs] for check, globs in SCOPES.items()}


def selects(check, path):
    return any(rx.match(path) for rx in REGEXES[check])


def verdict(check, paths):
    if any(selects(check, p) for p in paths):
        return "run"
    return f"{check} skipped: diff touches none of {' '.join(SCOPES[check])}"


def main(argv):
    if len(argv) != 2 or argv[1] not in SCOPES:
        sys.exit(f"usage: {argv[0]} <{'|'.join(SCOPES)}> < changed paths")
    print(verdict(argv[1], [line.strip() for line in sys.stdin if line.strip()]))


if __name__ == "__main__":
    main(sys.argv)
