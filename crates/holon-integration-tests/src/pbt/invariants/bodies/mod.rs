//! Invariant bodies — each invariant is a free-function-style
//! `Invariant<R, S>` impl with explicit capability-trait bounds.
//!
//! Slice opt-in is structural: an invariant whose `where` clause the
//! slice's `S` doesn't satisfy simply doesn't compile into that slice's
//! invariant tuple.

pub mod advice_rows_woven;
pub mod audience_never_over_approximates;
pub mod block_ids_match_ref;
pub mod boundary_respected;
pub mod block_tags_references_exist;
pub mod displayed_text;
pub mod editable_text_has_draggable;
pub mod focus_matches_ref;
pub mod focus_roots;
pub mod frontend_bounds_rendered;
pub mod frontend_engine;
pub mod frontend_no_error_widgets;
pub mod frontend_root_not_error;
pub mod journal_one_per_day;
pub mod live_block_shell_present;
pub mod live_children_match_ref;
pub mod live_tree_matches_fresh;
pub mod main_panel_rows_match_focus;
pub mod mark_bounds_within_content;
pub mod paint_text_styling;
pub mod matview_recompute_matches;
pub mod two_instance_convergence;
// `navigation_focus` moved to `capability_pair!`'s `compare_navigation_focus`
// in holon-pbt-core (auto-derived `inv-navigation-focus`); body file deleted.
pub mod companion_has_no_child_page_headings;
pub mod display_placement_canonical_inert;
pub mod embedded_page_collapsed_lazy;
pub mod every_page_has_its_own_file;
pub mod no_errors;
pub mod no_orphan_blocks;
pub mod no_page_under_non_page;
pub mod no_parent_cycles;
pub mod org_render_fixed_point;
pub mod sidebar_page_tag_preserved;
pub mod source_language_iff_source;
pub mod sql_budget;
pub mod sticky_accordion_spec;
pub mod task_state_storage_coherence;
pub mod undo_redo_reference_heal;
pub mod value_fn_provider_arg_variance_13;
pub mod value_fn_provider_identity;
pub mod viewmodel_decompiled_rows_match_query;
pub mod viewmodel_editable_text_triggers;
pub mod viewmodel_entity_ids_subset_of_data;
pub mod viewmodel_no_error_widgets;
pub mod viewmodel_root_matches_render_expr;
pub mod viewmodel_shows_source_when_no_query;
pub mod viewmodel_snapshot;
pub mod viewmodel_state_toggle_correct;
pub mod viewmodel_tree_virtual_slots;
pub mod watch_rows_match_ref;
pub mod wheel_occlusion_routing;
pub mod wheel_two_mode_motion_law;
pub mod window_focus_matches_engine_focus;
