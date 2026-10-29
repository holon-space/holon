#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/undeclared-test-files.py.

Run: `python3 scripts/flow-checks/test-undeclared-test-files.py [cases]`. Exits non-zero on the first violation.
Each case is a workspace in a temp dir. The generated cases compare the check with rustc: the files a test target
compiles are the ones `rustc --emit dep-info` lists for its root.
fixtures/undeclared-test-files/orphan-test-file.patch adds a test file that the `main` target does not declare.
"""
import os
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
CHECKER = HERE / "undeclared-test-files.py"
REPO = HERE.parents[1]
ORPHAN_PATCH = HERE / "fixtures/undeclared-test-files/orphan-test-file.patch"
PROBLEM_SUFFIX = " is compiled by no [[test]] target"


def workspace(root, crates):
    """`crates` maps a member path to (Cargo.toml text, {path under the crate: text})."""
    (root / "Cargo.toml").write_text("[workspace]\nmembers = [" + ", ".join(f'"{m}"' for m in crates) + "]\n")
    for member, (manifest, files) in crates.items():
        (root / member).mkdir(parents=True, exist_ok=True)
        (root / member / "Cargo.toml").write_text(manifest)
        for path, text in files.items():
            (root / member / path).parent.mkdir(parents=True, exist_ok=True)
            (root / member / path).write_text(text)


def run(root):
    return subprocess.run([sys.executable, str(CHECKER), "--base", "3936d83", "--summary", "unused.md"],
                          cwd=root, capture_output=True, text=True)


def manifest(targets, checked=True):
    head = '[package]\nname = "c"\nversion = "0.1.0"\nedition = "2021"\n' + ("autotests = false\n" if checked else "")
    return head + "".join(f'\n[[test]]\nname = "{name}"\n' + (f'path = "{path}"\n' if path else "")
                          for name, path in targets)


def expect(name, r, code, *needles):
    assert (r.returncode == 0) == (code == 0), f"{name}: exit {r.returncode}, stderr:\n{r.stderr}"
    for n in needles:
        assert n in r.stderr, f"{name}: {n!r} not on stderr:\n{r.stderr}"
    print(f"[undeclared-test-files] {name}: {'PASS' if code == 0 else 'FAIL'} as expected")


def case(crates):
    with tempfile.TemporaryDirectory() as root:
        workspace(Path(root), crates)
        return run(root)


def fixtures():
    main = [("main", "tests/main.rs")]
    with tempfile.TemporaryDirectory() as root:
        root = Path(root)
        crate = "crates/holon-transition-contract"
        workspace(root, {crate: ((REPO / crate / "Cargo.toml").read_text(), {})})
        shutil.copytree(REPO / crate / "tests", root / crate / "tests")
        expect("real crate tree", run(root), 0)
        subprocess.run(["patch", "-p0", "-s", "-d", str(root), "-i", str(ORPHAN_PATCH)], check=True)
        expect("real crate tree with an undeclared test file", run(root), 1,
               f"{crate}/tests/orphan_probe.rs{PROBLEM_SUFFIX}")
    orphan = {"tests/main.rs": "mod a;\n", "tests/a.rs": "", "tests/orphan.rs": "#[test] fn t() {}\n"}
    expect("crate without autotests = false", case({"c": (manifest(main, checked=False), orphan)}), 0)
    expect("undeclared file", case({"c": (manifest(main), orphan)}), 1, f"c/tests/orphan.rs{PROBLEM_SUFFIX}")
    expect("declared helper modules", case({"c": (manifest(main), {
        "tests/main.rs": "mod suite;\n#[path = \"shared/h.rs\"]\nmod h;\n",
        "tests/suite/mod.rs": "mod one;\n", "tests/suite/one.rs": "", "tests/shared/h.rs": ""})}), 0)
    expect("files that are not Rust", case({"c": (manifest(main), {
        "tests/main.rs": "", "tests/fixtures/page.org": "* a\n", "tests/data.rs.txt": "", "tests/golden/x.json": "{}"})}), 0)
    expect("undeclared directory", case({"c": (manifest(main), {"tests/main.rs": "", "tests/suite/a.rs": ""})}), 1,
           f"c/tests/suite/a.rs{PROBLEM_SUFFIX}")
    expect("missing module file", case({"c": (manifest(main), {"tests/main.rs": "mod gone;\n"})}), 1, "`mod gone;`")


class Layout:
    """One random crate: test target roots, module trees, and files no target declares."""

    def __init__(self, rng):
        self.rng, self.files, self.ids, self.path_files = rng, {}, iter(range(10 ** 6)), []
        self.targets = []

    def fresh(self, prefix="m"):
        return f"{prefix}{next(self.ids)}"

    def put(self, path, text):
        path = os.path.normpath(path)
        assert path not in self.files, path
        self.files[path] = text

    def free(self, *paths):
        return not any(os.path.normpath(p) in self.files for p in paths)

    def noise(self):
        """Rust items that hold `mod` text, braces and quotes outside a module declaration; `ghost` is a file at the
        path such a `mod` line would name."""
        n, ghost = next(self.ids), self.fresh("ghost")
        return ghost, self.rng.choice([
            f"// mod {ghost};",
            f"/* mod {ghost}; /* nested */ mod {ghost}; */",
            f'const S{n}: &str = "mod {ghost}; \\" {{";',
            f'const R{n}: &str = r#"mod {ghost}; "{{"#;',
            f"const C{n}: char = '{{'; // mod {ghost};",
            f"const D{n}: [char; 2] = ['}}', '\\''];",
            f"const B{n}: u8 = b'{{';",
            f"fn f{n}<'a>(x: &'a str) -> &'a str {{ x }}",
            f"#[test]\nfn t{n}() {{ let _ = '\\u{{7b}}'; }}",
        ])

    def body(self, d, rel, depth, d_is_file_dir=True):
        """The text of a module whose `mod x;` lines resolve in directory `d` (plus `rel` for a non-mod-rs file)."""
        lines = []
        for _ in range(self.rng.randint(0, 3 if depth < 3 else 0)):
            ghost, text = self.noise()
            lines.append(text)
            if "mod " in text and self.rng.random() < 0.5 and self.free(f"{d}/{rel or ''}/{ghost}.rs"):
                self.put(f"{d}/{rel or ''}/{ghost}.rs", "")
            name, kind = self.fresh(), self.rng.choice(["file", "mod-rs", "path", "shared", "inline", "inline-path"])
            vis = self.rng.choice(["", "pub ", "pub(crate) ", "#[allow(dead_code)]\n"])
            sub = f"{d}/{rel or ''}"
            if kind == "file" and self.free(f"{sub}/{name}.rs", f"{sub}/{name}/mod.rs"):
                self.put(f"{sub}/{name}.rs", self.body(sub, name, depth + 1))
                lines.append(f"{vis}mod {name};")
            elif kind == "mod-rs" and self.free(f"{sub}/{name}.rs", f"{sub}/{name}/mod.rs"):
                self.put(f"{sub}/{name}/mod.rs", self.body(f"{sub}/{name}", None, depth + 1))
                lines.append(f"{vis}mod {name};")
            elif kind == "path":
                # rustc opens `..` through the file system, so it climbs only out of a directory that exists.
                up = [f"../{os.path.basename(os.path.normpath(d))}/{name}_x.rs"] if d_is_file_dir else []
                attr = self.rng.choice([f"{name}_impl.rs", f"h/{name}.rs", f"h/{name}/mod.rs", *up])
                target = os.path.normpath(f"{d}/{attr}")
                if self.free(target):
                    self.put(target, self.body(os.path.dirname(target), None, depth + 1))
                    self.path_files.append(target)
                    lines.append(f'#[path = "{attr}"]\n{vis}mod {name};')
            elif kind == "shared" and self.path_files and d_is_file_dir:
                attr = os.path.relpath(self.rng.choice(self.path_files), os.path.normpath(d))
                lines.append(f'#[path = "{attr}"]\n{vis}mod {name};')
            elif kind == "inline":
                lines.append(f"{vis}mod {name} {{\n{self.body(f'{sub}/{name}', None, depth + 1, False)}\n}}")
            elif kind == "inline-path":
                attr = f"ip_{name}"
                lines.append(f'#[path = "{attr}"]\n{vis}mod {name} {{\n{self.body(f"{d}/{attr}", None, depth + 1, False)}\n}}')
        return "\n".join(lines) + "\n"

    def draw(self):
        for _ in range(self.rng.randint(1, 3)):
            name = self.fresh("t")
            path = self.rng.choice([f"tests/{name}.rs", f"tests/{name}/main.rs"])
            self.targets.append((name, path if self.rng.random() < 0.5 else None))
            self.put(path, self.body(os.path.dirname(path), None, 0))
        dirs = sorted({os.path.dirname(p) for p in self.files if p.startswith("tests/")})
        for _ in range(self.rng.randint(0, 3)):
            where = self.rng.choice([*dirs, "tests", f"tests/{self.fresh('dir')}"])
            orphan = self.rng.choice([f"{where}/{self.fresh('o')}.rs", f"{where}/{self.fresh('o')}/mod.rs"])
            if self.free(orphan):
                self.put(orphan, "#[test] fn orphan() {}\n")
        for _ in range(self.rng.randint(0, 2)):
            self.put(f"{self.rng.choice(dirs)}/{self.fresh('data')}.{self.rng.choice(['txt', 'org', 'json'])}", "x\n")


def rustc_reached(crate, root, out):
    """The files rustc compiles for the crate root `root`, from its dep-info."""
    dep = out / "deps.d"
    r = subprocess.run(["rustc", "--edition", "2021", "--crate-type", "lib", "--crate-name", "t", "-A", "warnings",
                        "--emit", f"dep-info={dep}", root], cwd=crate, capture_output=True, text=True)
    assert r.returncode == 0, f"rustc rejects the generated crate root {root}:\n{r.stderr}"
    first = dep.read_text().splitlines()[0]
    return {(crate / p).resolve() for p in first.split(":", 1)[1].split()}


def generated(cases):
    rng = random.Random(0)
    for n in range(cases):
        crates = {}
        for k in range(rng.randint(1, 2)):
            layout = Layout(rng)
            layout.draw()
            autotests = rng.random() < 0.2
            crates[f"crates/c{k}"] = (manifest(layout.targets, checked=not autotests), layout, autotests)
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as out:
            root = Path(root).resolve()
            workspace(root, {m: (text, layout.files) for m, (text, layout, _) in crates.items()})
            expected = set()
            for member, (_, layout, autotests) in crates.items():
                crate = root / member
                if autotests:
                    continue
                reached = set()
                for name, path in layout.targets:
                    target = path or next(p for p in (f"tests/{name}.rs", f"tests/{name}/main.rs") if (crate / p).is_file())
                    reached |= rustc_reached(crate, target, Path(out))
                expected |= {str(p.relative_to(root)) for p in (crate / "tests").rglob("*.rs") if p.resolve() not in reached}
            r = run(root)
            got = {line.removeprefix("undeclared-test-files: ").split(PROBLEM_SUFFIX)[0]
                   for line in r.stderr.splitlines()}
            assert got == expected and (r.returncode == 0) == (not expected), \
                f"case {n}: check reports {sorted(got)}, rustc leaves {sorted(expected)} uncompiled; stderr:\n" \
                f"{r.stderr}\nfiles: {sorted(f for _, l, _ in crates.values() for f in l.files)}"
    print(f"[undeclared-test-files] generated: {cases} cases PASS")


if __name__ == "__main__":
    fixtures()
    generated(int(sys.argv[1]) if len(sys.argv) > 1 else 200)
