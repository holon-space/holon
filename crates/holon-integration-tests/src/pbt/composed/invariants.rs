//! One module per composed invariant. Each exposes a
//! `pub fn wire() -> Box<dyn CapInvariant>` (its body + `Needs`) and a
//! `#[cfg(test)] mod tests` holding that invariant's positive / negative-
//! containment / catch triad, driven by the hand-crafted [`super::fixtures`]
//! doubles (no real backend required).
//!
//! ## Recipe — adding an invariant (the parallelizable unit)
//! 1. Pick the registry body (`crate::pbt::invariants::bodies::*`) and the caps
//!    its `where` bounds require.
//! 2. Copy the nearest existing module here (e.g. `block_parent` for a
//!    `SutBackend + RefBlockTree` body) and adjust the `wire()` `Needs`.
//! 3. Write the test triad against `crate::pbt::composed::fixtures::*`:
//!    a positive (cap wired ⇒ selected + passes), a negative-containment
//!    (cap absent ⇒ in `report.deselected`, not faked), and a catch (inject a
//!    divergence ⇒ the invariant id appears in `report.failures()`).
//! 4. Append one `invariants::<name>::wire()` line to [`super::catalog`].
//!
//! The new invariant then lights up automatically in *every* slice whose
//! components provide its `Needs` — no per-slice wiring.
//!
//! Full process (router, recipes, anti-patterns, the §8.10 convergence litmus):
//! the `pbt-composition` skill (`.claude/skills/pbt-composition/`).

pub mod block_content;
pub mod block_content_sql;
pub mod block_parent;
pub mod blocks_match;
pub mod displayed_text;
pub mod editable_text_has_draggable;
pub mod editor_caret;
pub mod editor_text;
pub mod focus_matches_ref;
pub mod focus_roots;
pub mod frontend_bounds_rendered;
pub mod frontend_engine;
pub mod frontend_no_error_widgets;
pub mod frontend_root_not_error;
pub mod live_children_match_ref;
pub mod live_tree_matches_fresh;
pub mod loro_children_match_ref;
pub mod loro_no_errors;
pub mod matview_consistent_with_ref;
pub mod navigation_focus;
pub mod no_errors;
pub mod no_orphan;
pub mod no_parent_cycles;
pub mod org_render_fixed_point;
pub mod source_language;
/// `inv-sql-budget` — `otel-testing`-gated (the budget body + composed host both
/// require the OTel span collector; the `pbt` feature always enables it).
#[cfg(feature = "otel-testing")]
pub mod sql_budget;
pub mod task_state_storage_coherence;
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
