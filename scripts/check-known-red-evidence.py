#!/usr/bin/env python3
"""Fail when a known-red registry cites evidence that does not resolve from a
clean checkout.

A registry row's evidence (a run log, a report, a capture) is either committed
or not cited by path. Lane workspaces (`.claude/worktrees/`), `target/` and
temp dirs are git-ignored or outside the repo, so a citation into them dies
with the workspace that held it.

  check-known-red-evidence.py [REGISTRY.md ...]   default: docs/Testing/KeystoneKnownReds.md
  check-known-red-evidence.py --self-check        prove each rule fails a bad citation
  check-known-red-evidence.py --rows REGISTRY.md  print the `known-red` rows the classifier
                                                  reads, one per line: key, pattern, site,
                                                  companions (separated by \x1f, so an empty
                                                  cell stays a field)

An evidence citation is a backticked path that ends in an evidence extension or
lies under a lane, scratch or temp location. Source paths (`.rs`, `.org`, ...)
cite sites, not evidence, and are not checked here. Per citation:

- `lost:`path``      the evidence is gone; nothing is checked.
- `path`             exists from the repo root and is tracked by the checked-out
                     tree's own repository (jj when `.jj` is present, else git
                     when its top level is the repo root).
- `path:A-B` quotes `snippet`
                     additionally, lines A..B (or `:A`, `:A,B,C`) of the file,
                     zstd-decoded for `.zst`, contain the snippet verbatim. A
                     line spec without `quotes` fails: a range proves nothing
                     unless it says what it holds.

The default run also fails each hand-authored case (a JSON line of
crates/holon-integration-tests/hand-authored-regressions/*.jsonl) whose
`known_red` key names no `known-red` row of the default registry: the
classifier accepts that case's panic only against the named row.

The default run also reads every run record (a line of a
docs/Testing/known-red-evidence/<row key>/gate-load-runs.tsv) and fails each
whose `log` cell does not name a committed file in that row's evidence
directory: a rate verdict over runs whose logs never landed is no evidence. A
`lost:` cell is no exception. Finding no gate-load-runs.tsv at all fails too.

A registry table whose header ends in `Companions` gives each row a last cell
of `—` or a code span of comma-separated invariant ids, `inv-x/*` naming every
view of `inv-x`. A `known-red` row must sit in such a table.
"""

import csv
import functools
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent / "flow-checks"))
import known_reds_table  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
DEFAULT_REGISTRIES = [REPO / "docs/Testing/KeystoneKnownReds.md"]
EVIDENCE_ROOT = Path("docs/Testing/known-red-evidence")
RUN_RECORD_COLUMNS = ("test", "job", "started", "log")
DEFAULT_CASE_FILES = sorted((REPO / "crates/holon-integration-tests/hand-authored-regressions").glob("*.jsonl"))

EVIDENCE_SUFFIXES = (".log", ".zst", ".txt", ".jsonl", ".json", ".md")
SCRATCH_ROOTS = ("/Users/", "/tmp/", "/private/", "/var/", "~/", ".claude/", "target/")
SCRATCH_DIRS = {"lane-logs", ".lane-logs", ".lane-tmp", "scratchpad", "target"}
NOT_A_PATH = re.compile(r"[\s\\^$()\[\]|*+?]")
LINE_SPEC = re.compile(r"(?P<path>.+?)(?::(?P<lines>\d+(?:[-,]\d+)*)(?::\d+)?)?")
QUOTES = re.compile(r"\s+quotes\s+$")
COMPANION = re.compile(r"inv-[a-z0-9-]+(/([a-z0-9_]+|\*))?")


def code_spans(line):
    """Yield (start, end, content) for every CommonMark code span of `line`."""
    i = 0
    while True:
        open_at = line.find("`", i)
        if open_at < 0:
            return
        n = len(line[open_at:]) - len(line[open_at:].lstrip("`"))
        ticks = "`" * n
        body_at = open_at + n
        search = body_at
        while True:
            close_at = line.find(ticks, search)
            if close_at < 0:
                break
            run_end = len(line) - len(line[close_at:].lstrip("`"))
            if run_end - close_at == n:
                break
            search = run_end
        if close_at < 0:
            i = body_at
            continue
        content = line[body_at:close_at]
        if len(content) > 2 and content[0] == " " and content[-1] == " ":
            content = content[1:-1]
        yield open_at, close_at + n, content
        i = close_at + n


def is_scratch(path):
    return path.startswith(SCRATCH_ROOTS) or bool(SCRATCH_DIRS & set(path.split("/")))


def is_evidence(path):
    if NOT_A_PATH.search(path) or "/" not in path or not re.search(r"[A-Za-z0-9]", path):
        return False
    return is_scratch(path) or path.endswith(EVIDENCE_SUFFIXES)


def wanted_lines(spec):
    for part in spec.split(","):
        lo, _, hi = part.partition("-")
        yield from range(int(lo), int(hi or lo) + 1)


def read_text(file):
    if file.suffix == ".zst":
        return subprocess.run(["zstd", "-dcq", str(file)], check=True, capture_output=True, text=True).stdout
    return file.read_text()


@functools.cache
def landing_paths():
    """Every file and directory, relative to REPO, that a commit of this tree carries.

    Asked of the repository that owns REPO itself: a jj workspace without a
    `.git` file would otherwise resolve `git` to an enclosing repository.
    """
    if (REPO / ".jj").is_dir():
        cmd = ["jj", "file", "list", "-r", "@"]
    else:
        top = subprocess.run(["git", "-C", str(REPO), "rev-parse", "--show-toplevel"], capture_output=True, text=True)
        if top.returncode != 0 or Path(top.stdout.strip()).resolve() != REPO:
            raise SystemExit(f"[known-red-evidence] {REPO} is neither a jj workspace nor a git top level: {top.stdout.strip() or top.stderr.strip()}")
        cmd = ["git", "ls-files", "--cached", "--others", "--exclude-standard"]
    files = subprocess.run(cmd, cwd=REPO, check=True, capture_output=True, text=True).stdout.splitlines()
    assert files, f"{' '.join(cmd)} listed no files in {REPO}"
    return set(files) | {str(parent) for f in files for parent in Path(f).parents}


def check_citation(path, lines, snippet):
    """Return the reason the citation does not resolve, or None."""
    file = Path(path).expanduser()
    if not file.is_absolute():
        file = REPO / file
    if not file.exists():
        return "does not exist"
    if not file.resolve().is_relative_to(REPO):
        return "is outside the repository"
    if os.path.relpath(file.resolve(), REPO) not in landing_paths():
        return "is ignored or untracked, so it never lands"
    if lines is None:
        return None
    if snippet is None:
        return "cites lines but quotes no snippet (write: `path:A-B` quotes `snippet`)"
    text = read_text(file).splitlines()
    held = "\n".join(text[n - 1] for n in wanted_lines(lines) if 0 < n <= len(text))
    if snippet not in held:
        return f"lines {lines} do not contain the quoted snippet {snippet!r}"
    return None


def registry_rows(registry):
    """Yield (lineno, key, status, pattern, site, companions) per registry row,
    or (lineno, key, error) for a row the registry grammar does not admit.
    """
    try:
        for lineno, header, cells in known_reds_table.table_rows(registry.read_text()):
            if header is None:
                if cells[0].startswith("`") and cells[1:2] == ["known-red"]:
                    yield lineno, known_reds_table.unticked(cells[0]), "is a known-red row in a table without a Companions column"
                continue
            try:
                row = known_reds_table.registry_row(header, cells)
            except known_reds_table.RowError as e:
                yield lineno, e.key, str(e)
                continue
            key, status, pattern = row["Key"], row["Status"], row["Match pattern"]
            if not pattern:
                continue
            site = row["Site"]
            # A site is one whitespace-free ERE; `—` leaves the row unpinned.
            if site == "—" or re.search(r"\s", site):
                site = ""
            cell = row["Companions"]
            if cell == "—":
                companions = []
            else:
                companions = [c.strip() for c in known_reds_table.unticked(cell).split(",")] if cell.startswith("`") and cell.endswith("`") else None
                if not companions or not all(COMPANION.fullmatch(c) for c in companions):
                    yield lineno, key, f"Companions cell {cell!r} is not `—` or a code span of invariant ids"
                    continue
            yield lineno, key, status, pattern, site, companions
    except known_reds_table.HeaderError as e:
        yield e.lineno, "table header", str(e)


def lint(registry):
    failures = [
        f"{registry.name}:{row[0]} [{row[1]}] {row[2]}" for row in registry_rows(registry) if len(row) == 3
    ]
    for lineno, line in enumerate(registry.read_text().splitlines(), 1):
        row = re.match(r"\|\s*`([^`]+)`", line)
        where = f"{registry.name}:{lineno}" + (f" [{row.group(1)}]" if row else "")
        spans = list(code_spans(line))
        for k, (start, end, content) in enumerate(spans):
            m = LINE_SPEC.fullmatch(content)
            if not is_evidence(m["path"]) or line[:start].endswith("lost:"):
                continue
            snippet = None
            if k + 1 < len(spans) and QUOTES.fullmatch(line[end : spans[k + 1][0]]):
                snippet = spans[k + 1][2]
            reason = check_citation(m["path"], m["lines"], snippet)
            if reason:
                failures.append(f"{where} `{content}` {reason}")
    return failures


def row_key_failures(registry, case_files):
    """One line per hand-authored case of `case_files` whose `known_red` value is not the key of a `known-red` row
    of `registry`. A line that is not blank, a `#` comment or a JSON object raises."""
    keys = {row[1] for row in registry_rows(registry) if len(row) == 6 and row[2] == "known-red"}
    failures = []
    for file in case_files:
        where = file.relative_to(REPO) if file.is_relative_to(REPO) else file
        for lineno, line in enumerate(file.read_text().splitlines(), 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            try:
                case = json.loads(line)
            except json.JSONDecodeError as e:
                raise ValueError(f"{where}:{lineno}: a hand-authored case is not JSON: {e}") from e
            if not isinstance(case, dict):
                raise ValueError(f"{where}:{lineno}: a hand-authored case is not a JSON object: {line}")
            if "known_red" not in case:
                continue
            if "name" not in case:
                raise ValueError(f"{where}:{lineno}: a hand-authored case has no name: {line}")
            key = case["known_red"]
            if not isinstance(key, str) or not key:
                failures.append(f"{where}:{lineno}: case {case['name']} has known_red {json.dumps(key)},"
                                f" which is not a row key")
            elif key not in keys:
                failures.append(f"{where}:{lineno}: case {case['name']} names row {key},"
                                f" which the registry has no known-red row for")
    return failures


class RunRecord(NamedTuple):
    file: Path
    lineno: int
    test: str
    job: str
    started: str
    log: str


def run_records(tsv):
    """Parse one gate-load-runs.tsv into RunRecords. Raise on a line whose column count differs from the header's."""
    with tsv.open(newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        missing = [c for c in RUN_RECORD_COLUMNS if c not in (reader.fieldnames or [])]
        if missing:
            return missing, []
        records = []
        for row in reader:
            if None in row or None in row.values():
                raise ValueError(f"{tsv}:{reader.line_num}: a run record's cell count differs from the header's"
                                 f" {len(reader.fieldnames)}")
            records.append(RunRecord(tsv, reader.line_num, row["test"], row["job"], row["started"], row["log"]))
    return [], records


def check_run_records(evidence_root):
    """One line per run record under `evidence_root` whose `log` is not a committed file in its row's evidence
    directory, per gate-load-runs.tsv without a needed column, and one when there is no gate-load-runs.tsv at all."""
    failures = []
    count = 0
    tsvs = sorted(evidence_root.glob("*/gate-load-runs.tsv"))
    for tsv in tsvs:
        where = tsv.relative_to(REPO) if tsv.is_relative_to(REPO) else tsv
        row_key = tsv.parent.name
        missing, records = run_records(tsv)
        if missing:
            failures.append(f"{where} [{row_key}] has no column {', '.join(missing)}")
        count += len(records)
        for r in records:
            reason = run_log_failure(r.log, EVIDENCE_ROOT / row_key)
            if reason:
                failures.append(f"{where}:{r.lineno} [{row_key}] record {r.test} {r.job} {r.started}: log `{r.log}` {reason}")
    print(f"[known-red-evidence] read {count} run record(s) from {len(tsvs)} gate-load-runs.tsv under {evidence_root}")
    if not tsvs:
        failures.append(f"{evidence_root} holds no */gate-load-runs.tsv")
    return failures


def run_log_failure(log, row_dir):
    """Return the reason a run record's `log` is no committed file in `row_dir`, or None."""
    if not log:
        return "is empty"
    if Path(log).expanduser().is_absolute():
        return "is not a repo-relative path"
    # shortcut: a log that the working copy holds but no commit yet counts as committed; the gate runs on the landed tree.
    reason = check_citation(log, None, None)
    if reason:
        return f"{reason}: it cites a scratch location" if is_scratch(log) else reason
    if Path(log).parent != row_dir:
        return f"is not beside the record (expected in {row_dir}/)"
    return None


def self_check():
    companions_header = f"| {' | '.join(known_reds_table.HEADER)} |\n|{' --- |' * len(known_reds_table.HEADER)}\n"
    known_red = "| `k` | known-red | `p` | — | sig | ev | — | — |"
    good = "docs/Testing/known-red-evidence/slot-withdrawn-last-claim-no-retraction/green.log.zst"
    quote = "the slot of 'journals' differs between the devices"
    cases = {
        "a range quoting what it holds": (f"| `k` | `{good}:166-168` quotes `{quote}` |", 0),
        "a lost citation": ("| `k` | lost:`.claude/worktrees/gone/lane-logs/x.log` |", 0),
        "a missing log": ("| `k` | `lane-logs/x.log` |", 1),
        "an ignored path": ("| `k` | `target/known-red-evidence-probe.log` |", 1),
        "a range without a snippet": (f"| `k` | `{good}:166-168` |", 1),
        "a range without the snippet": (f"| `k` | `{good}:1-2` quotes `{quote}` |", 1),
        "a known-red row with companions": (f"{companions_header}{known_red} `inv-a/*, inv-b/sql` |", 0),
        "a known-red row without companions": (f"{companions_header}{known_red} — |", 0),
        "a known-red row outside a companions table": ("| `k` | known-red | `p` | — | — |", 1),
        "a companion that is no invariant id": (f"{companions_header}{known_red} `inv-a, block_raw` |", 1),
        "a companions cell without a code span": (f"{companions_header}{known_red} inv-a |", 1),
        "a row under a changed registry header": (f"{companions_header.replace('| Key |', '| **Key** |')}{known_red.replace('known-red', 'proposed')} — |", 1),
        "a table without registry columns": ("| Run | Summary line |\n| --- | --- |\n| r1 | `639 tests run` |", 0),
    }
    good_key = Path(good).parent.name
    untracked = f"{EVIDENCE_ROOT}/{good_key}/known-red-evidence-probe.log"
    header = "\t".join(RUN_RECORD_COLUMNS)
    record = "t\tj\t2026-01-01T00:00:00Z"
    record_key = "t j 2026-01-01T00:00:00Z"
    run_cases = {
        "a record whose log is committed": (f"{header}\n{record}\t{good}", 0, []),
        "a record whose log is missing": (f"{header}\n{record}\t{EVIDENCE_ROOT}/{good_key}/gone.log.zst", 1,
                                          [record_key, f"{EVIDENCE_ROOT}/{good_key}/gone.log.zst"]),
        "a record whose log is under target/": (f"{header}\n{record}\ttarget/known-red-evidence-probe.log", 1,
                                                [record_key, "target/known-red-evidence-probe.log", "scratch"]),
        "a record whose log is untracked": (f"{header}\n{record}\t{untracked}", 1, [record_key, untracked, "never lands"]),
        "a record whose log is outside the repository": (f"{header}\n{record}\t/tmp/x.log.zst", 1, [record_key, "/tmp/x.log.zst"]),
        "a record whose log is not beside it": (f"{header}\n{record}\tdocs/Testing/KeystoneKnownReds.md", 1,
                                                [record_key, "docs/Testing/KeystoneKnownReds.md", "beside"]),
        "a record with an empty log cell": (f"{header}\n{record}\t", 1, [record_key, "is empty"]),
        "a record whose log is lost": (f"{header}\n{record}\tlost:{good}", 1, [record_key, f"lost:{good}"]),
        "a run record file without a log column": (f"{header.replace('log', 'logfile')}\n{record}\t{good}", 1,
                                                   ["gate-load-runs.tsv", "no column log"]),
    }
    probes = [REPO / "target/known-red-evidence-probe.log", REPO / untracked]
    wrong = []
    try:
        for probe in probes:
            probe.parent.mkdir(exist_ok=True)
            probe.write_text("probe\n")
        with tempfile.TemporaryDirectory() as tmp:
            for name, (row, want) in cases.items():
                registry = Path(tmp) / "registry.md"
                registry.write_text(row + "\n")
                got = len(lint(registry))
                print(f"[known-red-evidence] self-check {name}: {got} failure(s), expected {want}")
                if got != want:
                    wrong.append(name)
        for name, (tsv, want, needles) in run_cases.items():
            with tempfile.TemporaryDirectory() as tmp:
                (Path(tmp) / good_key).mkdir()
                (Path(tmp) / good_key / "gate-load-runs.tsv").write_text(tsv + "\n")
                failures = check_run_records(Path(tmp))
            lacking = [n for f in failures for n in needles if n not in f]
            print(f"[known-red-evidence] self-check {name}: {len(failures)} failure(s), expected {want}")
            for f in failures:
                print(f"[known-red-evidence]   {f}")
            if len(failures) != want or lacking:
                wrong.append(name + (f" (failure line lacks {', '.join(lacking)})" if lacking else ""))
    finally:
        for probe in probes:
            probe.unlink(missing_ok=True)
    return wrong


def print_rows(registry):
    rows = list(registry_rows(registry))
    errors = [row for row in rows if len(row) == 3]
    for lineno, key, error in errors:
        print(f"[known-red-evidence] {registry.name}:{lineno} [{key}] {error}", file=sys.stderr)
    if errors:
        return 1
    for _, key, status, pattern, site, companions in rows:
        if status == "known-red":
            print("\x1f".join([key, pattern, site, " ".join(companions)]))
    return 0


def main(argv):
    if len(argv) == 2 and argv[0] == "--rows":
        return print_rows(Path(argv[1]))
    if argv == ["--self-check"]:
        wrong = self_check()
        if wrong:
            print(f"[known-red-evidence] SELF-CHECK FAILED: {', '.join(wrong)}", file=sys.stderr)
            return 1
        return 0
    failures = []
    for registry in [Path(a) for a in argv] or DEFAULT_REGISTRIES:
        failures += lint(registry)
    if not argv:
        failures += row_key_failures(DEFAULT_REGISTRIES[0], DEFAULT_CASE_FILES)
        failures += check_run_records(REPO / EVIDENCE_ROOT)
    for failure in failures:
        print(f"[known-red-evidence] {failure}", file=sys.stderr)
    if failures:
        print("[known-red-evidence] FAIL: commit the evidence or mark it lost:`path`; point each case's known_red"
              " at a known-red row; commit each run record's log beside it or delete the record", file=sys.stderr)
        return 1
    print("[known-red-evidence] PASS: every cited evidence path resolves; every hand-authored known_red key names a"
          " known-red row; every run record's log is committed beside it")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
