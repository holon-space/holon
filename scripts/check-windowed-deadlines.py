#!/usr/bin/env python3
"""Fail when a windowed test's deadline is not below its nextest cap.

A deadline at or past the cap never fires: nextest kills the test first, and the
kill prints a bare TIMEOUT instead of the deadline's message with the last value
it observed. The cap of a `holon-gpui::main` module is `period x terminate-after`
of the first `[[profile.default.overrides]]` block whose filter names it as
`binary_id(holon-gpui::main) & test(/^<module>::/)`, else of `[profile.default]`.

A deadline is the longest `Duration` a statement computes when the statement
names a deadline or adds to a `now()`. A `Duration` is any of its constructors,
`Duration::new`, `Duration::MAX`, a `const` of the tests tree, and sums and
products of those with numbers.

  check-windowed-deadlines.py [NEXTEST.toml]   default: .config/nextest.toml
  check-windowed-deadlines.py --self-check     prove the check fails every spelling of a 180 s deadline
"""

import itertools
import pathlib
import re
import sys
from fractions import Fraction

REPO = pathlib.Path(__file__).resolve().parent.parent
NEXTEST = REPO / ".config/nextest.toml"
TESTS = REPO / "frontends/gpui/tests"

SECTION = re.compile(r"^\[\[?([\w.-]+)\]\]?\s*$")
FILTER = re.compile(r'^filter\s*=\s*"(.*)"\s*(#.*)?$')
SLOW = re.compile(
    r'^slow-timeout\s*=\s*\{\s*period\s*=\s*"(\d+)s"\s*,\s*terminate-after\s*=\s*(\d+)\s*\}\s*(#.*)?$'
)
GPUI_MODULE = re.compile(r"^binary_id\(holon-gpui::main\) & test\(/\^(\w+)::/\)$")

LITERAL = re.compile(r'r(#*)"[\s\S]*?"\1|"(?:\\[\s\S]|[^"\\])*"|//[^\n]*|/\*[\s\S]*?\*/')
TOKEN = re.compile(
    r"(?P<num>\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?(?:_?(?:[ui](?:8|16|32|64|128|size)|f32|f64))?)"
    r"|(?P<path>[A-Za-z_]\w*(?:\s*::\s*[A-Za-z_]\w*)*)"
    r"|(?P<punct>[()*+\-/,.])"
    r"|(?P<other>\S)"
)
ANCHOR = re.compile(r"(?i:deadline)|\bnow\s*\(\s*\)\s*(?:\+|\.\s*checked_add\s*\()")
CONST = re.compile(r"\b(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:[^=;]*=([^;]*);")
UNIT = {
    "from_secs": 1,
    "from_secs_f64": 1,
    "from_secs_f32": 1,
    "from_millis": Fraction(1, 1000),
    "from_micros": Fraction(1, 10**6),
    "from_nanos": Fraction(1, 10**9),
    "from_mins": 60,
    "from_hours": 3600,
}
NUMBER = re.compile(r"_?(?:[ui](?:8|16|32|64|128|size)|f32|f64)$")


def strip(text):
    """`text` without comments and string contents, newlines kept."""
    return LITERAL.sub(lambda m: '""' + "\n" * m.group(0).count("\n"), text)


def tokens(code):
    """(kind, text, line) per token."""
    out = []
    for m in TOKEN.finditer(code):
        kind = m.lastgroup
        out.append((kind, re.sub(r"\s+", "", m.group(kind)), code.count("\n", 0, m.start()) + 1))
    return out


class Parser:
    """Recursive descent over arithmetic of numbers and Durations. A value is
    (is_duration, seconds or number); None where the tokens are no such
    expression."""

    def __init__(self, toks, consts):
        self.toks = toks
        self.consts = consts

    def at(self, i, text):
        return i < len(self.toks) and self.toks[i][1] == text

    def sum(self, i):
        r = self.product(i)
        while r and (self.at(r[1], "+") or self.at(r[1], "-")):
            nxt = self.product(r[1] + 1)
            if not nxt or nxt[0][0] != r[0][0]:
                break
            sign = 1 if self.at(r[1], "+") else -1
            r = ((r[0][0], r[0][1] + sign * nxt[0][1]), nxt[1])
        return r

    def product(self, i):
        r = self.postfix(i)
        while r and (self.at(r[1], "*") or self.at(r[1], "/")):
            nxt = self.postfix(r[1] + 1)
            if not nxt or (r[0][0] and nxt[0][0]):
                break
            if self.at(r[1], "*"):
                r = ((r[0][0] or nxt[0][0], r[0][1] * nxt[0][1]), nxt[1])
            elif nxt[0][0] or nxt[0][1] == 0:
                break
            else:
                r = ((r[0][0], r[0][1] / nxt[0][1]), nxt[1])
        return r

    def postfix(self, i):
        r = self.primary(i)
        while r and self.at(r[1], ".") and r[1] + 1 < len(self.toks):
            method = self.toks[r[1] + 1][1]
            if method in ("checked_add", "saturating_add") and self.at(r[1] + 2, "("):
                arg = self.sum(r[1] + 3)
                if not arg or arg[0][0] != r[0][0] or not self.at(arg[1], ")"):
                    break
                r = ((r[0][0], r[0][1] + arg[0][1]), arg[1] + 1)
            elif method == "unwrap" and self.at(r[1] + 2, "(") and self.at(r[1] + 3, ")"):
                r = (r[0], r[1] + 4)
            else:
                break
        return r

    def args(self, i):
        """The values of `( a, b, ... )` from the `(` at `i`, and the index past `)`."""
        if not self.at(i, "("):
            return None
        vals, i = [], i + 1
        while True:
            r = self.sum(i)
            if not r or r[0][0]:
                return None
            vals.append(r[0][1])
            i = r[1]
            if self.at(i, ","):
                i += 1
                if self.at(i, ")"):
                    return vals, i + 1
            elif self.at(i, ")"):
                return vals, i + 1
            else:
                return None

    def primary(self, i):
        if i >= len(self.toks):
            return None
        kind, text, _ = self.toks[i]
        if kind == "num":
            return (False, Fraction(NUMBER.sub("", text).replace("_", ""))), i + 1
        if text == "(":
            r = self.sum(i + 1)
            if r and self.at(r[1], ")"):
                return r[0], r[1] + 1
            return None
        if kind != "path":
            return None
        name = text.split("::")
        if name[-2:] == ["Duration", "MAX"]:
            return (True, Fraction(2**64)), i + 1
        if name[-2:] == ["Duration", "ZERO"]:
            return (True, Fraction(0)), i + 1
        if len(name) >= 2 and name[-2] == "Duration" and name[-1] in UNIT:
            a = self.args(i + 1)
            if a and len(a[0]) == 1:
                return (True, a[0][0] * UNIT[name[-1]]), a[1]
            return None
        if name[-2:] == ["Duration", "new"]:
            a = self.args(i + 1)
            if a and len(a[0]) == 2:
                return (True, a[0][0] + a[0][1] / 10**9), a[1]
            return None
        if name[-1] in self.consts:
            return self.consts[name[-1]], i + 1
        return None


def longest(toks, consts):
    """(seconds, line) of the longest Duration any sub-expression of `toks`
    computes, or None."""
    p = Parser(toks, consts)
    best = None
    for i in range(len(toks)):
        r = p.sum(i)
        if r and r[0][0] and (best is None or r[0][1] > best[0]):
            best = (r[0][1], toks[i][2])
    return best


def consts_of(codes):
    """{name: value} of every `const`/`static` of `codes` ({path: stripped
    text}) that is a number or a Duration, resolved through each other."""
    defs = [(m.group(1), m.group(2)) for code in codes.values() for m in CONST.finditer(code)]
    consts = {}
    while True:
        found = {}
        for name, rhs in defs:
            toks = tokens(rhs)
            r = Parser(toks, consts).sum(0)
            if r and r[1] == len(toks):
                # shortcut: one value per name across files, the longest; key by
                # module once two files bind a name to different values.
                old = found.get(name)
                found[name] = r[0] if old is None or r[0][1] > old[1] else old
        if found == consts:
            return consts
        consts = found


def statements(code):
    """Token lists of `code` split at `;`, `{` and `}`."""
    out, cur = [], []
    for tok in tokens(code):
        if tok[0] == "other" and tok[1] in ";{}":
            out.append(cur)
            cur = []
        else:
            cur.append(tok)
    out.append(cur)
    return out


def violations(sources, default, modules):
    """`path:line` lines for every deadline in `sources` ({path: text}) not below its cap."""
    codes = {path: strip(text) for path, text in sources.items()}
    consts = consts_of(codes)
    out = []
    for path, code in sorted(codes.items()):
        cap = modules.get(pathlib.Path(path).stem, default)
        for stmt in statements(code):
            if not ANCHOR.search(" ".join(t[1] for t in stmt)):
                continue
            best = longest(stmt, consts)
            if best and best[0] >= cap:
                out.append(
                    f"{path}:{best[1]}: deadline {float(best[0]):g} s is not below the nextest cap "
                    f"{cap} s (.config/nextest.toml period x terminate-after)"
                )
    return out


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


def sources_of(root):
    return {
        str(p.relative_to(REPO)): p.read_text()
        for p in sorted(root.rglob("*.rs"))
    }


def durations(secs):
    """Spellings of a Duration of `secs` (a multiple of 60) seconds."""
    third, half, mins = secs // 3, secs // 2, secs // 60
    yield f"Duration::from_secs({secs})"
    yield f"Duration::from_secs({secs}_u64)"
    yield f"std::time::Duration::from_secs({secs})"
    yield f"Duration::from_millis({secs * 1000:_})"
    yield f"Duration::from_millis({secs * 1000})"
    yield f"Duration::from_micros({secs * 10**6:_})"
    yield f"Duration::from_nanos({secs * 10**9:_})"
    yield f"Duration::from_secs_f64({secs}.0)"
    yield f"Duration::from_secs_f32({secs}.0)"
    yield f"Duration::from_mins({mins})"
    yield f"Duration::from_hours(1) / {3600 // secs}"
    yield f"Duration::new({secs}, 0)"
    yield f"Duration::new({secs - 1}, 1_000_000_000)"
    yield f"Duration::from_secs({third}) * 3"
    yield f"3 * Duration::from_secs({third})"
    yield f"Duration::from_secs(3 * {third})"
    yield f"Duration::from_secs({half}) + Duration::from_secs({half})"
    yield f"Duration::from_secs({mins} * 60)"


ANCHORS = [
    "let boot_deadline = {d};",
    "let until = Instant::now() + {d};",
    "let until = Instant::now().checked_add({d}).unwrap();",
    "let until = clock.now() + {d};",
    "let wait = Wait {{ deadline: {d}, step: 1 }};",
    "settle(&app, HANG_DEADLINE.max({d}));",
]


def layouts(src):
    yield "on one line", src
    yield "over several lines", src.replace("(", "(\n        ").replace(" + ", "\n        + ").replace(" * ", "\n        * ")


def indirections(d):
    """(name, {path: text} prefix, the expression a statement names)."""
    yield "inline", {}, d
    yield "through a const of another file", {"t/consts.rs": f"pub const WAIT: Duration = {d};\n"}, "WAIT"
    if d.startswith("Duration::from_secs(") and d.endswith(")") and d.count("(") == 1:
        n = d[len("Duration::from_secs("):-1]
        yield "through a number const", {"t/consts.rs": f"const SECS: u64 = {n};\n"}, "Duration::from_secs(SECS)"


def deadline_cases():
    """(name, sources, expected failures) for each spelling of a 180 s deadline
    (one failure) and of a 60 s one (none), under a 120 s cap."""
    for secs, want in ((180, 1), (60, 0)):
        for d, anchor in itertools.product(list(durations(secs)), ANCHORS):
            for via, prefix, expr in indirections(d):
                for layout, src in layouts(anchor.format(d=expr)):
                    name = f"{secs} s as `{d}` in `{anchor}` {via} {layout}"
                    yield name, {**prefix, "t/form.rs": f"fn t() {{\n    {src}\n}}\n"}, want


def self_check():
    toml = (
        '[profile.default]\nslow-timeout = { period = "30s", terminate-after = 4 }\n'
        "[[profile.default.overrides]]\n"
        'filter = "binary_id(holon-gpui::main) & test(/^long_loop::/)"\n'
        'slow-timeout = { period = "60s", terminate-after = 10 }\n'
    )
    default, modules = caps(toml)
    cases = [
        ("a 180 s boot deadline", {"t/boot.rs": "let boot_deadline = Instant::now() + Duration::from_secs(180);"}, 1),
        ("a deadline equal to the cap", {"t/eq.rs": "let deadline = Duration::from_secs(120);"}, 1),
        ("a 60 s hang guard", {"t/ok.rs": "let deadline = Instant::now() + Duration::from_secs(60);"}, 0),
        ("a 180 s deadline under a 600 s override", {"t/long_loop.rs": "let deadline = Instant::now() + Duration::from_secs(180);"}, 0),
        ("a 180 s duration that is no deadline", {"t/hold.rs": "let hold = Duration::from_secs(180);"}, 0),
        ("a 180 s deadline in a comment", {"t/c.rs": "// let deadline = Duration::from_secs(180);\nlet x = 1;"}, 0),
        ("a 180 s deadline in a string", {"t/s.rs": 'let msg = "deadline Duration::from_secs(180)";'}, 0),
        ("a 180 s hold beside a deadline statement", {"t/h.rs": "let hold = Duration::from_secs(180);\nlet deadline = Duration::from_secs(60);"}, 0),
        ("a 180 s deadline after a multi-line string", {"t/m.rs": 'let s = "a\nb";\nlet deadline =\n    Duration::from_secs(180);'}, 1),
    ]
    cases += list(deadline_cases())
    wrong = 0
    for name, sources, want in cases:
        got = violations(sources, default, modules)
        if len(got) != want:
            print(f"[windowed-deadlines] self-check {name}: {len(got)} failure(s), expected {want}: {got}")
            wrong += 1
    print(f"[windowed-deadlines] self-check: {len(cases) - wrong} of {len(cases)} cases as expected")
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
