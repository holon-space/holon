#!/usr/bin/env python3
"""Fail when a test-running `nextest run` in the justfile or under scripts/ does not pass `--no-tests=fail` once.

    scripts/check-nextest-no-tests.py [repo]          judge every call site; exit 1 names each offending call
    scripts/check-nextest-no-tests.py --list [repo]   print one `<kind> <name>` line per site with a judged call

Reads the source, so it sees every call site, also those no gate step runs. Comments and Python docstrings are
dropped first. Every other `nextest` ... `run` must be a call the check can read, or it fails as "cannot parse":
- shell (recipes, *.sh, sh shebangs): a command whose words hold `nextest` directly followed by `run`;
- Python (*.py, python shebangs): a list or tuple literal holding the strings "nextest", "run" in a row;
- any other file: no `nextest` ... `run` line at all.
A call's literal arguments up to a bare `--` must hold `--no-tests=fail` exactly once; one with `--no-run` builds
only. Expansions (`"$@"`, `{{FLAGS}}`, `*args`) are not read: their callers never pass `--no-tests`.
"""
import ast
import json
import re
import shlex
import subprocess
import sys
from pathlib import Path

FLAG = "--no-tests=fail"
NEXTEST_RUN = re.compile(r"(?<![\w-])nextest(?![\w-]).*\brun\b")
PUNCTUATION = ";&|()\n"
EXEMPT = {
    "scripts/check-nextest-no-tests.py": "names the call shapes it judges",
    "scripts/test-check-nextest-no-tests.py": "plants calls without the flag",
    "scripts/test-no-tests-check-reach.py": "names the call shapes it looks for",
    "scripts/check-gate-scripts.sh": "stubs and patterns of the gate's cargo calls",
    "scripts/check-gated-test-suites.sh": "planted recipes",
    "scripts/check-compile-shape.py": "judges recorded argv; its strings are patterns",
    "scripts/test-check-compile-shape.py": "planted call records",
    "scripts/test-lane-nextest.py": "planted selection lines",
    "scripts/check-red-logs.py": "patterns of red-log headers",
    "scripts/flow-checks/test-tests-cover-diff.py": "planted summaries",
    "scripts/flow-checks/fixtures/owned-known-reds-summary.md": "fixture summary",
    "scripts/flow-checks/tests-cover-diff.py": "reads nextest summary lines, runs none",
    "scripts/keystone-known-reds-fixture.sh": "planted nextest logs",
    "scripts/test-target-gc.py": "planted call records",
    "scripts/lane-nextest.py": "its run refuses arguments without the flag (scripts/test-lane-nextest.py)",
}


class Unparsed(Exception):
    pass


def shell_statements(text):
    """(separator before, words, separator after) of each simple command, comments dropped."""
    text = text.replace("\\\n", " ")
    # shlex drops a comment together with its line end; the doubled one keeps the command boundary.
    lex = shlex.shlex(text.replace("\n", "\n\n"), posix=True, punctuation_chars=PUNCTUATION)
    lex.whitespace = " \t\r"
    try:
        tokens = list(lex)
    except ValueError as e:
        raise Unparsed(str(e))
    out, words, before = [], [], "\n"
    for i, t in enumerate(tokens):
        redirect = t.startswith("&") and i and tokens[i - 1].endswith((">", "<"))
        brace = (t == "{" and not words) or (t == "}" and i and set(tokens[i - 1]) <= set(";\n"))
        if (set(t) <= set(PUNCTUATION) and not redirect) or brace:
            if words:
                out.append((before, words, t))
            words, before = [], t
        else:
            words.append(t)
    if words:
        out.append((before, words, "\n"))
    return out


def shell_calls(text):
    """Argument lists of the `nextest run` commands in a shell text; a word that holds one is read as shell."""
    try:
        statements = shell_statements(text)
    except Unparsed as e:
        hit = next((line for line in text.splitlines() if NEXTEST_RUN.search(line)), None)
        if hit is not None:
            raise Unparsed("%s: %s" % (e, hit.strip()[:120]))
        return []
    calls = []
    for _, words, _ in statements:
        for i, w in enumerate(words):
            if w == "nextest" and words[i + 1:i + 2] == ["run"]:
                calls.append(words[i + 2:])
            elif w == "nextest" and "run" in words[i + 1:]:
                raise Unparsed(" ".join(words))
            elif w != "nextest" and NEXTEST_RUN.search(w):
                calls += shell_calls(w)
    return calls


def docstrings(tree):
    nodes = (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)
    return {id(n.body[0].value) for n in ast.walk(tree) if isinstance(n, nodes) and n.body
            and isinstance(n.body[0], ast.Expr) and isinstance(n.body[0].value, ast.Constant)}


def string(node, value=None):
    return isinstance(node, ast.Constant) and isinstance(node.value, str) and value in (None, node.value)


def python_calls(text):
    """Argument lists of the ["nextest", "run", ...] literals in a Python text; a non-literal argument is None."""
    try:
        tree = ast.parse(text)
    except SyntaxError as e:
        raise Unparsed(str(e))
    calls, seen = [], docstrings(tree)
    for node in ast.walk(tree):
        if not isinstance(node, (ast.List, ast.Tuple)):
            continue
        for i, e in enumerate(node.elts):
            if not string(e, "nextest"):
                continue
            seen.add(id(e))
            following = node.elts[i + 1:i + 2]
            if following and string(following[0], "run"):
                seen.add(id(following[0]))
                calls.append([a.value if string(a) else None for a in node.elts[i + 2:]])
            elif following and not string(following[0]):
                raise Unparsed("line %d: nextest followed by a non-literal" % e.lineno)
    for node in ast.walk(tree):
        if string(node) and id(node) not in seen and NEXTEST_RUN.search(node.value):
            raise Unparsed("line %d: %s" % (node.lineno, node.value.strip()[:120]))
    return calls


def raw_calls(text):
    hit = next((line for line in text.splitlines() if NEXTEST_RUN.search(line)), None)
    if hit is not None:
        raise Unparsed(hit.strip()[:120])
    return []


def reader(path, text):
    first = text.split("\n", 1)[0]
    if path.suffix == ".py" or first.startswith("#!") and "python" in first:
        return python_calls
    if path.suffix == ".sh" or first.startswith("#!") and re.search(r"\b(ba)?sh\b", first):
        return shell_calls
    return raw_calls


def recipes(repo):
    """Name to (body, reader) of every recipe; `{{...}}` reads as one word."""
    dump = json.loads(subprocess.run(["just", "--justfile", str(repo / "justfile"), "--dump", "--dump-format", "json"],
                                     capture_output=True, text=True, check=True).stdout)
    assert not NEXTEST_RUN.search(json.dumps(dump["assignments"])), "a justfile variable holds a nextest run"
    out = {}
    for name, r in dump["recipes"].items():
        lines = ["".join(f if isinstance(f, str) else "JUSTEXPR" for f in line) for line in r["body"]]
        if r["shebang"]:
            read = python_calls if "python" in lines[0] else shell_calls
        else:
            lines, read = [l.lstrip("@-") for l in lines], shell_calls
        out[name] = ("\n".join(lines), read)
    return out


def script_files(repo):
    return sorted(p for p in (repo / "scripts").rglob("*")
                  if p.is_file() and not {"__pycache__", "target"} & set(p.relative_to(repo).parts))


def sites(repo):
    """(kind, name, calls or the Unparsed error) of every site with a nextest occurrence."""
    found = []
    for name, (body, read) in recipes(repo).items():
        found.append(("recipe", name, read, body))
    for p in script_files(repo):
        name = p.relative_to(repo).as_posix()
        if name not in EXEMPT:
            text = p.read_text(errors="replace")
            found.append(("script", name, reader(p, text), text))
    out = []
    for kind, name, read, text in found:
        try:
            calls = [c[:c.index("--")] if "--" in c else c for c in read(text)]
        except Unparsed as e:
            out.append((kind, name, e))
            continue
        calls = [c for c in calls if "--no-run" not in c]
        if calls:
            out.append((kind, name, calls))
    return out


def main():
    args = sys.argv[1:]
    listing = args[:1] == ["--list"]
    args = args[1:] if listing else args
    repo = Path(args[0] if args else Path(__file__).resolve().parent.parent)
    found = sites(repo)
    assert found, f"no nextest call site in {repo}: the site search is broken"
    if listing:
        for kind, name, calls in found:
            if not isinstance(calls, Unparsed):
                print(kind, name)
        return 0
    fails = 0
    for kind, name, calls in found:
        if isinstance(calls, Unparsed):
            fails += 1
            print("NO-TESTS: %s %s cannot parse a nextest run: %s" % (kind, name, calls))
            continue
        for c in calls:
            if [a for a in c if a is not None and a.startswith("--no-tests")] != [FLAG]:
                fails += 1
                print("NO-TESTS: %s %s does not pass %s exactly once: %s" % (kind, name, FLAG,
                                                                             " ".join(a or "<expr>" for a in c)))
    n = sum(len(c) for _, _, c in found if not isinstance(c, Unparsed))
    print("check-nextest-no-tests: %d call(s) at %d site(s), %d failure(s)" % (n, len(found), fails))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
