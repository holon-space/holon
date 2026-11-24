#!/usr/bin/env python3
"""Fail when a windowed test's deadline is not below its nextest cap.

A deadline at or past the cap never fires: nextest kills the test first, and the
kill prints a bare TIMEOUT instead of the deadline's message with the last value
it observed. The cap of a `holon-gpui::main` module is `period x terminate-after`
of the first `[[profile.default.overrides]]` block whose filter names it as
`binary_id(holon-gpui::main) & test(/^<module>::/)`, else of `[profile.default]`.

A deadline is a `Duration::from_secs(N)` on a line that names a deadline, or one
added to `Instant::now()`.

  check-windowed-deadlines.py [NEXTEST.toml]   default: .config/nextest.toml
  check-windowed-deadlines.py --self-check     prove the check fails a 180 s deadline
"""

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
NEXTEST = REPO / ".config/nextest.toml"
TESTS = REPO / "frontends/gpui/tests"

SECTION = re.compile(r"^\[\[?([\w.-]+)\]\]?\s*$")
FILTER = re.compile(r'^filter\s*=\s*"(.*)"\s*(#.*)?$')
SLOW = re.compile(
    r'^slow-timeout\s*=\s*\{\s*period\s*=\s*"(\d+)s"\s*,\s*terminate-after\s*=\s*(\d+)\s*\}\s*(#.*)?$'
)
GPUI_MODULE = re.compile(r"^binary_id\(holon-gpui::main\) & test\(/\^(\w+)::/\)$")
DEADLINE = re.compile(
    r"(?i:deadline).*Duration::from_secs\((\d+)\)|Instant::now\(\)\s*\+\s*Duration::from_secs\((\d+)\)"
)


def caps(text):
    """(default cap, {module: cap}) in seconds, from the default profile."""
    default = None
    modules = {}
    section = None
    filt = None
    for line in text.splitlines():
        line = line.strip()
        m = SECTION.match(line)
        if m:
            section, filt = m.group(1), None
            continue
        m = FILTER.match(line)
        if m:
            filt = m.group(1)
            continue
        m = SLOW.match(line)
        if not m:
            continue
        cap = int(m.group(1)) * int(m.group(2))
        if section == "profile.default":
            default = cap
        elif section == "profile.default.overrides":
            assert filt is not None, f"a slow-timeout override with no filter before it: {line}"
            g = GPUI_MODULE.match(filt)
            if g:
                modules.setdefault(g.group(1), cap)
            else:
                assert "holon-gpui" not in filt, (
                    f"an override caps holon-gpui by a filter this check cannot read: {filt}"
                )
    assert default is not None, "no slow-timeout in [profile.default]"
    return default, modules


def violations(sources, default, modules):
    """`path:line` lines for every deadline in `sources` ({path: text}) not below its cap."""
    out = []
    for path, text in sorted(sources.items()):
        cap = modules.get(pathlib.Path(path).stem, default)
        for n, line in enumerate(text.splitlines(), 1):
            m = DEADLINE.search(line)
            if not m:
                continue
            secs = int(m.group(1) or m.group(2))
            if secs >= cap:
                out.append(
                    f"{path}:{n}: deadline {secs} s is not below the nextest cap {cap} s "
                    f"(.config/nextest.toml period x terminate-after)"
                )
    return out


def sources_of(root):
    return {
        str(p.relative_to(REPO)): p.read_text()
        for p in sorted(root.rglob("*.rs"))
    }


def self_check():
    toml = (
        '[profile.default]\nslow-timeout = { period = "30s", terminate-after = 4 }\n'
        "[[profile.default.overrides]]\n"
        'filter = "binary_id(holon-gpui::main) & test(/^long_loop::/)"\n'
        'slow-timeout = { period = "60s", terminate-after = 10 }\n'
    )
    default, modules = caps(toml)
    cases = {
        "a 180 s boot deadline": ({"t/boot.rs": "let boot_deadline = Instant::now() + Duration::from_secs(180);"}, 1),
        "a deadline equal to the cap": ({"t/eq.rs": "let deadline = Duration::from_secs(120);"}, 1),
        "a 60 s hang guard": ({"t/ok.rs": "let deadline = Instant::now() + Duration::from_secs(60);"}, 0),
        "a 180 s deadline under a 600 s override": ({"t/long_loop.rs": "let deadline = Instant::now() + Duration::from_secs(180);"}, 0),
        "a 180 s duration that is no deadline": ({"t/hold.rs": "let hold = Duration::from_secs(180);"}, 0),
    }
    wrong = 0
    for name, (sources, want) in cases.items():
        got = len(violations(sources, default, modules))
        print(f"[windowed-deadlines] self-check {name}: {got} failure(s), expected {want}")
        wrong += got != want
    return wrong


def main(argv):
    if argv == ["--self-check"]:
        wrong = self_check()
        print(f"[windowed-deadlines] self-check {'FAILED' if wrong else 'passed'}")
        return 1 if wrong else 0
    toml = pathlib.Path(argv[0]) if argv else NEXTEST
    default, modules = caps(toml.read_text())
    bad = violations(sources_of(TESTS), default, modules)
    for line in bad:
        print(line)
    print(f"[windowed-deadlines] {len(bad)} deadline(s) not below the cap (default cap {default} s)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
