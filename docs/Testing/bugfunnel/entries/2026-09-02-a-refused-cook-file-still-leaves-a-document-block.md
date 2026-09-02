---
id: 2026-09-02-a-refused-cook-file-still-leaves-a-document-block
date: 2026-09-02
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  A `.cook` file whose parse is refused still gets a document block in the
  store, so the sidebar shows an empty recipe page and the write-back gate
  raises WRITE-BACK REFUSED for it forever.
---

## Bug

Found dogfooding the kitchen feature on a copy of Martin's real vault (lane
`kitchen-dogfood`), immediately after
[[2026-09-02-a-german-timer-unit-refuses-the-whole-recipe]]: all three recipes
had failed to parse and were quarantined, yet the store held a page block for
each one.

```sql
SELECT id, content FROM block_raw
WHERE parent_id = 'block:14322374-ff1d-41ed-9f2f-d3415e17f9ff';
-- Spaghetti-Carbonara  block:8a802b12-4e49-4a63-3e06-c71696ebc072
-- Linsensuppe          block:16db6fd4-765b-b9fa-227d-e1272030d3fc
-- Pfannkuchen          block:aea4980b-faac-2ddf-43dd-371e0d4921b9
```

Each of those blocks then drove a repeating error pair from the file-sync
controller, at both the `write_back` and the `on_block_changed` site:

```
WRITE-BACK REFUSED: .../Resources/Rezepte/Spaghetti-Carbonara.cook is a
read-only format (authoritative input only). The store holds changes for
block:8a802b12-... that will NOT reach this file
```

So the user sees three recipe pages in the sidebar that contain nothing, and
the log fills with a refusal about changes that only exist because the file was
refused.

Evidence: `kd-logs/app.log` lines 1195–1200 in the lane scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/kd-logs/app.log`).

## Root cause

The document block is minted by the vault directory walk before the format
adapter is asked to parse, so it survives a parse that never returns. The
adapter's own document — `EntityUri::file(rel)` in
`crates/holon-kitchen/src/file_format.rs` `parse` — never reaches the store; no
`file:`-schemed block exists in `block_raw` at all after a successful ingest
either, which is the same seam seen from the other side (see
[[2026-09-02-a-cook-recipe-loses-its-title-and-all-its-metadata]]).

The quarantine in `crates/holon-filesystem/src/file_sync_controller.rs` is
doing its job — it stops the truncated DB state being rendered over disk — but
it quarantines the file while leaving the block that represents it, and that
block is what the write-back gate keeps tripping over.

## Missing piece

No test asserts what the store contains after a REFUSED ingest. The kitchen
suite tests refusals at the adapter boundary
(`crates/holon-kitchen/tests/cook_ingest.rs`) and the real-boot ingest on the
happy path (`crates/holon-integration-tests/tests/cook_vault_ingest.rs`), but
nothing puts an unparseable `.cook` file in a real vault and then asks whether
a block was left behind. The keystone has no `.cook` transition at all.

## Remedy

OPEN. The fix is that a refused ingest leaves no block for that path: either
the walk defers block creation until the adapter has parsed, or the quarantine
removes the block it was about to fill. The closing test belongs in
`cook_vault_ingest.rs` — boot a real vault holding one unparseable `.cook`
file, assert no block names it and no WRITE-BACK REFUSED is emitted — red
before the fix.
