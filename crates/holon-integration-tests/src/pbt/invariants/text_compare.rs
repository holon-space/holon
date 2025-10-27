//! Shared, non-panicking text-equivalence core for the `inv-displayed-text`
//! family.
//!
//! The family checks the SAME property — "the text bound to a block-bound text
//! widget equals the reference text for that block" — against two layers of the
//! render axis, mirroring how [`crate::pbt::invariants::block_compare`] checks
//! one block-equivalence rule against every store:
//!
//! - `/widget` — the on-screen geometry string (`SutLayout::rendered_elements`,
//!   the `FrontendBounds` layer). Includes the active editor, whose on-screen
//!   value is the live `InputState`. GPUI suite only.
//! - `/viewmodel` — the resolved `content` prop in the frontend-agnostic
//!   ViewModel tree (`SutRenderer::widget_tree_snapshot`, the `ViewModel`
//!   layer), available headless too.
//!
//! Both arms build a `Vec<TextSample>` from their layer and call
//! [`compare_text_to_ref`] with the same reference accessors. Localisation: a
//! `/widget` failure while `/viewmodel` passes means the ViewModel held the
//! right content but the paint / `InputState` layer is stale; both failing
//! points upstream of the renderer (bad projection into the VM tree).

use holon_pbt_core::capabilities::EntityUri;

/// Widget kinds whose bound text this family checks. Only these carry a
/// reference-comparable `content`; `state_toggle`/`badge`/containers do not.
pub const TEXT_WIDGET_KINDS: [&str; 3] = ["editable_text", "rendered_text", "text"];

/// One block-bound text widget observed at some layer.
pub struct TextSample {
    /// The widget kind string, for the failure message.
    pub widget_kind: String,
    /// The block this widget renders, already in SUT id space.
    pub entity_id: EntityUri,
    /// The text the layer is showing for that block.
    pub displayed: String,
}

/// A widget whose shown text diverged from the reference.
pub struct TextMismatch {
    pub widget_kind: String,
    pub entity_id: EntityUri,
    pub displayed: String,
    pub expected: String,
}

/// Compare each sample's text against the reference and return the divergences.
///
/// Expected text per block: the live editor text for the block currently being
/// edited, the committed block content otherwise. A sample whose block the
/// reference doesn't know (e.g. a non-block column binding) is skipped.
///
/// `skip_active` excludes the actively-edited block entirely. The ViewModel arm
/// sets this: the live, uncommitted `InputState` value is a widget/editor-layer
/// concern (owned by the `/widget` arm), not something the ViewModel tree is
/// expected to mirror keystroke-for-keystroke — comparing it there would be a
/// false positive on every in-flight edit.
pub fn compare_text_to_ref(
    samples: &[TextSample],
    active_block: Option<&EntityUri>,
    active_text: Option<&str>,
    block_content: impl Fn(&EntityUri) -> Option<String>,
    skip_active: bool,
) -> Vec<TextMismatch> {
    let mut out = Vec::new();
    for s in samples {
        let is_active = active_block == Some(&s.entity_id);
        if is_active && skip_active {
            continue;
        }
        let expected = if is_active {
            active_text.map(str::to_string)
        } else {
            block_content(&s.entity_id)
        };
        let Some(expected) = expected else {
            continue;
        };
        if s.displayed != expected {
            out.push(TextMismatch {
                widget_kind: s.widget_kind.clone(),
                entity_id: s.entity_id.clone(),
                displayed: s.displayed.clone(),
                expected,
            });
        }
    }
    out
}

/// Render one mismatch as a `shown` / `expected` block for a failure message.
pub fn format_mismatch(m: &TextMismatch) -> String {
    format!(
        "  {kind}@block={id}\n    shown:    {shown:?}\n    expected: {exp:?}",
        kind = m.widget_kind,
        id = m.entity_id,
        shown = m.displayed,
        exp = m.expected,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> EntityUri {
        EntityUri::parse(s).unwrap()
    }

    fn sample(kind: &str, id: &str, text: &str) -> TextSample {
        TextSample {
            widget_kind: kind.into(),
            entity_id: uri(id),
            displayed: text.into(),
        }
    }

    #[test]
    fn committed_content_matches() {
        let samples = vec![sample("rendered_text", "block:a", "hello")];
        let m = compare_text_to_ref(
            &samples,
            None,
            None,
            |id| (id == &uri("block:a")).then(|| "hello".to_string()),
            false,
        );
        assert!(m.is_empty());
    }

    #[test]
    fn committed_content_diverges() {
        let samples = vec![sample("rendered_text", "block:a", "stale")];
        let m = compare_text_to_ref(&samples, None, None, |_| Some("fresh".to_string()), false);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].expected, "fresh");
        assert_eq!(m[0].displayed, "stale");
    }

    #[test]
    fn active_editor_uses_live_text_for_widget_arm() {
        let active = uri("block:a");
        let samples = vec![sample("editable_text", "block:a", "typ")];
        // committed content is "old" but the live editor holds "typ" — the
        // widget arm (skip_active=false) compares against live and passes.
        let m = compare_text_to_ref(
            &samples,
            Some(&active),
            Some("typ"),
            |_| Some("old".to_string()),
            false,
        );
        assert!(
            m.is_empty(),
            "widget arm should compare active block to live text"
        );
    }

    #[test]
    fn active_editor_skipped_for_viewmodel_arm() {
        let active = uri("block:a");
        // VM holds committed "old" while the editor shows uncommitted "typ".
        // skip_active=true means the VM arm ignores the active block, so no
        // false positive on the in-flight edit.
        let samples = vec![sample("editable_text", "block:a", "old")];
        let m = compare_text_to_ref(
            &samples,
            Some(&active),
            Some("typ"),
            |_| Some("old".to_string()),
            true,
        );
        assert!(
            m.is_empty(),
            "viewmodel arm must skip the active editor block"
        );
    }

    #[test]
    fn unknown_block_is_skipped() {
        let samples = vec![sample("text", "block:ghost", "x")];
        let m = compare_text_to_ref(&samples, None, None, |_| None, false);
        assert!(
            m.is_empty(),
            "a block the reference doesn't know is skipped"
        );
    }
}
