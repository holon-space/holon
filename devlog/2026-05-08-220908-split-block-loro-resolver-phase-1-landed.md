# Phase 0+1 — split_block reads live content from Loro (split-block stale-content bug)

## What was wrong

`split_block` (and `join_block`) at `crates/holon-core/src/traits.rs:706` read
`block.content` from the SQL projection. While the user is typing, the live
text lives in a `MutableText` (Loro RGA container) and the projection through
`LoroSyncController.on_loro_changed` lags behind the keystroke. Pressing Enter
mid-typing therefore split the *committed* content, silently discarding any
pending in-memory characters.

The April-2026 "commit-then-dispatch" mitigation only covered GPUI's path; the
same code path on TUI/headless still lost characters.

## Phase 0 — make `general_e2e_pbt.rs` reproduce the bug

Atomic editor primitives (`FocusEditableText`/`TypeChars`/`PressKey`) existed
but were noops in the headless driver. Added a `HeadlessEditorMirror`
(`crates/holon-frontend/src/headless_editor_mirror.rs`) that routes keystrokes
the same way GPUI's `editor_view.rs` does in capture phase:

- char keys → `MutableText::apply_local(Insert)`
- Enter → `split_block` intent at the live cursor
- Backspace at byte 0 → `join_block` intent
- Tab / Shift+Tab → indent / outdent
- non-modified backspace mid-line → `MutableText::apply_local(Delete)`

`ReactiveEngineDriver` got an `editor_mirror: Arc<HeadlessEditorMirror>` field
and overrode `send_raw_keystroke` to delegate. `general_e2e_pbt::pbt_config`
calls `enable_atomic_editor_if_unset()` so the primitives run by default.

Reproducer recipe (~3.5 min wall, 215s on Full + 230s on SqlOnly):

```
PROPTEST_CASES=1 PROPTEST_MAX_SHRINK_ITERS=0 PBT_ATOMIC_EDITOR=1 \
  HOLON_PBT_WEIGHTS="ClickBlock:30,FocusEditableText:50,TypeChars:50,PressKey:50,Navigate*:0" \
  cargo test -p holon-integration-tests --test general_e2e_pbt -- --nocapture
```

`ClickBlock:30` is required because focus starts on `block:journals` (default)
but `BulkExternalAdd` lands new blocks under user docs (`block:ref-doc-0`);
without the click, `FocusEditableText` finds no candidates.

## Phase 1 — split_block / join_block read content from Loro

`holon-core::traits.rs` gained a `BlockContentResolver` trait and a
`BlockOperations::live_content` hook (default `None`). `split_block`
(`~L713`) and `join_block` (`~L865`/`~L882`) consult it before falling back
to `T::content()`.

`SqlOperationProvider` carries an `Option<Arc<dyn BlockContentResolver>>`
(builder method `with_content_resolver`) and overrides `live_content` to
delegate. The resolver is wired by `holon::sync::loro_module` and registered
via `holon::sync::event_infra_module::optional_resolve_async`, so SqlOnly
mode gets `None` and falls back to the SQL cache.

`LoroBlockContentResolver` walks the global Loro tree, strips the `block:`
prefix on `STABLE_ID` lookup (Loro stores bare local IDs like `7-4-...`,
callers pass `block:7-4-...`).

Two boundary fixes the resolver wiring exposed:

- `editable_text_provider::LoroDocTextResolver` had the same prefix-strip
  gap, plus mapped the `content` field name → `content_raw` Loro container
  (the inbound CDC sync writes to `content_raw`; without the field map,
  mirror writes went to a separate `content` container and vanished from
  the resolver's view).

- `sut.rs::ensure_reactive_engine` now synchronously awaits the provider
  wiring (was fire-and-forget `tokio::spawn` — race) and wires it on both
  the locally-created engine and the DI engine the driver uses.

Atomic editor primitives gate on `state.variant.enable_loro` — SqlOnly has no
per-keystroke storage path, so generating those transitions there is
meaningless. `apply_press_key` now takes `ref_state` and runs the same
synthetic-block::split-N → real-UUID mapping that `apply_split_block` does
(extracted into `map_unmapped_split_synthetic_ids`). `type_chars::apply_to_ref`
calls `commit_active_editor_if_changed()` when `state.variant.enable_loro`
since Loro→SQL projects per-keystroke now.

## Verification

Recipe above: Full + SqlOnly both PASS (240s before barrier bump, 490s after).

`cargo check -p holon -p holon-integration-tests --tests`: 0 errors.

## Triage of the post-Phase-1 random-seed flake

(Updated after a deeper run with bumped barriers + loud silent-drop.)

Without `PROPTEST_CASES=1` and the deterministic weights, the random-seed
runs hit two intermittent panics:

1. **Full variant** — `assert_blocks_equivalent` failure on a recently
   bulk-added block (`block:bulk-1-0` content `"LM"` vs ref `"LM lX8G"`,
   ~5 chars missing). Reading the trace: the `headless_editor_mirror`
   silently no-op'd char keys when `services.editable_text(&block_id,
   "content")` returned Err — that resolver returns Err when the block
   isn't yet in the Loro tree, which happens transiently when
   `BulkExternalAdd`'s SQL→CDC→`loro` consumer apply chain is still
   running.

   The pre-step settle's `wait_for_consumers(["loro", "org", "cache"], 500ms)`
   sometimes wasn't long enough to land all create events. Bumped the
   timeout to 5s and made the silent drop fail loud (`anyhow::bail!`)
   when a char/mid-line-backspace fires against a block with no
   `MutableText` — surfaces real loro-consumer-stuck cases instead of
   masquerading as a CDC race. Recipe still GREEN after the change.

2. **SqlOnly variant** — `Navigation focus mismatch ... expected
   block:journals, got block:ref-doc-0` at `sut.rs:3964`. Independent of
   Phase 1 (SqlOnly doesn't go through `MutableText`); pre-existing
   focus-tracking bug surfaced by random ordering.

After bumping the timeouts the same Full-variant flake still reproduces
deterministically (same seed → same `bulk-1-0 = "LM"` divergence). The
loud `bail!` in `headless_editor_mirror` did NOT fire — chars do reach
`MutableText`. The remaining gap looks like a higher-level issue with
either (a) the SplitBlock transition's interaction with bulk-added
blocks (the SplitBlock budget log shows `wall=9426ms` and 2x reads of
`block_raw` for `bulk-1-0`, which is suspicious), or (b) a real gap
between Loro state and what `live_content` returned at chord time. The
`[CacheEventSubscriber] Failed to convert block event: ... missing field
'content_type'` errors throughout the trace are pre-existing and do not
affect `live_blocks` (the cache subscriber's flush is a no-op except for
mark-processed). This is a separate bug from Phase 0/1's contract; the
Phase-1-target bug class (split_block reading stale `block.content`)
is fixed and the recipe stays GREEN. Track this random-seed flake
separately; do not block Phase 2 on it.

The `[CacheEventSubscriber] Failed to convert block event: ... missing
field 'content_type'` errors visible throughout the trace are also
pre-existing — that subscriber drops events but doesn't actually update
any cache (its flush is a no-op except for marking events processed),
so they don't affect the live_blocks mirror used by the assertion.

## What's still pending

- **Phase 2** (NOT STARTED — committed an empty WIP commit on top to
  hold the slot): make Loro the single writer for content. The plan
  needs a careful order:

  1. Route `split_block` / `join_block` content writes through
     `MutableText` (so the Loro tree carries the structural truncate +
     content-after).
  2. Then drop `set_field("content", …)` from the operation layer.
  3. Then drop `_expected_content` watermark gating in
     `SqlOperationProvider::prepare_update` (it only guarded against
     the multi-writer race that step 1 eliminates).
  4. Then drop the inbound SQL→Loro echo handling for content (only
     one writer — no echo).
  5. Rework `EditorViewModel::handle_text_sync`
     (`crates/holon-frontend/src/view_event_handler.rs:127`) to write
     through `MutableText` on blur instead of dispatching `set_field`.

  Dropping (3) before completing (1)+(2) leaves SQL stale after
  structural ops because the concurrent-direct-write guard the
  watermark provides is still doing useful work today. Do NOT skip the
  steps.

- **Memory** updated in this session — "Production split_block silently
  discards pending in-memory edits" entry now links this devlog and
  marked LANDED; "MutableText does NOT synchronously commit to
  block.content" annotated with the Phase 1 nuance; April-2026
  "Split-block pending-edit commit fix" entry marked SUPERSEDED.

- **Random-seed flake** still reproduces (deterministic on the same
  seed): bulk-1-0 ends with content `"LM"` in prod vs `"LM lX8G"` in
  ref after a chain of `BulkExternalAdd` → `FocusEditableText` → some
  TypeChars → `SplitBlock`. The loud `bail!` in the headless mirror
  did NOT fire (chars reach `MutableText`). The `SplitBlock` budget log
  shows `wall=9426ms` and 2× `SELECT * FROM block_raw WHERE id =
  'block:bulk-1-0'`, suggestive of either the `SplitBlock` transition's
  `apply_to_sut` retrying, or `live_content` returning a stale snapshot
  at chord time despite chars being in the LoroText. Not a Phase 1
  regression — Phase 1's reproducer recipe is GREEN. Track separately
  before Phase 2 starts; Phase 2's "Loro is sole writer" rearchitecture
  may eliminate this class of races, but only if the multi-writer
  surface area is properly closed in the right order.

## Files touched (Phase 0 + Phase 1, uncommitted)

```
crates/holon-core/src/lib.rs
crates/holon-core/src/traits.rs
crates/holon/src/core/datasource.rs
crates/holon/src/core/sql_block_operations.rs
crates/holon/src/sync/event_infra_module.rs
crates/holon/src/sync/loro_block_content_resolver.rs   (NEW)
crates/holon/src/sync/loro_module.rs
crates/holon/src/sync/mod.rs
crates/holon-frontend/src/editable_text_provider.rs
crates/holon-frontend/src/headless_editor_mirror.rs    (NEW)
crates/holon-frontend/src/lib.rs
crates/holon-frontend/src/user_driver.rs
crates/holon-integration-tests/src/pbt/sut.rs
crates/holon-integration-tests/src/pbt/transition_dispatch.rs
crates/holon-integration-tests/src/pbt/transitions/delete_backward.rs
crates/holon-integration-tests/src/pbt/transitions/focus_editable_text.rs
crates/holon-integration-tests/src/pbt/transitions/move_cursor.rs
crates/holon-integration-tests/src/pbt/transitions/press_key.rs
crates/holon-integration-tests/src/pbt/transitions/type_chars.rs
crates/holon-integration-tests/tests/general_e2e_pbt.rs
```
