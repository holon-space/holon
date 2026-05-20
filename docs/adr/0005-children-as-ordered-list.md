# ADR 0005: Children-as-ordered-list (`sort_key` is an adapter detail)

**Status:** Proposed (2026-05-27)
**Deciders:** Martin
**Context:** Domain modelling for block sibling order

## Problem

`holon_api::block::Block` carries a `sort_key: String` field that holds a hex-encoded Loro fractional index (consumed by `gen_key_between`). This is:

- A **storage encoding** chosen by one adapter (Loro), leaking onto the domain entity that every other adapter, every actor, and every test must also carry and reason about.
- Required by `Block::sort_key()` to avoid panics in `gen_key_between` (the previous bug where the fallback to `self.id.as_str()` exposed non-hex characters).
- Conceptually wrong: when two adapters disagree about ordering encoding (Loro fractional index vs. Org document position vs. Markdown line order vs. Turso column), the domain has no canonical answer — the truth must come from whichever adapter is currently authoritative under the DI graph.

The semantic concept users care about is **"children of a parent are a list with order"**, full stop. Mutations are *"insert this child after that one"*, *"move this block after that one"*, *"reorder these siblings"*. No user expresses an intent in terms of a sort_key string.

## Decision

The block domain represents sibling order as **an ordered list of child ids per parent**. The domain entity `Block` does *not* carry a `sort_key` field. Sibling ordering is a property of the parent-child relation, not of the block itself.

### Domain-level vocabulary

The mutation operations are:

- `Create { id, parent, after: Option<EntityUri>, init }` — `after = None` means *first child*.
- `Move { id, new_parent, after: Option<EntityUri> }` — reparent and/or reorder.
- `MoveAfter { id, after: Option<EntityUri> }` — reorder within the same parent.

**Precondition contract (all operations):**

| Condition | Outcome |
|---|---|
| `after == Some(id)` (self-reference) | Reject with `InvalidMove::SelfReference` |
| `after` is not currently a child of the target parent | Reject with `InvalidMove::AfterNotSibling` |
| `id` is an ancestor of `new_parent` (would create cycle) | Reject with `InvalidMove::WouldCreateCycle` |
| `id` does not exist / `parent` does not exist | Reject with `InvalidMove::Missing` |

Cycle detection is a domain-level check, performed before dispatch to any adapter. Adapters MAY re-check (defense in depth) but MUST NOT be the sole guard.

**Concurrent moves under Loro:** when two peers issue `MoveAfter(X, after=A)` and `MoveAfter(X, after=B)` concurrently, the domain accepts *both as legal* and the post-merge order is whatever Loro's fractional-index conflict resolution produces. The domain does NOT pretend ordering is deterministic under concurrency — this is an explicit acknowledged leak from the adapter into the order semantics. PBT invariants must check convergence (both peers agree on *some* order) not predetermined order.

`CoreOperations` gains:

```rust
fn children_of(&self, parent: &EntityUri) -> Vec<EntityUri>;
fn children_of_window(&self, parent: &EntityUri, after: Option<&EntityUri>, limit: usize) -> Vec<EntityUri>;
```

`children_of` is the convenience full-read; `children_of_window` is the cursored variant for large parents (calendars, imports, long journals) — it MUST be used on any read path that can hit O(thousands) of children. Hot UI paths (virtualized lists) take the windowed form. `children_of` returns the authoritative order as a list. Implementations:

- `MemoryBackend` stores `BTreeMap<EntityUri, Vec<EntityUri>>` (child order per parent).
- `LoroBackend` reads fractional indices and returns blocks sorted by them.
- Org adapter returns blocks in document order.
- Markdown adapter returns blocks in line order.
- Turso adapter sorts its internal `sort_key` column and returns the resulting list.

### Canonical sibling order is grouped: section content before headings

The ordered child list is **not** free interleaving. It is canonically defined as:

> **section content (`Source`/`Image`) first, then headings (`Text`), with insertion/`after` order preserved within each group.**

This is a structural requirement of the outline formats, not an aesthetic choice. In org-mode and Markdown a heading captures everything after it until the next heading, so a non-heading child placed *after* a sibling heading would re-parent under that heading on the next read. The only round-trip-stable encoding is "section content before child headings".

Decision (was an open question; resolved 2026-05-29): the domain **adopts** this grouping as its order semantics and **auto-normalizes** to it rather than rejecting the interleaving. Authoring `[heading A, snippet, heading B]` under one parent yields the canonical order `[snippet, heading A, heading B]` — the source is pulled ahead of its sibling headings. (Rejecting it outright — failing the mutation — was considered and declined: every round-trip already converges to the grouped order, so rejection would surprise users for something that "worked".) Alternatives that preserve literal interleaving (explicit `:PARENT:`/`outdent` properties, synthetic wrapper headings) were rejected because they make the file's visual nesting disagree with the real tree, breaking external (Emacs) editability — a core invariant.

This grouping is **one rule**, `ContentType::sibling_order_group()` (`holon-api`), through which every renderer (org, Markdown) and the reference oracle order siblings — so the rule cannot drift between adapters (it previously did: renderers grouped `Source`+`Image`, the reference grouped only `Source`).

### "Authoritative adapter under the current DI graph"

When code needs a position-sensitive value (rare — most callers want the list), it asks the authoritative adapter via `children_of`. When multiple adapters are wired, the wiring manifest (ADR 0007) declares which is authoritative for ordering. The cross-adapter convergence invariant (ADR 0004) ensures they agree after quiescence.

### `sort_key` lifecycle

- `holon_api::block::Block.sort_key` field is **removed**.
- `default_sort_key()` is removed.
- The Turso adapter keeps a `sort_key` column *internally* — it is no longer exposed on the domain entity.
- The Loro adapter keeps fractional indices *internally* — never exposed on the domain entity.

### Reference-side representation

`pbt_infrastructure::MemoryBackend` already maintains child relationships; it gains an explicit ordered child list per parent (replacing whatever current ad-hoc ordering it uses).

`assign_per_parent_sort_keys` / `assign_reference_sequences_canonical` (currently in the integration-tests reference) are deleted as part of Phase 8 of the migration — the reference no longer needs to compute sort_keys because no domain consumer asks for them.

## Consequences

- Every site that reads `block.sort_key` becomes a compile error in Phase 8 of the migration. The compiler-driven rewrite forces each site to either ask `backend.children_of(parent)` (the common case) or, if it really needs an encoding (extremely rare — round-trip equivalence checks within an adapter), route through that adapter's own internal API.
- The integration-tests PBT loses the ability to assert sort_key-byte equality between backends; replaces it with "child position in `children_of(parent)`" equality, which is the actually-domain-meaningful assertion.
- Generators get simpler: instead of generating sort_key strings, they pick an `after: Option<EntityUri>` cursor from the existing children list.

## Resolved decisions (Phase 8 execution, 2026-05-29)

1. **Read API: `children_of` everywhere.** Order-dependent read sites are rewritten to call `children_of(parent) -> Vec<EntityUri>` explicitly rather than relying on adapters returning a pre-sorted `Vec<Block>`. Order becomes a first-class query. This is the larger but cleaner change; accepted deliberately.
2. **Persistence: keep the fractional index persisted internally.** Loro keeps writing its fractional index under its existing node property; the Turso `blocks`/`block_matview` schema keeps its `sort_key` column. Only the domain `Block` struct stops carrying the field. No on-disk Loro migration and no Turso schema change — this also closes ADR 0004 open item #2 (serialization compatibility) for the ordering field: old data is read unchanged, the field is simply no longer surfaced on the domain entity.
3. **Monotonicity invariants land in Phase 8.** The per-adapter "internal ordering encoding stays monotone vs the order returned by `children_of`" invariants ship together with the field removal, so cross-backend coverage never regresses (no green-line gap).
4. **Delete-first, no deprecation window.** Per the project's refactor-completely rule, the `Block.sort_key` field is deleted outright and the compiler enumerates the call sites; no `#[deprecated]` migration window (overrides the weakness-3 hedge below).

## Known weaknesses / open questions

1. **Position-equality is weaker than byte-equality.** The current PBT compares `sort_key` bytes across backends, catching encoding drift (cluster #6 / #7 ancestry). Replacing this with `children_of` position equality is *weaker*. Mitigation (now committed — see Resolved #3): every adapter that carries an internal ordering encoding (Loro fractional index, Turso `sort_key` column) MUST additionally publish a per-adapter invariant that its internal encoding remains monotone for the order returned by `children_of`. These per-adapter invariants are non-negotiable additions to Phase 8.

2. **Org/Markdown "document order" is not stable under reparenting until next save.** Between a `Move` and the next file write, the in-memory adapter's children list and the file's line order disagree. `children_of` reads in-memory state (not file); the file becomes consistent only after the next flush. Phase 8 must document this and the file-watcher must not re-read mid-flush.

3. **Blast radius.** 102 files reference `sort_key`; 8 read `.sort_key()` directly. The "compiler will drive the rewrite" is true but underspecified. Phase 8 must (a) land `sort_key` as `#[deprecated]` first with a migration window, (b) provide a per-callsite triage table (PRQL templates, render_eval, change_set, org parser, integration tests), (c) avoid a single flag-day commit.

## Migration

This ADR is realized by **Phase 8** of the componentization migration. See the plan file.

## References

- ADR 0004 — domain/adapter split; this ADR is one concrete instance.
- ADR 0003 — LoroTree architecture; Loro's fractional indices stay inside its adapter.
