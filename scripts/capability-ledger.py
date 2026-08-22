#!/usr/bin/env python3
"""The capability ledger: works-but-undeclared findings, as counted files.

A tightening prompt says the format is BETTER than its profile admits. Per CV-C
that is never a failure — failing the suite because a format over-delivers
would be red for good news, and it depends on whether the run happened to
generate that construct. So prompts are recorded, counted, and reviewed
periodically, exactly like `scripts/bugfunnel.py`.

Three commands, and the split between them is the point:

  counts              derive the distribution from the entry files
  sync --from REPORT  materialize missing entries — A HUMAN RUNS THIS
  diff --from REPORT  print what the run saw that the ledger lacks; ALWAYS
                      exits 0, so a gate can call it without ever blocking

The certifier itself writes only `target/capability-certification/*.json`. It
never writes under `docs/`: a proptest run creating files in the source tree
dirties the working copy nondeterministically, which is a known way to
contaminate a commit.
"""

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ENTRIES = ROOT / "docs" / "Testing" / "capability-ledger" / "entries"
STATUSES = ("OPEN", "PROMOTED", "ACCEPTED", "WONTFIX")

FRONTMATTER = re.compile(r"\A---\n(.*?)\n---\n", re.S)


def slug(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def key(rec):
    """The de-duplication key: one entry per (profile, axis, leg, construct).

    Matched on the TUPLE, not on the filename slug: an entry a human wrote by
    hand names its finding in prose, and forcing its id to equal a derived slug
    would make hand-written and generated entries silently fail to match — the
    ledger would then re-materialize a finding somebody had already triaged.
    """
    return (
        slug(rec.get("profile", "")),
        slug(rec.get("axis", "")),
        slug(rec.get("leg", "")),
        slug(rec.get("construct", "")),
    )


def entry_id(rec):
    return "-".join(part for part in key(rec) if part)


def read_entries():
    out = []
    if not ENTRIES.is_dir():
        return out
    for path in sorted(ENTRIES.glob("*.md")):
        text = path.read_text(encoding="utf-8")
        m = FRONTMATTER.match(text)
        if not m:
            out.append({"path": path, "problem": "no frontmatter"})
            continue
        fields = {}
        for line in m.group(1).splitlines():
            if ":" in line and not line.startswith(" "):
                k, _, v = line.partition(":")
                fields[k.strip()] = v.strip().strip("'\"")
        fields["path"] = path
        out.append(fields)
    return out


def load_report(path):
    doc = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    return doc.get("tightening_prompts", [])


def cmd_counts(_):
    entries = read_entries()
    problems = [e for e in entries if e.get("problem") or not e.get("status")]
    if problems:
        for e in problems:
            print(f"invalid entry: {e['path'].name}", file=sys.stderr)
        return 1
    by_status = {}
    by_profile = {}
    for e in entries:
        by_status[e["status"]] = by_status.get(e["status"], 0) + 1
        by_profile[e.get("profile", "?")] = by_profile.get(e.get("profile", "?"), 0) + 1
    for profile in sorted(by_profile):
        print(f"{profile}: {by_profile[profile]}")
    print(f"TOTAL: {len(entries)}")
    for status in STATUSES:
        if by_status.get(status):
            print(f"  {status}: {by_status[status]}")
    return 0


def missing(report_path):
    known = {key(e) for e in read_entries()}
    return [r for r in load_report(report_path) if key(r) not in known]


def cmd_diff(args):
    """NON-BLOCKING by contract — always exits 0.

    CV-C rules that a tightening prompt never blocks. A gate that failed on an
    unsynced prompt would smuggle the rejected "fail the gate" option back in
    through the ledger.
    """
    gaps = missing(args.from_report)
    if not gaps:
        print("capability-ledger: every tightening prompt in the report has an entry")
        return 0
    print(f"capability-ledger: {len(gaps)} prompt(s) with no ledger entry (NOT a failure):")
    for r in gaps:
        print(f"  {entry_id(r)} — {r['note']}")
    print("run `scripts/capability-ledger.py sync --from <report>` to materialize them")
    return 0


def cmd_sync(args):
    gaps = missing(args.from_report)
    if not gaps:
        print("capability-ledger: nothing to sync")
        return 0
    ENTRIES.mkdir(parents=True, exist_ok=True)
    for r in gaps:
        eid = entry_id(r)
        path = ENTRIES / f"{eid}.md"
        path.write_text(
            "---\n"
            f"id: {eid}\n"
            f"date: {args.date}\n"
            f"profile: {r['profile']}\n"
            f"axis: {r['axis']}\n"
            f"leg: {r['leg']}\n"
            f"construct: {r['construct']}\n"
            "status: OPEN\n"
            "summary: >-\n"
            f"  {r['note']}\n"
            "---\n\n"
            "## What the certifier saw\n\n"
            f"{r['note']}\n\n"
            "## What it prompts\n\n"
            "TODO (human): decide whether to PROMOTE the construct into the profile\n"
            "(the format carries it, so the declaration was too narrow) or to record\n"
            "why the narrower declaration is deliberate.\n",
            encoding="utf-8",
        )
        print(f"wrote {path.relative_to(ROOT)}")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)
    sub.add_parser("counts").set_defaults(func=cmd_counts)
    for name, fn in (("sync", cmd_sync), ("diff", cmd_diff)):
        sp = sub.add_parser(name)
        sp.add_argument("--from", dest="from_report", required=True)
        sp.add_argument("--date", default="2026-08-22")
        sp.set_defaults(func=fn)
    args = p.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
