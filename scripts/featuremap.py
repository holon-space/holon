#!/usr/bin/env python3
"""Generate and check `docs/Architecture/FeatureMap.md`.

The map is an overlay (`docs/Architecture/featuremap.yaml`, hand-edited: the
grouping into areas, the one-clause descriptions, the ADR links) rendered
against the repo's own machine-readable pin sources, so a pin list can never
disagree with the code that declares it.

  featuremap.py generate    write docs/Architecture/FeatureMap.md
  featuremap.py check       exit non-zero when the file differs from a fresh render

Stdlib only, and that includes YAML: `python3` on PATH here has no PyYAML, so
the overlay is read by the strict subset parser below.
"""

import argparse
import difflib
import re
import sys
from pathlib import Path

TRANSITIONS_MOD = Path("crates/holon-integration-tests/src/pbt/transitions/mod.rs")
KNOWN_REDS = Path("docs/Testing/KeystoneKnownReds.md")
OVERLAY = Path("docs/Architecture/featuremap.yaml")
TARGET = Path("docs/Architecture/FeatureMap.md")

# `entry` / `ruled by` cells name real files; a backticked token starting with
# one of these is checked against the working tree.
REPO_ROOTS = ("crates/", "frontends/", "assets/", "docs/", "scripts/", "experiments/", "tools/")

# `capability_pair!`'s docs spell their placeholder `inv-…`; ASCII-only keeps it
# out of the id universe.
ID = r"inv-[a-z0-9][a-z0-9_/-]*"


class SourceError(Exception):
    """A pin source could not be read the one way this script understands it."""


class OverlayError(Exception):
    """The overlay is malformed, or claims something the sources do not have."""


# ─────────────────────────── overlay format ───────────────────────────

def parse_overlay(text, path):
    """Parse the overlay's YAML subset: block mappings, `- ` sequences, flow
    sequences (`[a, b]`), plain and double-quoted scalars, and `>` folded
    scalars. Anything else raises — the overlay is authored against this
    parser, not against YAML at large.
    """
    lines = []
    for number, raw in enumerate(text.splitlines(), 1):
        if "\t" in raw:
            raise OverlayError(f"{path}:{number}: tab indentation")
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        lines.append((number, raw))
    if not lines:
        raise OverlayError(f"{path}: empty overlay")
    value, index = _parse_node(lines, 0, _indent(lines[0][1]), path)
    if index != len(lines):
        number = lines[index][0]
        raise OverlayError(f"{path}:{number}: unindented content after the document")
    return value


def _indent(raw):
    return len(raw) - len(raw.lstrip(" "))


def _parse_node(lines, index, indent, path):
    if lines[index][1].lstrip().startswith("- "):
        return _parse_sequence(lines, index, indent, path)
    return _parse_mapping(lines, index, indent, path)


def _parse_sequence(lines, index, indent, path):
    items = []
    while index < len(lines) and _indent(lines[index][1]) == indent:
        number, raw = lines[index]
        body = raw.lstrip()
        if not body.startswith("- "):
            raise OverlayError(f"{path}:{number}: expected a `- ` item at indent {indent}")
        # The item's own content starts where `- ` ends, so a mapping opened on
        # the dash line continues at that column.
        item_indent = indent + 2
        lines[index] = (number, " " * item_indent + body[2:])
        item, index = _parse_node(lines, index, item_indent, path)
        items.append(item)
    return items, index


def _parse_mapping(lines, index, indent, path):
    mapping = {}
    while index < len(lines) and _indent(lines[index][1]) == indent:
        number, raw = lines[index]
        body = raw.strip()
        if ":" not in body:
            raise OverlayError(f"{path}:{number}: expected `key: value`, got {body!r}")
        key, _, rest = body.partition(":")
        key = key.strip()
        rest = rest.strip()
        if key in mapping:
            raise OverlayError(f"{path}:{number}: duplicate key `{key}`")
        index += 1
        if rest in (">", "|"):
            mapping[key], index = _parse_block_scalar(lines, index, indent, path, rest)
        elif rest:
            mapping[key] = _parse_scalar(rest, number, path)
        else:
            if index >= len(lines) or _indent(lines[index][1]) <= indent:
                raise OverlayError(f"{path}:{number}: key `{key}` has no value")
            mapping[key], index = _parse_node(lines, index, _indent(lines[index][1]), path)
    return mapping, index


def _parse_block_scalar(lines, index, indent, path, style):
    """`>` folds the more-indented lines into one (table cells and paragraphs
    reflow); `|` keeps them, for the bullet lists that render as markdown.
    """
    start = index
    parts = []
    while index < len(lines) and _indent(lines[index][1]) > indent:
        parts.append(lines[index][1])
        index += 1
    if not parts:
        number = lines[start - 1][0] if start else 0
        raise OverlayError(f"{path}:{number}: `{style}` with no indented lines")
    if style == ">":
        return " ".join(part.strip() for part in parts), index
    margin = min(_indent(part) for part in parts)
    return "\n".join(part[margin:] for part in parts), index


def _parse_scalar(text, number, path):
    if text.startswith("["):
        if not text.endswith("]"):
            raise OverlayError(f"{path}:{number}: flow sequence must close on its own line")
        inner = text[1:-1].strip()
        if not inner:
            return []
        return [_parse_scalar(item.strip(), number, path) for item in inner.split(",")]
    if text.startswith('"'):
        if not text.endswith('"') or len(text) < 2:
            raise OverlayError(f"{path}:{number}: unterminated quoted scalar")
        return text[1:-1].replace('\\"', '"').replace("\\\\", "\\")
    if text.startswith(("|", "&", "*", "!", "{")):
        raise OverlayError(f"{path}:{number}: unsupported YAML construct in {text!r}")
    return text


# ─────────────────────────── pin sources ───────────────────────────

def scan_transitions(root):
    """Variant names of the `declare_e2e_transitions!` enum, in declaration
    order, each with the `@pbt covers` clause of the file that defines it.
    """
    source = (root / TRANSITIONS_MOD).read_text()
    after = source.split("crate::declare_e2e_transitions!", 1)
    if len(after) != 2:
        raise SourceError(f"{TRANSITIONS_MOD}: no `crate::declare_e2e_transitions!` invocation")
    body = _macro_enum_body(after[1], TRANSITIONS_MOD)
    variants = re.findall(r"^\s*([A-Z][A-Za-z0-9]*)\(", body, re.MULTILINE)
    if not variants:
        raise SourceError(f"{TRANSITIONS_MOD}: the macro body declares no variants")
    clauses = _clauses_by_snake_name(root)
    return {name: clauses.get(_snake(name), "") for name in variants}


def _macro_enum_body(after_marker, where):
    """The enum body between the macro's `{` and the enum's matching `}`."""
    depth = 0
    start = None
    for index, char in enumerate(after_marker):
        if char == "{":
            depth += 1
            if depth == 2:
                start = index + 1
        elif char == "}":
            if depth == 2:
                return after_marker[start:index]
            depth -= 1
    raise SourceError(f"{where}: the macro body is unbalanced")


def _snake(name):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def _clauses_by_snake_name(root):
    clauses = {}
    for path in _rust_files(root):
        clause = _covers_clause(path.read_text())
        if clause:
            clauses.setdefault(path.stem, clause)
    return clauses


def _covers_clause(source):
    """The prose after `@pbt covers <slug> — `, folded across `//!` / `///`
    continuations and stopping at the next `@pbt` marker.
    """
    match = re.search(r"@pbt covers\s+(.*)", source)
    if not match:
        return ""
    text = match.group(1)
    remainder = source[match.end():]
    for line in remainder.splitlines()[1:]:
        body = line.strip()
        if not body.startswith(("//!", "///")):
            break
        body = body[3:].strip()
        if not body or body.startswith("@pbt"):
            break
        text += " " + body
    text = text.split("@pbt")[0].strip()
    clause = re.split(r"—|\s--\s", text, maxsplit=1)[-1] if re.search(r"—|\s--\s", text) else ""
    return " ".join(clause.split())


def scan_invariants(root):
    """Every invariant id the repo declares, split by how it is declared:
    `InvariantId("…")` bodies and correspondence-family `id: "…"` entries.
    """
    bodies = {}
    families = {}
    for path in _rust_files(root):
        source = path.read_text()
        # Three declaration forms: an invariant body's `InvariantId`, a
        # `capability_pair!` id override, and a correspondence family's entry.
        found = (re.findall(rf'InvariantId\("({ID})"\)', source)
                 + re.findall(rf'id\s*=\s*"({ID})"', source),
                 re.findall(rf'id: "({ID})"', source))
        # A file's `@pbt covers` clause describes what that file declares, so it
        # only speaks for an id when the file declares one or two of them; the
        # correspondence registry declares a dozen and describes none of them.
        clause = _covers_clause(source) if len(set(found[0]) | set(found[1])) <= 2 else ""
        for name in found[0]:
            if not bodies.get(name):
                bodies[name] = clause
        for name in found[1]:
            if not families.get(name):
                families[name] = clause
    if not bodies:
        raise SourceError("no `InvariantId(\"inv-…\")` declarations found under crates/")
    return bodies, families


def _rust_files(root):
    return sorted((root / "crates").rglob("*.rs"))


def scan_known_reds(root):
    """Key → status for every row of every registry table in the known-reds
    doc. The `Key` column is the nightly's classification handle.
    """
    reds = {}
    for line in (root / KNOWN_REDS).read_text().splitlines():
        match = re.match(r"^\|\s*`([a-z0-9-]+)`\s*\|\s*([a-z-]+)\s*\|", line)
        if match:
            reds[match.group(1)] = match.group(2)
    if not reds:
        raise SourceError(f"{KNOWN_REDS}: no registry rows matched the `| `key` | status |` shape")
    return reds


# ─────────────────────────── link + path checks ───────────────────────────

def check_references(root, where, text, problems):
    for token in re.findall(r"`([^`]+)`", text):
        candidate = token.split()[0].rstrip(",.;:)")
        if candidate.startswith(REPO_ROOTS) and not (root / candidate).exists():
            problems.append(f"{where}: `{candidate}` does not exist")
    for target in re.findall(r"\]\(([^)]+)\)", text):
        if target.startswith(("#", "http://", "https://")):
            continue
        target = target.split("#")[0]
        if target and not (root / TARGET.parent / target).exists():
            problems.append(f"{where}: link target `{target}` does not resolve")


# ─────────────────────────── rendering ───────────────────────────

def render(overlay, sources, root):
    transitions, bodies, families, reds = sources
    invariants = dict(bodies)
    invariants.update(families)
    problems = []
    claimed_transitions = set()
    claimed_invariants = set()
    claimed_reds = set()
    unpinned = []

    out = [f"# {overlay['title']}", "", overlay["generated_banner"], "", overlay["lead"], ""]
    check_references(root, "lead", overlay["lead"], problems)

    for section in overlay["sections"]:
        check_references(root, f"section `{section['heading']}`", section["body"], problems)
        out += [f"## {section['heading']}", "", section["body"], ""]

    out += ["## Legend", "", "| Column | What it holds |", "|---|---|"]
    for column in overlay["legend"]:
        check_references(root, f"legend `{column['column']}`", column["holds"], problems)
        out.append(f"| **{column['column']}** | {column['holds']} |")
    out.append("")

    note = overlay["keystone_note"].format(
        transitions=len(transitions),
        invariants=len(bodies),
        families=len(families),
    )
    check_references(root, "keystone_note", note, problems)
    out += [note, "", "---", ""]

    for area in overlay["areas"]:
        out += [f"## {area['name']}", ""]
        out += ["| Feature | What it is | Pinned by | Ruled by | Mode axes | Key entry point |",
                "|---|---|---|---|---|---|"]
        for row in area["rows"]:
            where = f"area `{area['name']}` row `{row['feature']}`"
            row_transitions = row.get("transitions", [])
            row_invariants = row.get("invariants", [])
            for name in row_transitions:
                if name not in transitions:
                    problems.append(f"{where}: transition `{name}` is not in the keystone alphabet")
                claimed_transitions.add(name)
            for name in row_invariants:
                if name not in invariants:
                    problems.append(f"{where}: invariant `{name}` is declared nowhere under crates/")
                claimed_invariants.add(name)
            if not row_transitions and not row_invariants:
                if "unpinned" not in row and "unpinned_exempt" not in row:
                    problems.append(
                        f"{where}: has no pins and declares neither `unpinned` nor `unpinned_exempt`")
            if "unpinned" in row:
                unpinned.append((row["feature"], row["unpinned"]))
            for cell in ("what", "ruled_by", "mode_axes", "entry", "pin_note"):
                if cell in row:
                    check_references(root, where, row[cell], problems)
            out.append("| {} | {} | {} | {} | {} | {} |".format(
                row["feature"], row["what"], _pin_cell(row, row_transitions, row_invariants),
                row["ruled_by"], row["mode_axes"], row["entry"]))
        out.append("")

        open_reds = []
        for key in area.get("open_reds", []):
            if key not in reds:
                problems.append(f"area `{area['name']}`: known red `{key}` is not in the registry")
                continue
            claimed_reds.add(key)
            if reds[key] == "known-red":
                open_reds.append(key)
        if open_reds:
            out += ["Open reds here: " + ", ".join(f"`{key}`" for key in open_reds) + ".", ""]
        if "note" in area:
            check_references(root, f"area `{area['name']}` note", area["note"], problems)
            out += [area["note"], ""]

    out += ["---", "", "## Unpinned features", "", overlay["unpinned_lead"], ""]
    for feature, why in unpinned:
        out.append(f"- **{feature}** — {why}")
    out.append("")

    out += ["## Unclaimed by any row", "", overlay["unclaimed_lead"], ""]
    out += _unclaimed(
        "Transitions", {k: v for k, v in transitions.items() if k not in claimed_transitions})
    out += _unclaimed(
        "Invariant ids", {k: v for k, v in invariants.items() if k not in claimed_invariants})
    loose_reds = sorted(k for k, status in reds.items()
                        if status == "known-red" and k not in claimed_reds)
    out += ["### Known reds", ""]
    out += ([f"- `{key}`" for key in loose_reds] if loose_reds
            else ["Every open known red is attributed to an area."])
    out.append("")

    out += ["## Sources", "", overlay["sources_lead"], "",
            "| Content | Source of truth |", "|---|---|"]
    for entry in overlay["source_table"]:
        check_references(root, f"source `{entry['content']}`", entry["source"], problems)
        out.append(f"| {entry['content']} | {entry['source']} |")
    out += ["", overlay["sources_closing"], ""]

    if problems:
        raise OverlayError("the overlay disagrees with the pin sources:\n  " + "\n  ".join(problems))
    return "\n".join(out).rstrip("\n") + "\n"


def _pin_cell(row, transitions, invariants):
    parts = []
    if transitions:
        parts.append(", ".join(f"`{name}`" for name in transitions))
    if invariants:
        parts.append(", ".join(f"`{name}`" for name in invariants))
    if "pin_note" in row:
        parts.append(row["pin_note"])
    cell = "; ".join(parts)
    return f"{row['pin_prefix']}: {cell}" if "pin_prefix" in row else cell


def _unclaimed(heading, entries):
    out = [f"### {heading}", ""]
    if not entries:
        return out + ["None.", ""]
    for name in sorted(entries):
        clause = entries[name]
        out.append(f"- `{name}`" + (f" — {clause}" if clause else ""))
    return out + [""]


# ─────────────────────────── commands ───────────────────────────

def build(root):
    overlay = parse_overlay((root / OVERLAY).read_text(), OVERLAY)
    bodies, families = scan_invariants(root)
    sources = (scan_transitions(root), bodies, families, scan_known_reds(root))
    return render(overlay, sources, root)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("generate", "check"))
    parser.add_argument("--root", default=".")
    args = parser.parse_args()
    root = Path(args.root).resolve()

    rendered = build(root)
    target = root / TARGET

    if args.command == "generate":
        target.write_text(rendered)
        print(f"wrote {TARGET} ({len(rendered.splitlines())} lines)")
        return 0

    committed = target.read_text() if target.exists() else ""
    if committed == rendered:
        print(f"{TARGET} is up to date")
        return 0
    diff = list(difflib.unified_diff(
        committed.splitlines(), rendered.splitlines(),
        fromfile=f"{TARGET} (committed)", tofile=f"{TARGET} (regenerated)", lineterm=""))
    print(f"{TARGET} has drifted from the overlay + pin sources "
          f"({sum(1 for line in diff if line.startswith(('+', '-')) and line[1:2] != line[0])} "
          "changed lines); run `python3 scripts/featuremap.py generate`", file=sys.stderr)
    print("\n".join(diff[:80]), file=sys.stderr)
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OverlayError, SourceError) as error:
        print(f"featuremap: {error}", file=sys.stderr)
        sys.exit(2)
