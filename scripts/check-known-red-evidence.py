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

A registry table whose header ends in `Companions` gives each row a last cell
of `—` or a code span of comma-separated invariant ids, `inv-x/*` naming every
view of `inv-x`. A `known-red` row must sit in such a table.
"""

import functools
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_REGISTRIES = [REPO / "docs/Testing/KeystoneKnownReds.md"]

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


def is_evidence(path):
    if NOT_A_PATH.search(path) or "/" not in path or not re.search(r"[A-Za-z0-9]", path):
        return False
    if path.startswith(SCRATCH_ROOTS) or SCRATCH_DIRS & set(path.split("/")):
        return True
    return path.endswith(EVIDENCE_SUFFIXES)


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


def unticked(cell):
    return cell.strip().removeprefix("`").removesuffix("`")


def registry_rows(registry):
    """Yield (lineno, key, status, pattern, site, companions) per registry row,
    or (lineno, key, error) for a row whose Companions cell does not parse.

    Cells split on every `|`, as the classifier always has, so the companions
    are the LAST cell: a prose cell holding a stray `|` cannot shift them.
    """
    companions_table = False
    for lineno, line in enumerate(registry.read_text().splitlines(), 1):
        if not line.startswith("|"):
            companions_table = False
            continue
        cells = line.split("|")
        if cells[1].strip() == "Key":
            companions_table = cells[-2].strip() == "Companions"
            continue
        if not cells[1].strip().startswith("`") or len(cells) < 4:
            continue
        key, status, pattern = unticked(cells[1]), cells[2].strip(), unticked(cells[3]).replace("&#124;", "|")
        if not key or not pattern:
            continue
        if not companions_table:
            if status == "known-red":
                yield lineno, key, "is a known-red row in a table without a Companions column"
            continue
        site = unticked(cells[4]).replace("&#124;", "|")
        # A site is one whitespace-free ERE; `—` leaves the row unpinned.
        if site == "—" or re.search(r"\s", site):
            site = ""
        cell = cells[-2].strip()
        if cell == "—":
            companions = []
        else:
            companions = [c.strip() for c in unticked(cell).split(",")] if cell.startswith("`") and cell.endswith("`") else None
            if not companions or not all(COMPANION.fullmatch(c) for c in companions):
                yield lineno, key, f"Companions cell {cell!r} is not `—` or a code span of invariant ids"
                continue
        yield lineno, key, status, pattern, site, companions


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


def self_check():
    companions_header = "| Key | Status | Match pattern | Site | Companions |\n| --- | --- | --- | --- | --- |\n"
    good = "docs/Testing/known-red-evidence/slot-withdrawn-last-claim-no-retraction/green.log.zst"
    quote = "the slot of 'journals' differs between the devices"
    cases = {
        "a range quoting what it holds": (f"| `k` | `{good}:166-168` quotes `{quote}` |", 0),
        "a lost citation": ("| `k` | lost:`.claude/worktrees/gone/lane-logs/x.log` |", 0),
        "a missing log": ("| `k` | `lane-logs/x.log` |", 1),
        "an ignored path": ("| `k` | `target/known-red-evidence-probe.log` |", 1),
        "a range without a snippet": (f"| `k` | `{good}:166-168` |", 1),
        "a range without the snippet": (f"| `k` | `{good}:1-2` quotes `{quote}` |", 1),
        "a known-red row with companions": (f"{companions_header}| `k` | known-red | `p` | — | `inv-a/*, inv-b/sql` |", 0),
        "a known-red row without companions": (f"{companions_header}| `k` | known-red | `p` | — | — |", 0),
        "a known-red row outside a companions table": ("| `k` | known-red | `p` | — | — |", 1),
        "a companion that is no invariant id": (f"{companions_header}| `k` | known-red | `p` | — | `inv-a, block_raw` |", 1),
        "a companions cell without a code span": (f"{companions_header}| `k` | known-red | `p` | — | inv-a |", 1),
    }
    ignored = REPO / "target/known-red-evidence-probe.log"
    ignored.parent.mkdir(exist_ok=True)
    ignored.write_text("probe\n")
    wrong = []
    with tempfile.TemporaryDirectory() as tmp:
        for name, (row, want) in cases.items():
            registry = Path(tmp) / "registry.md"
            registry.write_text(row + "\n")
            got = len(lint(registry))
            print(f"[known-red-evidence] self-check {name}: {got} failure(s), expected {want}")
            if got != want:
                wrong.append(name)
    ignored.unlink()
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
    for failure in failures:
        print(f"[known-red-evidence] {failure}", file=sys.stderr)
    if failures:
        print("[known-red-evidence] FAIL: commit the evidence, or mark it lost:`path`", file=sys.stderr)
        return 1
    print("[known-red-evidence] PASS: every cited evidence path resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
