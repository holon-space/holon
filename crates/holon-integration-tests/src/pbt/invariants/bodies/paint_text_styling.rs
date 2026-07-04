//! `inv-paint-text-styling` — read-mode inline styling must reach the PAINT.
//!
//! @pbt oracle cross-layer differential (paint vs write-side marks)
//! @pbt covers read-mode text styling (bold/italic/underline/strike/code/link)
//!   rendering — the marks fix that is otherwise pinned only by GPUI unit tests
//!   and is structurally invisible to the headless keystone (no gpui dep).
//! @pbt slips-if-removed a block that HAS marks renders as plain text on screen
//!   (dogfood 2026-07-22 bug 1: the styled path gated on a Link mark, so a
//!   Bold/Italic/Underline-only block fell to `static_inner` and its formatting
//!   silently vanished) — no headless invariant can see the paint, so the drop
//!   ships.
//!
//! The GPUI-tier composed PBT is where paint-level observation belongs. Each
//! `rendered_text` / `text` widget records the styled runs it actually hands to
//! `StyledText::with_highlights` (`RenderedElement::styled_runs`, byte-range +
//! theme-independent `StyleFlags`, read from the REAL painted
//! `HighlightStyle`). This invariant compares that painted fingerprint against
//! the one the block's convergent write-side `(content, marks)` demand
//! ([`holon_api::style_fingerprint`], the SINGLE source the read-mode
//! renderer's `merge_marks` also layers its theme colors on top of).
//!
//! Needs `SutLayout` (paint) AND `SutBackend` (intended marks); the headless
//! slice supplies no `SutLayout`, so an empty snapshot is `Skipped`, exactly
//! like `inv-displayed-text/widget`. Only the windowed composed slice makes it
//! non-vacuous.

use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// The pure comparison core (window-free, so the mutation-check can drive it
/// directly). For every painted `rendered_text` / `text` element bound to a
/// block, the styled-run fingerprint it painted must equal the fingerprint the
/// block's `(content, marks)` demand. Returns one detail string per mismatch.
pub(crate) fn compare_styling(els: &[RenderedElement], blocks: &[holon_api::Block]) -> Vec<String> {
    let by_id: std::collections::HashMap<&str, &holon_api::Block> =
        blocks.iter().map(|b| (b.id.as_str(), b)).collect();

    let mut out = Vec::new();
    for el in els {
        if !matches!(el.widget_type.as_str(), "rendered_text" | "text") {
            continue;
        }
        let Some(eid) = &el.entity_id else { continue };
        let Some(block) = by_id.get(eid.as_str()) else {
            continue;
        };
        let marks = block.marks.as_deref().unwrap_or(&[]);
        let expected = holon_api::style_fingerprint(&block.content, marks);
        let observed = el.styled_runs.clone().unwrap_or_default();
        if expected != observed {
            out.push(format!(
                "block {} ({}): painted styling diverges from its marks.\n  \
                 expected runs (from write-side marks): {:?}\n  \
                 painted  runs (from window highlights): {:?}\n  \
                 content={:?} marks={:?}",
                eid.as_str(),
                el.widget_type,
                expected,
                observed,
                block.content,
                marks,
            ));
        }
    }
    out
}

pub struct InvPaintTextStyling;

impl InvPaintTextStyling {
    pub const ID: InvariantId = InvariantId("inv-paint-text-styling");
    const LABEL: &'static str = "inv-paint-text-styling";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvPaintTextStyling
where
    S: SutLayout + SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let els = sut.rendered_elements().await;
        if els.is_empty() {
            return InvariantResult::Skipped(format!(
                "[{}] no geometry installed / nothing rendered yet (headless slice)",
                Self::LABEL
            ));
        }
        let blocks = sut.block_raw_snapshot().await;
        let mismatches = compare_styling(&els, &blocks);
        if mismatches.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[{}] {} block(s) painted read-mode styling that diverges from their \
                 marks — a marked block rendering plain (formatting vanished on screen) or \
                 a mark that failed to reach the paint. Read-mode styling is otherwise \
                 pinned only by GPUI unit tests.\n{}",
                Self::LABEL,
                mismatches.len(),
                mismatches.join("\n"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;
    use holon_api::InlineMark;
    use holon_api::MarkSpan;
    use holon_api::StyleFlags;
    use holon_api::StyledRun;

    use super::*;

    fn rendered_text(id: &str, styled_runs: Option<Vec<StyledRun>>) -> RenderedElement {
        RenderedElement {
            el_id: format!("rendered-text-{id}"),
            widget_type: "rendered_text".to_string(),
            entity_id: Some(EntityUri::block(id)),
            displayed_text: None,
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 20.0,
            has_content: true,
            parent_id: None,
            expected_size_violation: None,
            is_error_widget: false,
            focused: None,
            styled_runs,
        }
    }

    fn text_block(id: &str, content: &str, marks: Option<Vec<MarkSpan>>) -> holon_api::Block {
        let mut b =
            holon_api::Block::new_text(EntityUri::block(id), EntityUri::no_parent(), content);
        b.marks = marks;
        b
    }

    fn bold_run(start: usize, end: usize) -> StyledRun {
        StyledRun {
            start,
            end,
            flags: StyleFlags {
                bold: true,
                ..Default::default()
            },
        }
    }

    /// A bold block whose painted runs match its marks: no mismatch.
    #[test]
    fn aligned_bold_block_passes() {
        // "hello" bold over 0..5 (ASCII → byte == scalar).
        let block = text_block(
            "b",
            "hello",
            Some(vec![MarkSpan::new(0, 5, InlineMark::Bold)]),
        );
        let el = rendered_text("b", Some(vec![bold_run(0, 5)]));
        let out = compare_styling(&[el], &[block]);
        assert!(out.is_empty(), "aligned styling must pass, got {out:?}");
    }

    /// The dogfood bug 1 shape: a block WITH marks that painted PLAIN
    /// (`styled_runs = None`). The paint diverges from the marks → mismatch.
    #[test]
    fn marked_block_painted_plain_is_caught() {
        let block = text_block(
            "b",
            "hello",
            Some(vec![MarkSpan::new(0, 5, InlineMark::Bold)]),
        );
        let el = rendered_text("b", None);
        let out = compare_styling(&[el], &[block]);
        assert_eq!(out.len(), 1, "a marked block painted plain must be caught");
    }

    /// Mutation-check: perturb the painted styled-run extraction (bold →
    /// italic) and the honest comparison must go RED — proof the invariant
    /// cannot silently no-op.
    #[test]
    fn perturbed_styled_run_goes_red() {
        let block = text_block(
            "b",
            "hello",
            Some(vec![MarkSpan::new(0, 5, InlineMark::Bold)]),
        );
        let mutated = StyledRun {
            start: 0,
            end: 5,
            flags: StyleFlags {
                italic: true, // was bold
                ..Default::default()
            },
        };
        let el = rendered_text("b", Some(vec![mutated]));
        let out = compare_styling(&[el], &[block]);
        assert_eq!(out.len(), 1, "a mismatched paint flag must be caught");
    }

    /// A plain (mark-less) block that painted plain (`None`) is correct — no
    /// spurious mismatch.
    #[test]
    fn plain_block_painted_plain_passes() {
        let block = text_block("b", "hello", None);
        let el = rendered_text("b", None);
        let out = compare_styling(&[el], &[block]);
        assert!(out.is_empty(), "plain block must not trip the invariant");
    }
}
