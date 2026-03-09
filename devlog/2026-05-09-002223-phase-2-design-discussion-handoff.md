# Handoff — Phase 2 mid-stream, design discussion open

## Where we are

Stack on the current branch (uncommitted to remote):

```
@  rokmplpx (empty)                — placeholder for next step
○  vwzowtxv 56495cf9               — Phase 2 step 3: drop _expected_content watermark
○  xtytokul b0252de4               — Phase 2 step 2: route split/join/embed_entity content via Loro
○  zpxvpplt 2a8d3999               — Phase 2 step 1: handle_text_sync no-op for content
○  qownouvo 2e481cc4               — Phase 0+1: BlockContentResolver + headless mirror + reproducer
○  uqtvotzp 48cee3d6                — fix: PBT panics + Turso IVM bug capture (parent of branch)
```

Recipe stays GREEN (~540s for 2 cases on Full + SqlOnly) at every commit.

The deterministic `bulk-1-0 = "LM"` random-seed flake is **NOT** a
Phase 1 / Phase 2 regression — same panic shape pre-Phase-1 and on
Phase 2 step 1, and the loud `bail!` in `headless_editor_mirror`
never fires (chars do reach Loro's `MutableText`). Documented in
`devlog/2026-05-08-220908-…`. Track separately.

## The user's concerns from the last turn — read these carefully

The user pushed back on my Phase 2 framing. Their observations are
sharp and the next session should engage them on each one:

1. **Same trait for Loro and Turso?** "Would it be possible that Loro
   and Turso implement the same trait(s) and the decision which one
   is the leading system is just a matter of how we wire them up?"
   → Almost certainly yes, and that's the cleaner architecture. The
   current `BlockContentResolver` is shaped around Loro's
   `MutableText` and bolted on as an option; if we lift it to a
   `LiveContentStore` (or similar) trait that both Loro and Turso
   implement, the "Loro is optional" property becomes a wiring
   choice rather than a bolt-on with fallbacks.

2. **My "Loro not up to date in Full mode" claim was wrong.** The
   user pointed out: `apply_fields_changed` is called from
   `on_inbound_event` for every block event regardless of origin
   (with origin=Loro skipped for echo suppression). External
   writers (org parser / MCP) write SQL, CDC fires, the `loro`
   consumer applies the change to Loro. Loro stays up to date.
   So step 5 ("drop inbound SQL→Loro for content") was wrong even
   in the originally-framed sense. Inbound stays.

3. **`set_live_content` vs `set_field` may not deserve to be two
   methods.** "The signature looks almost identical (except for `&str`
   vs `Value`). Do we need these two as separate methods?"
   → Their semantics ARE the same: "write content for block X". The
   difference is purely implementation: which storage layer accepts
   the write. The split came from my needing a Phase-1-compatible
   way to short-circuit the SQL path; that distinction shouldn't
   leak into the trait.
   - `set_field(id: &str, field: &str, value: Value) -> Result<OperationResult>` (async)
   - `set_live_content(block_id: &str, field: &str, new_value: &str) -> Result<bool>` (sync)
   The `bool` return is "did Loro accept" — that's a wiring concern,
   not an API concern. A single `set_content(...)` (or even just
   `set_field` with the leading store doing the right thing) is the
   right shape.

4. **Is `BlockContentResolver` actually a good abstraction?** "It fits
   the `MutableText` wrapping Loro nicely, but apart from that it
   just seems to have the same semantics as listening to the event
   bus for changes with some kind of caching and delay so we don't
   write every single character. But maybe even that would be fine
   for Turso?"
   → Sharp. `live_content(id, field) -> Option<String>` is just
   "current value of this field on this block". For Turso that's
   `SELECT content FROM block WHERE id = ?` (or a debounced
   in-memory cache). For Loro it's `LoroText.to_string()`. Same
   shape, different backends. The "live" prefix is a smell — it's
   really just a read-side cache that beat-by-beat reflects writes
   from one specific writer (Loro's MutableText).

## What the next session should do

This is a design discussion. Don't rush into code. The right order:

### 1. Decide the trait shape

The unified abstraction is something like:

```rust
pub trait LiveContentStore: Send + Sync {
    fn read(&self, block_id: &str, field: &str) -> Result<Option<String>>;
    fn write(&self, block_id: &str, field: &str, new_value: &str) -> Result<()>;
}
```

- `LoroLiveContentStore` reads/writes via the Loro tree's
  `content_raw` LoroText. Writes commit the doc; the existing
  `on_loro_changed` outbound projector sees the diff and emits the
  SQL UPDATE (origin=Loro).
- `TursoLiveContentStore` reads via SELECT (or a CDC-driven
  in-memory mirror), writes via `set_field("content", new_value)`
  on the SqlOperationProvider. Inbound consumers don't apply back
  to itself (no echo).
- `WireUp::leading_store` chooses which one chord ops, the editor,
  etc. talk to. SqlOnly = TursoLiveContentStore. Full = LoroLive.

The current `BlockContentResolver` collapses into `LiveContentStore`,
the `set_live_content` method on `BlockOperations<T>` becomes a
single `live_store().write(id, field, value)` call. The "fallback"
language goes away — chord ops always call the leading store.

### 2. Question to settle before coding

Should chord ops know about `LiveContentStore` at all, or should they
keep calling `self.set_field("content", …)` and have the SqlBlockOperations
internally route to the leading store? The latter is more invisible
(callers don't change), the former is more explicit. The previous
approach (Phase 2 step 2) was the former; the trait-unified version
might pull either way.

Related: **OperationResult.changes contract**. Today, `set_field`
returns synchronous `FieldDelta`s for the immediate UPDATE.
`set_live_content` returns `Ok(bool)` with no changes — they arrive
asynchronously via on_loro_changed → events. If we collapse to one
method, we have to pick: does it return changes synchronously
(SQL-flavored) or via events (Loro-flavored)? Downstream consumers
to audit: `crates/holon/src/core/operation_wrapper.rs:98`
(`sync_provider.sync_changes(&result.changes)`) is the most concrete.

### 3. After the trait is settled

Replay Phase 2 step 2 + step 3 on top of the new trait. Steps 4 and
5 as I originally framed them are wrong (my mistake — see the
user's points 2+3). Phase 2 should end at "the leading-store wiring
chooses Loro or SQL; chord ops call one method that does the right
thing; `_expected_content` is gone (already done); inbound
SQL→Loro stays for non-leading-store writes (org/MCP)".

### 4. Memory + devlog updates

Once the trait shape is settled, update the
`devlog/2026-05-08-220908-…` "What's still pending" section to
reflect the new framing. The MEMORY.md entries about Phase 1 are
still accurate.

## Files to look at, not exhaustively

- `crates/holon-core/src/traits.rs:498-547` — `BlockContentResolver`
  + `BlockOperations::live_content` / `set_live_content`. This is
  what the unified trait would replace.
- `crates/holon/src/sync/loro_block_content_resolver.rs` — Loro
  impl. `set_content` wraps `LoroText::update`; `live_content`
  reads `LoroText::to_string`. Becomes `LoroLiveContentStore`.
- `crates/holon/src/core/sql_block_operations.rs:87-99` —
  `SqlBlockOperations` overrides `live_content` and
  `set_live_content` to delegate to its `Option<Arc<dyn
  BlockContentResolver>>`. Becomes "leading store" wiring.
- `crates/holon-core/src/traits.rs:730-870` — `split_block` +
  `join_block` callsites that route content writes through
  `set_live_content` with `set_field` fallback. Collapse to a
  single call once the trait is unified.
- `crates/holon-core/src/traits.rs:1078-1115` — `embed_entity`
  same pattern.
- `crates/holon/src/core/sql_operation_provider.rs:529-540` —
  `_expected_content` was here, now removed.
- `crates/holon/src/sync/loro_sync_controller.rs:533-560` —
  outbound `_expected_content` emission, removed.

## Open random-seed flake (do not block on)

`bulk-1-0 = "LM"` (prod) vs `"LM lX8G"` (ref) deterministic
divergence on a specific seed when running `cargo test
general_e2e_pbt` without the recipe weights. The chord ops chain
runs: BulkExternalAdd → FocusEditableText → PinBlock → SplitBlock,
no TypeChars between BulkExternalAdd and the panic. The CDC
watcher consistently sees `content="LM"` from the very first
emission for that block — meaning the org-renderer round-trip
write from the BulkExternalAdd generator already lands "LM" in
SQL while ref retains "LM lX8G" via `normalize_content_for_org_roundtrip`.
Probably a renderer bug specific to content shapes containing a
space + uppercase letters. Pre-existing, not Phase 2.

## What the user's last messages added that's NOT in this devlog yet

(Quoting verbatim so it survives the context wipe.)

> Would it be possible that Loro and Turso implement the same
> trait(s) and the decision which one is the leading system is
> just a matter of how we wire them up?
>
> I'm not sure I get your point about Loro not being up to date
> in Full mode. `apply_fields_changed` is called from
> `on_inbound_event` as a reaction to events on the event bus.
> That should work no matter where the events come from.
>
> What do you mean with `set_live_context` -> `set_field`
> fallback? What is the difference between the two? The signature
> looks almost identical (except for `&str` vs `Value`). Do we
> need these two as separate methods?
>
> I'm not sure the `BlockContentResolver` is a great abstraction.
> It fits the `MutableText` wrapping Loro nicely, but apart from
> that it just seems to have the same semantics as listening to
> the event bus for changes with some kind of caching and delay
> so we don't write every single character. But maybe even that
> would be fine for Turso?

The next session should engage these directly before writing any
new code. My read is the user is pointing toward a unified
`LiveContentStore` trait with Loro and Turso impls behind one
"leading store" wiring decision — and they're right.

---

## CLOSING ADDENDUM (2026-05-09 — Phase 2 LANDED)

Resolved by the cells plan (`~/.claude/plans/ok-i-think-we-snappy-pnueli.md`)
and Phase 2 of its execution. The "unified live-content store" question
landed as: `BlockCellRegistry` is the per-entity-type registry, `Cell<T>`
is the universal reactive read/write primitive, and Loro is sole writer
for SQL block columns via `BlockCellRegistry::write_field` →
`LoroSyncController.on_loro_changed`. The "leading store" decision is
authority-first / projection-driven: Loro authoritative, SQL projected.
See `devlog/2026-05-09-175751-phase2-authority-flip-landed.md` and
MEMORY entry `cells_and_loro_authority.md`.
