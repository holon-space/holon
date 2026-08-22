---
id: 2026-08-22-org-ingest-drops-collapsed-into-property-bag
date: 2026-08-22
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  An org file carrying `:COLLAPSED: t` ingests with the typed `collapsed`
  column left false and a stray uppercase `COLLAPSED` string in the untyped
  properties bag, so a user's fold state is silently lost on import.
---

## Bug

Ingesting an org file whose headline carries the fold marker:

```org
* Folded parent
:PROPERTIES:
:COLLAPSED: t
:ID: folded-parent
:END:
```

leaves the store disagreeing with the parser:

```
inv-blocks-match-ref/block_raw: block:folded-parent
  SUT:       properties: {"COLLAPSED": String("t")}, collapsed: false
  reference: properties: {},                        collapsed: true
```

`collapsed` is document state (Martin ruling 2026-07-11) — shared, synced,
survives restart — so this is a real data loss on import, not a view-state
nicety. Found by agent exploration (lane `gv-vocab`) while giving the new
`block "<id>" is collapsed` matcher a live home in the parity corpus.

Red log: `lane-logs/item3-green3.log` (the composed catalog divergence above).
Localization log: `lane-logs/probe-collapsed.log`.

## Root cause

PARTIALLY LOCALIZED — and the obvious hypothesis is REFUTED. Recording both
so the next lane does not re-run the same dead end.

`Block::drawer_properties()` (`holon-org-format/src/models.rs:972-984`)
deliberately re-inserts `COLLAPSED` / `WIDGET_ONLY` AFTER its `INTERNAL_KEYS`
filter (models.rs:855-878) has removed them, because org WRITEBACK needs those
keys to recreate the drawer. `build_block_params`
(`holon-orgmode/src/block_params.rs:153-162`) iterates the same function on
the INGEST leg, and `is_storage_column_key` (block_params.rs:224) matches
case-sensitively, so uppercase `COLLAPSED` is not refused and rides along as
an ordinary property.

That much is confirmed. What it does NOT explain is the false column, and a
direct probe of the boundary shows the params are CORRECT there:

```
PROBE block.collapsed    = true
PROBE drawer_properties  = {"COLLAPSED": "t"}
PROBE param "collapsed"  = Boolean(true)     <-- typed param IS emitted
PROBE param "COLLAPSED"  = String("t")       <-- stray property rides along
```

`SqlOperationProvider::partition_params`
(`holon/src/core/sql_operation_provider.rs:502-562`) matches columns
case-sensitively too (`write_schema.is_column`), so the two keys should
COEXIST: `collapsed` → the SQL column, `COLLAPSED` → `extra_props`. Both are
present and correctly typed when they leave `build_block_params`.

**Therefore the column is lost DOWNSTREAM of `build_block_params`, and making
`is_storage_column_key` case-insensitive would remove only the stray property
— it would not restore the column.** That fix alone cannot close this bug.
Note also that the case-sensitivity is deliberate and documented
(block_params.rs:220-222): matching case-insensitively would over-refuse an
ordinary user property such as `:Sort_Key:`, and would fire
`warn_unrepresentable_drawer_key`'s "Rename the drawer key" advice on every
`:COLLAPSED:` file Holon itself writes.

Still to localize: what consumes the correct `collapsed=Boolean(true)` param
between `partition_params` and the `block_raw` row. Prime suspects, untested:
the org-writeback round trip (`org-writeback=on` in the failing wiring)
re-ingesting its own rendered file, and `value_to_sql` / `optional_bool`
handling of the boolean.

## Missing piece

Two, one per gap.

ORACLE (primary): no invariant covered the fold field on the ingest leg.
`inv-blocks-match-ref/*` compares `Block` field-by-field and DOES cover
`collapsed` — but nothing ever drove an org file carrying `:COLLAPSED:` into
the composed slice, so the invariant never had a case to fire on. The field
had storage, a parser, a renderer and a round-trip test
(`holon-org-format/src/parser.rs:2019`) at the UNIT level, and no
end-to-end ingest coverage at all.

COVERAGE (secondary): the parity corpus had no scenario seeding a folded
block, and until this lane there was no `block "<id>" is collapsed` matcher
to write one with.

## Remedy

OPEN — deliberately not fixed in lane `gv-vocab`, because the scoped one-line
fix is refuted above and the real fix needs the downstream localization first.

The gap half IS closed, so the bug is now caught automatically the moment
someone works on it:

* `block "<id>" is collapsed` / `is not collapsed` exist
  (`pbt/fixtures/assert_steps.rs`, oracle `block_raw.collapsed`).
* `logseq-parity/outliner.feature` carries a written, runnable scenario ("A
  folded block carries its collapsed mark into the store") that reds on
  exactly this divergence. It is `@wip` ONLY because of this bug — un-`@wip`
  it as the red-for-the-right-reason proof when the fix lands.

What remains: localize the downstream consumer, fix it, un-`@wip` that
scenario. Do NOT touch `drawer_properties()` — org writeback depends on it
emitting these keys. If the stray uppercase property is also to be refused on
the ingest leg, prefer a narrow allowlist of the typed fields Holon itself
serializes into the drawer over a blanket case-insensitive match, so
`:Sort_Key:` and friends keep working.
