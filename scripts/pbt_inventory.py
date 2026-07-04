#!/usr/bin/env python3
"""PBT suite inventory: extract @pbt annotations + mechanical facts -> YAML,
then render a markdown table view and a mermaid diagram FROM the YAML.

Annotations carry only what code cannot tell us (oracle class, covered bug
class, driver rung, generator distribution notes, teeth statement). Everything
mechanical — invariant ids, Ref*/Sut* capability requirements, caps provided by
SUT arms, env/feature gates — is extracted from the source against a capability
universe scanned from `pub trait Ref*/Sut*` declarations.

Usage:
  python3 scripts/pbt_inventory.py extract   # -> docs/Testing/pbt-inventory.yaml
  python3 scripts/pbt_inventory.py render    # yaml -> docs/Testing/pbt-inventory.md
  python3 scripts/pbt_inventory.py all       # both

Format spec: see docs/Testing/PBT-Audit-2026-07-16.md ("Annotation format").
"""

import re
import sys
from pathlib import Path

import yaml

REPO = Path(__file__).resolve().parent.parent
SCAN_ROOTS = [
    "crates/holon-integration-tests/src",
    "crates/holon-integration-tests/tests",
    "crates/holon-pbt-core/src",
    "crates/holon-loro-testing/src",
    "crates/holon-block-roundtrip-testing/src",
    "crates/holon-turso-testing/src",
]
YAML_OUT = REPO / "docs/Testing/pbt-inventory.yaml"
MD_OUT = REPO / "docs/Testing/pbt-inventory.md"

ANN_RE = re.compile(r"^\s*//[/!]?\s*@pbt\s+(\S+)\s*(.*)$")
CONT_RE = re.compile(r"^\s*//[/!]\s{2,}(\S.*)$")
NEEDS_RE = re.compile(r"CapId::of::<dyn\s+(\w+)>")
TRAIT_DECL_RE = re.compile(r"\bpub\s+trait\s+((?:Ref|Sut)[A-Z]\w*)")
IMPL_RE = re.compile(r"\bimpl(?:<[^>]*>)?\s+((?:Ref|Sut)[A-Z]\w*)(?:<[^>]*>)?\s+for\s+(\w+)")
INV_ID_RE = re.compile(r'InvariantId\("([^"]+)"\)')
ENV_RE = re.compile(r'"(HOLON_[A-Z0-9_]+)"')
FEATURE_RE = re.compile(r'feature\s*=\s*"([^"]+)"')
CAP_MENTION_RE = re.compile(r"\b((?:Ref|Sut)[A-Z]\w*)\b")

MULTI_KEYS = {"covers", "overlaps", "gen", "oracle", "slips-if-removed"}
SINGLE_KEYS = {"rung", "kind"}

ORACLE_VOCAB = {
    "correspondence", "construction", "metamorphic", "convergence",
    "internal-consistency", "sut-internal", "budget", "roundtrip",
}
RUNG_VOCAB = {"ui", "input-pipeline", "intent", "dispatch", "mcp", "external"}


def rs_files():
    for root in SCAN_ROOTS:
        yield from sorted((REPO / root).rglob("*.rs"))


def build_cap_universe():
    caps = set()
    for f in rs_files():
        caps.update(TRAIT_DECL_RE.findall(f.read_text(errors="replace")))
    return caps


def infer_kind(rel: str) -> str:
    if "/composed/invariants/" in rel:
        return "invariant-wiring"
    if "/invariants/bodies/" in rel or rel.endswith("invariants.rs"):
        return "invariant"
    if "/transitions/" in rel:
        return "transition"
    if rel.endswith("components.rs") or "/sut_" in rel or rel.endswith("component.rs"):
        return "sut-arm"
    if "/ref_caps/" in rel or "reference_state" in rel or "ref_ext" in rel:
        return "ref"
    if "generators" in rel or "content_generators" in rel:
        return "generator"
    if rel.startswith("crates/holon-integration-tests/tests/"):
        return "harness"
    return "module"


def parse_annotations(text: str):
    ann = {k: [] for k in MULTI_KEYS}
    problems = []
    last = None  # (key, index-or-None) for continuation lines
    for line in text.splitlines():
        m = ANN_RE.match(line)
        if m:
            key, val = m.group(1), m.group(2).strip()
            if key in MULTI_KEYS:
                ann[key].append(val)
                last = (key, len(ann[key]) - 1)
            elif key in SINGLE_KEYS:
                if ann.get(key):
                    problems.append(f"duplicate @pbt {key}")
                ann[key] = val
                last = (key, None)
            else:
                problems.append(f"unknown @pbt key '{key}'")
                last = None
            continue
        c = CONT_RE.match(line)
        if c and last:
            key, idx = last
            if idx is None:
                ann[key] += " " + c.group(1)
            else:
                ann[key][idx] += " " + c.group(1)
        else:
            last = None
    for o in ann.get("oracle", []):
        if o.split()[0] not in ORACLE_VOCAB:
            problems.append(f"oracle '{o}' not in vocab {sorted(ORACLE_VOCAB)}")
    if ann.get("rung") and ann["rung"].split()[0] not in RUNG_VOCAB:
        problems.append(f"rung '{ann['rung']}' not in vocab {sorted(RUNG_VOCAB)}")
    ann = {k: v for k, v in ann.items() if v}
    return ann, problems


def extract():
    universe = build_cap_universe()
    records = []
    for f in rs_files():
        rel = str(f.relative_to(REPO))
        text = f.read_text(errors="replace")
        ann, problems = parse_annotations(text)
        # Cap mentions must come from CODE, not doc comments (a stale doc
        # mention of a cap would otherwise fabricate a requirement).
        code = "\n".join(l for l in text.splitlines()
                         if not l.lstrip().startswith("//"))
        mentioned = set(CAP_MENTION_RE.findall(code)) & universe
        provides = sorted({t for t, _ in IMPL_RE.findall(code) if t in universe})
        requires = sorted(mentioned - set(provides))
        kind = ann.get("kind") or infer_kind(rel)
        rec = {
            "file": rel,
            "kind": kind,
            "invariant_ids": sorted(set(INV_ID_RE.findall(code))),
            "requires_ref_caps": [c for c in requires if c.startswith("Ref")],
            "requires_sut_caps": [c for c in requires if c.startswith("Sut")],
            "provides_caps": provides,
            "env_gates": sorted(set(ENV_RE.findall(text))),
            "feature_gates": sorted(set(FEATURE_RE.findall(text))),
            "uses_cap_transition_macro": "cap_transition!" in text,
        }
        if kind == "invariant-wiring":
            rec["declared_needs"] = sorted(set(NEEDS_RE.findall(code)))
        rec.update({k: v for k, v in ann.items()})
        if problems:
            rec["annotation_problems"] = problems
        # Keep the inventory signal-dense: skip modules with no caps, no
        # annotations, no gates (pure plumbing).
        interesting = (
            ann or rec["invariant_ids"] or provides
            or rec["requires_ref_caps"] or rec["requires_sut_caps"]
            or kind in ("invariant", "transition", "sut-arm", "generator", "harness")
        )
        if interesting:
            records.append({k: v for k, v in rec.items() if v not in ([], False, None)})
    out = {
        "generated_by": "scripts/pbt_inventory.py extract",
        "cap_universe": {
            "ref": sorted(c for c in universe if c.startswith("Ref")),
            "sut": sorted(c for c in universe if c.startswith("Sut")),
        },
        "components": records,
    }
    YAML_OUT.parent.mkdir(parents=True, exist_ok=True)
    YAML_OUT.write_text(yaml.safe_dump(out, sort_keys=False, width=100,
                                       allow_unicode=True))
    n_ann = sum(1 for r in records if "oracle" in r or "covers" in r or "rung" in r)
    print(f"extracted {len(records)} components ({n_ann} annotated) -> {YAML_OUT}")
    probs = [(r["file"], p) for r in records for p in r.get("annotation_problems", [])]
    for f, p in probs:
        print(f"  WARN {f}: {p}", file=sys.stderr)


def short(path: str) -> str:
    return Path(path).name


def md_table(rows, headers):
    out = ["| " + " | ".join(headers) + " |", "|" + "---|" * len(headers)]
    for r in rows:
        out.append("| " + " | ".join(str(c).replace("|", "\\|") for c in r) + " |")
    return "\n".join(out)


def render():
    data = yaml.safe_load(YAML_OUT.read_text())
    comps = data["components"]
    by_kind = {}
    for c in comps:
        by_kind.setdefault(c["kind"], []).append(c)

    lines = ["# PBT suite inventory (generated — do not edit)",
             "", f"Source: `{YAML_OUT.relative_to(REPO)}`; regenerate with "
             "`python3 scripts/pbt_inventory.py all`.", ""]

    invs = by_kind.get("invariant", [])
    lines += ["## Invariants", ""]
    rows = []
    for c in invs:
        ids = ", ".join(c.get("invariant_ids", [])) or short(c["file"])
        rows.append([
            ids,
            "; ".join(c.get("oracle", [])) or "**UNANNOTATED**",
            " ".join(c.get("requires_ref_caps", [])) or "—",
            " ".join(c.get("requires_sut_caps", [])) or "—",
            " ".join(c.get("env_gates", []) + c.get("feature_gates", [])) or "—",
            "; ".join(c.get("covers", [])) or "—",
        ])
    lines += [md_table(rows, ["invariant", "oracle", "ref caps", "sut caps",
                              "gates", "covers"]), ""]

    trans = by_kind.get("transition", [])
    lines += ["## Transitions", ""]
    rows = []
    for c in trans:
        rows.append([
            short(c["file"]).removesuffix(".rs"),
            c.get("rung", "**UNANNOTATED**"),
            " ".join(c.get("requires_ref_caps", [])) or "—",
            " ".join(c.get("requires_sut_caps", [])) or "—",
            "yes" if c.get("uses_cap_transition_macro") else "no",
        ])
    lines += [md_table(rows, ["transition", "rung", "ref caps", "sut caps",
                              "cap_transition!"]), ""]

    arms = by_kind.get("sut-arm", [])
    lines += ["## SUT arms (capability providers)", ""]
    rows = [[c["file"], " ".join(c.get("provides_caps", [])) or "—"] for c in arms]
    lines += [md_table(rows, ["arm", "provides"]), ""]

    # Overlap: invariants grouped by identical SUT-cap footprint.
    lines += ["## Overlap candidates (identical SUT-cap footprint)", ""]
    foot = {}
    for c in invs:
        foot.setdefault(tuple(c.get("requires_sut_caps", [])), []).append(
            ", ".join(c.get("invariant_ids", [])) or short(c["file"]))
    any_overlap = False
    for fp, members in sorted(foot.items()):
        if len(members) > 1:
            any_overlap = True
            lines.append(f"- `{' '.join(fp) or '(none)'}`: {', '.join(members)}")
    if not any_overlap:
        lines.append("- none")
    lines.append("")

    # Drift check: wiring shim's declared Needs vs body's actual cap bounds.
    lines += ["## Wiring-vs-body drift (declared Needs ⊉ body cap bounds)", ""]
    bodies = {Path(c["file"]).stem: c for c in invs}
    drift = False
    for w in by_kind.get("invariant-wiring", []):
        b = bodies.get(Path(w["file"]).stem)
        if not b:
            continue
        body_caps = set(b.get("requires_sut_caps", []) + b.get("requires_ref_caps", []))
        declared = set(w.get("declared_needs", []))
        missing = body_caps - declared
        if missing:
            drift = True
            lines.append(f"- `{Path(w['file']).stem}`: body binds "
                         f"`{' '.join(sorted(missing))}` not in wiring Needs")
    if not drift:
        lines.append("- none detected (name-matched pairs only)")
    lines.append("")

    # Mermaid: invariant -> sut cap -> providing arm.
    lines += ["## Capability graph", "", "```mermaid", "flowchart LR"]
    cap_nodes, arm_nodes = set(), set()
    provider_of = {}
    for a in arms:
        for cap in a.get("provides_caps", []):
            provider_of.setdefault(cap, []).append(short(a["file"]) + ":" +
                                                   Path(a["file"]).parent.name)
    for c in invs:
        iid = (c.get("invariant_ids") or [short(c["file"])])[0]
        node = re.sub(r"[^A-Za-z0-9]", "_", iid)
        lines.append(f'  {node}(["{iid}"])')
        for cap in c.get("requires_sut_caps", []):
            if cap not in cap_nodes:
                cap_nodes.add(cap)
                lines.append(f"  {cap}[{cap}]")
            lines.append(f"  {node} --> {cap}")
    for cap in sorted(cap_nodes):
        for arm in provider_of.get(cap, []):
            an = re.sub(r"[^A-Za-z0-9]", "_", arm)
            if an not in arm_nodes:
                arm_nodes.add(an)
                lines.append(f'  {an}[/"{arm}"/]')
            lines.append(f"  {cap} -.-> {an}")
    lines += ["```", ""]

    MD_OUT.write_text("\n".join(lines))
    print(f"rendered -> {MD_OUT}")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "all"
    if cmd in ("extract", "all"):
        extract()
    if cmd in ("render", "all"):
        render()
