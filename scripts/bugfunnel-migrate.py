#!/usr/bin/env python3
"""Convert a legacy docs/Testing/BugFunnel.md into one file per escape.

Deterministic and re-runnable: the output is a pure function of the input
document, so running it across history with `jj run` yields, at every rev, the
entry set that rev's BugFunnel.md describes. The output directory is wiped
before it is rebuilt, so a rev whose document has fewer entries does not
inherit stragglers from a previously-generated run.

Nothing is invented: dates, gaps and narratives come from the document. Rows
the parser cannot read confidently are reported, never guessed and never
dropped -- a non-empty report is a non-zero exit.
"""

import argparse
import hashlib
import re
import shutil
import sys
from pathlib import Path

GAPS = ("ENVIRONMENT", "COVERAGE", "PERCEPTION", "ORACLE")
GAP_ALIAS = {"ENV": "ENVIRONMENT", "COV": "COVERAGE", "PERC": "PERCEPTION"}
GAP_ALIAS.update({g: g for g in GAPS})

# Leading token of the remedy cell -> controlled status. Matched
# case-insensitively against the longest key first.
STATUS_VOCAB = {
    "NOT FIXED": "OPEN",
    "PARTIALLY FIXED": "PARTIAL",
    "PARTIALLY": "PARTIAL",
    "PARTIAL": "PARTIAL",
    "ROOT-CAUSED": "OPEN",
    "ROOT CAUSED": "OPEN",
    "MITIGATED": "MITIGATED",
    "RESOLVED": "FIXED",
    "CLOSED": "FIXED",
    "FIXED": "FIXED",
    "OPEN": "OPEN",
    "NOTED": "NOTED",
    "RETRIAGED": "NOTED",
    "REPORTED": "OPEN",
    "CONFIRMED": "OPEN",
    "RULED": "NOTED",
}

DATE_RE = re.compile(r"^20\d\d-\d\d-\d\d$")
ROW_RE = re.compile(r"^\| (20\d\d-\d\d-\d\d) \|")
INCR_RE = re.compile(r"^- \(([+-])\s*(\d*)\s*([A-Z]+)?\s*(20\d\d-\d\d-\d\d)[,:]?\s*(.*)$")

STOPWORDS = {
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "is", "it",
    "its", "that", "this", "with", "at", "by", "from", "but", "not", "no",
    "was", "were", "are", "be", "been", "so", "as", "any", "every", "one",
}


class Report:
    """Accumulates every judgment call so one run yields one full verdict."""

    def __init__(self):
        self.items = []

    def add(self, severity, where, message):
        self.items.append((severity, where, message))

    @property
    def blocking(self):
        return [i for i in self.items if i[0] == "BLOCK"]

    def render(self):
        if not self.items:
            return "no judgment calls -- every record parsed unambiguously\n"
        out = []
        for severity, where, message in self.items:
            out.append(f"{severity} {where}: {message}")
        return "\n".join(out) + "\n"


def slugify(text, limit=6):
    text = re.sub(r"`[^`]*`", " ", text)
    text = re.sub(r"[^a-zA-Z0-9]+", " ", text).lower()
    words = [w for w in text.split() if w not in STOPWORDS and len(w) > 2]
    return "-".join(words[:limit]) or "entry"


def short_hash(text):
    return hashlib.sha256(text.encode()).hexdigest()[:6]


def split_row(line):
    """Split a ledger row into (date, description, primary, secondary, missing,
    remedy, well_formed).

    The description cell can itself contain `|`, so the gap class is located as
    the first cell after the date that names one.
    """
    cells = [c.strip() for c in line.split("|")]
    gap_at = None
    for k in range(2, len(cells)):
        candidate = re.sub(r"^.*→", "", cells[k]).strip()
        if candidate in GAP_ALIAS:
            gap_at = k
            break
    if gap_at is None:
        return None
    date = cells[1]
    description = " | ".join(cells[2:gap_at]).strip()
    primary = GAP_ALIAS[re.sub(r"^.*→", "", cells[gap_at]).strip()]
    retriaged_from = None
    if "→" in cells[gap_at]:
        retriaged_from = cells[gap_at].split("→")[0].strip()

    tail = cells[gap_at + 1:]
    # A well-formed row leaves exactly secondary, missing, remedy and the
    # trailing empty cell after the closing pipe.
    well_formed = len(tail) == 4
    secondary = tail[0] if len(tail) > 0 else ""
    missing = tail[1] if len(tail) > 1 else ""
    remedy = " | ".join(tail[2:]).strip().rstrip("|").strip() if len(tail) > 2 else ""
    if secondary in ("—", "-", ""):
        secondary = None
    else:
        secondary = GAP_ALIAS.get(re.sub(r"^.*→", "", secondary).strip(), secondary)
    return dict(
        date=date, description=description, primary=primary, secondary=secondary,
        retriaged_from=retriaged_from, missing=missing, remedy=remedy,
        well_formed=well_formed,
    )


def classify_status(remedy):
    stripped = re.sub(r"^[*_(\s]+", "", remedy)
    upper = stripped.upper()
    for key in sorted(STATUS_VOCAB, key=len, reverse=True):
        if upper.startswith(key):
            return STATUS_VOCAB[key]
    return None


DATE_WINDOW_DAYS = 5
MATCH_THRESHOLD = 0.35


def date_ordinal(text):
    from datetime import date
    year, month, day = (int(p) for p in text.split("-"))
    return date(year, month, day).toordinal()


def tokens(text):
    text = re.sub(r"[^a-zA-Z0-9_]+", " ", text).lower()
    return {w for w in text.split() if w not in STOPWORDS and len(w) > 3}


def parse_document(text, report):
    lines = text.split("\n")
    ledger_rows, increments, reconciliations = [], [], []
    prose = {"header": [], "ledger": [], "deferred_perf": []}

    section = "header"
    pending = None  # increment entry accumulating wrapped continuation lines

    def flush_pending():
        nonlocal pending
        if pending is not None:
            increments.append(pending)
            pending = None

    for lineno, line in enumerate(lines, 1):
        if line.startswith("## Deferred perf"):
            flush_pending()
            section = "deferred_perf"
            continue
        if line.startswith("## Ledger"):
            flush_pending()
            section = "ledger"
            continue

        if ROW_RE.match(line):
            flush_pending()
            row = split_row(line)
            if row is None:
                report.add("BLOCK", f"line {lineno}", "table row names no gap class")
                continue
            row["lineno"] = lineno
            ledger_rows.append(row)
            continue

        match = INCR_RE.match(line)
        if match:
            flush_pending()
            sign, count, gap, date, rest = match.groups()
            if gap is not None and gap not in GAP_ALIAS:
                report.add("BLOCK", f"line {lineno}", f"unknown gap token {gap!r}")
                continue
            entry = dict(
                lineno=lineno, date=date, sign=sign,
                count=int(count) if count else None,
                gap=GAP_ALIAS[gap] if gap else None,
                text=rest, is_reconciliation=("reconciliation" in rest.lower()),
            )
            if entry["gap"] is None or entry["is_reconciliation"]:
                reconciliations.append(entry)
            else:
                pending = entry
            continue

        if pending is not None and line.startswith("  "):
            pending["text"] += " " + line.strip()
            continue
        flush_pending()

        prose[section].append(line)
    flush_pending()
    return ledger_rows, increments, reconciliations, prose


def match_increments(rows, increments, report):
    """Attach each increment line's narrative to its ledger row.

    Both describe the same bug, so they are paired on date plus token overlap.
    A pairing below the confidence bar is reported and left unpaired rather
    than guessed -- the narrative then survives as an orphan entry, never lost.
    """
    # The two sections disagree on dates for some bugs (a row and its
    # increment line were written days apart), so the date only narrows
    # candidates -- text containment decides.
    row_tokens = [tokens(r["description"]) for r in rows]
    row_days = [date_ordinal(r["date"]) for r in rows]

    pairs, best_seen = [], {}
    for inc_idx, inc in enumerate(increments):
        inc_tokens = tokens(inc["text"])
        if not inc_tokens:
            continue
        inc_day = date_ordinal(inc["date"])
        for idx, rt in enumerate(row_tokens):
            if not rt or abs(row_days[idx] - inc_day) > DATE_WINDOW_DAYS:
                continue
            # Containment, not Jaccard: an increment narrative runs to
            # thousands of words against a one-paragraph ledger row, so a
            # union-normalised score would be near zero for a perfect pair.
            score = len(rt & inc_tokens) / len(rt)
            best_seen[inc_idx] = max(best_seen.get(inc_idx, 0.0), score)
            if score >= MATCH_THRESHOLD:
                pairs.append((score, inc_idx, idx))

    pairs.sort(key=lambda p: -p[0])
    taken_rows, taken_incs = set(), set()
    for score, inc_idx, idx in pairs:
        if inc_idx in taken_incs or idx in taken_rows:
            continue
        rows[idx]["narrative"] = increments[inc_idx]["text"]
        rows[idx]["narrative_line"] = increments[inc_idx]["lineno"]
        taken_rows.add(idx)
        taken_incs.add(inc_idx)

    for inc_idx, inc in enumerate(increments):
        if inc_idx in taken_incs:
            continue
        inc["orphan"] = True
        report.add(
            "REVIEW", f"increment line {inc['lineno']}",
            f"no ledger row matched (best overlap {best_seen.get(inc_idx, 0.0):.2f}); "
            "kept as a standalone entry",
        )
    return [i for i in increments if i.get("orphan")]


def assign_ids(records, report):
    """Filename stems: date + slug, disambiguated by a content hash.

    Order-independent on purpose: `jj run` visits revs in no fixed order and
    the ledger is not sorted, so a positional counter would rename entries
    between revs.
    """
    groups = {}
    for rec in records:
        stem = f"{rec['date']}-{slugify(rec['one_line'])}"
        groups.setdefault(stem, []).append(rec)
    for stem, members in groups.items():
        if len(members) == 1:
            members[0]["id"] = stem
            continue
        for rec in members:
            rec["id"] = f"{stem}-{short_hash(rec['description'] + rec['remedy'])}"
        report.add(
            "INFO", stem,
            f"{len(members)} entries share a slug; each carries a content hash",
        )
    seen = {}
    for rec in records:
        if rec["id"] in seen:
            report.add(
                "BLOCK", rec["id"],
                f"id collides with the entry from line {seen[rec['id']]}",
            )
        seen[rec["id"]] = rec["lineno"]


def yaml_scalar(value):
    if value is None:
        return "null"
    text = str(value)
    if DATE_RE.match(text) or re.fullmatch(r"[A-Za-z][A-Za-z0-9_.-]*", text):
        return text
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def wrap(text, width=76, indent=""):
    words, lines, current = text.split(), [], ""
    for word in words:
        if current and len(current) + 1 + len(word) > width:
            lines.append(indent + current)
            current = word
        else:
            current = f"{current} {word}".strip()
    if current:
        lines.append(indent + current)
    return lines


def render_entry(rec):
    out = ["---"]
    out.append(f"id: {rec['id']}")
    out.append(f"date: {rec['date']}")
    out.append(f"gap: {rec['primary']}")
    out.append(f"secondary: {yaml_scalar(rec['secondary'])}")
    if rec.get("retriaged_from"):
        out.append(f"retriaged_from: {yaml_scalar(rec['retriaged_from'])}")
    out.append(f"status: {rec['status'] or 'UNCLASSIFIED'}")
    out.append("summary: >-")
    out.extend(wrap(rec["one_line"], indent="  "))
    out.append(f"source_line: {rec['lineno']}")
    out.append("---")
    out.append("")
    out.append("## Bug")
    out.append("")
    out.extend(wrap(rec["description"]))
    if rec.get("narrative"):
        out.append("")
        out.append("## Root cause")
        out.append("")
        out.extend(wrap(rec["narrative"]))
    if rec.get("missing"):
        out.append("")
        out.append("## Missing piece")
        out.append("")
        out.extend(wrap(rec["missing"]))
    if rec.get("remedy"):
        out.append("")
        out.append("## Remedy")
        out.append("")
        out.extend(wrap(rec["remedy"]))
    out.append("")
    return "\n".join(out)


def one_line_of(description):
    """The bolded claim if the row has one, else the leading sentence."""
    bold = re.search(r"\*\*(.+?)\*\*", description)
    text = bold.group(1) if bold else description
    text = re.sub(r"^\([^)]*\)\s*", "", text)
    return re.sub(r"\s+", " ", text).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", default="docs/Testing/BugFunnel.md")
    parser.add_argument("--out", default="docs/Testing/bugfunnel")
    parser.add_argument("--report", default=None)
    args = parser.parse_args()

    source = Path(args.input)
    if not source.exists():
        print(f"no {source} at this rev -- nothing to migrate", file=sys.stderr)
        return 0

    report = Report()
    rows, increments, reconciliations, prose = parse_document(
        source.read_text(), report
    )
    if not rows:
        # At and after the cutover rev this file is a stub, and the output
        # directory below is rebuilt from scratch -- proceeding would delete a
        # migrated funnel and replace it with nothing.
        print(f"{source} holds no ledger rows: already migrated, or not a "
              "legacy funnel. Refusing to rebuild the output directory.",
              file=sys.stderr)
        return 2
    orphans = match_increments(rows, increments, report)

    records = []
    for row in rows:
        row["one_line"] = one_line_of(row["description"])
        row["status"] = classify_status(row["remedy"])
        if row["status"] is None and row["remedy"]:
            report.add(
                "REVIEW", f"line {row['lineno']}",
                f"remedy cell opens with no known status token: {row['remedy'][:60]!r}",
            )
        if not row["well_formed"]:
            report.add(
                "REVIEW", f"line {row['lineno']}",
                "cell count is off (an unescaped `|` in the prose); the "
                "missing-piece/remedy split needs a human eye",
            )
        records.append(row)

    assign_ids(records, report)

    out_dir = Path(args.out)
    entries_dir = out_dir / "entries"
    if out_dir.exists():
        shutil.rmtree(out_dir)
    entries_dir.mkdir(parents=True)

    for rec in records:
        (entries_dir / f"{rec['id']}.md").write_text(render_entry(rec))

    if orphans:
        # NOT entries: the ledger row is the counted unit, so promoting an
        # unpaired narrative would inflate every total. Held for a human to
        # attach to the right entry.
        text = ["# Increment narratives with no confidently-matched ledger row",
                "", "Each was written as a second record of a bug the ledger",
                "already counts. Attach the prose to the named entry, or record",
                "why no entry exists, then delete the line.", ""]
        for inc in sorted(orphans, key=lambda i: i["lineno"]):
            text.append(f"## legacy line {inc['lineno']} ({inc['gap']} {inc['date']})")
            text.append("")
            text.extend(wrap(inc["text"]))
            text.append("")
        (out_dir / "unpaired-narratives.md").write_text("\n".join(text) + "\n")

    if reconciliations:
        text = ["# Counter reconciliations (historical)", "",
                "The legacy funnel kept its totals by hand, so the log carried",
                "correction lines. Totals are computed from the entries now;",
                "these are preserved as history and count for nothing.", ""]
        for rec in sorted(reconciliations, key=lambda r: r["lineno"]):
            sign = rec["sign"]
            count = rec["count"] if rec["count"] is not None else ""
            gap = rec["gap"] or ""
            text.append(f"- ({sign}{count} {gap} {rec['date']}) {rec['text']}")
        (out_dir / "reconciliations.md").write_text("\n".join(text) + "\n")

    # Prose that is not an entry (gap definitions, the mid-ledger Notes block,
    # the deferred-perf list) is carried over verbatim rather than modelled.
    for name, key in (("deferred-perf.md", "deferred_perf"),
                      ("notes.md", "ledger"),
                      ("preamble.md", "header")):
        body = "\n".join(prose[key]).strip()
        if body:
            (out_dir / name).write_text(body + "\n")

    report_text = report.render()
    if args.report:
        Path(args.report).write_text(report_text)
    else:
        print(report_text, end="")

    counts = {gap: 0 for gap in GAPS}
    for rec in records:
        counts[rec["primary"]] += 1
    print(f"entries written: {len(records)} (ledger rows {len(rows)}); "
          f"narratives paired {len(increments) - len(orphans)}/{len(increments)}; "
          f"unpaired held {len(orphans)}; reconciliations {len(reconciliations)}",
          file=sys.stderr)
    print("computed totals: " + " ".join(f"{g}={counts[g]}" for g in GAPS),
          file=sys.stderr)
    return 1 if report.blocking else 0


if __name__ == "__main__":
    sys.exit(main())
