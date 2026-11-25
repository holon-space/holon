#!/usr/bin/env python3
"""Fail when a windowed test's deadline is not below its nextest cap.

A deadline at or past the cap never fires: nextest kills the test first, and the
kill prints a bare TIMEOUT instead of the deadline's message with the last value
it observed. The cap of a `holon-gpui::main` module is `period x terminate-after`
of the first `[[profile.default.overrides]]` block whose filter names it as
`binary_id(holon-gpui::main) & test(/^<module>::/)`, else of `[profile.default]`.

A deadline is the longest `Duration` a statement computes when the statement
names a deadline, an `elapsed()` or adds to a `now()`. A `Duration` is any of
its constructors, `Duration::new`, `Duration::MAX`, a `const` of the tests tree
or a `let` of its fn, and sums and products of those with numbers.

The check fails a deadline it cannot evaluate: a Duration added to a `now()`,
compared with an `elapsed()` or a `now() - start`, or given to `time::timeout`
or `recv_timeout` must be such a value, or a name that its fn asserts below the
cap (`assert!(timeout <= HANG_DEADLINE, ..)`).

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
ANCHOR = re.compile(r"(?i:deadline)|\belapsed\b|\bnow\s*\(\s*\)\s*(?:\+|\.\s*checked_add\s*\()")
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
            elif method in ("checked_mul", "saturating_mul", "mul_f32", "mul_f64") and self.at(r[1] + 2, "("):
                arg = self.sum(r[1] + 3)
                if not r[0][0] or not arg or arg[0][0] or not self.at(arg[1], ")"):
                    break
                r = ((True, r[0][1] * arg[0][1]), arg[1] + 1)
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


def statements(toks):
    """(start, end) index ranges of `toks` split at `;`, `{` and `}`."""
    out, start = [], 0
    for i, tok in enumerate(toks):
        if tok[0] == "other" and tok[1] in ";{}":
            out.append((start, i))
            start = i + 1
    out.append((start, len(toks)))
    return out


def fn_bodies(toks):
    """(start, end) index ranges of every `fn` body of `toks`."""
    out = []
    for i, tok in enumerate(toks):
        if tok[1] != "fn":
            continue
        depth, j = 0, i + 1
        while j < len(toks) and not (depth == 0 and toks[j][1] in ";{"):
            depth += {"(": 1, ")": -1}.get(toks[j][1], 0)
            j += 1
        if j == len(toks) or toks[j][1] == ";":
            continue
        start, depth = j, 0
        while j < len(toks):
            depth += {"{": 1, "}": -1}.get(toks[j][1], 0)
            j += 1
            if depth == 0:
                break
        out.append((start, j))
    return out


def scope_of(i, bodies, n):
    """The innermost fn body around token `i`, else the whole file."""
    inside = [b for b in bodies if b[0] <= i < b[1]]
    return max(inside) if inside else (0, n)


def is_ident(toks, k):
    """Whether `toks[k]` is a bare local name, not a field, call or path."""
    return (
        k < len(toks)
        and toks[k][0] == "path"
        and "::" not in toks[k][1]
        and (k == 0 or toks[k - 1][1] != ".")
        and (k + 1 == len(toks) or toks[k + 1][1] not in (".", "("))
    )


def locals_of(toks, scope, consts):
    """`consts` plus every `let` of `scope` bound to a number or a Duration."""
    out = dict(consts)
    for i in range(*scope):
        if toks[i][1] != "let":
            continue
        k = i + 2 if toks[i + 1][1] == "mut" else i + 1
        if not is_ident(toks, k):
            continue
        j = k + 1
        while j < scope[1] and toks[j][1] not in ("=", ";"):
            j += 1
        if j < scope[1] and toks[j][1] == "=":
            r = Parser(toks, out).sum(j + 1)
            if r and r[1] < len(toks) and toks[r[1]][1] == ";":
                out[toks[k][1]] = r[0]
    return out


def asserted_below(toks, scope, name, consts, cap):
    """Whether `scope` holds `assert!(name <= D` or `assert!(name < D` with D
    a Duration below `cap`."""
    for i in range(scope[0], scope[1] - 5):
        if [t[1] for t in toks[i : i + 5]] != ["assert", "!", "(", name, "<"]:
            continue
        k = i + 6 if toks[i + 5][1] == "=" else i + 5
        r = Parser(toks, consts).sum(k)
        if r and r[0][0] and r[0][1] < cap and toks[r[1]][1] in (",", ")"):
            return True
    return False


ARITH = ("+", "-", "*", "/", ".")


def operand(toks, start, end, consts):
    """The Duration operand that starts at `start`, or (`start` None) ends at
    `end`: ("value", seconds), ("name", its name), or None for anything else."""
    p = Parser(toks, consts)
    for j in [start] if start is not None else range(end):
        r = p.sum(j)
        if r and r[0][0] and (r[1] == end if start is None else r[1] >= len(toks) or toks[r[1]][1] not in ARITH):
            return "value", r[0][1]
    j = start if start is not None else end - 1
    if is_ident(toks, j) and (start is None or j + 1 >= len(toks) or toks[j + 1][1] not in ARITH):
        return "name", toks[j][1]
    return None


def positions(toks, s, e):
    """(start, end) of every Duration operand in a deadline position of
    `toks[s:e]`, as `operand` takes them: added to a `now()`, compared with an
    `elapsed()` or a `now() - start`, or given to `time::timeout` or
    `recv_timeout`."""
    out = []
    texts = [t[1] for t in toks]

    def at(i, *seq):
        return texts[i : i + len(seq)] == list(seq)

    for i in range(s, e):
        t = texts[i]
        name = t.split("::")
        marker = None
        if name[-1] == "now" and at(i + 1, "(", ")", "+"):
            out.append((i + 4, e))
        elif name[-1] == "now" and at(i + 1, "(", ")", ".", "checked_add", "("):
            out.append((i + 6, e))
        elif name[-2:] == ["time", "timeout"] and at(i + 1, "("):
            out.append((i + 2, e))
        elif t == "recv_timeout" and at(i - 1, ".") and at(i + 1, "("):
            out.append((i + 2, e))
        elif t == "elapsed" and at(i + 1, "(", ")"):
            marker = i + 3
        elif name[-1] == "now" and at(i + 1, "(", ")", "-") and is_ident(toks, i + 4):
            marker = i + 5
        if marker is None:
            continue
        if marker < e and texts[marker] in ("<", ">"):
            out.append((marker + 2 if at(marker + 1, "=") else marker + 1, e))
        b = i
        while b - 2 >= s and texts[b - 1] == "." and toks[b - 2][0] == "path":
            b -= 2
        c = b - 2 if b - 2 >= s and texts[b - 1] == "=" and texts[b - 2] in ("<", ">") else b - 1
        if c >= s and texts[c] in ("<", ">"):
            out.append((None, c))
    return out


def violations(sources, default, modules):
    """`path:line` lines, at most one per statement, for every deadline in
    `sources` ({path: text}) not below its cap, and for every Duration in a
    deadline position that is no literal, const or `let` of them, and no name
    its fn asserts below the cap."""
    codes = {path: strip(text) for path, text in sources.items()}
    consts = consts_of(codes)
    out = []
    for path, code in sorted(codes.items()):
        cap = modules.get(pathlib.Path(path).stem, default)
        toks = tokens(code)
        bodies = fn_bodies(toks)
        known_in = {}
        for s, e in statements(toks):
            spots = positions(toks, s, e)
            if not spots and not ANCHOR.search(" ".join(t[1] for t in toks[s:e])):
                continue
            scope = scope_of(s, bodies, len(toks))
            if scope not in known_in:
                known_in[scope] = locals_of(toks, scope, consts)
            known = known_in[scope]
            found = [operand(toks, k, end, known) for k, end in spots]
            best = longest(toks[s:e], known)
            longest_spot = max((f[1] for f in found if f and f[0] == "value"), default=None)
            if longest_spot is not None and (best is None or longest_spot > best[0]):
                best = (longest_spot, toks[s][2])
            unbounded = [
                (k, end) for (k, end), f in zip(spots, found)
                if f is None or (f[0] == "name" and not asserted_below(toks, scope, f[1], known, cap))
            ]
            if unbounded:
                k, end = unbounded[0]
                out.append(
                    f"{path}:{toks[k if k is not None else end - 1][2]}: a Duration in a deadline position that is "
                    f"no literal, const or `let` of them, and no name its fn asserts below the nextest cap {cap} s"
                )
            elif best and best[0] >= cap:
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
    yield f"Duration::from_secs({third}).saturating_mul(3)"
    yield f"Duration::from_secs({third}).checked_mul(3).unwrap()"
    yield f"Duration::from_secs({half}).mul_f64(2.0)"
    yield f"Duration::from_secs({half}).mul_f32(2.0)"


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


def deadline_forms():
    """Every spelling of a 180 s deadline: each Duration constructor of that
    length, bound to a name that says deadline or added to `Instant::now()`."""
    durations = [
        "Duration::from_secs(180)",
        "Duration::from_millis(180_000)",
        "Duration::from_millis(180000)",
        "Duration::from_micros(180_000_000)",
        "Duration::from_secs_f64(180.0)",
        "Duration::from_secs_f32(180.0)",
    ]
    for d in durations:
        yield f"{d} named a deadline", f"let boot_deadline = {d};"
        yield f"Instant::now() + {d}", f"let until = Instant::now() + {d};"
        yield f"Instant::now().checked_add({d})", f"let until = Instant::now().checked_add({d}).unwrap();"


def deadline_cases():
    """(name, sources, expected failures) for each spelling of a 180 s deadline
    (one failure) and of a 60 s one (none), under a 120 s cap."""
    for secs, want in ((180, 1), (60, 0)):
        for d, anchor in itertools.product(list(durations(secs)), ANCHORS):
            for via, prefix, expr in indirections(d):
                for layout, src in layouts(anchor.format(d=expr)):
                    name = f"{secs} s as `{d}` in `{anchor}` {via} {layout}"
                    yield name, {**prefix, "t/form.rs": f"fn t() {{\n    {src}\n}}\n"}, want


def carried_deadlines():
    """(name, sources, expected failures) for a 180 s deadline (one failure)
    and a 60 s one (none, unless a helper's argument that nothing bounds
    carries it) whose Duration reaches the deadline through a local binding, a
    helper's argument, or an `elapsed()` comparison."""
    carriers = [
        ("a local binding", "fn t() {{\n    let wait = {d};\n    let deadline = Instant::now() + wait;\n}}\n", 0),
        ("a helper argument added to now()", "fn f(timeout: Duration) {{\n    let until = Instant::now() + timeout;\n}}\nfn t() {{\n    f({d});\n}}\n", 1),
        ("a helper argument against elapsed()", "fn f(timeout: Duration) {{\n    let start = Instant::now();\n    while start.elapsed() < timeout {{}}\n}}\nfn t() {{\n    f({d});\n}}\n", 1),
        ("elapsed() against the Duration", "fn t() {{\n    let start = Instant::now();\n    while start.elapsed() < {d} {{}}\n}}\n", 0),
    ]
    for secs, want in ((180, 1), (60, 0)):
        for d, (via, src, unbounded) in itertools.product(list(durations(secs)), carriers):
            yield f"{secs} s as `{d}` through {via}", {"t/carried.rs": src.format(d=d)}, max(want, unbounded)


POSITIONS = [
    "let until = Instant::now() + {x};",
    "let until = clock.now().checked_add({x}).unwrap();",
    "while start.elapsed() <= {x} {{}}",
    "assert!({x} > start.elapsed(), \"hung\");",
    "if clock.now() - start >= {x} {{}}",
    "let row = rx.recv_timeout({x});",
    "let row = tokio::time::timeout({x}, fut).await;",
]


def bounded_positions():
    """(name, sources, expected failures) for each deadline position given a
    Duration by each carrier: a literal or a `let` of one fails at 180 s and
    passes at 60 s; a helper's argument fails unless the helper asserts it
    below the cap; a field fails."""
    hang = "const HANG_DEADLINE: Duration = Duration::from_secs(60);\n"
    for pos in POSITIONS:
        for secs, want in ((180, 1), (60, 0)):
            for d in durations(secs):
                carriers = [
                    ("inline", f"fn t() {{\n    {pos.format(x=d)}\n}}\n", want),
                    ("through a let", f"fn t() {{\n    let wait = {d};\n    {pos.format(x='wait')}\n}}\n", want),
                ]
                for via, src, n in carriers:
                    yield f"{secs} s as `{d}` {via} in `{pos}`", {"t/pos.rs": src}, n
        for via, guard, n in [
            ("an argument nothing bounds", "", 1),
            ("an argument asserted below the cap", 'assert!(timeout <= HANG_DEADLINE, "too long");', 0),
            ("an argument asserted strictly below the cap", "assert!(timeout < Duration::from_secs(90));", 0),
            ("an argument asserted below 180 s", "assert!(timeout <= Duration::from_secs(180));", 1),
            ("another argument asserted", 'assert!(other <= HANG_DEADLINE, "too long");', 1),
        ]:
            src = f"{hang}fn f(timeout: Duration, other: Duration) {{\n    {guard}\n    {pos.format(x='timeout')}\n}}\n"
            yield f"{via} in `{pos}`", {"t/pos.rs": src}, n
        yield f"a field in `{pos}`", {"t/pos.rs": f"fn t(&self) {{\n    {pos.format(x='self.timeout')}\n}}\n"}, 1


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
    cases += [(f"a 180 s deadline written as {form}", {"t/form.rs": src}, 1) for form, src in deadline_forms()]
    cases += list(carried_deadlines())
    cases += list(bounded_positions())
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
