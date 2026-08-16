//! One module per composed invariant. Each exposes a
//! `pub fn wire() -> Box<dyn CapInvariant>` (its body + `Needs`) and a
//! `#[cfg(test)] mod tests` holding that invariant's positive / negative-
//! containment / catch triad, driven by the hand-crafted [`super::fixtures`]
//! doubles (no real backend required).
//!
//! ## Recipe — adding an invariant (the parallelizable unit)
//! 1. Pick the registry body (`crate::pbt::invariants::bodies::*`) and the caps
//!    its `where` bounds require.
//! 2. Copy the nearest existing module here (e.g. `no_orphan` for a `SutBackend
//!    + RefBackend` body) and adjust the `wire()` `Needs` and `Attribution` —
//!    the pipeline layer this invariant OBSERVES (or `cross_cutting` for a
//!    health/budget guard), always with `file!()` as the wiring pointer.
//! 3. Write the test triad against `crate::pbt::composed::fixtures::*`: a
//!    positive (cap wired ⇒ selected + passes), a negative-containment (cap
//!    absent ⇒ in `report.deselected`, not faked), and a catch (inject a
//!    divergence ⇒ the invariant id appears in `report.failures()`).
//! 4. Append one `invariants::<name>::wire()` line to [`super::catalog`].
//!
//! The new invariant then lights up automatically in *every* slice whose
//! components provide its `Needs` — no per-slice wiring.
//!
//! Full process (router, recipes, anti-patterns, the §8.10 convergence litmus):
//! the `pbt-composition` skill (`.claude/skills/pbt-composition/`).

pub mod advice_rows_woven;
pub mod audience_never_over_approximates;
pub mod birth_contract;
pub mod boundary_respected;
pub mod companion_has_no_child_page_headings;
/// `inv-complexity-class-trend` — `otel-testing`-gated for the same reason
/// `sql_budget` is: its host is the span-metrics collector.
#[cfg(feature = "otel-testing")]
pub mod complexity_trend;
pub mod display_placement_canonical_inert;
pub mod displayed_text;
pub mod drawer_open_matches_ref;
pub mod editable_text_has_draggable;
pub mod embedded_page_collapsed_lazy;
pub mod every_page_has_its_own_file;
pub mod focus_matches_ref;
pub mod focus_roots;
pub mod frontend_bounds_rendered;
pub mod frontend_engine;
pub mod frontend_no_error_widgets;
pub mod frontend_root_not_error;
pub mod inline_row_mount_present;
pub mod journal_feed_viewport_lazy;
pub mod journal_one_per_day;
pub mod live_block_shell_present;
pub mod live_children_match_ref;
pub mod live_tree_matches_fresh;
pub mod loro_children_match_ref;
pub mod loro_no_errors;
pub mod main_panel_rows_match_focus;
pub mod mark_bounds_within_content;
pub mod matview_recompute_matches;
pub mod observed_errors;
pub mod paint_text_styling;
pub mod reseed_leak;
pub mod sticky_accordion_spec;
pub mod wheel_occlusion_routing;
pub mod wheel_two_mode_motion_law;
// `navigation_focus` is now auto-derived by `capability_pair! { pub trait Focus
// }` in holon-pbt-core (`inv_pair_focus_current_focus_rows`); its hand-written
// wiring + body files were deleted.
pub mod no_errors;
pub mod no_orphan;
pub mod no_page_under_non_page;
pub mod no_parent_cycles;
pub mod no_write_outside_vault_root;
pub mod org_render_fixed_point;
pub mod settle_budget;
pub mod sidebar_page_tag_preserved;
pub mod source_language;
/// `inv-sql-budget` — `otel-testing`-gated (the budget body + composed host
/// both require the OTel span collector; the `pbt` feature always enables it).
#[cfg(feature = "otel-testing")]
pub mod sql_budget;
pub mod task_state_matches_ref;
pub mod task_state_storage_coherence;
pub mod two_instance_convergence;
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
pub mod watch_rows;
pub mod window_focus;
