#!/usr/bin/env python3
"""Print the test steps of a lane: crates owning a changed path (or reading a changed asset) plus their workspace reverse deps.

    scripts/select-tests.py --base <commit>     the steps for `jj diff --from <commit> --to @`
    scripts/select-tests.py --self-check        run the golden cases and the output contract

Run from the workspace root. The changed files and the dependency graph (`cargo metadata --no-deps`)
both come from that one working copy.

stdout: first the `BASE:` and `TREE:` lines (commit ids of the fork point of `@` and `--base`, and of `@`), so a tee
  of the output is a log tied to a tree. Then one `<step>: <args>` line per step, each step name at most once, and
  printed only after the whole selection is computed: a failure prints no step line and exits non-zero.
  `clippy:` and `nextest:` hold the arguments of that command. Both carry the canonical compile shape
  `--workspace --features <the justfile's CANON_FEATURES>`; the nextest line adds `--no-tests=fail`, so a run that
  matches no test fails under any nextest default, and `-E '<filterset>'` naming the crates to run. There is no `nextest:` line when no crate owns a changed path, and no `-E` when a root build
  file changed (every crate runs).
  A crate that a `package()` term of the profile's default filter excludes is left out of `-E` and logged on stderr
  as `<crate>: no test of this profile`; its owner line stays. When every crate is left out there is no `nextest:` line.
  `holon-integration-tests` reached as a reverse dep enters the filterset by its test binaries, never as a whole
  crate. A binary drops its env-bound top-level modules and those the default profile excludes, by test name;
  a binary whose root file is env-bound or that the default profile excludes is left out.
  The nextest line carries `--profile load-tolerant` when the repo's nextest config defines that profile, and one
  further line per recipe that owns an excluded test the selection reaches: `windowed: just windowed-gate`, read
  from the owner table in scripts/timing-split.py. Excluded tests the table does not own share one `unowned:` line.
  A module the default profile drops from a reached binary counts as reached, unless NEVER_OWNER_MODULES names it.
  A recipe whose prefix in that file's `PATH_RECIPES` table a changed path starts with gets a line of the same form
  (`keystone-smoke: just keystone-smoke`), also when there is no `nextest:` line.
stderr: why each crate is in the list.
A crate whose changed paths all sit under its own `tests/` is listed alone, without its reverse deps.
A changed path under `assets/` adds every crate whose `.rs` sources hold that path or one of its parent directories
as a complete string literal.
A diff whose every path is registry data (REGISTRY_DATA) and that holds the known-reds registry prints only the
registry's own checks: clippy, a nextest of the registry reader's lib tests and `known-reds-fixture`. Its readers'
reverse deps and their owner lines stay out, since a row edit changes no code they build.
A changed path under a Cargo.toml that no workspace member owns (excluded crate) is an error, unless a
`PATH_RECIPES` prefix covers it: that row's owner line then states its check.
"""
import argparse
import ast
import contextlib
import functools
import io
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath

ALL_TRIGGERS = ("Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml")
INTEGRATION = "holon-integration-tests"
ENV_BOUND_VARS = ("HOLON_WEB_URL",)
LOAD_TOLERANT = "load-tolerant"
OWNER_TABLE_FILE = "scripts/timing-split.py"
OWNER_TABLE = "OWNER_RECIPES"
PATH_TABLE = "PATH_RECIPES"
STEP_LINE = re.compile(r"([a-z][a-z0-9-]*): \S.*")
# Disclosure lines: a lane runs no command for them (`timing:` belongs to the landing gate, `unowned:` names no recipe).
NOT_RUN_STEPS = ("timing", "unowned")
# shortcut: a `#[path]` module is not followed; module_files then fails loudly on the missing file.
MOD_DECL = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.M)
# Integration modules whose owner recipe only the landing gate runs: a lane gets no owner line for them.
NEVER_OWNER_MODULES = frozenset({"latency_slo_gate"})
KNOWN_REDS_REGISTRY = "docs/Testing/KeystoneKnownReds.md"
# A path equal to or under an entry is registry data.
REGISTRY_DATA = frozenset({KNOWN_REDS_REGISTRY, "docs/Testing/known-red-evidence", "docs/Architecture/FeatureMap.md"})
REGISTRY_TESTS = f"package({INTEGRATION}) & kind(lib) & test(/^pbt::known_reds::/)"
REGISTRY_FIXTURE_LINE = "known-reds-fixture: just known-reds-fixture"


def workspace_graph(meta):
    root = PurePosixPath(meta["workspace_root"])
    members = set(meta["workspace_members"])
    pkgs = [p for p in meta["packages"] if p["id"] in members]
    dirs = {p["name"]: PurePosixPath(p["manifest_path"]).parent.relative_to(root) for p in pkgs}
    names = set(dirs)
    rdeps = {n: set() for n in names}
    for p in pkgs:
        for d in p["dependencies"]:
            if d["name"] in names:
                rdeps[d["name"]].add(p["name"])
    return dirs, rdeps


def owner(path, dirs):
    best = None
    for name, d in dirs.items():
        if d != PurePosixPath(".") and (path == d or d in path.parents):
            if best is None or len(d.parts) > len(dirs[best].parts):
                best = name
    return best


def is_all_trigger(path):
    return path.parts[0] == ".cargo" or (len(path.parts) == 1 and path.name in ALL_TRIGGERS)


def compute(meta, changed, has_manifest=lambda rel_dir: False, asset_readers=lambda path: (), test_binaries=lambda: {},
            path_owners={}):
    """Returns (None, reasons, {}) for all crates, else (sorted package names, {name: reason}, integration binaries).

    The integration binaries are `test_binaries()`, {binary: its dropped top-level modules}, when the integration
    crate is reached as a reverse dep, else empty.
    """
    dirs, rdeps = workspace_graph(meta)
    paths = []
    for c in changed:
        p = PurePosixPath(c)
        if not c or p.is_absolute() or ".." in p.parts:
            raise ValueError(f"changed path must be repo-relative, got {c!r}")
        paths.append(p)
    for p in paths:
        if is_all_trigger(p):
            return None, {"*": f"{p} changed: all crates"}, {}
    reasons = {}
    outside_tests = set()
    for p in paths:
        o = owner(p, dirs)
        if o:
            reasons.setdefault(o, f"owns changed path {p}")
            if dirs[o] / "tests" not in p.parents:
                outside_tests.add(o)
            continue
        readers = asset_readers(p) if len(p.parts) > 1 else ()
        for r in sorted(readers):
            reasons.setdefault(r, f"reads changed file {p}")
            outside_tests.add(r)
        if readers or p.parts[0] == "assets" or covering_recipes(p, path_owners):
            continue
        for d in p.parents:
            if d != PurePosixPath(".") and has_manifest(d):
                raise ValueError(f"{p} is under {d}/Cargo.toml, a crate outside the workspace: its tests are not covered"
                                 f" by this list; a {PATH_TABLE} row in {OWNER_TABLE_FILE} states the check for it")
    for name in list(reasons):
        if name not in outside_tests:
            reasons[name] += " (only tests/ changed: no reverse deps)"
    expanded = set(outside_tests)
    frontier = list(expanded)
    while frontier:
        cur = frontier.pop()
        for r in rdeps[cur]:
            reasons.setdefault(r, f"depends on {cur}")
            if r not in expanded:
                expanded.add(r)
                frontier.append(r)
    bins = {}
    if reasons.get(INTEGRATION, "").startswith("depends on"):
        del reasons[INTEGRATION]
        bins = test_binaries()
    return sorted(reasons), reasons, bins


RS_STRING = re.compile(r'"((?:[^"\\\n]|\\.)*)"')
# A path built from `.join("a").join("b")` segments reads as the literal "a/b".
RS_JOIN_CHAIN = re.compile(r'(?:\.join\(\s*"[^"\\\n]*"\s*\)\s*){2,}')
RS_JOIN_ARG = re.compile(r'\.join\(\s*"([^"\\\n]*)"')
_LITERAL_TAILS = {}


def literal_tails(root, dirs):
    """{every `/`-suffix of a string literal in a crate's `.rs` sources: the crates holding one}, read once per root."""
    key = (Path(root).resolve(), tuple(sorted((n, str(d)) for n, d in dirs.items())))
    if key not in _LITERAL_TAILS:
        tails = {}
        for name, d in dirs.items():
            for src in (root / d).rglob("*.rs"):
                text = src.read_text()
                joined = ["/".join(RS_JOIN_ARG.findall(m)) for m in RS_JOIN_CHAIN.findall(text)]
                for lit in RS_STRING.findall(text) + joined:
                    parts = lit.rstrip("/").split("/")
                    for i in range(len(parts) - 1):
                        tails.setdefault("/".join(parts[i:]), set()).add(name)
        _LITERAL_TAILS[key] = tails
    return _LITERAL_TAILS[key]


def asset_readers(root, dirs, path):
    """The crates whose `.rs` sources end a string literal in `path` or in one of its parent directories."""
    tails = literal_tails(root, dirs)
    return set().union(*(tails.get("/".join(path.parts[:i]), ()) for i in range(2, len(path.parts) + 1)))


def nextest_config(root):
    """The one read of the repo's nextest config."""
    return (root / ".config" / "nextest.toml").read_text()


def profile_default_filter(cfg, profile):
    """One profile's `default-filter`, or None when the config defines no such profile."""
    section = re.search(rf"^\[profile\.{re.escape(profile)}\]$(.*?)^\[", cfg + "\n[", re.M | re.S)
    if not section:
        return None
    return re.search(r'^default-filter = "(.*)"$', section.group(1), re.M).group(1)


@functools.cache
def owner_tables(root):
    """{name: table} of the owner file.

    Each `<name> = {...}` (annotated or not) of that module is read as a literal and `**<name>` merges an
    earlier one; nothing is executed. A tree without that file is an error, not an empty table.
    """
    tables = {}
    for node in ast.parse((root / OWNER_TABLE_FILE).read_text()).body:
        if not (isinstance(node, (ast.Assign, ast.AnnAssign)) and isinstance(node.value, ast.Dict)):
            continue
        target = node.targets[0] if isinstance(node, ast.Assign) else node.target
        if not isinstance(target, ast.Name):
            continue
        merged = {}
        for key, value in zip(node.value.keys, node.value.values):
            if key is None:
                merged.update(tables[value.id])
            else:
                merged[ast.literal_eval(key)] = ast.literal_eval(value)
        tables[target.id] = merged
    return tables


def owner_recipes(root):
    """Selector -> the recipe that runs it.

    Empty when the table is not there: every excluded test the selection reaches is then
    disclosed as unowned, which is what `timing-split.py check` reds on.
    """
    return owner_tables(root).get(OWNER_TABLE, {})


def path_recipes(root):
    """Changed-path prefix -> the recipe name that checks that code; empty when the table is not there."""
    return owner_tables(root).get(PATH_TABLE, {})


def recipe_line(recipe):
    return f"{recipe.split()[-1].removesuffix('-gate')}: {recipe}"


def covering_recipes(path, path_owners):
    """The recipes whose prefix the changed `path` starts with."""
    return [recipe for prefix, recipe in path_owners.items() if str(path).startswith(prefix)]


def path_owner_lines(changed, path_owners):
    """One line per recipe whose prefix a changed path starts with."""
    return sorted({recipe_line(f"just {recipe}") for c in changed
                   for recipe in covering_recipes(PurePosixPath(c), path_owners)})


def test_binary_packages(meta):
    """{test target name: the workspace package that holds it}."""
    members = set(meta["workspace_members"])
    return {t["name"]: p["name"] for p in meta["packages"] if p["id"] in members
            for t in p.get("targets", ()) if "test" in t["kind"]}


def _unwrap(expr):
    """`(package(A) & test(=b))` -> `package(A) & test(=b)`; a term that is not one group is unchanged."""
    while expr.startswith("("):
        depth = 0
        for i, ch in enumerate(expr):
            depth += (ch == "(") - (ch == ")")
            if depth == 0:
                break
        if i != len(expr) - 1:
            break
        expr = expr[1:-1].strip()
    return expr


def excluded_terms(filt):
    """Every term a `default-filter` excludes: the terms of its `not (...)` groups."""
    terms = []
    for m in re.finditer(r"\bnot\s*\(", filt):
        depth, group = 0, ""
        for ch in filt[m.end() - 1:]:
            depth += (ch == "(") - (ch == ")")
            group += ch
            if depth == 0:
                break
        cur, depth = "", 0
        for ch in group[1:-1] + "|":
            if ch in "+|" and depth == 0:
                terms.append(_unwrap(cur.strip()))
                cur = ""
                continue
            depth += (ch == "(") - (ch == ")")
            cur += ch
    return terms


def split_top(expr, ops):
    """`expr` cut at each character of `ops` outside parentheses, each part stripped."""
    parts, cur, depth = [], "", 0
    for ch in expr:
        if ch in ops and depth == 0:
            parts.append(cur.strip())
            cur = ""
            continue
        depth += (ch == "(") - (ch == ")")
        cur += ch
    return parts + [cur.strip()]


def excluded_packages(filt):
    """The packages a `default-filter` excludes by proof: the filter is `not (...) & not (...) ...` and a term of the
    sum in one of those groups is exactly `package(<name>)`. Any other shape proves nothing: sound, never complete."""
    found = set()
    for conjunct in split_top(filt, "&"):
        m = re.fullmatch(r"not\s*(\(.*\))", conjunct, re.S)
        if not m or _unwrap(m.group(1)) == m.group(1):
            return frozenset()
        for term in split_top(_unwrap(m.group(1)), "|+"):
            name = re.fullmatch(r"package\(=?([\w-]+)\)", _unwrap(term))
            if name:
                found.add(name.group(1))
    return frozenset(found)


def term_reaches_selection(term, pkgs, bins, binary_pkgs, never_owner):
    """Whether the printed selection runs anything the excluded `term` names.

    A top-level module of a reached integration binary counts as reached also when the default profile drops it,
    since its owner recipe is then the only run of it; a module in `never_owner` never does.

    shortcut: a term naming only test names resolves to no package and counts as out of reach;
    upgrade when the owner table carries each row's package.
    """
    module = re.search(r"\btest\(/\^(\w+)::/\)", term)
    if module and module.group(1) in never_owner and f"binary_id({INTEGRATION}::" in term:
        return False
    if pkgs is None:
        return True
    hits = []
    for kind, arg in re.findall(r"\b(package|binary_id|binary)\(([^)]*)\)", term):
        if kind == "package":
            pkg, binary = arg, None
        elif kind == "binary":
            pkg, binary = binary_pkgs.get(arg), arg
        else:
            pkg, _, binary = arg.partition("::")
        if pkg is None:
            continue
        if pkg == INTEGRATION and INTEGRATION not in pkgs:
            hits.append(binary in bins)
        else:
            hits.append(pkg in pkgs)
    return bool(hits) and all(hits)


def owner_lines(filt, pkgs, bins, binary_pkgs, owners, never_owner):
    """One line per recipe that owns an excluded term the selection reaches, and one `unowned:` line for the rest."""
    recipes, unowned = set(), []
    for term in excluded_terms(filt):
        if not term_reaches_selection(term, pkgs, bins, binary_pkgs, never_owner):
            continue
        if term in owners:
            recipes.add(recipe_line(owners[term]))
        else:
            unowned.append(f"{term} is excluded by the {LOAD_TOLERANT} profile and has no recipe in {OWNER_TABLE_FILE}")
    return sorted(recipes) + ([f"unowned: {'; '.join(sorted(unowned))}"] if unowned else [])


def test_targets(meta, package):
    """[(name, root file)] of one workspace package's test targets, as cargo resolved them."""
    pkg, = (p for p in meta["packages"] if p["name"] == package and p["id"] in meta["workspace_members"])
    return [(t["name"], Path(t["src_path"])) for t in pkg.get("targets", ()) if "test" in t["kind"]]


def module_files(src, module):
    """The files of the top-level `module` that the test binary rooted at `src` declares."""
    own = src.parent / f"{module}.rs"
    sub = sorted((src.parent / module).rglob("*.rs"))
    if not own.exists() and src.parent / module / "mod.rs" not in sub:
        raise ValueError(f"{src} declares `mod {module};` but neither {own} nor {src.parent / module}/mod.rs exists")
    return ([own] if own.exists() else []) + sub


def env_bound(files):
    return any(v in f.read_text() for f in files for v in ENV_BOUND_VARS)


def module_term(binary, module):
    """How a nextest filter of this repo names one top-level module of an integration test binary."""
    return f"binary_id({INTEGRATION}::{binary}) & test(/^{module}::/)"


def integration_units(meta, cfg):
    """{test binary of the integration crate: its dropped top-level modules}.

    A module is dropped when it is env-bound or the default profile excludes it. A binary is left out when its
    root file is env-bound or the default profile excludes it.
    """
    default = set(excluded_terms(profile_default_filter(cfg, "default") or ""))
    units = {}
    for name, src in test_targets(meta, INTEGRATION):
        if f"binary({name})" in default or env_bound([src]):
            continue
        units[name] = sorted(m for m in MOD_DECL.findall(src.read_text())
                             if module_term(name, m) in default or env_bound(module_files(src, m)))
    return units


def canon_features(root):
    """The feature list every host-profile gate shares, read from the workspace justfile."""
    m = re.search(r'^CANON_FEATURES\s*:=\s*"([^"]*)"', (root / "justfile").read_text(), re.M)
    if not m:
        raise ValueError(f"{root}/justfile holds no CANON_FEATURES assignment: the canonical feature list moved")
    return m.group(1).split(",")


def filterset(pkgs, bins):
    """The nextest `-E` expression selecting `pkgs`, with `holon-integration-tests` cut to `bins`.

    Empty for all crates (nothing to narrow) and for no crate (there is no run).
    """
    if pkgs is None:
        return ""
    terms = [f"package({n})" for n in pkgs]
    whole = sorted(b for b, dropped in bins.items() if not dropped)
    if whole:
        terms.append(f"(package({INTEGRATION}) & ({' | '.join(f'binary({b})' for b in whole)}))")
    for b, dropped in sorted(bins.items()):
        if dropped:
            tests = " | ".join(f"test(/^{m}::/)" for m in dropped)
            terms.append(f"(package({INTEGRATION}) & binary({b}) & not ({tests}))")
    return " | ".join(terms)


def canon_args(features):
    return "--workspace --features " + ",".join(features)


def run_args(canon, profile):
    return canon + " --no-tests=fail" + (f" --profile {profile}" if profile else "")


def registry_data_only(changed, registry_data):
    data = [PurePosixPath(d) for d in registry_data]
    return KNOWN_REDS_REGISTRY in changed and all(
        any(p == d or d in p.parents for d in data) for p in map(PurePosixPath, changed))


def arg_lines(pkgs, bins, features, profile=None, owners=(), path_owners=()):
    """The canonical compile shape for clippy, and the same shape plus the run filter for nextest.

    Both halves of cargo's feature hash stay fixed (the justfile's CANON block): a `-p` selection
    would narrow the feature resolve and mint a build directory per lane, so what RUNS is narrowed by `-E`.
    Without a selected crate there is no owner line; one whose crates the profile excludes keeps its owner lines.
    """
    canon = canon_args(features)
    lines = [f"clippy: {canon}"]
    run = run_args(canon, profile)
    sel = filterset(pkgs, bins)
    if sel:
        lines.append(f"nextest: {run} -E '{sel}'")
    elif pkgs is None:
        lines.append(f"nextest: {run}")
    return lines + sorted(set(owners) | set(path_owners))


def checked(lines):
    """`lines` once each is a `<step>: <args>` line and no step name repeats; the caller runs each step once."""
    names = []
    for line in lines:
        m = STEP_LINE.fullmatch(line)
        if not m:
            raise ValueError(f"not a `<step>: <args>` line: {line!r}")
        names.append(m.group(1))
    repeated = sorted({n for n in names if names.count(n) > 1})
    if repeated:
        raise ValueError(f"step names printed more than once: {repeated} in {lines}")
    return lines


def selection(root, meta, changed, never_owner=NEVER_OWNER_MODULES, registry_data=REGISTRY_DATA):
    """The step lines for the `changed` paths of the workspace at `root`; an empty `registry_data` turns the
    registry-data rule off."""
    dirs, _ = workspace_graph(meta)
    cfg = nextest_config(root)
    path_owners = path_recipes(root)
    pkgs, reasons, bins = compute(
        meta, changed, lambda d: (root / d / "Cargo.toml").exists(),
        lambda path: asset_readers(root, dirs, path),
        lambda: integration_units(meta, cfg), path_owners)
    excluding = profile_default_filter(cfg, LOAD_TOLERANT)
    profile = LOAD_TOLERANT if excluding else None
    paths = path_owner_lines(changed, path_owners)
    if registry_data_only(changed, registry_data):
        print(f"registry data only: {len(changed)} paths; the registry's own checks", file=sys.stderr)
        canon = canon_args(canon_features(root))
        return checked([f"clippy: {canon}", f"nextest: {run_args(canon, profile)} -E '{REGISTRY_TESTS}'",
                        *sorted({REGISTRY_FIXTURE_LINE, *paths})])
    if pkgs is None:
        print(f"ALL CRATES: {reasons['*']}", file=sys.stderr)
    else:
        if not pkgs and not bins:
            print("no workspace crate owns a changed path: no crate tests", file=sys.stderr)
        for n in pkgs:
            print(f"  {n}: {reasons[n]}", file=sys.stderr)
        if bins:
            print(f"  {INTEGRATION}: reverse dep, named by binary()", file=sys.stderr)
    owners = (owner_lines(excluding, pkgs, bins, test_binary_packages(meta), owner_recipes(root), never_owner)
              if excluding else [])
    if pkgs is not None and excluding:
        excluded = excluded_packages(excluding)
        for n in sorted(excluded & {*pkgs, *([INTEGRATION] if bins else [])}):
            print(f"{n}: no test of this profile", file=sys.stderr)
        pkgs = [n for n in pkgs if n not in excluded]
        bins = {} if INTEGRATION in excluded else bins
    return checked(arg_lines(pkgs, bins, canon_features(root), profile, owners, paths))


# ── landing gate ─────────────────────────────────────────────────────

GATE_SCRIPTS = {"integrate": "scripts/gate-integrate.sh", "timing": "scripts/gate-timing.sh"}
# The code that selects and runs the gate steps: flow runs `select` from main, so a batch changing it runs every step.
GATE_MACHINERY = frozenset({"scripts/select-tests.py", OWNER_TABLE_FILE, "scripts/gate-integrate.sh",
                            "scripts/gate-integrate-steps.sh", "scripts/gate-timing.sh"})
# Inputs of every compile or test step: a change runs every step.
FULL_FILES = frozenset({*ALL_TRIGGERS, "justfile", "proptest.toml"})
FULL_DIRS = frozenset({".cargo", ".config", "workspace-hack"})
# Roots no compile or test step reads except through a crate's string literal or a step's own sources.
INERT_ROOTS = frozenset({"docs", "devlog", "wiki", "scripts", "assets", "archlint", ".archidoc", ".beads", ".claude",
                         ".devcontainer", ".gc", ".github", ".qlty", ".resolve-scratch", ".runtime", "dist",
                         "lane-logs", ".lane-logs", "observability", "report"})
# Steps that read the whole tree (docs, scripts, every manifest) and run on every batch.
TREE_STEPS = ("fmt", "deny", "machete", "gate-arch", "arch-validate", "featuremap", "analyze-arch", "guests-verify")
INTEGRATION_STEPS = ("keystone-smoke", "hand-authored", "loro-suite", "projector-lag-lock", "keystone-tour")
# Steps that build a crate outside the workspace: they run when a workspace crate it names by `path` is reached.
NON_MEMBER_STEPS = {"check-dioxus-web-wasm": "frontends/dioxus-web", "check-worker-wasm": "frontends/holon-worker"}
TIMING_RECIPES = frozenset({"just timing-gate", "just latency-slo-gate", "just org-deep-text-timing"})
TIMING_STEPS = ("timing-gate-build", "timing-gate")
# Gate steps run as an arm of this script instead of a justfile recipe.
STEP_SCRIPT = "scripts/gate-integrate-steps.sh"
SCRIPT_STEPS = ("keystone-smoke", "hand-authored", "extra-nextest")


@functools.cache
def gate_step_names(root, gate):
    """The step names the gate script declares, from the script itself."""
    out = subprocess.run(["bash", str(root / GATE_SCRIPTS[gate]), "--steps"], cwd=root,
                         stdout=subprocess.PIPE, text=True, check=True)
    names = out.stdout.split()
    if not names:
        raise ValueError(f"{GATE_SCRIPTS[gate]} --steps printed no step name")
    return names


def justfile_recipes(justfile):
    """{recipe name: its body text} of a justfile."""
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


@functools.cache
def step_sources(root, step):
    """The text of the recipe or script arm a gate step runs and of every recipe and repo script it calls."""
    bodies = justfile_recipes((root / "justfile").read_text())
    todo = [("script", STEP_SCRIPT)] if step in SCRIPT_STEPS else [("recipe", step)] if step in bodies else []
    seen, texts = set(), []
    while todo:
        item = todo.pop()
        if item in seen:
            continue
        seen.add(item)
        kind, name = item
        text = bodies[name] if kind == "recipe" else (root / name).read_text()
        if name == OWNER_TABLE_FILE:
            # Its path table routes paths to recipes; the selection applies that table itself.
            text = re.sub(r"(?ms)^PATH_RECIPES = \{.*?^\}", "", text)
        texts.append(text)
        todo += [("recipe", r) for r in re.findall(r"\bjust\s+([a-z][\w-]*)", text) if r in bodies]
        # A change to a gate script runs every step, so its text names no path of one step; the timing gate reads the owner table.
        todo += [("script", s) for s in re.findall(r"\bscripts/[\w./-]*\.(?:sh|py)\b", text)
                 if (root / s).is_file() and (s not in GATE_MACHINERY or s == OWNER_TABLE_FILE)]
    return "\n".join(texts)


@functools.cache
def step_paths(root, step):
    """Every whole path-like word of a step's sources, without a trailing slash."""
    return frozenset(w.rstrip("/") for w in re.findall(r"[\w./-]+", step_sources(root, step)) if "/" in w)


def names_path(named, path):
    """Whether `named` holds `path` or one of its parent directories (two parts or more)."""
    return any("/".join(path.parts[:i]) in named for i in range(len(path.parts), 1, -1))


def path_deps(root, crate_dir):
    """The directories a crate outside the workspace names by `path = "..."` in its manifest, repo-relative."""
    deps = set()
    for d in re.findall(r'\bpath\s*=\s*"([^"]+)"', (root / crate_dir / "Cargo.toml").read_text()):
        parts = []
        for part in (PurePosixPath(crate_dir) / d).parts:
            if part == "..":
                parts.pop()
            elif part != ".":
                parts.append(part)
        deps.add(PurePosixPath(*parts))
    return deps


def term_packages(term, binary_pkgs, module_pkgs):
    """The workspace packages a nextest filter term can select tests of."""
    pkgs = set()
    for kind, arg in re.findall(r"\b(package|binary_id|binary)\(([^)]*)\)", term):
        pkgs.add(arg if kind == "package" else binary_pkgs[arg] if kind == "binary" else arg.partition("::")[0])
    if not pkgs:
        m = re.search(r"\btest\(=(\w+)::", term)
        pkgs = module_pkgs.get(m.group(1), set()) if m else set()
    if not pkgs:
        raise ValueError(f"{OWNER_TABLE_FILE}: the term {term!r} names no package, test binary or test module of the"
                         f" workspace: its timing step cannot be scoped")
    return pkgs


_TIMING_PACKAGES = {}


def timing_packages(root, meta):
    """The packages whose tests the timing gate runs: every owner-table row whose recipe the timing gate runs."""
    if root in _TIMING_PACKAGES:
        return _TIMING_PACKAGES[root]
    module_pkgs = {}
    for p in meta["packages"]:
        if p["id"] in meta["workspace_members"]:
            for name, src in test_targets(meta, p["name"]):
                for m in MOD_DECL.findall(src.read_text()):
                    module_pkgs.setdefault(m, set()).add(p["name"])
    binary_pkgs = test_binary_packages(meta)
    _TIMING_PACKAGES[root] = set().union(*(term_packages(term, binary_pkgs, module_pkgs)
                                           for term, recipe in owner_recipes(root).items() if recipe in TIMING_RECIPES))
    return _TIMING_PACKAGES[root]


def full_reason(path, dirs, root):
    """Why a changed path runs every gate step, or None."""
    p = PurePosixPath(path)
    if str(p) in GATE_MACHINERY:
        return f"{p} selects or runs the gate steps"
    if is_all_trigger(p) or p.name == "Cargo.toml" or (len(p.parts) == 1 and p.name in FULL_FILES) or p.parts[0] in FULL_DIRS:
        return f"{p} is an input of every compile"
    if owner(p, dirs) or len(p.parts) == 1 or p.parts[0] in INERT_ROOTS:
        return None
    if any(str(p).startswith(d + "/") for d in NON_MEMBER_STEPS.values()) or p.parts[0] == "guests":
        return None
    if any((root / d / "Cargo.toml").exists() for d in p.parents if d != PurePosixPath(".")):
        return None
    return f"{p} is under no crate and no root the gate rules know"


def gate_selection(root, meta, gate, changed):
    """[(step, reason, args)] for one gate's steps, or [("*", reason, "")] for a full run."""
    paths = sorted({c.strip() for c in changed if c.strip()})
    if not paths:
        raise ValueError("no changed path: a batch with no change has nothing to select for")
    dirs, _ = workspace_graph(meta)
    for c in paths:
        why = full_reason(c, dirs, root)
        if why:
            print(f"FULL RUN: {why}", file=sys.stderr)
            return [("*", re.sub(r"\s", "_", c), "")]
    path_owners = path_recipes(root)
    pkgs, reasons, bins = compute(meta, paths, asset_readers=lambda path: asset_readers(root, dirs, path),
                                  test_binaries=lambda: {INTEGRATION: []}, path_owners=path_owners)
    reached = set(pkgs) | ({INTEGRATION} if bins else set())
    for n in sorted(reached):
        print(f"  {n}: {reasons.get(n, 'depends on a reached crate')}", file=sys.stderr)
    names = gate_step_names(root, gate)
    sources = {step: step_paths(root, step) for step in {*names, *TIMING_STEPS}}
    sel = {}

    def add(step, reason, args=""):
        if step in names and step not in sel:
            sel[step] = (reason, args)

    timing_named = False
    for c in paths:
        p = PurePosixPath(c)
        for step in sources if owner(p, dirs) is None else ():
            if names_path(sources[step], p):
                print(f"  {step}: its sources name {p}", file=sys.stderr)
                add(step, "named-path")
                timing_named = timing_named or step in TIMING_STEPS
        for recipe in covering_recipes(p, path_owners):
            add(recipe, "path-recipe")
        for step, d in NON_MEMBER_STEPS.items():
            if c.startswith(d + "/"):
                add(step, "path-recipe")
    for step in TREE_STEPS:
        add(step, "tree-check")
    if reached:
        add("gate-compile", "crate-reached")
        add("check-release-shape", "crate-reached")
    tested = sorted(reached - excluded_packages(profile_default_filter(nextest_config(root), LOAD_TOLERANT)))
    if tested:
        add("extra-nextest", "crate-reached", " | ".join(f"package({n})" for n in tested))
    if "holon-frontend" in reached:
        add("check-frontend-wasm", "crate-reached")
    for step, d in NON_MEMBER_STEPS.items():
        if {dirs[n] for n in reached} & path_deps(root, d):
            add(step, "crate-reached")
    if INTEGRATION in reached:
        for step in INTEGRATION_STEPS:
            add(step, "crate-reached")
    if "holon-gpui" in reached:
        add("windowed-gate", "crate-reached")
    # The integrate gate builds every binary the timing gate runs, so both gates take one timing rule.
    if timing_named or reached & timing_packages(root, meta):
        for step in TIMING_STEPS:
            add(step, "named-path" if timing_named else "timing-closure")
    return [(step, *sel[step]) for step in names if step in sel]


def gate_lines(selected):
    return [" ".join(x for x in line if x) for line in selected]


def load_metadata():
    out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"],
                         stdout=subprocess.PIPE, text=True, check=True)
    return json.loads(out.stdout)


def assert_workspace_root(meta, cwd):
    if Path(meta["workspace_root"]).resolve() != cwd.resolve():
        raise ValueError(f"cwd {cwd} is not the cargo workspace root {meta['workspace_root']}: run from the workspace root")


def jj_changed(base):
    out = subprocess.run(["jj", "diff", "--name-only", "--from", base, "--to", "@"],
                         stdout=subprocess.PIPE, text=True, check=True)
    return out.stdout.splitlines()


def jj_header(base):
    def commit_id(rev):
        return subprocess.run(["jj", "log", "--no-graph", "-r", rev, "-T", "commit_id"],
                              stdout=subprocess.PIPE, text=True, check=True).stdout
    # In a fix round `@-` is the earlier round's commit; the fork point stays the lane base.
    return [f"BASE: {commit_id(f'fork_point(@ | {base})')}", f"TREE: {commit_id('@')}"]


# ── self-check ───────────────────────────────────────────────────────

FIXTURE_PACKAGES = {
    # name: (dependencies, test targets as {name: root file under the crate})
    "core": ((), {}),
    "rows": (("core",), {}),
    "app": (("rows",), {}),
    "gpui": (("app",), {"window": "tests/window.rs"}),
    "leaf": ((), {}),
    "bare": ((), {}),
    "ofmt": ((), {"deep": "tests/deep.rs"}),
    INTEGRATION: (("app",), {"integration": "tests/main.rs"}),
}
FIXTURE_FILES = {
    "crates/core/src/lib.rs": 'const SEED: &str = "assets/default";',
    "crates/ofmt/tests/deep.rs": "#[test] fn t() {}",
    "crates/gpui/tests/window.rs": "#[test] fn w() {}",
    f"crates/{INTEGRATION}/tests/main.rs": "mod latency_slo_gate;\nmod hand;\nmod e2e;\nmod plain;\nmod suite;\nmod web;\n",
    f"crates/{INTEGRATION}/tests/latency_slo_gate.rs": "#[test] fn s() {}",
    f"crates/{INTEGRATION}/tests/hand.rs": "#[test] fn h() {}",
    f"crates/{INTEGRATION}/tests/e2e.rs": "#[test] fn e() {}",
    f"crates/{INTEGRATION}/tests/plain.rs": "#[test] fn p() {}",
    f"crates/{INTEGRATION}/tests/suite/mod.rs": "mod a;",
    f"crates/{INTEGRATION}/tests/suite/a.rs": "#[test] fn a() {}",
    f"crates/{INTEGRATION}/tests/web.rs": 'fn url() -> String { std::env::var("HOLON_WEB_URL").unwrap() }',
    "frontends/worker/Cargo.toml": "",
    "frontends/other/Cargo.toml": "",
    "justfile": f'CANON_FEATURES := "{INTEGRATION}/pbt,gpui/pbt"\n',
}
SLOW, HAND, E2E = (module_term("integration", m) for m in ("latency_slo_gate", "hand", "e2e"))
FIXTURE_NEXTEST = f"""[profile.default]
default-filter = "not (({SLOW}) | ({HAND}))"

[profile.load-tolerant]
default-filter = "not ((binary_id(ofmt::deep) & test(=t)) + (package(rows) & test(=r))) & not ({SLOW} | {HAND} | package(gpui) | {E2E} | package(leaf))"
"""
FIXTURE_OWNERS = f'''OWNER_RECIPES = {{
    "{SLOW}": "just latency-slo-gate",
    "{HAND}": "just hand-authored",
    "package(gpui)": "just windowed-gate",
    "{E2E}": "just timing-gate",
    "binary_id(ofmt::deep) & test(=t)": "just org-deep-text-timing",
    "package(rows) & test(=r)": "just timing-gate",
}}
PATH_RECIPES = {{
    "crates/{INTEGRATION}/src/pbt/": "keystone-smoke",
    "crates/{INTEGRATION}/tests/hand.rs": "keystone-smoke",
    "frontends/worker/": "check-worker-wasm",
}}
'''


def fixture_workspace(root):
    """Write a small workspace with a one-binary integration crate under `root`; returns its `cargo metadata`."""
    files = {**FIXTURE_FILES, ".config/nextest.toml": FIXTURE_NEXTEST, OWNER_TABLE_FILE: FIXTURE_OWNERS}
    packages = []
    for name, (deps, tests) in FIXTURE_PACKAGES.items():
        files.setdefault(f"crates/{name}/Cargo.toml", "")
        packages.append({"name": name, "id": name, "manifest_path": str(root / f"crates/{name}/Cargo.toml"),
                         "dependencies": [{"name": d, "kind": None} for d in deps],
                         "targets": [{"name": t, "kind": ["test"], "src_path": str(root / f"crates/{name}/{src}")}
                                     for t, src in tests.items()]})
    for rel, text in files.items():
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(text)
    return {"workspace_root": str(root), "workspace_members": list(FIXTURE_PACKAGES), "packages": packages}


CANON = f"--workspace --features {INTEGRATION}/pbt,gpui/pbt"
RUN = f"{CANON} --no-tests=fail --profile load-tolerant"
OLD_BINARIES = f"(package({INTEGRATION}) & (binary(e2e) | binary(plain) | binary(suite)))"
ONE_BINARY = f"(package({INTEGRATION}) & binary(integration) & not (test(/^hand::/) | test(/^latency_slo_gate::/) | test(/^web::/)))"
UNOWNED_LEAF = f"unowned: package(leaf) is excluded by the load-tolerant profile and has no recipe in {OWNER_TABLE_FILE}"
RDEP_OF_CORE = f"nextest: {RUN} -E 'package(app) | package(core) | package(rows) | {OLD_BINARIES}'"
# Each case: the changed paths, the lines the per-file-binary selector printed for them on the same workspace laid
# out as one test binary per tests/*.rs file (`ValueError` when it failed) plus the owner lines of the modules the
# default profile drops (none for a module in NEVER_OWNER_MODULES), less the crates the profile excludes by
# `package()`, and the integration term this layout prints.
GOLDEN = [
    (["crates/core/src/lib.rs"],
     [f"clippy: {CANON}", RDEP_OF_CORE, "hand-authored: just hand-authored", "timing: just timing-gate",
      "windowed: just windowed-gate"], ONE_BINARY),
    (["assets/default/seed.yaml"],
     [f"clippy: {CANON}", RDEP_OF_CORE, "hand-authored: just hand-authored", "timing: just timing-gate",
      "windowed: just windowed-gate"], ONE_BINARY),
    (["crates/leaf/src/lib.rs"], [f"clippy: {CANON}", UNOWNED_LEAF], None),
    (["crates/gpui/tests/window.rs"], [f"clippy: {CANON}", "windowed: just windowed-gate"], None),
    (["crates/bare/src/lib.rs"], [f"clippy: {CANON}", f"nextest: {RUN} -E 'package(bare)'"], None),
    (["crates/rows/tests/r.rs"],
     [f"clippy: {CANON}", f"nextest: {RUN} -E 'package(rows)'", "timing: just timing-gate"], None),
    (["crates/ofmt/src/lib.rs"],
     [f"clippy: {CANON}", f"nextest: {RUN} -E 'package(ofmt)'", "org-deep-text-timing: just org-deep-text-timing"],
     None),
    ([f"crates/{INTEGRATION}/tests/latency_slo_gate.rs"],
     [f"clippy: {CANON}", f"nextest: {RUN} -E 'package({INTEGRATION})'", "hand-authored: just hand-authored",
      "timing: just timing-gate"], None),
    ([f"crates/{INTEGRATION}/tests/hand.rs"],
     [f"clippy: {CANON}", f"nextest: {RUN} -E 'package({INTEGRATION})'", "hand-authored: just hand-authored",
      "keystone-smoke: just keystone-smoke", "timing: just timing-gate"], None),
    ([f"crates/{INTEGRATION}/src/pbt/step.rs"],
     [f"clippy: {CANON}", f"nextest: {RUN} -E 'package({INTEGRATION})'", "hand-authored: just hand-authored",
      "keystone-smoke: just keystone-smoke", "timing: just timing-gate"], None),
    (["Cargo.lock"],
     [f"clippy: {CANON}", f"nextest: {RUN}", "hand-authored: just hand-authored",
      "org-deep-text-timing: just org-deep-text-timing", "timing: just timing-gate", UNOWNED_LEAF,
      "windowed: just windowed-gate"], None),
    (["docs/Testing/Testing.md"], [f"clippy: {CANON}"], None),
    (["frontends/worker/src/lib.rs"], [f"clippy: {CANON}", "check-worker-wasm: just check-worker-wasm"], None),
    (["frontends/other/src/lib.rs"], ValueError, None),
]


def without_integration_term(lines):
    return [re.sub(rf" \| \(package\({INTEGRATION}\) & .*(?='$)", "", line) for line in lines]


def golden_problems(root, meta):
    found = []
    for changed, want, term in GOLDEN:
        try:
            with contextlib.redirect_stderr(io.StringIO()):
                got = selection(root, meta, changed)
        except ValueError as e:
            if want is not ValueError:
                found.append(f"{changed}: failed: {e}")
            continue
        if want is ValueError:
            found.append(f"{changed}: printed {got}, the per-file selector failed")
        elif without_integration_term(got) != without_integration_term(want):
            found.append(f"{changed}: printed {got}, want {want} apart from the integration term")
        elif term and not any(line.endswith(f" | {term}'") for line in got):
            found.append(f"{changed}: the nextest line does not end in the integration term {term}: {got}")
    return found


def never_owner_problems(root, meta):
    """A dropped module put into the never-owner set loses its owner line, also when the integration crate changes."""
    found = []
    for changed in (["crates/core/src/lib.rs"], [f"crates/{INTEGRATION}/tests/plain.rs"]):
        with contextlib.redirect_stderr(io.StringIO()):
            lines = selection(root, meta, changed, NEVER_OWNER_MODULES | {"hand"})
        if "hand-authored: just hand-authored" in lines:
            found.append(f"{changed}: `hand` is in the never-owner set but its owner line is printed: {lines}")
    return found


def removed_binary_problems(root):
    """Names the selection gives the integration crate that are not its cargo test targets, for a change its
    reverse-dep walk reaches: each such name is a per-file binary nextest refuses."""
    meta = load_metadata()
    targets = {name for name, _ in test_targets(meta, INTEGRATION)}
    dirs, rdeps = workspace_graph(meta)
    reached = next(c for c in sorted(rdeps) if INTEGRATION in rdeps[c])
    with contextlib.redirect_stderr(io.StringIO()):
        lines = selection(root, meta, [str(dirs[reached] / "src/lib.rs")])
    nextest, = (line for line in lines if line.startswith("nextest: "))
    named = set(re.findall(r"\bbinary\((\w+)\)", nextest))
    if not named:
        return [f"a change in {reached} names no integration test binary: {nextest}"]
    return [f"a change in {reached} names {sorted(named - targets)}, not test targets of {INTEGRATION}"
            f" ({sorted(targets)})"] if named - targets else []


def dropped_module_owner_problems(root):
    """A change that reaches the integration binary through the real dependency graph prints the owner line of a
    module the default profile drops; a change that reaches no crate prints none."""
    meta = load_metadata()
    found = []
    for changed, want in ((["crates/holon-loro/src/lib.rs"], True), (["crates/holon-orgmode/src/lib.rs"], True),
                          (["docs/Testing/Testing.md"], False)):
        with contextlib.redirect_stderr(io.StringIO()):
            lines = selection(root, meta, changed)
        if ("hand-authored: just hand-authored" in lines) != want:
            found.append(f"{changed}: the `hand-authored:` owner line is {'missing' if want else 'printed'}: {lines}")
    return found


def keystone_tour_problems(root):
    """A change to the code the keystone tour settles through prints its owner line; a change to other keystone code
    or outside it does not."""
    meta = load_metadata()
    pbt = f"crates/{INTEGRATION}/src/pbt"
    found = []
    for changed, want in (([f"{pbt}/convergence.rs"], True), ([f"{pbt}/composed/wide_e2e.rs"], True),
                          ([f"crates/{INTEGRATION}/tests/keystone_tour.rs"], True),
                          ([f"crates/{INTEGRATION}/hand-authored-regressions/keystone-tour.ndjson"], True),
                          ([f"{pbt}/generators.rs"], False), (["crates/holon-loro/src/lib.rs"], False),
                          (["docs/Testing/Testing.md"], False)):
        with contextlib.redirect_stderr(io.StringIO()):
            lines = selection(root, meta, changed)
        if ("keystone-tour: just keystone-tour" in lines) != want:
            found.append(f"{changed}: the `keystone-tour:` line is {'missing' if want else 'printed'}: {lines}")
    return found


def contract_problems(root):
    """The output contract of the real command on this tree: a valid base, and a base that does not resolve."""
    script = Path(__file__).resolve()
    found = []
    ok = subprocess.run([sys.executable, str(script), "--base", "@"], cwd=root, capture_output=True, text=True)
    steps = [line for line in ok.stdout.splitlines() if line and not line.startswith(("BASE: ", "TREE: "))]
    if ok.returncode != 0:
        found.append(f"--base @ exited {ok.returncode}: {ok.stderr.strip()[-300:]}")
    try:
        checked(steps)
    except ValueError as e:
        found.append(f"--base @: {e}")
    bad = subprocess.run([sys.executable, str(script), "--base", "no-such-revision-zz"], cwd=root,
                         capture_output=True, text=True)
    if bad.returncode == 0 or not bad.stderr.strip():
        found.append(f"a base that does not resolve exited {bad.returncode} with stderr {bad.stderr!r}")
    if any(STEP_LINE.fullmatch(line) for line in bad.stdout.splitlines()):
        found.append(f"a base that does not resolve printed a step line: {bad.stdout!r}")
    # gate-step.sh refuses a line without a reason, a blank one too: an empty selection is no output at all.
    with tempfile.NamedTemporaryFile("w", suffix=".txt") as docs:
        docs.write("docs/Plans/x.md\n")
        docs.flush()
        for gate in GATE_SCRIPTS:
            out = subprocess.run([sys.executable, str(script), "--gate", gate, "--paths-from", docs.name], cwd=root,
                                 capture_output=True, text=True)
            if out.returncode != 0 or any(len(line.split()) < 2 for line in out.stdout.splitlines()):
                found.append(f"--gate {gate} for a docs-only change exited {out.returncode} and printed {out.stdout!r}")
    for lines in (["clippy: a", "nextest"], ["timing: just timing-gate", "timing: just timing"], ["Clippy: a"]):
        try:
            checked(lines)
            found.append(f"{lines} passed the line check")
        except ValueError:
            pass
    return found



def dependents_oracle(meta):
    """{crate: the crate and every workspace crate whose dependency closure holds it}, by a forward walk per crate."""
    members = {p["name"]: p for p in meta["packages"] if p["id"] in meta["workspace_members"]}
    closure = {}
    for name in members:
        seen, todo = set(), [name]
        while todo:
            for d in members[todo.pop()]["dependencies"]:
                if d["name"] in members and d["name"] not in seen:
                    seen.add(d["name"])
                    todo.append(d["name"])
        closure[name] = seen
    return {c: {c} | {x for x in members if c in closure[x]} for c in members}


def quiet_gate_selection(root, meta, gate, changed):
    with contextlib.redirect_stderr(io.StringIO()):
        return gate_selection(root, meta, gate, changed)


def gate_full_problems(root, meta):
    """A change to a root build file, a manifest, the build config or the gate machinery runs every step of both gates,
    also beside a docs change."""
    dirs, _ = workspace_graph(meta)
    found = []
    for path in ["Cargo.lock", "Cargo.toml", "rust-toolchain.toml", "justfile", "proptest.toml", ".cargo/config.toml",
                 ".config/nextest.toml", "workspace-hack/Cargo.toml", f"{dirs[INTEGRATION]}/Cargo.toml",
                 "frontends/dioxus-web/Cargo.toml", *sorted(GATE_MACHINERY)]:
        for gate in GATE_SCRIPTS:
            got = quiet_gate_selection(root, meta, gate, ["docs/Plans/x.md", path])
            if [line[0] for line in got] != ["*"]:
                found.append(f"{gate} gate, {path} changed: selects {gate_lines(got)}, not a full run")
    return found


def gate_crate_problems(root, meta):
    """For a source change in each workspace crate: every gate step whose crates the change reaches is selected, the
    extra nextest filter names exactly the crate and its dependents, and no step outside them runs."""
    dirs, _ = workspace_graph(meta)
    dependents = dependents_oracle(meta)
    timing = timing_packages(root, meta)
    excluded = excluded_packages(profile_default_filter(nextest_config(root), LOAD_TOLERANT))
    names = set(gate_step_names(root, "integrate"))
    found = []
    # A change in a FULL_DIRS crate runs every step: gate_full_problems covers it.
    for crate in sorted(c for c in dependents if dirs[c].parts[0] not in FULL_DIRS):
        reached = dependents[crate]
        want = set(TREE_STEPS) | {"gate-compile", "check-release-shape"} | ({"extra-nextest"} if reached - excluded else set())
        want |= set(INTEGRATION_STEPS) if INTEGRATION in reached else set()
        want |= {"windowed-gate"} if "holon-gpui" in reached else set()
        want |= {"check-frontend-wasm"} if "holon-frontend" in reached else set()
        want |= {step for step, d in NON_MEMBER_STEPS.items() if {dirs[n] for n in reached} & path_deps(root, d)}
        timed = bool(reached & timing)
        want = (want | (set(TIMING_STEPS) if timed else set())) & names
        path = f"{dirs[crate]}/src/gate_probe.rs"
        got = {step: args for step, _, args in quiet_gate_selection(root, meta, "integrate", [path])}
        # A path-table row adds a step for one crate's paths; the rows' own check is golden_problems.
        extra = set(got) - want - {r for r in path_recipes(root).values() if r in got and str(dirs[crate]) in
                                   " ".join(k for k, v in path_recipes(root).items() if v == r)}
        if want - set(got) or extra:
            found.append(f"{path}: integrate misses {sorted(want - set(got))}, adds {sorted(extra)}")
        filt = " | ".join(f"package({n})" for n in sorted(reached - excluded)) or None
        if got.get("extra-nextest") != filt:
            found.append(f"{path}: extra-nextest filter {got.get('extra-nextest')!r}, want {filt!r}")
        timing_got = [step for step, _, _ in quiet_gate_selection(root, meta, "timing", [path])]
        if timing_got != (list(TIMING_STEPS) if timed else []):
            found.append(f"{path}: timing gate selects {timing_got}; the change reaches timing crates: {timed}")
    return found


def gate_example_problems(root, meta):
    """The fixed cases: a docs-only change runs only the tree checks, a change to the code the keystone tour settles
    through runs the tour, a change to a crate's tests/ only reaches that crate."""
    dirs, _ = workspace_graph(meta)
    leaf = dirs["holon-frontend"]
    cases = [
        ("integrate", ["docs/Plans/x.md"], lambda got: set(got) == set(TREE_STEPS)),
        ("timing", ["docs/Plans/x.md"], lambda got: got == {}),
        ("integrate", [f"{dirs[INTEGRATION]}/src/pbt/convergence.rs"], lambda got: "keystone-tour" in got),
        ("integrate", [f"{leaf}/tests/gate_probe.rs"], lambda got: got.get("extra-nextest") == "package(holon-frontend)"),
        # The tour classifies its divergences against this registry at run time.
        ("integrate", ["docs/Testing/KeystoneKnownReds.md"], lambda got: "keystone-tour" in got),
    ]
    found = []
    for gate, changed, ok in cases:
        got = {step: args for step, _, args in quiet_gate_selection(root, meta, gate, changed)}
        if not ok(got):
            found.append(f"{gate} gate, {changed}: selects {got}")
    return found


def gate_tree_problems(root, meta):
    """Every tracked path outside the workspace crates gives a selection of declared steps in the line format the
    gate runner reads."""
    dirs, _ = workspace_graph(meta)
    paths = subprocess.run(["jj", "--ignore-working-copy", "file", "list", "-r", "@"], cwd=root, check=True,
                           stdout=subprocess.PIPE, text=True).stdout.split("\n")
    found = []
    for gate in GATE_SCRIPTS:
        names = set(gate_step_names(root, gate))
        for path in (p for p in paths if p and owner(PurePosixPath(p), dirs) is None):
            try:
                lines = gate_lines(quiet_gate_selection(root, meta, gate, [path]))
            except Exception as e:
                found.append(f"{gate} gate, {path}: {type(e).__name__}: {e}")
                continue
            for line in lines:
                first, _, rest = line.partition(" ")
                if not rest or (first != "*" and (first not in names or not re.fullmatch(r"[\w.-]+", first))):
                    found.append(f"{gate} gate, {path}: line {line!r} is not `<declared step> <reason> [args]`")
    return found


def self_check():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        meta = fixture_workspace(root)
        found = golden_problems(root, meta) + never_owner_problems(root, meta)
    cwd = Path.cwd()
    meta = load_metadata()
    found += removed_binary_problems(cwd) + dropped_module_owner_problems(cwd) + keystone_tour_problems(cwd)
    found += contract_problems(cwd)
    found += gate_full_problems(cwd, meta) + gate_crate_problems(cwd, meta) + gate_example_problems(cwd, meta)
    found += gate_tree_problems(cwd, meta)
    for line in found:
        print(f"[select-tests] FAIL {line}")
    print(f"[select-tests] self-check {'FAIL' if found else 'PASS'}: {len(GOLDEN)} golden cases,"
          f" the never-owner set, the integration test targets, the dropped modules' owner lines, the keystone tour,"
          f" the output contract, both landing gates' selections")
    return 1 if found else 0


def main(argv):
    if argv == ["--self-check"]:
        return self_check()
    ap = argparse.ArgumentParser()
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--base", help="jj revision the lane started from, e.g. main")
    mode.add_argument("--gate", choices=sorted(GATE_SCRIPTS), help="print the selection of one landing gate")
    ap.add_argument("--paths-from", help="with --gate: the changed paths, one per line (default: $GATE_CHANGED)")
    a = ap.parse_args(argv)
    cwd = Path.cwd()
    meta = load_metadata()
    assert_workspace_root(meta, cwd)
    if a.gate:
        changed = Path(a.paths_from or os.environ["GATE_CHANGED"]).read_text().splitlines()
        for line in gate_lines(gate_selection(cwd, meta, a.gate, changed)):
            print(line)
        return 0
    if a.paths_from:
        ap.error("--paths-from goes with --gate")
    header = jj_header(a.base)
    lines = selection(cwd, meta, jj_changed(a.base))
    print(*header, *lines, sep="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
