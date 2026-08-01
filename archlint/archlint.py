#!/usr/bin/env python3
"""archlint — fast architecture-rule checker for Claude Code edit hooks and CI.

Modes:
  archlint check FILE [FILE ...]   Lint one or more files. Exits 2 on violations.
  archlint hook                    Read Claude Code hook JSON from stdin, lint
                                   tool_input.file_path (or edits[].file_path).
  archlint --all                   Scan the entire repo (replacement for the
                                   cargo arch-tests). Includes cross-file rules.
  archlint discover                Rebuild discovery cache (e.g. #[jsonb] field set).

Layers:
  ast-grep YAML rules in rules/         (CST-shape checks)
  ripgrep regex smells in smells/       (raw-text checks)
  Discovery cache in cache/             (project-wide pre-pass results)
  Aggregate rules: only run with --all  (need cross-file state)

Suppression:
  // ALLOW(<id>): <reason>   on the same or preceding line.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from collections import defaultdict
from fnmatch import fnmatch
from pathlib import Path

ARCHLINT_DIR = Path(__file__).resolve().parent
REPO_ROOT = ARCHLINT_DIR.parent
SGCONFIG = ARCHLINT_DIR / "sgconfig.yml"
SMELLS_DIR = ARCHLINT_DIR / "smells"
CACHE_DIR = ARCHLINT_DIR / "cache"
JSONB_CACHE = CACHE_DIR / "jsonb-fields.json"
BASELINE_FILE = ARCHLINT_DIR / "baseline.txt"

# Standard skips (mirror architecture_rules.rs::scan_rs_files).
DEFAULT_SKIPS = (
    "/tests/", "/pbt/", "architecture-tests", "examples/",
    "integration-tests/", "_pbt.rs", "_test.rs", "pbt_infrastructure",
    "pbt_test",
)

# Files that look like proper Rust sources for --all mode.
RUST_GLOB = "**/*.rs"


def read_lines(path: str) -> list[str]:
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.readlines()
    except OSError:
        return []


def has_allow(
    file_lines: list[str],
    start_1indexed: int,
    tag: str,
    end_1indexed: int | None = None,
) -> bool:
    """ALLOW(<tag>) on the line above the match, or anywhere within the match span.

    Multi-line ast-grep matches (e.g. method chains spanning several lines)
    require checking the entire span — the user typically annotates the .ok()
    line itself, which is below the match's start.

    rustfmt's `wrap_comments` reflows a long `// ALLOW(...)` line across
    multiple `//` lines, so the marker can land above the line directly
    preceding the match — walk upward through contiguous `//` comment lines
    until the marker is found or a non-comment line is hit.
    """
    marker = f"ALLOW({tag})"
    if end_1indexed is None or end_1indexed < start_1indexed:
        end_1indexed = start_1indexed
    start_idx = start_1indexed - 1
    idx = start_idx - 1
    while idx >= 0 and file_lines[idx].lstrip().startswith("//"):
        if marker in file_lines[idx]:
            return True
        idx -= 1
    for idx in range(max(start_idx, 0), min(end_1indexed, len(file_lines))):
        if marker in file_lines[idx]:
            return True
    return False


# A diagnostic is suppressed by `// ALLOW(<tag>): <reason>` where <tag> is either
# the rule id itself OR any ALLOW(...) tag mentioned in the rule's own message
# (e.g. ast-grep rule `no-block-on-in-async` whose message says
# `To suppress: // ALLOW(block_on): <reason>` accepts both `no-block-on-in-async`
# and `block_on`). This keeps the canonical ALLOW name with the rule definition
# instead of a side-table here.
ALLOW_TAG_RE = re.compile(r"ALLOW\(([A-Za-z_][A-Za-z0-9_]*)\)")


def extract_allow_tags(message: str) -> list[str]:
    return ALLOW_TAG_RE.findall(message or "")


def has_allow_for_diag(
    file_lines: list[str],
    start_1indexed: int,
    rule_id: str,
    message: str,
    end_1indexed: int | None = None,
) -> bool:
    if has_allow(file_lines, start_1indexed, rule_id, end_1indexed):
        return True
    for tag in extract_allow_tags(message):
        if tag != rule_id and has_allow(file_lines, start_1indexed, tag, end_1indexed):
            return True
    return False


# Rules that should ignore default-skipped paths (test files, PBT, examples)
# regardless of the run mode. Matches the original arch-test scope for these
# checks (production-code-only).
PRODUCTION_ONLY_RULES: set[str] = {
    "ok",
    "filter_map_ok",
    "unwrap_or_default",
    "catch_unwind_debug",
}

# Per-rule file-glob exclusions (Path.match style). Mirrors special-case skips
# from the original cargo arch-tests. Use this for "this rule fires on real
# code that's nevertheless an accepted convention in this path" — not for
# blanket suppressions, which belong on the rule definition itself.
RULE_EXTRA_FILE_SKIPS: dict[str, tuple[str, ...]] = {
    # Frontend main.rs / lib.rs use block_on at the sync→async runtime boundary.
    # The rule walks up to the enclosing `async fn main()`, but the block_on is
    # actually inside a sync `std::thread::spawn` closure — not a real violation.
    "no-block-on-in-async": (
        "frontends/*/src/main.rs",
        "frontends/*/src/lib.rs",
        "frontends/*/main.rs",
        "frontends/*/lib.rs",
    ),
}

# Universal exclusions for the `ok` rule that don't need explicit ALLOW comments.
# Mirror is_universally_allowed_ok() in
# crates/holon-architecture-tests/tests/no_defensive_programming.rs.
OK_UNIVERSAL_TEXT_MARKERS = (
    "writeln!",
    "write!",
    ".set(",
    ".send(",
    "env::var",
    "try_resolve",
)
OK_UNIVERSAL_FILE_MARKERS = ("build.rs", "holon-macros/", "theme.rs")
FILTER_MAP_OK_UNIVERSAL_FILE_MARKERS = ("build.rs", "holon-macros/")


def matches_glob(rel: str, pattern: str) -> bool:
    # fnmatch has no `**` semantics: `src/**/*.rs` forces at least one
    # directory between src/ and the file, silently exempting top-level
    # files (e.g. crates/holon-orgmode/src/di.rs). Expand variants where
    # each `**/` also matches zero directories.
    variants = {pattern}
    queue = [pattern]
    while queue:
        p = queue.pop()
        if p.startswith("**/"):
            v = p[3:]
            if v not in variants:
                variants.add(v)
                queue.append(v)
        idx = p.find("/**/")
        while idx != -1:
            v = p[:idx] + p[idx + 3:]  # collapse "/**/" to "/"
            if v not in variants:
                variants.add(v)
                queue.append(v)
            idx = p.find("/**/", idx + 1)
    return any(fnmatch(rel, v) for v in variants)


def matches_any(rel: str, patterns: list[str]) -> bool:
    return any(matches_glob(rel, p) for p in patterns)


def is_default_skipped(rel: str) -> bool:
    return any(s in rel for s in DEFAULT_SKIPS)


WORKTREE_PREFIX_RE = re.compile(r"^.*\.claude/worktrees/[^/]+/")


def relpath(p: Path) -> str:
    try:
        rel = str(p.resolve().relative_to(REPO_ROOT))
    except ValueError:
        rel = str(p)
    # jj/git workspaces nest under .claude/worktrees/<name>/ (possibly
    # repeatedly for nested workspaces); baseline keys are relative to the
    # true repo root, so strip every such prefix greedily before returning.
    return WORKTREE_PREFIX_RE.sub("", rel)


# ---------------------------------------------------------------- ast-grep

def run_ast_grep(rust_files: list[Path]) -> list[dict]:
    if not rust_files:
        return []
    cmd = ["ast-grep", "scan", "-c", str(SGCONFIG), "--json=stream",
           *(str(f) for f in rust_files)]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    diags: list[dict] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        rng = obj.get("range", {})
        rng_start = rng.get("start", {})
        rng_end = rng.get("end", {})
        diags.append({
            "id": obj.get("ruleId", "<unknown>"),
            "file": obj.get("file", "<unknown>"),
            "line": rng_start.get("line", 0) + 1,
            "end_line": rng_end.get("line", rng_start.get("line", 0)) + 1,
            "message": (obj.get("message") or "").strip(),
            "_meta": obj.get("metaVariables", {}),
        })
    return diags


# ---------------------------------------------------------------- smells

def load_smells() -> list[dict]:
    smells: list[dict] = []
    for toml_file in sorted(SMELLS_DIR.glob("*.toml")):
        with open(toml_file, "rb") as fh:
            data = tomllib.load(fh)
        smells.extend(data.get("smell", []))
    return smells


def smell_scope(files: list[Path], smell: dict) -> list[Path]:
    files_glob = smell.get("files", "**/*.rs")
    exclude = smell.get("exclude")
    excludes: list[str] = []
    if isinstance(exclude, str):
        excludes = [exclude]
    elif isinstance(exclude, list):
        excludes = list(exclude)

    scope: list[Path] = []
    for f in files:
        rel = relpath(f) if f.is_absolute() else str(f)
        if not matches_glob(rel, files_glob):
            continue
        if matches_any(rel, excludes):
            continue
        if smell.get("default_skips", True) and is_default_skipped(rel):
            continue
        scope.append(f)
    return scope


def run_smells(files: list[Path], smells: list[dict]) -> list[dict]:
    diags: list[dict] = []
    for smell in smells:
        sid = smell["id"]
        pattern = smell["pattern"]
        case_sensitive = smell.get("case_sensitive", False)
        message = (smell.get("message") or "").strip()

        scope = smell_scope(files, smell)
        if not scope:
            continue

        cmd = ["rg", "--pcre2", "--json", "--no-heading"]
        if not case_sensitive:
            cmd.append("-i")
        cmd += [pattern, *(str(p) for p in scope)]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        for line in proc.stdout.splitlines():
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            if obj.get("type") != "match":
                continue
            data = obj["data"]
            diags.append({
                "id": sid,
                "file": data["path"]["text"],
                "line": data["line_number"],
                "message": message,
            })
    return diags


# ---------------------------------------------------------------- defensive-pattern post-filters

def filter_defensive_post(diags: list[dict]) -> list[dict]:
    """Apply rule-specific post-filters for the defensive ast-grep rules.

    - `ok`: drop matches whose line text contains a universally-allowed marker
      (writeln!, write!, .set(, .send(, env::var, try_resolve) or whose file
      lives under build.rs / holon-macros / theme.rs.
    - `filter_map_ok`: drop matches whose 5-line window does not contain `.ok()`,
      and apply file-marker exclusions (build.rs, holon-macros).
    Mirrors the canonical arch-test in no_defensive_programming.rs.
    """
    if not diags:
        return diags
    line_cache: dict[str, list[str]] = {}
    kept: list[dict] = []
    for d in diags:
        rid = d["id"]
        if rid not in ("ok", "filter_map_ok"):
            kept.append(d)
            continue
        path = d["file"]
        if path not in line_cache:
            line_cache[path] = read_lines(path)
        lines = line_cache[path]
        line_idx = d["line"] - 1

        if rid == "ok":
            end_line = d.get("end_line", d["line"])
            end_idx = min(end_line, len(lines))
            window = "".join(lines[max(line_idx, 0):end_idx])
            if any(m in window for m in OK_UNIVERSAL_TEXT_MARKERS):
                continue
            if any(m in path for m in OK_UNIVERSAL_FILE_MARKERS):
                continue
            kept.append(d)
            continue

        # filter_map_ok: ast-grep matches every filter_map; keep only those whose
        # closure body actually contains `.ok()` (multi-line method chains common).
        if rid == "filter_map_ok":
            end_line = d.get("end_line", d["line"])
            end_idx = min(end_line, len(lines))
            window = "".join(lines[max(line_idx, 0):end_idx])
            if ".ok()" not in window:
                continue
            if any(m in path for m in FILTER_MAP_OK_UNIVERSAL_FILE_MARKERS):
                continue
            kept.append(d)
            continue
    return kept


def filter_production_only(diags: list[dict]) -> list[dict]:
    """Drop diagnostics for production-only rules when emitted from a default-skipped path."""
    kept: list[dict] = []
    for d in diags:
        if d["id"] in PRODUCTION_ONLY_RULES and is_default_skipped(relpath(Path(d["file"]))):
            continue
        kept.append(d)
    return kept


def filter_extra_file_skips(diags: list[dict]) -> list[dict]:
    kept: list[dict] = []
    for d in diags:
        skips = RULE_EXTRA_FILE_SKIPS.get(d["id"], ())
        if skips:
            rel = relpath(Path(d["file"]))
            if any(Path(rel).match(p) for p in skips):
                continue
        kept.append(d)
    return kept


# ---------------------------------------------------------------- catch_unwind multi-line check

def check_catch_unwind_at_debug(rust_files: list[Path]) -> list[dict]:
    """Flag `catch_unwind` blocks that log the swallowed panic at debug! level.

    Mirrors no_catch_unwind_at_debug_level() in no_defensive_programming.rs:
    a 15-line window starting at the catch_unwind line must contain a debug
    log call AND a panic/Caught marker.
    """
    diags: list[dict] = []
    msg = (
        "catch_unwind with debug!-level logging of the swallowed panic. "
        "Swallowed panics must be logged at error! level (or rethrown). "
        "To suppress: // ALLOW(catch_unwind_debug): <reason>"
    )
    for f in rust_files:
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        lines = text.splitlines()
        for i, line in enumerate(lines):
            if "catch_unwind" not in line:
                continue
            window = "\n".join(lines[i : i + 15])
            if (("tracing::debug!(" in window) or ("debug!(" in window)) and (
                ("panic" in window) or ("Caught" in window)
            ):
                diags.append({
                    "id": "catch_unwind_debug",
                    "file": str(f),
                    "line": i + 1,
                    "message": msg,
                })
    return diags


# ---------------------------------------------------------------- HANDOFF root check

def check_handoff_root(files: list[Path]) -> list[dict]:
    diags: list[dict] = []
    for f in files:
        try:
            parent = f.resolve().parent
        except OSError:
            continue
        if parent != REPO_ROOT:
            continue
        if f.name.startswith("HANDOFF_") and f.name.endswith(".md"):
            diags.append({
                "id": "no-handoff-md-at-repo-root",
                "file": str(f),
                "line": 1,
                "message": (
                    "HANDOFF_*.md at the repo root is forbidden (AC-7). "
                    "Add a sub-block to the relevant topic doc in "
                    "holon-pkm/Projects/Holon/ tagged :handoff:active: instead."
                ),
            })
    return diags


# ---------------------------------------------------------------- discoveries (jsonb fields)

JSONB_FIELD_RE = re.compile(r"^\s*pub\s+([A-Za-z_][A-Za-z0-9_]*)\s*:")


def discover_jsonb_fields() -> set[str]:
    fields: set[str] = set()
    for path in REPO_ROOT.glob("crates/" + RUST_GLOB):
        rel = relpath(path)
        if "architecture-tests" in rel:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        lines = text.splitlines()
        for i, line in enumerate(lines):
            if "#[jsonb]" not in line:
                continue
            j = i + 1
            while j < len(lines) and lines[j].strip().startswith("#["):
                j += 1
            if j >= len(lines):
                continue
            m = JSONB_FIELD_RE.match(lines[j])
            if not m:
                # No `pub` — try without
                m = re.match(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:", lines[j])
                if not m:
                    continue
            fields.add(m.group(1))
    return fields


def write_jsonb_cache(fields: set[str]) -> None:
    CACHE_DIR.mkdir(exist_ok=True)
    JSONB_CACHE.write_text(json.dumps(sorted(fields), indent=2))


def read_jsonb_cache() -> set[str]:
    if not JSONB_CACHE.exists():
        return set()
    try:
        return set(json.loads(JSONB_CACHE.read_text()))
    except json.JSONDecodeError:
        return set()


def jsonb_cache_is_fresh() -> bool:
    """True if the cache exists and is newer than every source file in scope.
    Walk stops at the first stale file, so the worst case (cache stale or missing)
    is fast; the steady state (cache fresh) is one stat per source file (~5 ms).
    """
    if not JSONB_CACHE.exists():
        return False
    try:
        cache_mtime = JSONB_CACHE.stat().st_mtime
    except OSError:
        return False
    for path in REPO_ROOT.glob("crates/" + RUST_GLOB):
        try:
            if path.stat().st_mtime > cache_mtime:
                return False
        except OSError:
            continue
    return True


def get_jsonb_fields_with_refresh() -> set[str]:
    """Hook/check entry point: read cache if fresh, rebuild otherwise."""
    if jsonb_cache_is_fresh():
        return read_jsonb_cache()
    fields = discover_jsonb_fields()
    write_jsonb_cache(fields)
    return fields


def filter_jsonb_diags(diags: list[dict], fields: set[str]) -> list[dict]:
    """Keep only `jsonb-as-string` diagnostics whose captured KEY is a jsonb field."""
    if not fields:
        # Cache empty — be conservative and keep all (parity bias toward false positives).
        return diags
    kept: list[dict] = []
    for d in diags:
        if d["id"] != "jsonb-as-string":
            kept.append(d)
            continue
        meta = d.get("_meta", {}) or {}
        single = meta.get("single", {}) or {}
        key_node = single.get("KEY")
        if not key_node:
            continue
        text = (key_node.get("text") or "").strip().strip('"')
        if text in fields:
            kept.append(d)
    return kept


# ---------------------------------------------------------------- aggregate (--all only)

ARM_RE = re.compile(r'"([^"]+)"')

def aggregate_scattered_match_as_str(rust_files: list[Path]) -> list[dict]:
    """Flag string-arm sets that appear as `match X.as_str() { ... }` in 3+ files."""
    pattern = "match $E.as_str() { $$$ARMS }"
    cmd = ["ast-grep", "run", "--lang", "rust", "-p", pattern,
           "--json=stream", *(str(f) for f in rust_files)]
    proc = subprocess.run(cmd, capture_output=True, text=True)

    arm_set_files: dict[tuple, set[str]] = defaultdict(set)
    arm_set_first: dict[tuple, dict] = {}
    for line in proc.stdout.splitlines():
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        text = obj.get("text") or ""
        arms = sorted(set(ARM_RE.findall(text)))
        if len(arms) < 3:
            continue
        key = tuple(arms)
        f = obj.get("file", "")
        arm_set_files[key].add(f)
        if key not in arm_set_first:
            rng_start = obj.get("range", {}).get("start", {})
            arm_set_first[key] = {
                "file": f,
                "line": rng_start.get("line", 0) + 1,
            }

    diags: list[dict] = []
    for key, files in arm_set_files.items():
        if len(files) < 3:
            continue
        first = arm_set_first[key]
        diags.append({
            "id": "no-scattered-match-as-str",
            "file": first["file"],
            "line": first["line"],
            "message": (
                f"match arms [{', '.join(key)}] appear in {len(files)} files — "
                f"this string set should be a typed enum (Parse, Don't Validate). "
                f"Files: {', '.join(sorted(relpath(Path(f)) for f in files))}"
            ),
        })
    return diags


# ---------------------------------------------------------------- suppression + format

def filter_suppressed(diags: list[dict]) -> list[dict]:
    cache: dict[str, list[str]] = {}
    kept: list[dict] = []
    for d in diags:
        path = d["file"]
        if path not in cache:
            cache[path] = read_lines(path)
        if has_allow_for_diag(
            cache[path],
            d["line"],
            d["id"],
            d.get("message", ""),
            d.get("end_line"),
        ):
            continue
        kept.append(d)
    return kept


def format_diagnostics(diags: list[dict]) -> str:
    if not diags:
        return ""
    out: list[str] = ["", f"archlint: {len(diags)} architecture violation(s):", ""]
    by_id: dict[str, list[dict]] = {}
    order: list[str] = []
    for d in diags:
        if d["id"] not in by_id:
            by_id[d["id"]] = []
            order.append(d["id"])
        by_id[d["id"]].append(d)
    for sid in order:
        items = by_id[sid]
        out.append(f"  [{sid}] {len(items)} occurrence(s)")
        for d in items:
            rel = relpath(Path(d["file"]))
            out.append(f"    {rel}:{d['line']}")
        for ln in items[0]["message"].splitlines():
            out.append(f"    | {ln}")
        out.append("")
    return "\n".join(out)


# ---------------------------------------------------------------- file collection

def collect_files(arg_files: list[str]) -> list[Path]:
    paths: list[Path] = []
    for f in arg_files:
        p = Path(f)
        if p.exists():
            paths.append(p.resolve())
    return paths


def collect_all_files() -> list[Path]:
    """Repo-wide file set for --all mode (Rust + Cargo.toml + md)."""
    files: list[Path] = []
    for sub in ("crates", "frontends"):
        files.extend((REPO_ROOT / sub).glob(RUST_GLOB))
        files.extend((REPO_ROOT / sub).glob("**/Cargo.toml"))
    # Repo-root Cargo.toml + workspace files
    if (REPO_ROOT / "Cargo.toml").exists():
        files.append(REPO_ROOT / "Cargo.toml")
    # Repo-root markdown (for HANDOFF check)
    files.extend(REPO_ROOT.glob("*.md"))
    return [f.resolve() for f in files if f.exists()]


# ---------------------------------------------------------------- pipelines

def lint_files(files: list[Path], jsonb_fields: set[str], apply_default_skips: bool = False) -> list[dict]:
    rust_files = [f for f in files if f.suffix == ".rs"]
    if apply_default_skips:
        rust_files = [f for f in rust_files if not is_default_skipped(relpath(f))]
    sg_diags = run_ast_grep(rust_files)
    sg_diags = filter_jsonb_diags(sg_diags, jsonb_fields)
    sg_diags = filter_defensive_post(sg_diags)
    rg_diags = run_smells(files, load_smells())
    handoff_diags = check_handoff_root(files)
    catch_unwind_diags = check_catch_unwind_at_debug(rust_files)
    diags = sg_diags + rg_diags + handoff_diags + catch_unwind_diags
    diags = filter_production_only(diags)
    diags = filter_extra_file_skips(diags)
    return diags


# ---------------------------------------------------------------- output emit

def emit_diagnostics(diags: list[dict], fmt: str, files_scanned: int = 0) -> None:
    """Write diagnostics. Text → stderr (current behavior). JSON → stdout (pipeable).
    JSON is emitted even when diags is empty so pipelines always see a stable shape.
    """
    if fmt == "json":
        payload = {
            "version": 1,
            "violations": len(diags),
            "files_scanned": files_scanned,
            "diagnostics": [
                {
                    "id": d["id"],
                    "file": relpath(Path(d["file"])),
                    "line": d["line"],
                    "message": (d.get("message") or "").strip(),
                }
                for d in diags
            ],
        }
        sys.stdout.write(json.dumps(payload, indent=2) + "\n")
        return
    if not diags:
        return
    sys.stderr.write(format_diagnostics(diags) + "\n")


# ---------------------------------------------------------------- commands

def cmd_check(file_args: list[str], fmt: str = "text") -> int:
    files = collect_files(file_args)
    if not files:
        if fmt == "json":
            emit_diagnostics([], fmt, files_scanned=0)
        return 0
    jsonb_fields = get_jsonb_fields_with_refresh()
    diags = filter_suppressed(lint_files(files, jsonb_fields))
    emit_diagnostics(diags, fmt, files_scanned=len(files))
    return 2 if diags else 0


def cmd_hook(fmt: str = "text") -> int:
    raw = sys.stdin.read()
    if not raw.strip():
        return 0
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return 0
    tool_input = payload.get("tool_input", {}) or {}
    files: list[str] = []
    if "file_path" in tool_input:
        files.append(tool_input["file_path"])
    for edit in tool_input.get("edits", []) or []:
        if isinstance(edit, dict) and "file_path" in edit:
            files.append(edit["file_path"])
    if not files:
        return 0
    return cmd_check(files, fmt=fmt)


# ---------------------------------------------------------------- baseline ratchet
#
# Identity of a violation = rule_id + repo-relative path + the normalised source
# line (whitespace-collapsed). We deliberately do NOT key on line number so a
# baselined violation survives edits elsewhere in the file (line drift). Multiple
# identical-text hits in the same file are tracked by MULTIPLICITY: the baseline
# is a multiset, so adding a 4th identical hit where 3 were baselined surfaces as
# 1 NEW violation, and removing one leaves a STALE baseline entry (ratchet down).

def diag_snippet(d: dict) -> str:
    lines = read_lines(d["file"])
    idx = d["line"] - 1
    if 0 <= idx < len(lines):
        return " ".join(lines[idx].split())
    return ""


def diag_identity(d: dict) -> str:
    return f"{d['id']}\t{relpath(Path(d['file']))}\t{diag_snippet(d)}"


def load_baseline() -> "Counter[str]":
    from collections import Counter
    if not BASELINE_FILE.exists():
        return Counter()
    entries: list[str] = []
    for raw in BASELINE_FILE.read_text(encoding="utf-8").splitlines():
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        entries.append(raw.rstrip("\n"))
    return Counter(entries)


def write_baseline(diags: list[dict]) -> None:
    from collections import Counter
    counts = Counter(diag_identity(d) for d in diags)
    by_rule: dict[str, int] = {}
    for ident, n in counts.items():
        by_rule[ident.split("\t", 1)[0]] = by_rule.get(ident.split("\t", 1)[0], 0) + n
    header = [
        "# archlint baseline — grandfathered architecture violations.",
        "# Format: <rule_id>\\t<repo-relative-path>\\t<whitespace-collapsed source line>",
        "# `archlint --all` FAILS ONLY on violations absent from this file; every",
        "# baselined hit is still disclosed in the summary so the tree never reads",
        "# as debt-free. Ratchet DOWN: as violations are fixed, regenerate with",
        "#   ./archlint/archlint --update-baseline",
        "# See archlint/BURNDOWN.md for the category table and burn-down priority.",
        f"# Snapshot total: {sum(counts.values())} violation(s) across {len(by_rule)} rule(s).",
        "#",
    ]
    for rule in sorted(by_rule):
        header.append(f"#   {rule}: {by_rule[rule]}")
    header.append("")
    body = sorted(counts.elements())
    BASELINE_FILE.write_text("\n".join(header + body) + "\n", encoding="utf-8")


def split_by_baseline(diags: list[dict]) -> tuple[list[dict], list[dict], int]:
    """Return (new_diags, baselined_diags, stale_count)."""
    baseline = load_baseline()
    remaining = dict(baseline)
    new: list[dict] = []
    baselined: list[dict] = []
    for d in diags:
        ident = diag_identity(d)
        if remaining.get(ident, 0) > 0:
            remaining[ident] -= 1
            baselined.append(d)
        else:
            new.append(d)
    stale = sum(c for c in remaining.values() if c > 0)
    return new, baselined, stale


def scan_all() -> tuple[list[dict], int]:
    """Full-repo scan. Returns (post-suppression diags, files scanned)."""
    files = collect_all_files()
    jsonb_fields = discover_jsonb_fields()
    write_jsonb_cache(jsonb_fields)

    diags = lint_files(files, jsonb_fields, apply_default_skips=True)
    rust_files = [f for f in files if f.suffix == ".rs" and not is_default_skipped(relpath(f))]
    diags += aggregate_scattered_match_as_str(rust_files)
    diags = filter_suppressed(diags)
    return diags, len(files)


def cmd_update_baseline() -> int:
    diags, nfiles = scan_all()
    write_baseline(diags)
    sys.stderr.write(
        f"archlint: wrote baseline with {len(diags)} violation(s) "
        f"across {nfiles} files -> {relpath(BASELINE_FILE)}\n"
    )
    return 0


def cmd_all(fmt: str = "text") -> int:
    diags, nfiles = scan_all()
    new, baselined, stale = split_by_baseline(diags)

    if fmt == "json":
        # JSON reports ONLY the gate-failing (new) violations, but discloses the
        # baselined + stale counts alongside so pipelines see the full picture.
        emit_diagnostics(new, fmt, files_scanned=nfiles)
        sys.stderr.write(
            f"archlint: {len(baselined)} baselined violation(s) suppressed "
            f"(see {relpath(BASELINE_FILE)}), {len(new)} new violation(s).\n"
        )
        if stale:
            sys.stderr.write(
                f"archlint: baseline stale - {stale} entry(ies) no longer fire; "
                f"run ./archlint/archlint --update-baseline to ratchet down.\n"
            )
        return 2 if new else 0

    # text mode
    if new:
        sys.stderr.write(format_diagnostics(new) + "\n")
    sys.stderr.write(
        f"archlint: {len(baselined)} baselined violation(s) suppressed "
        f"(see {relpath(BASELINE_FILE)}), {len(new)} new violation(s).\n"
    )
    if stale:
        sys.stderr.write(
            f"archlint: baseline stale - {stale} entry(ies) no longer fire; "
            f"run ./archlint/archlint --update-baseline to ratchet down.\n"
        )
    return 2 if new else 0


def cmd_discover() -> int:
    fields = discover_jsonb_fields()
    write_jsonb_cache(fields)
    sys.stderr.write(f"archlint: cached {len(fields)} #[jsonb] fields → {relpath(JSONB_CACHE)}\n")
    return 0


# ---------------------------------------------------------------- dylint (type-aware lints)

DYLINT_DIR = ARCHLINT_DIR / "dylint"


def discover_dylint_libs() -> list[Path]:
    """Each subdirectory of archlint/dylint/ that has a Cargo.toml is a lint crate."""
    if not DYLINT_DIR.exists():
        return []
    return sorted(p for p in DYLINT_DIR.iterdir() if (p / "Cargo.toml").exists())


def cmd_dylint(
    extra_args: list[str],
    no_deps: bool = False,
    keep_going: bool = True,
) -> int:
    """Run every lint under archlint/dylint/ against the holon workspace.

    dylint requires a nightly toolchain (pinned per-lint via rust-toolchain) and
    `cargo-dylint` 5.x on PATH. First run compiles each lint (~minutes); incremental
    runs reuse the cached cdylib. Slow enough that it lives behind a separate
    subcommand instead of being folded into `archlint --all`.

    `no_deps` skips checking dependent crates (cargo dylint --no-deps).
    `keep_going` is on by default so one crate's failure (e.g. an MSRV mismatch
    with the dylint-pinned nightly) doesn't abort the remaining lints.
    """
    libs = discover_dylint_libs()
    if not libs:
        sys.stderr.write(f"archlint: no dylint libs under {relpath(DYLINT_DIR)}\n")
        return 0

    overall = 0
    env = os.environ.copy()
    # Cargo's libgit2 SSL handshake fails on macOS in some setups (corp proxy,
    # stale system SSL). Use the system git binary instead — works everywhere.
    env.setdefault("CARGO_NET_GIT_FETCH_WITH_CLI", "true")

    # Strip a leading `--` from extra_args (argparse.REMAINDER may keep it).
    if extra_args and extra_args[0] == "--":
        extra_args = extra_args[1:]

    # Default `cargo check` args: scan the whole workspace.
    cargo_args = list(extra_args) if extra_args else ["--workspace"]

    for lib in libs:
        sys.stderr.write(f"archlint: running dylint `{lib.name}` …\n")
        # Note: cargo dylint takes cargo-check args as positional, not after `--`.
        # Its own flags (--no-deps, --keep-going, --path) come *before* them.
        cmd = ["cargo", "dylint"]
        if keep_going:
            cmd.append("--keep-going")
        if no_deps:
            cmd.append("--no-deps")
        cmd += ["--path", str(lib), *cargo_args]
        proc = subprocess.run(cmd, cwd=REPO_ROOT, env=env)
        if proc.returncode != 0:
            overall = proc.returncode
    return overall


def main() -> None:
    ap = argparse.ArgumentParser(prog="archlint")
    ap.add_argument("--all", action="store_true", help="Scan whole repo (CI mode)")
    ap.add_argument("--update-baseline", action="store_true",
                    help="Rescan the repo and overwrite archlint/baseline.txt "
                         "with the current violation set (ratchet snapshot).")
    ap.add_argument(
        "--format",
        choices=("text", "json"),
        default="text",
        help="Output format. text → human-readable on stderr (default). "
             "json → machine-readable on stdout, always emitted (even when clean).",
    )
    sub = ap.add_subparsers(dest="cmd")
    chk = sub.add_parser("check", help="Lint explicit file paths")
    chk.add_argument("files", nargs="+")
    sub.add_parser("hook", help="Read Claude Code hook JSON from stdin")
    sub.add_parser("discover", help="Rebuild discovery cache")
    dyl = sub.add_parser(
        "dylint",
        help="Run type-aware Rust lints from archlint/dylint/ (slow first run; "
             "use for CI / on-demand checks, not the per-edit hook)",
    )
    dyl.add_argument(
        "--no-deps",
        action="store_true",
        help="Skip checking dependent crates (cargo dylint --no-deps). "
             "Only the explicitly-targeted package is rustc-checked.",
    )
    dyl.add_argument(
        "--abort-on-failure",
        action="store_true",
        help="Stop the multi-lint run on the first failing crate. By default we "
             "pass --keep-going to cargo dylint so an MSRV-mismatched crate "
             "(e.g. constant_time_eq) doesn't abort the rest of the workspace.",
    )
    dyl.add_argument(
        "cargo_args",
        nargs=argparse.REMAINDER,
        help="Extra args forwarded to `cargo check` (default: --workspace)",
    )
    args = ap.parse_args()

    if args.update_baseline:
        sys.exit(cmd_update_baseline())
    if args.all:
        sys.exit(cmd_all(fmt=args.format))
    if args.cmd == "check":
        sys.exit(cmd_check(args.files, fmt=args.format))
    if args.cmd == "hook":
        sys.exit(cmd_hook(fmt=args.format))
    if args.cmd == "discover":
        sys.exit(cmd_discover())
    if args.cmd == "dylint":
        sys.exit(cmd_dylint(
            args.cargo_args,
            no_deps=args.no_deps,
            keep_going=not args.abort_on_failure,
        ))
    ap.print_help()
    sys.exit(2)


if __name__ == "__main__":
    main()
