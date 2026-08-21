# `holontest.sqlite` — LogSeq DB import fixture

A self-contained snapshot of the **HolonTest** LogSeq DB-version graph, the
keystone fixture for read-only LogSeq-DB import. Read-only; nothing writes it.

## Provenance

Byte copy of a throwaway HolonTest graph's `db.sqlite`, taken during the
stage-0 spike (`~/.claude/plans/logseq-db-spike-2026-08-20/`, see `REPORT.md`).
HolonTest is a fresh graph: **0 user classes, 0 user properties** — its content
is almost entirely LogSeq's built-in schema plus a handful of scaffolding blocks
(a "Project Alpha" page, three Aug-2026 journal days, sibling blocks, a probe
task, block-ref links).

Prepared for commit by:
1. `PRAGMA wal_checkpoint(TRUNCATE)` + `PRAGMA journal_mode=DELETE` on a working
   copy, so the single `.sqlite` file is standalone (no `-wal`/`-shm` sidecars).
2. **PII redaction, in TWO steps.** The sensitive-content gate found one
   personal email in `:logseq.property.user/email` (entity e194).
   a. It was replaced in the row, byte for byte (same length,
      structure-preserving), with `noreply@holontest.test`.
   b. The file was then `VACUUM`ed — **this step is not optional.**

   Why (b) is required: an in-place row edit rewrites the b-tree page but leaves
   the *previous page image* in the slack space of allocated pages. After step
   (a) alone, all 456 `kvs` rows read clean, `PRAGMA integrity_check` returned
   `ok`, and `freelist_count` was 0 — yet `strings -a | grep gmail` still
   recovered **four** copies of the original address from pages 96, 96, 109 and
   114. A logical redaction is not a byte redaction. `VACUUM` rewrites the file
   without slack and removes them.

   Verified after (b): 0 occurrences of the original address, 5 of the
   replacement, 456 `kvs` rows, `PRAGMA integrity_check` = `ok`, and the
   identity counts below unchanged (2631 / 215 / 57 / 206). No other personal
   data is present — `nightscape` is a public handle, and the only remaining
   `@`-address in the whole file is the synthetic replacement.

**If you ever re-redact this fixture, sweep the raw bytes, not the rows:**
`strings -a holontest.sqlite | grep -iE "gmail|<name>|@"`. Querying the table
cannot see a slack-space survivor.

## Expected facts (the keystone asserts these)

Measured directly from this file on 2026-08-21 (authoritative; where a number
differs from REPORT.md prose, this file's measurement wins):

| Quantity | Value |
|---|---|
| `kvs` nodes | 456 |
| Unique datoms `(e,a,v,tx)` | 2631 |
| Distinct entities | 215 |
| Entities carrying `:block/uuid` (= blocks) | 206 |
| `:logseq.kv/*` config singletons (uuid-less) | 7 |
| Uuid-less non-config remnants (e197, e199) | 2 |
| Distinct datom attributes | 57 *(REPORT.md §Obj1 prose says 58; a direct recount via the spike's own `exact_counts()` gives 57 — the identity counts 2631/215/206 match REPORT exactly)* |
| schema-version | `{:major 65, :minor 33}` |

Spot-check entities:

- **e207** — page "Project Alpha": `:block/uuid 6a86cf74-3882-4ebd-a19d-c1fa46f58380`,
  `:block/name "project alpha"`, `:block/title "Project Alpha"`, tags → class
  entity 4 (`Page`), `:block/refs [4, 22]`.
- **e193 / e196 / e204** — journal days: `:block/journal-day` = 20260820 /
  20260819 / 20260822, each tagged class entity 167 (`Journal`).
- **e203** — the probe task: empty `:block/title`, tags → class 169 (`Task`),
  `:block/parent 193`, `:block/order "a2"`, `:logseq.property/status` → entity 79
  (`Done`), `:logseq.property/deadline 1787349600000`.

The nine uuid-less entities do **not** all belong to `:logseq.kv/*`: seven are
config singletons (`:db/ident :logseq.kv/…` + `:kv/value`), while **e197**
(`:block/created-at` plus an empty `:block/title`) and **e199**
(`:block/created-at` only) are LogSeq's own half-created remnants. They are
classified as `EntityKind::Orphan` and counted, never folded in with the
singletons — see amendment A6 in `plan-lsqdb-import.md`.

**e193 carries TWO `:block/updated-at` datoms** (tx 536870916 → 1787218310305,
tx 536871019 → 1787221153038) although the attribute is declared
cardinality-one. The current value is the higher-tx one. The superseded value
equals the block's `created-at`, so resolving it wrongly produces a
plausible-looking `updated_at == created_at`; the keystone pins this explicitly.

## Known-untested guard

Nothing in this fixture exercises the **namespace-page** error path: all 147
`:block/name` values are flat, none contains a `/`. `ImportError::NamespacePage`
therefore fires only against real graphs that use `a/b/c` page names. This is
accepted for stage 1 (fail loud rather than silently flatten); full
page-under-page chain construction is a flagged fast-follow.
