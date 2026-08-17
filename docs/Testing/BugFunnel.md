# Bug Funnel — moved

The escape ledger now lives in **[docs/Testing/bugfunnel/](bugfunnel/)**, one
file per escape under `entries/`. Nothing was lost: every row, narrative,
counter-correction and prose block from the old single-file ledger was carried
over verbatim by `scripts/bugfunnel-migrate.py`.

| Want | Do |
|---|---|
| record a new escape | the `bug-gap-triage` skill — one new file under `docs/Testing/bugfunnel/entries/` |
| the gap distribution | `python3 scripts/bugfunnel.py counts` |
| scan the funnel | `python3 scripts/bugfunnel.py index` then read `docs/Testing/bugfunnel/INDEX.md` |
| find specific escapes | `python3 scripts/bugfunnel.py list --gap ORACLE --status OPEN --since 2026-08-01` |
| validate before landing | `python3 scripts/bugfunnel.py check` |

The rationale, the options that were rejected, and the measurements behind the
change are in [BugFunnelFormat.md](BugFunnelFormat.md). The short version: one
bug used to take three coordinated edits (increment line, ledger row, hand-kept
counter), every one of them at a fixed position in one file, so concurrent
lanes conflicted on every escape — and the counter silently merged to the wrong
total when they did not. Totals are now derived from the entry files and cannot
drift.

## READ THIS IF YOU FOLLOWED A "BugFunnel row N" CITATION

**Positional row citations are dead and were NOT rewritten.** Roughly sixty
comments across the codebase and docs cite this file as "BugFunnel row 26",
"row 144", "rows 230 + 232". Those numbers were the row's ordinal **at the time
the comment was written**, and the ledger table was never append-ordered — rows
were inserted mid-table as sections were reorganised. Measured at cutover: the
bug that `crates/holon-integration-tests/tests/journals_restart_survival.rs`
cites as "row 144" sat at ordinal 227 of 458.

So `row N` cannot be mapped to an entry mechanically, and no mapping was
invented. To resolve one:

1. `jj log` the file that carries the citation to find when the comment landed.
2. `jj file show -r <that rev> docs/Testing/BugFunnel.md` — the pre-cutover file
   is intact in history at every rev.
3. Count to row N in that rev's `## Ledger` table; that is the bug.
4. Find it in `docs/Testing/bugfunnel/entries/` by date and summary, and
   **replace the citation with the entry's `id`** so the next reader does not
   repeat this.

Date-based citations ("BugFunnel 2026-08-15 D15.b") still resolve — search the
entries by date.
