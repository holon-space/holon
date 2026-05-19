//! Phase 7 invariant bodies — each invariant migrates from an inline
//! closure in `sut.rs::check_invariants_async` to a free-function-style
//! `Invariant<R, S>` impl with explicit capability-trait bounds.
//!
//! Slice opt-in is structural: an invariant whose `where` clause the
//! slice's `S` doesn't satisfy simply doesn't compile into that slice's
//! invariant tuple. The wide PBT's `check_invariants_async` retains the
//! inline assertions until every invariant is migrated, then deletes
//! them in Phase 10.

pub mod backend_blocks_match_ref;
pub mod block_content_matches_ref;
pub mod block_ids_match_ref;
pub mod block_tags_references_exist;
pub mod displayed_text;
pub mod editable_text_has_draggable;
pub mod focus_matches_ref;
pub mod focus_roots;
pub mod frontend_bounds_rendered;
pub mod frontend_engine;
pub mod frontend_no_error_widgets;
pub mod frontend_root_not_error;
pub mod live_children_match_ref;
pub mod loro_no_errors;
pub mod matview_consistent_with_ref;
pub mod org_render_fixed_point;
pub mod sql_budget;
pub mod task_state_storage_coherence;
pub mod value_fn_provider_arg_variance_13;
pub mod value_fn_provider_identity;
pub mod viewmodel_decompiled_rows_match_query;
pub mod viewmodel_editable_text_triggers;
pub mod viewmodel_entity_ids_subset_of_data;
pub mod viewmodel_no_error_widgets;
pub mod viewmodel_root_matches_render_expr;
pub mod viewmodel_snapshot;
pub mod viewmodel_state_toggle_correct;
pub mod viewmodel_tree_virtual_slots;
pub mod watch_rows_match_ref;
