---
id: 2026-09-25-loro-ui-row-drops-edge-fields-so-tag-conditions-never-match
date: 2026-09-25
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The no-Turso render path (`LoroUiWatcher`) built each row from
  `Block::to_entity()`, which skips edge fields, so `tags` was absent and every
  tag-driven profile condition (the Page variant, `decision`) failed to match in a
  Loro-fed session.
---

## Bug
Found by a spike about a new `decision` profile condition
(`tags.contains("\"decision\"")`). In Loro mode that condition never matched,
and the existing Page condition (`is_page_row`,
`assets/default/types/block_profile.yaml:71`) did not match either. A scratch
probe that resolved a Page-tagged block through the Turso-free profile resolver
measured the defect:

```
PROBE row has tags key: None
PROBE loro-row profile=default is_page_row=Some(Null)
PROBE sql-shaped-row profile=embedded_page is_page_row=Some(Boolean(true))
```

## Root cause
`loro_ui_watcher::block_to_row` (`crates/holon-loro-wiring/src/loro_ui_watcher.rs`)
used `block.to_entity().fields`. The `Entity` derive leaves `#[edge_field]`
fields out of `to_entity` (`crates/holon-macros/src/entity.rs`, the
`skip_serialization` branch), so `tags`, `requires`, `advice_suppressed` and
`contributes_to` never reached the row. `marks` reached it as an array, which
the frontend's `read_marks` does not accept, so rich text rendered as plain
text. The SQL path reads the `block` matview, which hydrates the edge fields as
JSON-text arrays.

## Missing piece
The keystone had no renderer on the Loro path: `compose_sut` boots its
frontend only over Turso, so `LoroUiWatcher` rows ran in no keystone draw
(ENVIRONMENT). No invariant observed the row the Loro path hands to the reactive
engine (ORACLE).

## Remedy
- **Keystone:** every Loro draw now registers `SutLoroUiRows`
  (`crates/holon-integration-tests/src/pbt/composed/loro_ui_rows.rs`). It goes
  through the production `LoroBlockQuerySource` → `block_to_row` → Turso-free
  profile resolver path.
- **Invariant:** `inv-loro-ui-rows-match-ref` compares the resolved
  `is_page_row` with the reference's Page tag for every block the reference
  tracks. For non-seed blocks it also compares every edge field, read back
  through the canonical row parser.
- **Fix:** `Block::to_storage_row` (`crates/holon-api/src/block.rs`) is the
  inverse of `TryFrom<StorageEntity> for Block`. It writes the edge fields and
  `marks` in matview shape, and `block_to_row` uses it.
- **Follow-up:** a full no-Turso renderer arm in `compose_sut`, for when the
  Loro read model becomes the production frontend (D172.a).
