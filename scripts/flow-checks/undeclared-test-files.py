#!/usr/bin/env python3
"""Check that every Rust file under `tests/` of a crate with `autotests = false` is compiled into a test target.

    scripts/flow-checks/undeclared-test-files.py --base <commit> --summary <path>

Run from the repo or lane root; it checks the whole tree of the workspace members, so `--base` and `--summary` are
accepted for the flow check interface and not read. Cargo builds only the declared `[[test]]` targets of such a crate:
a `tests/**/*.rs` file that no target reaches through its `mod` lines (`#[path]` included) never compiles and never
runs. Module paths resolve as rustc resolves them. Files that are not Rust are not checked.
PASS (exit 0) when every such file is reached; each problem is one stderr line.
"""
import argparse
import re
import sys
import tomllib
from pathlib import Path

TOKEN = re.compile(r"""
    \s+ | //[^\n]*
  | (?P<block>/\*)
  | (?P<raw>b?r(?P<hashes>\#*)")
  | (?P<str>b?"(?:\\.|[^"\\])*")
  | b?'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])'
  | (?P<ident>r\#\w+|[A-Za-z_]\w*)
  | (?P<punct>.)
""", re.X | re.S)


def tokens(text):
    """The identifiers, string values and punctuation of Rust source; comments, chars and lifetimes' quotes dropped."""
    out, i = [], 0
    while i < len(text):
        m = TOKEN.match(text, i)
        if m.group("block"):
            depth, i = 1, m.end()
            while depth:
                close, open_ = text.find("*/", i), text.find("/*", i)
                assert close >= 0, f"unterminated block comment at offset {m.start()}"
                depth, i = (depth + 1, open_ + 2) if 0 <= open_ < close else (depth - 1, close + 2)
            continue
        if m.group("raw"):
            end = text.index('"' + m.group("hashes"), m.end())
            out.append(("str", text[m.end():end]))
            i = end + 1 + len(m.group("hashes"))
            continue
        if s := m.group("str"):
            out.append(("str", s[s.index('"') + 1:-1]))
        elif m.group("ident") or m.group("punct"):
            out.append(("ident" if m.group("ident") else "punct", m.group(0)))
        i = m.end()
    return out


def module_declarations(text):
    """Yield (inline, name, path_attr, has_body) for each `mod`; `inline` lists the enclosing inline modules as
    (name, path_attr) pairs."""
    toks = tokens(text)
    stack, pending_path, i = [], None, 0
    while i < len(toks):
        if toks[i:i + 2] == [("punct", "#"), ("punct", "[")]:
            j, depth = i + 2, 1
            while depth:
                depth += {("punct", "["): 1, ("punct", "]"): -1}.get(toks[j], 0)
                j += 1
            body = toks[i + 2:j - 1]
            if len(body) == 3 and body[:2] == [("ident", "path"), ("punct", "=")] and body[2][0] == "str":
                pending_path = body[2][1]
            i = j
            continue
        if toks[i] == ("ident", "pub"):
            if toks[i + 1:i + 2] == [("punct", "(")]:
                i = toks.index(("punct", ")"), i)
            i += 1
            continue
        if toks[i] == ("ident", "mod") and i + 2 < len(toks) and toks[i + 1][0] == "ident" \
                and toks[i + 2] in (("punct", ";"), ("punct", "{")):
            name, has_body = toks[i + 1][1].removeprefix("r#"), toks[i + 2][1] == "{"
            yield list(m for m in stack if m), name, pending_path, has_body
            if has_body:
                stack.append((name, pending_path))
            pending_path, i = None, i + 3
            continue
        if toks[i] == ("punct", "{"):
            stack.append(None)
        elif toks[i] == ("punct", "}"):
            stack.pop()
        pending_path = None
        i += 1


def reached_files(root):
    """Every file the crate root `root` compiles through its `mod` lines, or the problems that stop the walk."""
    # A module context is (file, relative): `relative` is the module name a non-mod-rs file adds to child paths.
    seen, todo, problems = set(), [(root, None)], []
    while todo:
        f, relative = todo.pop()
        if (f, relative) in seen:
            continue
        seen.add((f, relative))
        for inline, name, path_attr, has_body in module_declarations(f.read_text()):
            if has_body:
                continue
            d, rel = f.parent, relative
            for inner_name, inner_path in inline:
                d, rel = (d / inner_path, None) if inner_path else (d / (rel or "") / inner_name, None)
            if path_attr:
                todo.append(((d / path_attr).resolve(), None))
                continue
            d = d / (rel or "")
            found = [p for p in (d / f"{name}.rs", d / name / "mod.rs") if p.is_file()]
            if len(found) != 1:
                problems.append(f"{f} declares `mod {name};` and {len(found)} of {d}/{name}.rs, "
                                f"{d}/{name}/mod.rs exist")
                continue
            todo.append((found[0].resolve(), None if found[0].name == "mod.rs" else name))
    return {f for f, _ in seen}, problems


def check(repo):
    """The problems of the workspace at `repo`."""
    members = tomllib.loads((repo / "Cargo.toml").read_text())["workspace"]["members"]
    problems = []
    for member in members:
        assert not set(member) & set("*?["), f"workspace member glob {member!r} is not supported"
        crate = repo / member
        manifest = tomllib.loads((crate / "Cargo.toml").read_text())
        if manifest.get("package", {}).get("autotests", True):
            continue
        reached = set()
        for target in manifest.get("test", []):
            name = target["name"]
            if "path" in target:
                root = crate / target["path"]
            else:
                root = next((p for p in (crate / f"tests/{name}.rs", crate / f"tests/{name}/main.rs") if p.is_file()),
                            crate / f"tests/{name}.rs")
            if not root.is_file():
                problems.append(f"{crate / 'Cargo.toml'}: [[test]] {name} has no file {root}")
                continue
            files, bad = reached_files(root.resolve())
            reached |= files
            problems += bad
        problems += [f"{p.relative_to(repo.resolve())} is compiled by no [[test]] target of {member} "
                     f"(autotests = false): declare it with `mod` in a test target or add a [[test]] entry"
                     for p in sorted(p.resolve() for p in (crate / "tests").rglob("*.rs")) if p not in reached]
    return problems


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base", required=True)
    p.add_argument("--summary", required=True)
    p.parse_args()
    problems = check(Path.cwd())
    for problem in problems:
        print(f"undeclared-test-files: {problem}", file=sys.stderr)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
