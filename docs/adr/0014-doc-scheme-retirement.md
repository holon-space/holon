# ADR 0014: `doc:` URI scheme retirement

**Status:** Accepted (2026-07-02; closes BlockEventStorm hotspot H7)
**Deciders:** Martin
**Context:** BlockEventStorm H7 — "a block is a page" had three coexisting
encodings: the `"Page"` tag (`Block::is_page()`, canonical), the `doc:` URI
scheme, and the `set_is_document` op. `Block::is_document()` was already deleted
(2026-06-28); this ADR retires the remaining two.
**Relates to:** ADR 0003 (all-in-LoroTree — pages are ordinary tree blocks),
CONTEXT.md §4 synonym registry, `docs/Architecture/BlockEventStorm.md` §4 H7.

## Problem

A "document"/page never was a distinct entity kind — it is a block tagged
`Page` whose title is the first content line. But the `doc:` scheme encoded
page-ness *in the identifier*, so every consumer had a second, divergent way to
ask "is this a page?", and the link parser minted `doc:`-schemed UUIDs for
creation-intent wiki links that the rest of the system stored as `block:`.

## Decision

The `block:` scheme is the only entity-id scheme for blocks **and** pages.
`doc:` is removed from mint, read, and acceptance paths in one cut — no
compatibility read arm remains:

- **Mint stopped** (`crates/holon-api/src/link_parser.rs`): `classify_link`
  creation intents mint `block:` (was `doc:`); the `doc:`-prefix
  Resolved-acceptance arm is deleted. Name-linkage UUIDs are **unchanged**:
  `deterministic_entity_id` hashes `normalize_for_hash(target)` only — the
  scheme was never a hash input.
- **Read arms deleted** (`crates/holon-api/src/entity_uri.rs`):
  `as_document_id()` (deprecated) removed; `from_raw` no longer accepts a
  `doc` scheme for colon-leading ids; `new()`'s double-scheme guard no longer
  lists `doc:`.
- **Op deleted**: `set_is_document` (`crates/holon-core/src/traits.rs`) — zero
  callers; page promotion/demotion is a plain `tags` edit (`PAGE_TAG`).
- **Silently-empty query deleted**: the `roots` PRQL stdlib relation
  (`crates/holon/sql/prql_stdlib.prql`) filtered `parent_id starts_with "doc:"`
  — verified empty against the live DB and unmatched forever once the mint
  stops. It had no consumers; deleted rather than redefined.
- A negative tooth stays: `link_parser::tests::test_doc_scheme_no_longer_resolved`
  asserts a `doc:`-prefixed target is a creation intent, not a Resolved link.

## Data migration

None needed. Verified before the cut:

- The live vault, Loro snapshot, and active Turso DB are effectively
  `doc:`-free; a grep over `~/Workspaces/pkm/holon-pkm/` found **zero**
  `doc:<id>`-shaped link targets (no-op — no vault commit was needed).
- The abandoned legacy store `~/Library/Application Support/space.holon/`
  (~6k `doc:` rows) was decided (Martin, 2026-07-01) to be deleted outright,
  and **has been deleted** (executed 2026-07-02 by the main session,
  user-confirmed).

## Consequences

- `EntityUri` schemes in the tree: `block:`, `file:` (transient, parse-time
  only), `sentinel:no_parent`, plus external URL schemes. Exactly one way to
  encode page-ness: the `Page` tag.
- `grep -rn '"doc:' crates/` matches only the frozen turso repros
  (`crates/holon/examples/turso_ivm_*`, `tests/turso_storage_repros/` — kept
  byte-exact as upstream bug reproductions) and the negative tooth above.
- Any future `doc:` string entering via an old org file resolves through
  `from_raw`'s non-entity-scheme path or classifies as a creation intent —
  it will *visibly* mint a new page rather than silently aliasing an old id
  (fail loud, never fake).
