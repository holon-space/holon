---
id: 2026-08-11-org-drawer-key-named-collides-storage
date: 2026-08-11
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An org drawer key named `:properties:` collides with the storage column of
  that name, so its authored value is merged into the block's property map.
source_line: 728
---

## Bug

(task #2 ingest property-merge follow-on; found by an AGENT probing the F4
hazard the lane brief flagged; no automated test produced it) **An org
drawer key named `:properties:` collides with the storage column of that
name, so its authored value is merged into the block's property map.**
`partition_params` reads a `properties` param as the block's EXISTING
properties JSON, so `:properties: {"injected": "yes"}` injects `injected`
into the stored map and a non-JSON value is silently dropped. Pre-existing;
surfaced because the removal fix added a third manifestation (its
`Value::Null` sentinel is swallowed by the same arm). F4 proper — a
duplicate `properties` column in one SET clause — is confirmed UNREACHABLE:
the `properties` arm consumes the key before the known-columns arm.

## Root cause

task #2 ingest property-merge follow-on, found by an AGENT probing the F4
hazard the lane brief flagged — no automated test produced it: **an org
drawer key literally named `:properties:` collides with the STORAGE COLUMN
of that name, and its authored value is merged into the block's property
map.** `build_block_params` emitted the drawer key verbatim, and
`SqlOperationProvider::partition_params` reads a `properties` param as the
block's EXISTING properties JSON (`sql_operation_provider.rs:359-371`, the
arm that precedes the known-columns arm) — so `:properties: {"injected":
"yes"}` merges `injected` into the stored map (measured:
`Some(String("{\"injected\": \"yes\"}"))` reaching the params), and any
non-JSON value is dropped by the `if let Ok(map)` with no signal.
Pre-existing, NOT introduced by the removal fix; found because that fix
added a third manifestation (the `Value::Null` removal sentinel for this key
is swallowed by the same arm). The F4 hazard the brief warned about — a
duplicate `properties` column in one SET clause — is separately confirmed
UNREACHABLE: the `properties` arm CONSUMES the key before the known-columns
arm, so it can never reach `sql_fields` and `update_pairs` gets exactly one
`properties` entry, from the merge block. COVERAGE primary: the keystone's
custom-property alphabet is a fixed six-name list
(`generators.rs:1030-1036`: effort, story_points, column-order, collapse-to,
ideal-width, column-priority) that contains no storage-column name, so the
collision is ungeneratable. Not an ORACLE gap: `inv-blocks-match-ref`
compares the properties map, so an injected key present in the store and
absent from the ref would have convicted. Missing piece: a drawer-key
alphabet arm that draws names colliding with storage columns. Fixed by
refusing the key OUT LOUD at the org boundary (`refuse_column_collision`,
WARN naming the block and the remedy) in BOTH directions — never emitted,
never nulled; pinned by
`block_params::tests::a_properties_drawer_key_never_reaches_the_params`.)

## Missing piece

The keystone's custom-property alphabet is a fixed six-name list
(`generators.rs:1030-1036`) containing no storage-column name, so the
collision is ungeneratable. Not an ORACLE gap: `inv-blocks-match-ref`
compares the properties map and would have convicted an injected key.
Missing piece: a drawer-key alphabet arm drawing names that collide with
storage columns.

## Remedy

FIXED — `refuse_column_collision` drops the key at the org boundary with a
WARN naming the block and the remedy (disclosed fallback, not a silent one),
in both the emit and the removal direction. Pinned by
`block_params::tests::a_properties_drawer_key_never_reaches_the_params`. GAP
NOT CLOSED: the colliding-name alphabet arm is keystone work, fenced out of
this lane.
