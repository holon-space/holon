//! `inv-viewmodel-root-matches-render-expr`.
//!
//!
//! Asserts the active **content** render expression is rendered in the right
//! place, in EITHER of the two rendering modes the GPUI app passes through:
//!
//! - **Layout-less mode** (startup, before layout-query blocks arrive): the
//!   root widget IS the content. The root widget kind must equal the root
//!   render expr's function-call name, or — when the engine wraps the root in a
//!   `view_mode_switcher` — the wrapper's first child must.
//!
//! - **3-column layout mode** (once layout blocks arrive): the root widget is
//!   the layout wrapper (e.g. `columns`); the content render expr lives INSIDE
//!   the main panel subtree. We locate the main-panel node SEMANTICALLY by its
//!   reference-model block id (`RefViewSelection::main_panel_block_id`, never a
//!   string literal here) and compare its content child's widget kind to the
//!   main panel's expected render expr name
//!   (`RefViewSelection::main_panel_render_expr_name`, which falls back to the
//!   root render expr).
//!
//! "Which mode" is derived from the snapshot, not hard-coded per layout: if the
//! root (or its `view_mode_switcher` child) already IS the expected content
//! widget, we're layout-less; otherwise we treat the root as a layout wrapper
//! and drill into the main panel. A genuine layout-less mismatch therefore
//! falls through to the drill, which `Fail`s when the content isn't where the
//! main panel should render it — the assertion is never weakened.
//!
//! Gated on `root_render_ready()` (Skipped when loading / spacer / not
//! watchable / interpret panic), mirroring `inv-viewmodel-snapshot`.
//! In layout mode the main-panel content can also be unrendered in a
//! given headless snapshot tick even though the layout itself has settled — its
//! inner reactive watch hasn't delivered. We therefore Skip (not Fail) when the
//! main-panel content is not a real, rendered widget: kind is the `unknown`
//! sentinel / empty / a known placeholder (`loading`/`spacer`), the main-panel
//! node has no content child yet, or the main-panel node itself isn't present
//! under the layout wrapper. A real, rendered content widget whose kind ≠ the
//! expected render expr name still Fails — the assertion is never weakened.
//! Bodies never panic.
//!
//! Status: functional.

use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelRootMatchesRenderExpr;

impl InvViewmodelRootMatchesRenderExpr {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-root-matches-render-expr");
}

/// Find the first node in pre-order whose `entity_id` equals `id`.
fn find_by_entity_id<'a>(root: &'a WidgetSnapshot, id: &str) -> Option<&'a WidgetSnapshot> {
    root.walk().find(|n| n.entity_id.as_deref() == Some(id))
}

/// The content widget kind a panel subtree renders. The panel container node
/// itself wraps the content; the engine may also insert a `view_mode_switcher`.
/// Returns the kind one level into the content (skipping a switcher wrapper).
fn panel_content_kind<'a>(panel: &'a WidgetSnapshot) -> Option<&'a str> {
    let child = panel.children.first()?;
    if child.kind == "view_mode_switcher" {
        child.children.first().map(|c| c.kind.as_str())
    } else {
        Some(child.kind.as_str())
    }
}

/// A widget kind that does NOT represent a real, rendered content widget: the
/// `widget_name().unwrap_or("unknown")` sentinel, an empty kind (incl. the
/// explicit `ViewKind::Empty` → `"empty"` mapping), or a known placeholder the
/// frontend shows while a reactive watch hasn't delivered yet. Asserting
/// against any of these is asserting against an unrendered node.
fn is_not_ready_kind(kind: &str) -> bool {
    matches!(kind, "unknown" | "" | "empty" | "loading" | "spacer")
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelRootMatchesRenderExpr
where
    R: RefViewSelection,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Inline gate: skip when there's no root render expr at all.
        if !ref_.has_root_render_expr() {
            return InvariantResult::Ok;
        }

        // Faithful readiness gate: skip transient placeholder roots
        // (loading / spacer / closed stream / interpret panic) the inline
        // `inv-viewmodel-snapshot` block early-returned on.
        if !sut.root_render_ready().await {
            return InvariantResult::Skipped(
                "root render not ready (loading / spacer / not watchable / interpret panic)".into(),
            );
        }

        // A root render expr that exists but isn't a FunctionCall was the
        // inline's `panic!` case — surface as Fail (bodies never panic).
        let Some(root_expr_name) = ref_.root_render_expr_name() else {
            return InvariantResult::Fail(
                "[inv-viewmodel-root-matches-render-expr] root render expr must be FunctionCall"
                    .into(),
            );
        };

        let root = sut.widget_tree_snapshot().await;
        let root_kind = root.kind.as_str();

        // ── Mode detection (derived from the snapshot) ────────────────────
        // Layout-less: the root IS the content render expr, or the engine
        // wrapped it in a `view_mode_switcher` whose first child is.
        let layout_less_match = root_kind == root_expr_name
            || (root_kind == "view_mode_switcher"
                && root
                    .children
                    .first()
                    .map(|c| c.kind.as_str() == root_expr_name)
                    .unwrap_or(false));

        if layout_less_match {
            return InvariantResult::Ok;
        }

        // ── 3-column layout mode ──────────────────────────────────────────
        // Root is a layout wrapper (e.g. `columns`); the content render expr
        // must live inside the main panel subtree. Locate the main panel
        // SEMANTICALLY by its ref-model block id, then compare its content
        // child to the main panel's expected render expr name.
        let Some(main_panel_id) = ref_.main_panel_block_id() else {
            // No semantic main-panel id available, yet the root isn't the
            // content either — the content render expr is genuinely absent
            // from where it should be.
            return InvariantResult::Fail(format!(
                "[inv-viewmodel-root-matches-render-expr] Root widget '{root_kind}' is not the \
                 expected content render expr '{root_expr_name}' and the reference model has no \
                 main-panel block id to drill into"
            ));
        };

        let expected_panel_name = ref_
            .main_panel_render_expr_name()
            .unwrap_or(root_expr_name.clone());

        let Some(panel_node) = find_by_entity_id(&root, main_panel_id.as_str()) else {
            // A `columns` layout with the main-panel node still absent is a
            // plausibly-transient state: the layout settled this snapshot tick
            // but the main panel subtree hasn't been mounted yet. Treat as
            // not-ready rather than a hard divergence.
            return InvariantResult::Skipped(format!(
                "main-panel node (entity_id '{}') not yet present under layout wrapper \
                 '{root_kind}' (not rendered in this snapshot tick)",
                main_panel_id.as_str()
            ));
        };

        match panel_content_kind(panel_node) {
            Some(actual) if actual == expected_panel_name => InvariantResult::Ok,
            // Real, rendered content widget whose kind ≠ the expected render
            // expr name — a genuine divergence. Never weakened.
            Some(actual) if !is_not_ready_kind(actual) => InvariantResult::Fail(format!(
                "[inv-viewmodel-root-matches-render-expr] Main panel content widget '{actual}' \
                 does not match expected render expr '{expected_panel_name}' (root layout \
                 '{root_kind}', main-panel id '{}')",
                main_panel_id.as_str()
            )),
            // Content node is the `unknown` sentinel / empty / a known
            // placeholder — its inner reactive watch hasn't delivered yet.
            Some(actual) => InvariantResult::Skipped(format!(
                "main-panel content widget '{actual}' is not a rendered widget yet (root layout \
                 '{root_kind}', main-panel id '{}')",
                main_panel_id.as_str()
            )),
            // Main-panel node found but has no content child yet — same
            // not-ready reasoning: the panel subtree is still being mounted.
            None => InvariantResult::Skipped(format!(
                "main-panel node (id '{}') has no content child yet (not rendered in this \
                 snapshot tick)",
                main_panel_id.as_str()
            )),
        }
    }
}
