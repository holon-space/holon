# Phase 1.0 memo: `holon-orgmode` ↔ `holon-org-format`

**Date:** 2026-04-26
**Decision:** **(c) effective merge already done — delete dead duplicate files in `holon-orgmode/src/`.**

## Finding (corrects critique #3)

Plan critique #3 said "two parallel orgmode crates exist". Observation of the file tree supports that — `parser.rs`, `models.rs`, `org_renderer.rs`, `block_diff.rs`, `link_parser.rs` exist in both `crates/holon-orgmode/src/` and `crates/holon-org-format/src/`.

But `crates/holon-orgmode/src/lib.rs` does **not** declare these as modules. It declares only:

```rust
pub mod block_params;
pub mod di;
pub mod file_io;
pub mod file_utils;
pub mod file_watcher;
pub mod org_sync_controller;
pub mod orgmode_event_adapter;
pub mod orgmode_sync_provider;
pub mod traits;
```

For the format-layer concerns it re-exports from the canonical crate:

```rust
pub use holon_org_format::block_diff;
pub use holon_org_format::link_parser;
pub use holon_org_format::models;
pub use holon_org_format::org_renderer;
pub use holon_org_format::parser;
```

So `crate::parser`, `crate::models`, etc. inside `holon-orgmode` resolve to the **`holon-org-format` versions**. The same-named `.rs` files in `holon-orgmode/src/` are **not in the crate's module tree** — they aren't compiled, their tests don't run, and they cannot drift behavior of `holon-orgmode` because they're invisible to it.

## Evidence the duplicates are stale orphans

`diff -u crates/holon-org-format/src/parser.rs crates/holon-orgmode/src/parser.rs` shows the orgmode copy is **missing** at least:

1. `assign_per_parent_sort_keys` (the just-added fractional sort_key fix in the current uncommitted diff)
2. `extract_image_links` and image-block emission

These features are live in production through `holon-org-format`. If the duplicates were live, the project would currently have two divergent parsers. It doesn't, because they're dead files.

## Decision

Per plan §1.0 options:
- ❌ (a) merge `holon-orgmode` into `holon-org-format` — already done at the module level.
- ❌ (b) commit to dual maintenance with CI guard — would manufacture a problem.
- ✅ (c) **delete the dead duplicate files**: `crates/holon-orgmode/src/{parser,models,org_renderer,block_diff,link_parser}.rs`.

This unblocks Phase 1.1 to proceed against `holon-org-format` as the single canonical location for parser/renderer changes. No cross-crate sync work needed.

## Action items (Phase 1.0 closeout)

1. ✅ Delete five dead files from `crates/holon-orgmode/src/{parser,models,org_renderer,block_diff,link_parser}.rs`.
2. ✅ `cargo check -p holon-orgmode -p holon-org-format` — clean build (only pre-existing warnings).
3. ✅ `cargo nextest run -p holon-orgmode -p holon-org-format --features holon-orgmode/di` — 51/58 pass; the 7 failures are **pre-existing** in the in-progress sort_key + file_watcher work, not caused by deletions:
   - `round_trip_pbt::test_{round_trip,render_string_stability,in_memory_mutation,org_text_mutation}` — PBT scenarios that depend on the just-added `assign_per_parent_sort_keys` work in `parser.rs` (uncommitted).
   - `sync_controller_mutation_pbt::test_sync_{file_change_to_blocks,block_change_to_file}` — same.
   - `file_watcher::tests::test_file_watcher_respects_gitignore` — pre-existing flake.
   None reference the deleted files (which weren't compiled before deletion).
4. Side fix: two test fixtures (`org_renderer.rs:217`, `block_diff.rs:244`) needed `sort_key` field added to `Block { ... }` struct literals to match the new field. Trivial.
5. Update plan §"Critique findings" item 3 and §"Critical files modified" rows for Phase 1.1 to reference only `holon-org-format` paths.

## Risk reassessment

The plan's "two-crate fork is the single highest regression risk" callout (Risks section) is **resolved by deletion**, not by ongoing vigilance. After this memo's actions land, Phase 1.1 only edits one parser/renderer/models/block_diff/link_parser per change — same as any normal single-crate refactor.
