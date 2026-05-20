//! `inv-displayed-text/*` — the render-axis text-equivalence family.
//!
//! Same property at two layers: the text bound to a block-bound text widget
//! (`editable_text`, `rendered_text`, `text`) must equal what the reference
//! says for that block — the live editor text while an editor is open on it,
//! else the committed block content. Both arms share
//! [`crate::pbt::invariants::text_compare`].
//!
//! - [`InvDisplayedTextWidget`] (`/widget`) reads the on-screen geometry
//!   (`SutLayout::rendered_elements`, the `FrontendBounds` layer; empty headless
//!   → `Skipped`) and checks every widget, including the active editor.
//! - [`InvDisplayedTextViewModel`] (`/viewmodel`) reads the frontend-agnostic
//!   ViewModel tree (`SutRenderer::widget_tree_snapshot`, the `ViewModel`
//!   layer; available headless too) and checks the resolved `content` prop,
//!   skipping the actively-edited block (its uncommitted `InputState` value is a
//!   widget-layer concern, not a ViewModel one).
//!
//! Localisation (the point of splitting): `/widget` ✗ while `/viewmodel` ✓ means
//! the ViewModel held the right content but the paint / `InputState` layer is
//! stale; both ✗ points upstream of the renderer (a bad projection into the VM
//! tree). Both compared `entity_id`s and the resolved reference are in SUT id
//! space (`with_resolved_doc_uris` remaps `active_editor.block_id`), so no
//! per-comparison `reverse_map` is needed.

use holon_pbt_core::capabilities::{
    EntityUri, RefBlockTree, RefEditorMirror, SutLayout, SutRenderer,
};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::invariants::text_compare::{
    TEXT_WIDGET_KINDS, TextSample, compare_text_to_ref, format_mismatch,
};

/// Shared message builder so both arms read identically in a layer report.
fn fail_message(
    label: &str,
    layer: &str,
    mismatches: &[crate::pbt::invariants::text_compare::TextMismatch],
) -> String {
    format!(
        "[{label}] {n} text widget(s) show stale content at the {layer} layer. The \
         shown string diverged from the reference block content — typical after \
         split_block/join_block when the row's data signal fires but the text \
         (editable_text InputState, text col(...) snapshot, or a stale VM prop) \
         skips the update.\n{body}",
        n = mismatches.len(),
        body = mismatches
            .iter()
            .map(format_mismatch)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// ── /widget — on-screen geometry (FrontendBounds) ──────────────────────────

pub struct InvDisplayedTextWidget;

impl InvDisplayedTextWidget {
    pub const ID: InvariantId = InvariantId("inv-displayed-text/widget");
    const LABEL: &'static str = "inv-displayed-text/widget";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvDisplayedTextWidget
where
    R: RefEditorMirror + RefBlockTree,
    S: SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let elements = sut.rendered_elements().await;
        if elements.is_empty() {
            return InvariantResult::Skipped(format!(
                "[{}] no geometry installed / nothing rendered yet",
                Self::LABEL
            ));
        }

        let samples: Vec<TextSample> = elements
            .iter()
            .filter(|el| {
                matches!(
                    el.widget_type.as_str(),
                    "editable_text" | "rendered_text" | "text"
                )
            })
            .filter_map(|el| {
                Some(TextSample {
                    widget_kind: el.widget_type.clone(),
                    entity_id: el.entity_id.clone()?,
                    displayed: el.displayed_text.clone()?,
                })
            })
            .collect();

        let mismatches = compare_text_to_ref(
            &samples,
            ref_.active_editor_block().as_ref(),
            ref_.active_editor_text(),
            |id| ref_.block_content(id).map(str::to_string),
            // The geometry layer is the on-screen truth: the active editor's
            // live `InputState` value is checked here, not skipped.
            false,
        );

        if mismatches.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(fail_message(Self::LABEL, "widget/geometry", &mismatches))
        }
    }
}

// ── /viewmodel — frontend-agnostic ViewModel tree ──────────────────────────

pub struct InvDisplayedTextViewModel;

impl InvDisplayedTextViewModel {
    pub const ID: InvariantId = InvariantId("inv-displayed-text/viewmodel");
    const LABEL: &'static str = "inv-displayed-text/viewmodel";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvDisplayedTextViewModel
where
    R: RefEditorMirror + RefBlockTree,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Skip transient placeholder roots (loading / spacer / interpret panic):
        // the contract only holds for a settled content render.
        if !sut.root_render_ready().await {
            return InvariantResult::Skipped(format!(
                "[{}] root render not ready (loading/spacer/not interpretable)",
                Self::LABEL
            ));
        }

        let tree = sut.widget_tree_snapshot().await;
        // Filter to text-bearing widgets that carry a resolvable block id and a
        // `content` prop (only EditableText/RenderedText/Text set it).
        let samples: Vec<TextSample> = tree
            .walk()
            .filter(|n| TEXT_WIDGET_KINDS.contains(&n.kind.as_str()))
            .filter_map(|n| {
                let content = n.props.get("content")?;
                let raw = n.entity_id.as_ref()?;
                // ALLOW(entity_uri_from_raw): widget-tree entity_ids are SUT-space
                // UUIDs, parsed the same way the geometry mirror parses them.
                let entity_id = EntityUri::parse(raw).ok()?;
                Some(TextSample {
                    widget_kind: n.kind.clone(),
                    entity_id,
                    displayed: content.clone(),
                })
            })
            .collect();

        if samples.is_empty() {
            return InvariantResult::Skipped(format!(
                "[{}] no block-bound text widgets in the VM tree yet",
                Self::LABEL
            ));
        }

        let mismatches = compare_text_to_ref(
            &samples,
            ref_.active_editor_block().as_ref(),
            ref_.active_editor_text(),
            |id| ref_.block_content(id).map(str::to_string),
            // The actively-edited block's uncommitted text lives in InputState,
            // not the ViewModel tree — skip it here to avoid a false positive on
            // every in-flight edit; the `/widget` arm owns that case.
            true,
        );

        if mismatches.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(fail_message(Self::LABEL, "viewmodel", &mismatches))
        }
    }
}
