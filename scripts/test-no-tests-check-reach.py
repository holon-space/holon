#!/usr/bin/env python3
"""The check that every `cargo nextest run` passes `--no-tests=fail` sees every such call.

Run: `python3 scripts/test-no-tests-check-reach.py`. scripts/check-compile-shape.py judges the cargo calls the
landing gate recorded, so a call site counts as checked only when a gate step that runs before that judge reaches it.
Generator: every test-running nextest call site of the justfile (a recipe) and of scripts/ (a script file).
Property: each site is reached from a gate-integrate.sh step through `just <recipe>` and `scripts/<file>` references.
A site also counts as checked when a reached step runs scripts/check-nextest-no-tests.py and its `--list` names it.
Reach follows code only (comments and docstrings dropped). The source check counts as run only when a reached recipe
under `set -euo pipefail` runs it as a command whose failure is not caught (`|| ...`, `if`, `!`), not when a text
names it.
"""
import ast
import json
import re
import runpy
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CALL = re.compile(r"^\s*(exec\s+|[A-Z_]+=\S+\s+)*cargo nextest run\b|\[\s*\"cargo\",\s*\"nextest\",\s*\"run\"")
JUST = re.compile(r"\bjust\s+([a-z][a-z0-9-]*)")
SCRIPT = re.compile(r"scripts/[A-Za-z0-9_./-]+\.(?:sh|py)")
STEP_ALIAS = {"featuremap": "cargo-free-checks"}
STEP_SCRIPT = {"keystone-smoke", "hand-authored", "extra-nextest", "keystone-tour"}
SOURCE_CHECK = ("script", "scripts/check-nextest-no-tests.py")
PARSE = runpy.run_path(str(REPO / SOURCE_CHECK[1]))


def recipes():
    dump = json.loads(subprocess.run(["just", "--justfile", str(REPO / "justfile"), "--dump", "--dump-format", "json"],
                                     capture_output=True, text=True, check=True).stdout)
    out = {}
    for name, r in dump["recipes"].items():
        lines = ["".join(f if isinstance(f, str) else "{{}}" for f in line) for line in r["body"]]
        out[name] = "\n".join(lines)
    return out


def code_lines(text):
    return [l for l in text.splitlines() if not l.lstrip().startswith(("#", '"""', "'"))]


def runs_tests(line):
    return bool(CALL.search(line)) and "--no-run" not in line


def call_sites(rec):
    sites = [("recipe", n) for n, body in rec.items() if any(runs_tests(l) for l in code_lines(body))]
    for p in sorted((REPO / "scripts").glob("*")):
        if p.suffix not in (".sh", ".py") or p.name.startswith(("test-", "check-")):
            continue
        if any(runs_tests(l) for l in code_lines(p.read_text(errors="replace"))):
            sites.append(("script", "scripts/" + p.name))
    return sites


def gate_steps():
    text = (REPO / "scripts/gate-integrate.sh").read_text()
    tracks = re.search(r"TRACKS=\((.*?)\n\)", text, re.S)[1]
    return re.findall(r"\d+:([a-z0-9-]+)", tracks)


def is_python(name, text):
    return name.endswith(".py") or text.startswith("#!") and "python" in text.split("\n", 1)[0]


def code(name, text):
    """The words and strings of the code of a recipe or script, comments and docstrings dropped; a shell text
    shlex cannot read keeps all but its comment lines."""
    if is_python(name, text):
        tree = ast.parse(text)
        docs = PARSE["docstrings"](tree)
        return " ".join(n.value for n in ast.walk(tree) if PARSE["string"](n) and id(n) not in docs)
    try:
        return " ".join(w for _, words, _ in PARSE["shell_statements"](text) for w in words)
    except PARSE["Unparsed"]:
        return "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("#"))


def runs_source_check(name, text):
    """Whether a shell text runs the source check so that its failure fails the text's run."""
    if is_python(name, text):
        return False
    try:
        statements = PARSE["shell_statements"](text)
    except PARSE["Unparsed"]:
        return False
    if not any(words == ["set", "-euo", "pipefail"] for _, words, _ in statements):
        return False
    for before, words, after in statements:
        argv = words[1:] if words[0] in ("python3", "/usr/bin/python3") else words
        if argv[0] in ("./" + SOURCE_CHECK[1], SOURCE_CHECK[1]) and "--list" not in argv \
                and "||" not in before + after and after.strip("\n") != "&":
            return True
    return False


def reached(rec):
    todo = []
    for s in gate_steps():
        todo.append(("script", "scripts/gate-integrate-steps.sh") if s in STEP_SCRIPT else ("recipe", STEP_ALIAS.get(s, s)))
    seen = set()
    while todo:
        node = todo.pop()
        if node in seen:
            continue
        seen.add(node)
        kind, name = node
        if kind == "recipe":
            text = rec.get(name, "")
        else:
            p = REPO / name
            text = p.read_text(errors="replace") if p.exists() else ""
        if runs_source_check(name, text):
            todo.append(SOURCE_CHECK)
        text = code(name, text).replace(SOURCE_CHECK[1], "")
        todo += [("recipe", r) for r in JUST.findall(text) if r in rec]
        todo += [("script", s) for s in SCRIPT.findall(text)]
    return seen


def run_removals(rec):
    """Copies of the recipes, each with one command that runs the source check taken out of one recipe."""
    run = re.compile(r"(?:\./|python3 )?" + re.escape(SOURCE_CHECK[1]) + r"(?:\s+&&)?")
    for name, body in rec.items():
        for m in run.finditer(body):
            yield name, {**rec, name: body[:m.start()] + body[m.end():]}


def main():
    rec = recipes()
    removals = list(run_removals(rec))
    assert removals, "no recipe runs the source check"
    for name, cut in removals:
        assert SOURCE_CHECK not in reached(cut), f"the source check counts as run with its run cut from recipe {name}"
    sites = call_sites(rec)
    assert sites, "no nextest call site found: the generator is broken"
    seen = reached(rec)
    if SOURCE_CHECK in seen:
        listed = subprocess.run([sys.executable, str(REPO / SOURCE_CHECK[1]), "--list"], capture_output=True, text=True,
                                check=True).stdout
        seen |= {tuple(l.split(" ", 1)) for l in listed.splitlines()}
    missed = [s for s in sites if s not in seen]
    for kind, name in sites:
        print(("checked   " if (kind, name) in seen else "UNCHECKED ") + f"{kind} {name}")
    assert not missed, f"{len(missed)} of {len(sites)} nextest call sites run in no gate step before the judge: {missed}"
    print("ok")


if __name__ == "__main__":
    sys.exit(main())
