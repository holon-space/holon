# Bug funnel serialization — proposal

**Status**: proposal, awaiting Martin's ratification. Nothing has been cut over.

## What is wrong today

`docs/Testing/BugFunnel.md` is 1204 lines / 1.29 MB holding one dataset in
**two** hand-kept representations plus a hand-kept summary:

| Section | Shape | Records |
|---|---|---|
| Header distribution | four hand-maintained counters | 4 |
| Increment log | newest-first bullets, one line per bug, ~3000 words each | 230 bugs + 16 counter corrections |
| `## Ledger` | markdown table, 6 columns | 458 bugs |
| `## Deferred perf` | a prose list of one deferred optimisation | 1 |

Four independent defects follow from that shape.

**One bug = three edits.** The triage skill appends an increment line, appends
a ledger row, and bumps a counter. A skipped edit is invisible until someone
runs `scripts/bugfunnel-check.sh`. Measured on real history: at revs
`pnmlnroqznun` and `mkysxotrlwks` the header claimed COVERAGE 98 and 89 while
the ledger actually held 101 and 92 rows. The counter was wrong for weeks and
was eventually repaired by three `reconciliation` lines that exist only to
correct arithmetic.

**Every append conflicts.** Both sections are single files where new records go
at one fixed position. Two lanes branching from one base and each adding a row
collide there. Measured, since this is the load-bearing claim behind
requirement 1:

| Scenario | git 3-way | jj |
|---|---|---|
| Two lanes insert at the TOP (today's layout) | CONFLICT | CONFLICT |
| Two lanes append at the END | CONFLICT | CONFLICT |
| Two lanes each add a separate file | clean | clean |

**Switching to oldest-first append does not fix conflicts** — the diff sees two
insertions at the same offset either way. Only splitting records across files
does. And the counter has a worse failure mode than a conflict: two lanes
bumping `126` to `127` merge *cleanly* to `127` when the union needs `128`.

**It cannot be queried.** "Which ORACLE escapes are still open?" needs a human
or an LLM to read 1.29 MB of prose. The one automated consumer,
`bugfunnel-check.sh`, is 70 lines of awk whose comments enumerate the format's
irregularities: the description cell contains unescaped `|`, the gap class must
be found by scanning cells, and the table silently continues *below* the
`## Deferred perf` heading that interrupts it.

**It is expensive to read.** Entries are single physical lines of up to 3000
words, so an agent that reads "the last 50 lines" gets an arbitrary slice and
one that reads the file spends ~44k tokens. Martin already wrote the complaint
into the file as an HTML comment at line 669.

## What the format must carry

Every field the current file expresses anywhere, and where it lives today:

| Field | Increment log | Ledger | Notes |
|---|---|---|---|
| date | yes | yes | the two disagree for some bugs |
| primary gap | yes | yes | `ENV`/`COV`/`PERC` abbreviations appear in the log only |
| secondary gap | prose | column | |
| re-triage (`A→B`) | prose | inside the gap cell | changes which class the bug counts as |
| one-line summary | bolded span | bolded span | |
| root-cause narrative | yes, long | shorter restatement | the log is the richer copy |
| attribution (task, lane, finder) | yes | yes | free text, ~15 distinct phrasings |
| missing piece | prose | column | the remedy the gap implies |
| remedy status | prose | column | `FIXED`/`OPEN`/`PARTIALLY FIXED`/`MITIGATED`/… |
| evidence (log paths, test names, numbers) | yes | sometimes | free text |
| counter contribution | the `(+N GAP` prefix | — | derivable from the row |
| counter corrections | 16 `reconciliation` lines | — | pure arithmetic repair |

Plus three prose blocks that are not entries: the gap definitions, a mid-ledger
`Notes:` block, and `## Deferred perf`.

## Options

| | 1. fewer conflicts | 2. analyzable without LLM | 3. human read/write | 4. same information | 5. context economy |
|---|---|---|---|---|---|
| **A. Status quo + append at end** | no — measured above | no change | no change | yes | no change |
| **B. Single NDJSON log** | no — same single-file append | excellent (`jq`) | poor: a 3000-word narrative inside a JSON string with `\n` escapes is unreadable and unwritable by hand | yes | good (`jq` projections) |
| **C. One TOML file per entry** | yes | good, but Python 3.9 (this repo's interpreter) has no `tomllib` — needs a dependency | narrative lives in a `"""…"""` string, so markdown tooling and diffs treat it as opaque | yes | with an index |
| **D. TOON table** | no — one file, shared column header | good | designed for uniform value-heavy rows; a paragraph per cell is its stated worst case | yes | excellent for the index |
| **E. One markdown file per entry, YAML frontmatter + prose body** | yes | good (`yq`, or 30 lines of Python) | best: typed fields on top, narrative is ordinary markdown | yes | with an index |

TOON gets an explicit verdict rather than silence, since the repo owns
`crates/holon-toon/`: **rejected as the storage format, kept in mind for the
index.** Its own README records that the block-forest instantiation was
measured net-negative against ID-compressed org and rejected; the generic
tabular codec's sweet spot is "uniform, value-heavy rows", which is the
opposite of a paragraph-per-cell escape record. But the *generated index* is
uniform and value-heavy, so if the index ever becomes a token cost worth
optimising, emitting it as TOON instead of markdown is a one-function change in
`scripts/bugfunnel.py`.

## Recommendation: option E

```
docs/Testing/bugfunnel/
  entries/2026-08-16-page-switch-rendered-accordion-must-direct.md   # one per escape
  notes.md  deferred-perf.md  preamble.md  reconciliations.md        # non-entry prose, verbatim
  INDEX.md                                                          # GENERATED
```

An entry:

```markdown
---
id: 2026-08-16-page-switch-rendered-accordion-must-direct
date: 2026-08-16
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Every page switch rendered "accordion must be a direct child of a main-panel
  column".
---

## Bug
## Root cause
## Missing piece
## Remedy
```

Five properties follow.

**No counters are stored.** `scripts/bugfunnel.py counts` derives the
distribution from the entry files. The counter-conflict hazard and all 16
reconciliation lines stop being possible, not merely less likely. This is the
single largest win and it is available under options B, C and E alike — it is
the *stored* counter, not the file layout, that produced the measured drift.

**No shared insertion point.** Two lanes adding an escape write two filenames
that cannot collide, since the id carries the date and a summary slug.

**Analyzable with what is already installed.** `scripts/bugfunnel.py
list --gap ORACLE --status OPEN --since 2026-08-01` is one line of output per
match. The frontmatter is also plain `yq` fodder for anything the script does
not cover, and `check` validates every entry against the schema (gap and status
from a closed vocabulary, id matching the filename).

**Stable citation targets.** Code cites the funnel today as "BugFunnel row 144"
(`crates/holon-integration-tests/tests/journals_restart_survival.rs:1`) — a
positional reference that every subsequent insertion invalidates. The `id` is a
permanent handle.

**Context economy.** Full scan drops from 1,289,172 bytes to an 87,821-byte
index (14.7×); a filtered `list` is a few hundred bytes; a single entry is
~2 KB and reads as normal markdown rather than one 3000-word line.

### The two judgment calls in this recommendation

**The increment log is deleted, not migrated.** It exists to make the hand-kept
counter auditable — that is what the `(+1 GAP` prefix is for. With the counter
derived, a second append-only stream of the same bugs has no job. Its narratives
are richer than the ledger's, so they are merged into the entry as the
`## Root cause` section rather than discarded.

**`INDEX.md` is generated and should not be committed** (add it to
`.gitignore`; regenerate with `just bugfunnel-index`). A committed generated
index reintroduces exactly the shared-insertion-point conflict the split was
meant to remove. The cost is that GitHub's web view shows a directory of files
instead of a scan sheet. If Martin wants it browsable, the fallback is to commit
it and treat any conflict on it as "delete the file, re-run the generator" —
genuinely a one-command resolution, but it is a conflict per concurrent lane,
every time.

## Cutover

1. Run `scripts/bugfunnel-migrate.py` (see below) and land its output.
2. Rewrite the `bug-gap-triage` skill's Procedure steps 2 and 4: step 2 becomes
   "write one file under `docs/Testing/bugfunnel/entries/`", step 4 disappears —
   there is no counter to update.
3. Replace `scripts/bugfunnel-check.sh` with `scripts/bugfunnel.py check`; point
   the `just` recipe at it.
4. Reduce `docs/Testing/BugFunnel.md` to a stub pointing at the new directory,
   so the links in `CLAUDE.md`, `DEVELOPMENT.md` and the skill keep resolving.

## Migration script

`scripts/bugfunnel-migrate.py` — Python, matching the twelve existing
`scripts/*.py`; PyYAML is already installed and Python 3.9 has no `tomllib`,
which is also why option C loses on requirement 2.

- **Deterministic**: the output is a pure function of the input document. Entry
  ids are date + slug, disambiguated by a content hash rather than by position,
  because `jj run` visits revs in no fixed order and a positional counter would
  rename entries between revs.
- **Non-accumulating**: the output directory is removed and rebuilt, so a rev
  with fewer entries does not inherit stragglers from another rev's run.
- **No wall clock**: every date comes from the record.
- **Fails loud, in one report**: ambiguities accumulate into a report and a
  non-zero exit; nothing is guessed or dropped.

*`jj run` assumption, not verified*: that it materialises each rev's tree, runs
the command in it, and amends the result into that rev. If it instead runs
against a shared working copy, the `rmtree` needs to become scoped to the
generated directory only.

### Dry run against the current file

```
entries written: 458 (ledger rows 458); narratives paired 158/230;
unpaired held 72; reconciliations 16
computed totals: ENVIRONMENT=185 COVERAGE=126 PERCEPTION=72 ORACLE=75
```

The derived totals reproduce the hand-maintained header (185 / 126 / 72 / 75)
exactly, and `bugfunnel.py check` reports 458 entries and 0 schema problems.

Information preservation, verified rather than asserted: every one of the 1374
parsed ledger fields, all 230 increment narratives, all 16 reconciliation lines
and all three prose blocks appear **verbatim** in the output after whitespace
normalisation — 0 missing. The output is a strict superset (209k words out of
172k in), because a bug's ledger description and its increment narrative are
both kept.

Re-running produces byte-identical output. Run across four historical revs
(1125, 1048, 970 and 911 lines), the derived totals agree with the independent
awk checker at every rev except the two where **the checker showed the
hand-maintained header was itself wrong** (COVERAGE 98 vs 101 rows, 89 vs 92) —
the derived count is the correct one.

### The 106 records needing a human eye

None are data loss; every one is preserved and flagged.

| Count | What | Why it is not automatable |
|---|---|---|
| 72 | increment narratives with no confidently-matched ledger row | The two sections disagree on dates for some bugs, so pairing is text-similarity over a ±5 day window at a 0.35 containment bar. Below the bar the narrative is written to `unpaired-narratives.md` with its source line rather than promoted to an entry — promoting it would inflate every total, which is why entries total exactly 458 and not 530. |
| 20 | remedy cells opening with no known status token | e.g. `Fork B FIXED (2026-07-13, B1)`, `(1) FIXED 2026-07-20`. These get `status: UNCLASSIFIED`, which `check` accepts and `counts` reports separately. |
| 14 | rows whose cell count is off | An unescaped `` ` | ` `` inside code in the prose, so the missing-piece/remedy split is unreliable. Full text is preserved; the section boundary needs a human. |

The 72 unpaired narratives are the only item that costs real effort. They are
mechanical to resolve — each names its bug — so one cheap agent pass over
`unpaired-narratives.md` can attach them, or they can simply stay in that file
as an appendix, since the counted dataset is already complete without them.

## Open questions for Martin

1. Commit `INDEX.md` or generate it? (Recommendation: generate, gitignore.)
2. Are the 72 unpaired narratives worth an agent pass, or is the appendix fine?
3. `status: UNCLASSIFIED` on 20 entries — accept, or hand-classify before cutover?
