# Handoff — Item 4 deeper half: retire `Block::sort_key()` from the read side

Date: 2026-05-11

## Handoff prompt (paste into a new session)

> Item 4's write side is done — `BlockOrdering` trait encapsulates the
> `gen_key_between` + paired `set_field` path in `SqlBlockOperations`, and
> `holon-core::traits` no longer imports `gen_key_between`. The read side
> remains: `Block::sort_key()` is still observed by external code paths
> (Org renderer, event payloads, SQL `ORDER BY`, sibling-walking helpers).
> If a third backing for blocks materialises (or the column otherwise
> starts to bite), the next slice is to hide reads behind a
> `BlockOrdering::siblings_ordered(parent_id) -> Vec<block_id>` (and
> `prev/next_sibling`) so only the SQL adapter and the
> `LoroSyncController` outbound projector touch the string.
>
> **Concrete first step**: grep `Block::sort_key()` call sites and
> partition into "ordering reads" (most of `BlockQueryHelpers` /
> `BlockMaintenanceHelpers` defaults) vs "data plumbing" (event payloads,
> JSON, SQL `block_to_params`). The ordering reads are the migration
> target; the data-plumbing reads stay (they're persisting the value, not
> using it for ordering decisions).
>
> Read this file, then `devlog/2026-05-11-item4-phase1-3a-write-position.md`
> for the write-side context. Memory:
> `~/.claude/projects/.../memory/phase3_4_sortkey_via_loro_internal_index.md`.

## What's already done (do not redo)

- `BlockOrdering` trait at `crates/holon-core/src/block_ordering.rs` with
  `place(uri, parent, after_id)` and `new_child_anchor(parent, after_id)`.
- `BlockOperations::ordering()` accessor (default `None`; production
  impls override).
- `SqlBlockOperations` impls `BlockOrdering` directly (encapsulates the
  Loro `write_position` vs SqlOnly `gen_key_between` + paired `set_field`
  conditional).
- `MemStore` (test substrate in `block_operations_tests.rs`) impls
  `BlockOrdering` directly.
- `compute_sort_key_between_neighbors` deleted from
  `crates/holon-core/src/traits.rs`.
- `move_to_position` is a thin 3-line delegation; `split_block` no
  longer computes `fallback_sort_key` — calls `ordering().new_child_anchor`.
- `gen_key_between` is no longer imported in `holon-core::traits`. It
  survives at three call sites: `holon-core::fractional_index` (its
  definition), `SqlBlockOperations::new_child_anchor`, and `MemStore`.

## What's open

`Block::sort_key()` is still observable to the read side. Three classes
of reader:

### 1. Ordering-decision reads (the migration target)

These compare or sort by `sort_key` to make positional decisions. They
should route through `BlockOrdering`.

Concrete sites (grep `\.sort_key\(\)` in `crates/holon-core` and the
production crates that depend on `BlockEntity`):

- `BlockMaintenanceHelpers` / `BlockQueryHelpers` default methods like
  `get_prev_sibling`, `get_next_sibling`, `get_first_child`,
  `get_last_child` — they all do
  `.filter(|s: T| s.sort_key() < block.sort_key())` then `min/max_by`.
- `traits.rs::indent`'s `block_children.last().sort_key()` pattern (now
  migrated to `last_after_id` block ids in `join_block` — the prior
  pattern survives elsewhere).
- Anywhere in `holon` / `holon-orgmode` / `holon-frontend` that does
  `children.sort_by(|a, b| a.sort_key().cmp(b.sort_key()))`.

### 2. Data-plumbing reads (keep as-is)

These propagate the value without making ordering decisions:

- Event payloads — `SqlOperationProvider::build_event_payload` reads
  `block.sort_key` into the JSON. Required for CDC.
- SQL projection — `block_to_params` / `read_block_from_tree` set the
  `sort_key` column.
- Org renderer — currently sorts children by `sort_key` before rendering
  (this is technically an ordering read, but it's a leaf consumer).

Decision: keep these. The encapsulation goal is "no decisions outside
`BlockOrdering`", not "no string ever surfaces".

### 3. Inbound CDC writes (separate concern)

`BlockCellRegistry::compute_position_for_sort_key` + the registry's
`"sort_key"` arm in `write_field` translate inbound non-Loro sort_key
strings into positional moves (`apply_sort_key_hint`). Stays as long as
non-Loro origins (Org parser, Todoist, external peers) ship sort_key
strings. Could be retired when all upstream origin tagging migrates to
typed positional intents — pair with the item-3 follow-up.

## Suggested trait surface

Extend `BlockOrdering`:

```rust
#[async_trait]
pub trait BlockOrdering: Send + Sync {
    async fn place(&self, uri: &EntityUri, parent_id: &str, after_id: Option<&str>) -> Result<()>;
    async fn new_child_anchor(&self, parent_id: &str, after_id: Option<&str>) -> Result<String>;

    // NEW — read side:
    async fn siblings_ordered(&self, parent_id: &str) -> Result<Vec<String>>;
    async fn prev_sibling(&self, id: &str) -> Result<Option<String>>;
    async fn next_sibling(&self, id: &str) -> Result<Option<String>>;
    async fn first_child(&self, parent_id: &str) -> Result<Option<String>>;
    async fn last_child(&self, parent_id: &str) -> Result<Option<String>>;
}
```

Implementations:
- `SqlBlockOperations::BlockOrdering` — for Loro mode, queries
  `tree.children(parent_tree_id)` in order (Loro's RGA gives canonical
  ordering for free); for SqlOnly mode, queries the cache + sorts by
  `sort_key` (the same logic that's already in
  `BlockMaintenanceHelpers::get_prev_sibling` etc.).
- `MemStore::BlockOrdering` — wraps existing `sorted_children`.

Then `get_prev_sibling` / `get_next_sibling` / `get_first_child` /
`get_last_child` in `BlockMaintenanceHelpers` become thin delegations to
`self.ordering().prev_sibling(...)` etc. The default impls that read
`Block::sort_key` directly go away.

## Verification baseline (must stay green)

```
cargo check --workspace --tests                                                    GREEN
cargo test -p holon-core --lib                                                     47/47
cargo test -p holon --lib sync::loro_sync_controller                               16/16
cargo test -p holon-integration-tests --features otel-testing
    --test phantom_loro_exists_repro                                                2/2
cargo test -p holon-integration-tests --test inbound_runtime_gate                   3/3
RUST_LOG=error PROPTEST_CASES=1 cargo test -p holon-integration-tests
    --test general_e2e_pbt general_e2e_pbt -- --nocapture                          2/2 ~8-9min
```

## Why this is "if-and-when" rather than blocking

The current state is internally consistent: chord-op writes are
positional intents; the SQL column persists a sort_key for ORDER BY and
the read side observes it. Hiding the read side gains:

- Resilience to future backings (e.g., a graph DB or different CRDT)
  that wouldn't have a fractional-index string at all.
- Cleaner mental model — `BlockOrdering` becomes the only positional
  surface, not just the only write surface.

It costs: a non-trivial refactor that touches many call sites and may
ripple into the Org renderer's child-ordering pass. Worth doing only if
one of the wins becomes load-bearing.

## Files to read

- `crates/holon-core/src/block_ordering.rs` — trait shape.
- `crates/holon-core/src/traits.rs` — `BlockMaintenanceHelpers`,
  `BlockQueryHelpers` defaults are the migration target.
- `crates/holon/src/core/sql_block_operations.rs` — reference impl.
- `crates/holon-core/src/block_operations_tests.rs::MemStore` — test
  substrate impl to mirror in the read-side extension.
