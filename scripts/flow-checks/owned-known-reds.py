#!/usr/bin/env python3
"""Check that a lane summary names every known-red row the lane's cards own, with its outcome.

    scripts/flow-checks/owned-known-reds.py --base <commit> --summary <path>

Run from the lane root. The registry is every docs/Testing/*KnownReds*.md file, read at the base (`jj file show`) and
in the working tree; a row is owned when its Task or Owner cell names a card of the lane as `#N` or `holon-work#N`.
The lane's cards are the summary's `Cards: #N[, #N]...` line, else the N of a lane root named `lane-<N>-<slug>`.
The summary accounts for its rows in one `## Known-red rows` section, one table row per owned row:
    | <key> | deleted | <log> |
    | <key> | kept | <cause> | <log> | <load at start> | <seconds> |
with a cause of wait, lock, cpu, race or other:<word>, and a log under target/ that exists from the lane root.
PASS (exit 0) when every owned row is named exactly once, `deleted` exactly when the key is gone from the working tree,
and the section names no other key. Every problem is one stderr line (exit 1); an input it cannot read exits 2.
"""
import argparse
import importlib.util
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import known_reds_table  # noqa: E402

REGISTRY_GLOB = "docs/Testing/*KnownReds*.md"
SECTION = "## Known-red rows"
CARDS = re.compile(r"Cards: (#[1-9][0-9]*(?:, #[1-9][0-9]*)*)")
LANE_DIR = re.compile(r"lane-([1-9][0-9]*)-.+")
CARD = re.compile(r"(?:(?<![\w/#-])|(?<=holon-work))#([1-9][0-9]*)(?![0-9])")
CAUSE = re.compile(r"wait|lock|cpu|race|other:[a-z][a-z0-9-]*")


def log_path_rule():
    spec = importlib.util.spec_from_file_location("tests_cover_diff", HERE / "tests-cover-diff.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.LOG_PATH, module.BRACE


def lane_cards(summary_text, root_name):
    """The cards of the `Cards:` line, else of the lane root's name; None when neither names one."""
    lines = [line.rstrip() for line in summary_text.splitlines() if line.startswith("Cards:")]
    if len(lines) > 1:
        raise SystemExit(f"owned-known-reds: {len(lines)} `Cards:` lines; write one")
    if lines:
        if not (m := CARDS.fullmatch(lines[0])):
            raise SystemExit(f"owned-known-reds: `{lines[0][:60]}` is not `Cards: #N[, #N]...`")
        return {int(c) for c in m.group(1).replace("#", "").split(", ")}
    m = LANE_DIR.fullmatch(root_name)
    return {int(m.group(1))} if m else None


def registry(texts, where):
    """({key: row}, [(key, path, lineno, problem)], [(path, lineno, first cell, [second cell], cell count)]) of `texts` ({path: text}).
    The last list holds the table lines outside every registry table. A prose table may name a registry key in its
    first cell, so a short line counts as registry-shaped only with a Status in its second."""
    rows, bad, loose = {}, [], []
    for path, text in sorted(texts.items()):
        try:
            for lineno, header, cells in known_reds_table.table_rows(text):
                if header is None:
                    loose.append((path, lineno, known_reds_table.unticked(cells[0]), cells[1:2], len(cells)))
                    continue
                try:
                    row = known_reds_table.registry_row(header, cells)
                except known_reds_table.RowError as e:
                    bad.append((e.key, path, lineno, f"registry row {e.key} ({path}:{lineno} at {where}) {e}"))
                    continue
                rows[row["Key"]] = row
        except known_reds_table.HeaderError as e:
            bad.append((None, path, e.lineno, f"{path}:{e.lineno} at {where}: {e}"))
    return rows, bad, loose


def fixed_base_errors(base_texts, tip_texts):
    """The notes of the base errors that the tip fixes or deletes; a base error that the tip keeps is the tip's error."""
    tip_bad = registry(tip_texts, "the tip")[1]
    kept = {(key, path if key is None else None) for key, path, _, _ in tip_bad}
    return [f"base error at {path}:{lineno}, fixed by this diff"
            for key, path, lineno, _ in registry(base_texts, "the base")[1]
            if (key, path if key is None else None) not in kept]


def owners(row):
    return {int(n) for column in ("Task", "Owner") for n in CARD.findall(row[column])}


def summary_rows(summary_text, bad):
    """{key: cells} of the `## Known-red rows` table; cells start after the key."""
    lines = summary_text.splitlines()
    starts = [i for i, line in enumerate(lines) if line.rstrip() == SECTION]
    if len(starts) > 1:
        bad.append(f"the summary has {len(starts)} `{SECTION}` sections; write one")
    if not starts:
        return {}
    rows = {}
    for line in lines[starts[0] + 1:]:
        if line.startswith("## "):
            break
        if not line.startswith("|"):
            continue
        cells = known_reds_table.split(line)
        if cells[0] == "Key" or all(known_reds_table.SEPARATOR.fullmatch(c) for c in cells):
            continue
        key = known_reds_table.unticked(cells[0])
        if key in rows:
            bad.append(f"the summary names row {key} twice")
        rows[key] = cells[1:]
    return rows


def outcome_problem(key, cells, is_log):
    """The problem of one summary row's outcome cells, or None."""
    log_path, brace = log_path_rule()
    outcome = cells[0] if cells else ""
    if outcome == "deleted":
        logs = cells[1:2]
        if len(cells) != 2:
            return f"row {key} is deleted with {len(cells) - 1} cells after the outcome; write `| {key} | deleted | <log> |`"
    elif outcome == "kept":
        if len(cells) != 5 or not CAUSE.fullmatch(cells[1]) or not all(cells[2:]):
            return (f"row {key} is kept without a cause/log; write `| {key} | kept | <wait|lock|cpu|race|other:word> "
                    "| <log> | <load at start> | <seconds> |`")
        logs = cells[2:3]
    else:
        return f"row {key} has the outcome `{outcome}`, neither deleted nor kept"
    log = logs[0]
    if not log_path.fullmatch(log) or re.search(brace, log) or not is_log(log):
        return f"row {key} cites `{log}`, which is no existing log under target/ from the lane root"
    return None


def problems(base_texts, tip_texts, summary_text, cards, is_log):
    """The problems of `summary_text` against the registry at the base and the tip, for the lane's `cards`."""
    base, base_bad, _ = registry(base_texts, "the base")
    tip, tip_bad, loose = registry(tip_texts, "the tip")
    bad = [problem for _, _, _, problem in tip_bad]
    if not base and not base_bad:
        bad.append(f"no registry row in {', '.join(sorted(base_texts)) or REGISTRY_GLOB} at the base")
    keys = set(base) | set(tip) | {key for key, _, _, _ in base_bad + tip_bad}
    bad += [f"{path}:{lineno} at the tip: registry-shaped row outside a table"
            for path, lineno, first, second, count in loose
            if count == len(known_reds_table.HEADER) or (first in keys and second and second[0] in known_reds_table.STATUSES)]
    rows = {**base, **tip}
    owned = {key for key, row in rows.items() if owners(row) & cards}
    named = summary_rows(summary_text, bad)
    for key in sorted(owned - set(named)):
        bad.append(f"owned row {key} is not named in the summary")
    for key, cells in sorted(named.items()):
        if key not in rows:
            bad.append(f"summary names {key}, which is no row of the registry")
            continue
        if key not in owned:
            bad.append(f"summary names {key}, which is no row of the lane's cards {sorted(cards)}")
            continue
        if problem := outcome_problem(key, cells, is_log):
            bad.append(problem)
            continue
        if cells[0] == "kept" and key not in tip:
            bad.append(f"row {key} is deleted at the tip but the summary says kept")
        if cells[0] == "deleted" and key in tip:
            bad.append(f"row {key} is kept at the tip but the summary says deleted")
    return bad


def jj(*args):
    done = subprocess.run(["jj", "--ignore-working-copy", *args], capture_output=True, text=True)
    if done.returncode:
        raise SystemExit(f"owned-known-reds: jj {' '.join(args)} exited {done.returncode}: {done.stderr.strip()}")
    return done.stdout


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--base", required=True)
    p.add_argument("--summary", required=True, type=Path)
    a = p.parse_args()
    try:
        summary_text = a.summary.read_text()
        cards = lane_cards(summary_text, Path.cwd().name)
        if cards is None:
            raise SystemExit(f"owned-known-reds: name the cards: add a `Cards:` line to {a.summary} "
                             "(the lane root is not named lane-<N>-<slug>)")
        base_paths = jj("file", "list", f"--revision={a.base}", "--", f"glob:{REGISTRY_GLOB}").splitlines()
        base = {path: jj("file", "show", f"--revision={a.base}", "--", path) for path in base_paths}
        tip = {str(path): path.read_text() for path in sorted(Path.cwd().glob(REGISTRY_GLOB))}
    except SystemExit as e:
        print(e.code, file=sys.stderr)
        sys.exit(2)
    except (OSError, UnicodeDecodeError) as e:
        print(f"owned-known-reds: {e}", file=sys.stderr)
        sys.exit(2)
    tips = {str(Path(p).relative_to(Path.cwd())): t for p, t in tip.items()}
    bad = problems(base, tips, summary_text, cards, lambda log: Path(log).expanduser().is_file())
    for note in fixed_base_errors(base, tips):
        print(f"owned-known-reds: {note}")
    for problem in bad:
        print(f"owned-known-reds: {problem}", file=sys.stderr)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
