# Phase 1 deliverable: `SutHandle` method → capability trait mapping

**Source**: `crates/holon-integration-tests/src/pbt/transition_dispatch.rs:170-441` (52 methods).

**Methodology**: each `async fn` on `SutHandle` is bucketed by the smallest set of capability traits its body touches. "Cross-cap" tag marks methods that genuinely need ≥2 capabilities (cannot be split cleanly).

**Verdict (H3 PASS, comfortably)**: 47 of 52 methods belong to a single capability; **5 are cross-cap** (≈9.6%). H3 threshold was 20% — passes with margin.

## Single-capability methods (47)

### `SutBlockTreeWrite` — 13 methods

| Method | Stage A? | Notes |
|---|---|---|
| `apply_split_block` | ✓ | T0 |
| `apply_join_block` | ✓ | T0 |
| `apply_indent` | ✓ | T0 |
| `apply_outdent` | ✓ | T0 |
| `apply_move_up` | ✓ | T0 |
| `apply_move_down` | ✓ | T0 |
| `apply_toggle_state` |  | Phase 6b — block field write |
| `apply_edit_via_display_tree` |  | Phase 6b |
| `apply_bulk_external_add` |  | Phase 6b — takes `ref_state` for doc-uri map; can be elided via interior state |
| `apply_apply_mutation` |  | Phase 6b |
| `apply_undo_last_mutation` |  | Phase 6b |
| `apply_redo` |  | Phase 6b |
| `apply_create_document` |  | Phase 6b — could split into `SutDocumentsWrite` if needed |

### `SutEditorMirrorWrite` — 3 methods

| Method | Stage A? | Notes |
|---|---|---|
| `apply_type_chars` | ✓ | T0 |
| `apply_delete_backward` | ✓ | T0 |
| `apply_move_cursor` | ✓ | T0 |

### `SutFocusWrite` — 7 methods

| Method | Stage A? | Notes |
|---|---|---|
| `apply_navigate_focus` | ✓ | T0 (used by SplitBlock follow-up) |
| `navigate_back` |  | Phase 6 — focus/nav cluster |
| `apply_navigate_forward` |  | Phase 6 |
| `apply_navigate_home` |  | Phase 6 |
| `apply_arrow_navigate` |  | Phase 6 — also touches Driver; classifying as Focus because it ultimately moves focus. Could be Driver if you weight on the input pathway. |
| `apply_pin_block` |  | Phase 6 — sidebar pin |
| `apply_unpin_block` |  | Phase 6 |

### `SutLoro` — 6 methods

| Method | Stage A? | Notes |
|---|---|---|
| `apply_add_peer` |  | Phase 6a |
| `apply_peer_edit` |  | Phase 6a |
| `apply_sync_with_peer` |  | Phase 6a |
| `apply_merge_from_peer` |  | Phase 6a |
| `apply_peer_char_edit` |  | Phase 6a |
| `apply_create_stale_loro` |  | Phase 6a — pre-startup |

### `SutDriver` — 3 methods (pure-input)

| Method | Stage A? | Notes |
|---|---|---|
| `apply_press_key` |  | Phase 6e |
| `apply_click_block` |  | Phase 6e |
| `apply_trigger_slash_command` |  | Phase 6e |
| `apply_click_at_element` |  | Phase 6e — shared with layout PBT |

### `SutLifecycle` (NEW cluster, gates Phases 6) — 5 methods

| Method | Notes |
|---|---|
| `apply_start_app` | App startup |
| `apply_simulate_restart` | Restart for sync testing |
| `apply_write_org_file` | Pre-startup file I/O |
| `apply_create_directory` | Pre-startup |
| `apply_git_init` / `apply_jj_git_init` | Pre-startup VCS |
| `apply_deliver_block_content_loaded` | Layout-PBT stub |

→ Not in original plan's 6f cluster list. **Recommend adding `SutLifecycle` as Phase 6h** — small, cohesive, no overlap with the other clusters.

### `SutViewModel` — 3 methods

| Method | Notes |
|---|---|
| `apply_expand_toggle` | UI state flip on VM |
| `apply_collapse_toggle` | UI state flip on VM |
| `apply_switch_view` | View routing |

### `SutQueryCompile` — 3 methods

| Method | Notes |
|---|---|
| `apply_setup_watch` | Compile + run a watch query |
| `apply_remove_watch` | |
| `apply_concurrent_schema_init` | Schema concurrency stress |

### `SutSqlProjection` — 1 method

| Method | Notes |
|---|---|
| `apply_emit_mcp_data` | Triggers IVM re-evaluation |

## Cross-capability methods (5)

These genuinely require ≥2 capabilities. Recommendation: **keep as methods on the umbrella trait `SutCompound` (or a slice-specific super-trait), with a default impl that orchestrates the underlying single-cap methods.** This keeps single-cap traits clean.

| Method | Capabilities | Reason |
|---|---|---|
| `apply_focus_editable_text(id)` | `SutFocusWrite` + `SutEditorMirrorWrite` | Production: focus changes editor + opens edit mode. Stage A NEEDS this — orchestrated in EditorPureSut. |
| `apply_edit_via_view_model(id, content)` | `SutBlockTreeWrite` + `SutViewModel` | Edit goes through VM pipeline, lands in block. |
| `apply_drag_drop_block(src, tgt)` | `SutBlockTreeWrite` + `SutDriver` | UI gesture → tree mutation. |
| `apply_trigger_doc_link(id, target, ref_state)` | `SutDriver` + `SutBlockTreeWrite` | Triggers popup → inserts link → block field write. |
| `apply_concurrent_mutations(ui, ext, ref_state)` | `SutBlockTreeWrite` + `SutOrgRender` | UI mutation + org-file mutation in same step. |

## Discoveries / open questions

1. **`SutLifecycle` cluster missing from plan.** Added above. Recommend updating Phase 6 cluster list.
2. **`ref_state` parameter on several methods** (`apply_start_app`, `apply_simulate_restart`, `apply_create_document`, `apply_bulk_external_add`, `apply_apply_mutation`, `apply_concurrent_mutations`, `apply_trigger_doc_link`, `apply_split_block`, `apply_join_block`, `apply_undo_last_mutation`, `apply_redo`, `apply_press_key`, `apply_arrow_navigate`) — currently leaks the wide-PBT `ReferenceState` into method signatures. Stage A migration should drop the parameter and have impls keep their own ref→SUT mapping via interior state. Wide PBT's `E2ESut` already has `doc_uri_map`; extend it for the few methods that don't have a substitute.
3. **`apply_arrow_navigate` classification ambiguous** between `SutFocusWrite` and `SutDriver`. The method *can* be implemented either as a focus mutation (headless) or a key chord (full GPUI). Recommendation: classify as `SutFocusWrite`; concrete impls choose their realization.
4. **`apply_press_key` is the most-generic driver method** — many other methods could be expressed via it. Keeping the high-level methods as well makes for easier transition writing; the trait is somewhat fat as a result. Acceptable.

## Summary table

| Capability | Method count | Stage A? |
|---|---:|---|
| `SutBlockTreeWrite` | 13 | ✓ (6 of 13) |
| `SutEditorMirrorWrite` | 3 | ✓ (3 of 3) |
| `SutFocusWrite` | 7 | ✓ (1 of 7) |
| `SutLoro` | 6 | ✗ (Phase 6a) |
| `SutDriver` | 4 | ✗ (Phase 6e) |
| `SutLifecycle` (new) | 5 | ✗ (Phase 6h — recommend adding to plan) |
| `SutViewModel` | 3 | ✗ (Phase 6c) |
| `SutQueryCompile` | 3 | ✗ (Phase 6g) |
| `SutSqlProjection` | 1 | ✗ (Phase 6b) |
| Cross-cap | 5 | ✓ (1 of 5: `apply_focus_editable_text`) |
| **Total** | **52** | **11 in Stage A** |

11 methods in Stage A = manageable. Distribution by capability is balanced (no trait is empty).
