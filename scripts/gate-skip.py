#!/usr/bin/env python3
"""Print the landing-gate steps a combined diff cannot reach.

    gate-skip.py --base <rev> --tip <rev>     paths from `jj diff --name-only -r '<base>..<tip>'`
    gate-skip.py --paths-from <file>          one path per line

stdout is one line: `skip: 3 4 5 6 11 12 13 14 15 16 18` or `full`. stderr names,
for a `full`, each path that forces it. The crate graph is the cwd's workspace.

The steps are skipped only when every changed path is
  - not on TRIGGERS,
  - not lib src (any path outside `tests/`) of a crate in the
    holon-integration-tests dependency closure,
  - in an allow class: a doc under `docs/`, or one test binary of a member
    that no step-selecting source (justfile, scripts, the timing profile)
    names by binary or by package.
"""
import argparse
import fnmatch
import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath as P

SKIP = "skip: 3 4 5 6 11 12 13 14 15 16 18"
# The justfile recipes the skipped steps run; the scan follows their `just` and `scripts/` calls.
SKIPPED_RECIPES = [
    "check-release-shape", "check-frontend-wasm", "check-dioxus-web-wasm", "check-worker-wasm",
    "keystone-smoke", "loro-suite", "hand-authored", "projector-lag-lock", "windowed-gate",
    "timing-gate", "guests-verify",
]
CLOSURE_ROOT = "holon-integration-tests"

# Files every step reads, and files that tests or step recipes read at run time.
TRIGGERS = [
    "Cargo.lock", "*/Cargo.lock", "Cargo.toml", "*/Cargo.toml",
    "justfile", ".config/*", "rust-toolchain*", ".cargo/*", "workspace-hack/*",
    "docs/Testing/KeystoneKnownReds.md",
    "docs/Testing/TimingCritical.md",
    "docs/Testing/AdmissionFootprintCensus.md",
    "docs/Testing/HomeProfileLookupLatency.txt",
    "docs/Testing/known-red-evidence/*",
    "docs/Architecture/featuremap.yaml",
    "docs/Architecture/FeatureMap.md",
    "docs/Architecture/baseline/*",
    "scripts/timing-assertions-baseline.txt",
    "crates/holon-integration-tests/hand-authored-regressions/*",
    "crates/holon-integration-tests/src/pbt/transitions/mod.rs",
    "assets/*",
    "*.proptest-regressions", "*.pbt-regressions",
    "*.wasm",
]


class Workspace:
    def __init__(self, metadata, step_sources, timing_profile):
        self.root = Path(metadata["workspace_root"])
        ids = set(metadata["workspace_members"])
        pkgs = {p["name"]: p for p in metadata["packages"] if p["id"] in ids}
        self.dirs = {n: P(p["manifest_path"]).parent.relative_to(P(metadata["workspace_root"])) for n, p in pkgs.items()}
        self.test_srcs = {
            n: {P(t["src_path"]).relative_to(P(metadata["workspace_root"])) for t in p["targets"] if "test" in t["kind"]}
            for n, p in pkgs.items()
        }
        self.closure = closure(pkgs, CLOSURE_ROOT)
        self.named_words = set(re.findall(r"[\w-]+", step_sources))
        self.named_pkgs = set(re.findall(r"(?:package\(|binary_id\()([\w-]+)", step_sources))
        for line in step_sources.splitlines():
            if re.search(r"nextest run|cargo test", line) and not re.search(r"--(test|example|bin|lib)\b", line):
                self.named_pkgs |= set(re.findall(r"(?:-p|--package)\s+([\w-]+)", line))
        # A bare `test(=module::...)` row selects that module in whichever crate holds it.
        for module in re.findall(r"test\(=(\w+)::", timing_profile):
            for n, d in self.dirs.items():
                if (self.root / d / "tests" / module).exists() or (self.root / d / "tests" / f"{module}.rs").exists():
                    self.named_pkgs.add(n)

    def owner(self, p):
        owners = [n for n, d in self.dirs.items() if d != P(".") and d in p.parents]
        return max(owners, key=lambda n: len(self.dirs[n].parts), default=None)

    def test_binary(self, member, p):
        rel = p.relative_to(self.dirs[member])
        tests = self.dirs[member] / "tests"
        if len(rel.parts) == 2 and p.suffix == ".rs":
            if p in self.test_srcs[member] or not (self.root / p).exists():
                return p.stem
        if len(rel.parts) > 2:
            name = rel.parts[1]
            if {tests / name / "main.rs", tests / f"{name}.rs"} & self.test_srcs[member]:
                return name
        return None


def closure(pkgs, root):
    seen, todo = {root}, [(root, True)]
    while todo:
        name, at_root = todo.pop()
        for d in pkgs[name]["dependencies"]:
            if d["name"] in pkgs and d["name"] not in seen and (at_root or d["kind"] in (None, "build")):
                seen.add(d["name"])
                todo.append((d["name"], False))
    return seen


def blocker(path, ws):
    """Why `path` forces the full gate, or None when it cannot reach a skipped step."""
    p = P(path)
    if any(fnmatch.fnmatchcase(path, t) for t in TRIGGERS):
        return "trigger"
    member = ws.owner(p)
    if member is not None and p.relative_to(ws.dirs[member]).parts[0] != "tests":
        return f"lib src of {member}" + (f", in the {CLOSURE_ROOT} closure" if member in ws.closure else "")
    if p.parts[0] == "docs":
        return None
    if member is None:
        return "unknown path"
    binary = ws.test_binary(member, p)
    if binary is None:
        return f"test path of {member} that is not one test binary"
    if member in ws.named_pkgs:
        return f"tests of {member}, a package a gate step selects"
    if binary in ws.named_words:
        return f"test binary {binary}, named by a gate step"
    return None


def decide(paths, ws):
    blocked = [(path, why) for path in paths if (why := blocker(path, ws)) is not None]
    return ("full" if blocked or not paths else SKIP), blocked


def run(cmd, cwd=None):
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"gate-skip: `{' '.join(cmd)}` failed ({r.returncode}): {r.stderr.strip()}")
    return r.stdout


def load_workspace():
    metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1"]))
    root = Path(metadata["workspace_root"])
    nextest = (root / ".config/nextest.toml").read_text()
    timing = re.search(r"^\[profile\.timing\]\n(.*?)(?=^\[)", nextest, re.S | re.M)
    if timing is None:
        sys.exit("gate-skip: .config/nextest.toml has no [profile.timing] section")
    sources = step_sources(root, (root / "justfile").read_text())
    return Workspace(metadata, sources + "\n" + timing.group(1), timing.group(1))


def recipes(justfile):
    bodies, name = {}, None
    for line in justfile.splitlines():
        head = re.match(r"([a-z][\w-]*)(\s[^:]*)?:(?!=)", line)
        if head:
            name = head.group(1)
            bodies[name] = []
        elif line and not line[0].isspace() and not line.startswith("#"):
            name = None
        elif name is not None:
            bodies[name].append(line)
    return {n: "\n".join(b) for n, b in bodies.items()}


def step_sources(root, justfile):
    """The text of every recipe and repo script the skipped steps run, transitively."""
    bodies = recipes(justfile)
    todo, seen, texts = [("recipe", r) for r in SKIPPED_RECIPES], set(), []
    while todo:
        item = todo.pop()
        if item in seen:
            continue
        seen.add(item)
        kind, name = item
        if kind == "recipe":
            assert name in bodies, f"justfile has no recipe {name!r}"
            text = bodies[name]
        else:
            text = (root / name).read_text()
        texts.append(text)
        todo += [("recipe", r) for r in re.findall(r"\bjust\s+([a-z][\w-]*)", text) if r in bodies]
        todo += [("script", s) for s in re.findall(r"\bscripts/[\w./-]*\.(?:sh|py)\b", text) if (root / s).is_file()]
    return "\n".join(texts)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base")
    ap.add_argument("--tip")
    ap.add_argument("--paths-from")
    a = ap.parse_args()
    if a.paths_from:
        assert a.base is None and a.tip is None, "--paths-from excludes --base/--tip"
        text = Path(a.paths_from).read_text()
    else:
        assert a.base and a.tip, "give --base and --tip, or --paths-from"
        text = run(["jj", "diff", "--name-only", "-r", f"{a.base}..{a.tip}"])
    paths = [line.strip() for line in text.splitlines() if line.strip()]
    verdict, blocked = decide(paths, load_workspace())
    for path, why in blocked:
        print(f"full: {path}: {why}", file=sys.stderr)
    if not paths:
        print("full: empty diff", file=sys.stderr)
    print(verdict)


if __name__ == "__main__":
    main()
