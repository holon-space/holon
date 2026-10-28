#!/usr/bin/env python3
"""Rewrite downstream paths into the holon-api satellite crates.

The condition modules, `user_transition` and `storage_provider` live in
`holon-conditions`, `holon-transition-contract` and `holon-storage-provider`,
beside holon-api. This rewrites every
`holon_api::` path that names a moved item and adds the dependency line the
rewritten crate now needs (and the out-of-workspace lockfiles that follow).

Idempotent: a second run changes nothing. It never touches holon-api itself or
the satellite crates.

  rewrite-holon-api-satellite-paths.py [REPO_ROOT]
"""

import re
import subprocess
import sys
from pathlib import Path

CONDITIONS = "holon_conditions"
TRANSITION = "holon_transition_contract"
STORAGE = "holon_storage_provider"

# Names holon-api re-exported at its root from the condition modules.
CONDITION_ROOT_NAMES = (
    "Condition ConditionBus ConditionChange ConditionKey ConditionKind "
    "ConditionSubscription DatabaseHold DerivedFieldSeat DroppedPanics EarlierPanic "
    "EarlierPanics HolonChange IngestRefusals QuarantinedTable RefusedFile AllClear "
    "ClearingEvent ConditionPlacement ConditionProfile ConditionSeverity OpId "
    "RemedySlot SettingsSection"
).split()

REWRITES = [
    (
        re.compile(r"\bholon_api::(condition_(?:bus|profile|detail|source))\b"),
        CONDITIONS + r"::\1",
    ),
    (
        re.compile(r"\bholon_api::(" + "|".join(CONDITION_ROOT_NAMES) + r")\b"),
        CONDITIONS + r"::\1",
    ),
    (re.compile(r"\bholon_api::user_transition::"), TRANSITION + "::"),
    (re.compile(r"\bholon_api::user_transition\b"), TRANSITION),
    (re.compile(r"\bholon_api::storage_provider::"), STORAGE + "::"),
    (re.compile(r"\bholon_api::storage_provider\b"), STORAGE),
]

NEW_CRATES = {
    CONDITIONS: "holon-conditions",
    TRANSITION: "holon-transition-contract",
    STORAGE: "holon-storage-provider",
}

SKIP_DIRS = {"target", ".jj", ".git", "node_modules", ".resolve-scratch"}
OWN_CRATES = {"holon-api", *NEW_CRATES.values()}
NORMAL_DIRS = {"src", "build.rs"}


def source_files(root: Path):
    for path in root.rglob("*.rs"):
        rel = path.relative_to(root)
        if SKIP_DIRS & set(rel.parts):
            continue
        if len(rel.parts) > 1 and rel.parts[0] == "crates" and rel.parts[1] in OWN_CRATES:
            continue
        yield path


def manifest_of(path: Path, root: Path) -> Path:
    for parent in path.parents:
        if (parent / "Cargo.toml").is_file():
            return parent / "Cargo.toml"
        if parent == root:
            break
    raise SystemExit(f"{path}: no Cargo.toml above it")


def is_proc_macro(manifest_text: str) -> bool:
    return re.search(r"^proc-macro\s*=\s*true", manifest_text, re.M) is not None


def sections(text: str):
    """(header, start, end) of every `[...]` and `[[...]]` table, end exclusive, by line index."""
    lines = text.splitlines(keepends=True)
    heads = [i for i, line in enumerate(lines) if re.match(r"^\[\[?[^\]]+\]\]?\s*$", line)]
    return lines, [
        (lines[h].strip(), h, heads[k + 1] if k + 1 < len(heads) else len(lines))
        for k, h in enumerate(heads)
    ]


def declared_in(lines, start, end, crate):
    return any(re.match(rf"^{re.escape(crate)}\s*(\.|=)", lines[i]) for i in range(start + 1, end))


def dependency_line(api_line: str, crate: str, manifest: Path, root: Path) -> str:
    optional = "optional = true" in api_line
    if "workspace" in api_line.split("#")[0]:
        if optional:
            return f"{crate} = {{ workspace = true, optional = true }}\n"
        return f"{crate}.workspace = true\n"
    rel = Path(*[".."] * len(manifest.parent.relative_to(root).parts)) / "crates" / crate
    assert not optional, f"{manifest}: an optional path dependency on holon-api is not handled"
    return f'{crate} = {{ path = "{rel.as_posix()}" }}\n'


def add_dependency(manifest: Path, root: Path, crate: str, dev: bool) -> bool:
    text = manifest.read_text()
    lines, tables = sections(text)
    normal = [t for t in tables if t[0] == "[dependencies]" or re.match(r"^\[target\..*\.dependencies\]$", t[0])]
    devs = [t for t in tables if t[0] == "[dev-dependencies]"]
    if any(declared_in(lines, s, e, crate) for _, s, e in normal + (devs if dev else [])):
        return False
    targets = devs if dev else normal
    api = [(s, e, i) for _, s, e in targets for i in range(s + 1, e) if re.match(r"^holon-api\s*(\.|=)", lines[i])]
    if api:
        _, _, at = api[0]
        new = dependency_line(lines[at], crate, manifest, root)
        lines.insert(at + 1, new)
        if "optional = true" in new:
            dep_api = [i for i, line in enumerate(lines) if line.strip() == '"dep:holon-api",']
            assert len(dep_api) == 1, f"{manifest}: expected one `\"dep:holon-api\",` feature line"
            indent = lines[dep_api[0]][: len(lines[dep_api[0]]) - len(lines[dep_api[0]].lstrip())]
            lines.insert(dep_api[0] + 1, f'{indent}"dep:{crate}",\n')
    else:
        any_api = next(
            (line for _, s, e in tables for line in lines[s + 1 : e] if re.match(r"^holon-api\s*(\.|=)", line)),
            None,
        )
        assert any_api is not None, f"{manifest}: names a moved holon-api item but declares no holon-api"
        new = dependency_line(any_api.replace("optional = true", ""), crate, manifest, root)
        new = new.replace("{ workspace = true,  }", "{ workspace = true }").replace(", }", " }")
        header = "[dev-dependencies]" if dev else "[dependencies]"
        found = devs if dev else [t for t in tables if t[0] == "[dependencies]"]
        if found:
            _, _, end = found[0]
            while end > 0 and lines[end - 1].strip() == "":
                end -= 1
            lines.insert(end, new)
        else:
            if lines and not lines[-1].endswith("\n"):
                lines[-1] += "\n"
            lines.extend(["\n", f"{header}\n", new])
    manifest.write_text("".join(lines))
    return True


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    needs: dict[Path, dict[str, bool]] = {}
    changed = 0
    for path in source_files(root):
        text = path.read_text()
        new = text
        for pattern, replacement in REWRITES:
            new = pattern.sub(replacement, new)
        if new != text:
            path.write_text(new)
            changed += 1
        used = [c for c in NEW_CRATES if re.search(rf"\b{c}\b", new)]
        if not used:
            continue
        manifest = manifest_of(path, root)
        rel_parts = path.relative_to(manifest.parent).parts
        dev = rel_parts[0] not in NORMAL_DIRS or is_proc_macro(manifest.read_text())
        entry = needs.setdefault(manifest, {})
        for c in used:
            entry[c] = entry.get(c, True) and dev
    touched = []
    for manifest, crates in sorted(needs.items()):
        for c, dev in sorted(crates.items()):
            if add_dependency(manifest, root, NEW_CRATES[c], dev):
                touched.append(manifest)
    # An out-of-workspace lockfile lists the satellites once any crate it
    # builds depends on one.
    for lock in sorted(root.glob("*/*/Cargo.lock")):
        if 'name = "holon-api"' in lock.read_text():
            subprocess.run(
                ["cargo", "metadata", "--offline", "--format-version", "1"],
                cwd=lock.parent,
                check=True,
                stdout=subprocess.DEVNULL,
            )
    print(f"rewrote {changed} files; dependency lines in {len(set(touched))} manifests")
    return 0


if __name__ == "__main__":
    sys.exit(main())
