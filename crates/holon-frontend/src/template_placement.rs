//! Where a template instantiation lands relative to the block the user
//! triggered the slash command on.
//!
//! USER RULING ("Option B"): the template action is available on ANY block.
//! - target block **empty** → the instance replaces it *in place*.
//! - target block **non-empty** → the instance nests as its **children**;
//!   the existing block and its content are never touched.
//!
//! The decision is a pure function of the target block's emptiness. Encoding
//! it as an enum (parse-don't-validate) keeps the "never mutate existing
//! content" invariant in the type rather than in scattered `if content.is_empty()`
//! checks at each call site: a `TemplatePlacement` is *proof* that the caller
//! already classified the target, and `target_parent()` is the only parent a
//! caller can reach.

use anyhow::Result;
use anyhow::bail;

/// A template offered in the slash-command picker: the block id to instantiate
/// and its human-readable name (the `template` property value, falling back to
/// the block's content).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateChoice {
    pub template_id: String,
    pub name: String,
}

/// Enumerate the templates among `blocks` — the REAL picker enumeration logic
/// `ReactiveEngine::list_templates` runs over the block snapshot. A block is a
/// template iff it carries the marker property; the lookup is case-insensitive
/// via the shared authority (`holon_api::template::template_marker_value`), so
/// org-authored templates (uppercase `:TEMPLATE:` → `"TEMPLATE"` key) are
/// found — the round-3 live bug was a case-sensitive lookup here.
pub fn templates_from_blocks<'a>(
    blocks: impl Iterator<Item = &'a holon_api::block::Block>,
) -> Vec<TemplateChoice> {
    blocks
        .filter_map(|block| {
            let name = holon_api::template::template_marker_value(
                block,
                holon_api::TEMPLATE_MARKER_PROPERTY,
            )?;
            let name = if name.trim().is_empty() {
                block.content.clone()
            } else {
                name.to_string()
            };
            Some(TemplateChoice {
                template_id: block.id.as_str().to_string(),
                name,
            })
        })
        .collect()
}

/// The block the slash command fired on. Its `content`/`parent_id` come from a
/// block **resolved out of the projection** — never from the editor's
/// `context_params`, whose live DataRow carries only the block `id` (that
/// half-populated context is exactly what made every non-empty block look empty
/// and bail; see the live-drive regression). Fields are private and the only
/// constructor is [`TargetBlock::from_block`], so an id-only context cannot
/// masquerade as a resolved target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetBlock {
    id: String,
    content: String,
    parent_id: Option<String>,
}

impl TargetBlock {
    /// Parse a resolved block into placement input. This is the ONLY public
    /// constructor — you must hold a real [`holon_api::block::Block`], so
    /// content and parent are always populated from the projection.
    pub fn from_block(block: &holon_api::block::Block) -> Self {
        let parent = block.parent_id.as_str();
        Self {
            id: block.id.as_str().to_string(),
            content: block.content.clone(),
            parent_id: (!parent.is_empty()).then(|| parent.to_string()),
        }
    }

    /// Test-only builder for the pure-decision unit tests. Not available in
    /// production so the id-only-context bug cannot recur through this path.
    #[cfg(test)]
    pub(crate) fn from_parts(id: &str, content: &str, parent_id: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            content: content.to_string(),
            parent_id: parent_id.map(str::to_string),
        }
    }

    /// Remove the slash-command text the user typed to open the menu, so a
    /// bullet whose ENTIRE content is "/template" is judged EMPTY (the command
    /// is transient UI, not block content). Without this the resolved content
    /// always contains "/…" → every block looks non-empty → the in-place
    /// branch can never fire live (path-B regression).
    ///
    /// `prefix_start` is the byte offset of "/" in the content; `command_len`
    /// is the byte length of "/<filter>". Bounds-clamped: a stale/short
    /// snapshot (command not yet reflected) strips nothing and is judged on its
    /// real content.
    pub fn without_typed_command(mut self, prefix_start: usize, command_len: usize) -> Self {
        let end = prefix_start.saturating_add(command_len).min(self.content.len());
        if prefix_start <= end
            && self.content.is_char_boundary(prefix_start)
            && self.content.is_char_boundary(end)
        {
            self.content.replace_range(prefix_start..end, "");
        }
        self
    }
}

/// Resolves the placement-relevant facts of a block by id, out of the block
/// projection. The slash-menu picker holds one so it can build a real
/// [`TargetBlock`] at pick time instead of trusting the editor's id-only
/// `context_params`.
pub trait BlockResolver: Send + Sync {
    /// `None` when no block with this id exists in the projection.
    fn resolve(&self, id: &str) -> Option<TargetBlock>;
}

/// The resolved placement for `instantiate_template`'s `target_parent`, plus —
/// for the empty case — the empty block that the instance supersedes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePlacement {
    /// Target block is empty: create the instance under the empty block's
    /// parent and delete the empty block, so the template takes its slot.
    InPlace {
        parent_id: String,
        /// The now-redundant empty block, deleted after instantiation.
        replaced_block_id: String,
    },
    /// Target block has content: nest the instance as its children. The target
    /// block and its content are untouched.
    AsChildren { parent_id: String },
}

impl TemplatePlacement {
    /// Classify `target` per the Option-B ruling. Fails loud when an *empty*
    /// target has no parent — a page root cannot be replaced in place, and
    /// silently retargeting elsewhere would violate the ruling.
    pub fn decide(target: &TargetBlock) -> Result<Self> {
        if target.content.trim().is_empty() {
            let Some(parent_id) = target.parent_id.clone() else {
                bail!(
                    "cannot instantiate a template in place of empty block '{}': it is a page \
                     root (no parent to instantiate under)",
                    target.id
                );
            };
            Ok(TemplatePlacement::InPlace {
                parent_id,
                replaced_block_id: target.id.clone(),
            })
        } else {
            Ok(TemplatePlacement::AsChildren {
                parent_id: target.id.clone(),
            })
        }
    }

    /// The `target_parent` param for the `instantiate_template` op.
    pub fn target_parent(&self) -> &str {
        match self {
            TemplatePlacement::InPlace { parent_id, .. } => parent_id,
            TemplatePlacement::AsChildren { parent_id } => parent_id,
        }
    }

    /// The empty block to delete after instantiation, if any (empty→in-place
    /// only). `None` for the children case — existing content is never touched.
    pub fn block_to_replace(&self) -> Option<&str> {
        match self {
            TemplatePlacement::InPlace {
                replaced_block_id, ..
            } => Some(replaced_block_id),
            TemplatePlacement::AsChildren { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(id: &str, content: &str, parent: Option<&str>) -> TargetBlock {
        TargetBlock::from_parts(id, content, parent)
    }

    #[test]
    fn empty_block_replaces_in_place() {
        let t = target("block:child", "", Some("block:parent"));
        let placement = TemplatePlacement::decide(&t).unwrap();
        assert_eq!(
            placement,
            TemplatePlacement::InPlace {
                parent_id: "block:parent".into(),
                replaced_block_id: "block:child".into(),
            }
        );
        assert_eq!(placement.target_parent(), "block:parent");
        assert_eq!(placement.block_to_replace(), Some("block:child"));
    }

    #[test]
    fn whitespace_only_block_counts_as_empty() {
        let t = target("block:child", "   \t ", Some("block:parent"));
        let placement = TemplatePlacement::decide(&t).unwrap();
        assert!(matches!(placement, TemplatePlacement::InPlace { .. }));
    }

    #[test]
    fn non_empty_block_nests_as_children() {
        let t = target("block:meeting", "Weekly sync", Some("block:parent"));
        let placement = TemplatePlacement::decide(&t).unwrap();
        assert_eq!(
            placement,
            TemplatePlacement::AsChildren {
                parent_id: "block:meeting".into(),
            }
        );
        // Never touch existing content: no block is scheduled for deletion.
        assert_eq!(placement.block_to_replace(), None);
        assert_eq!(placement.target_parent(), "block:meeting");
    }

    #[test]
    fn empty_page_root_fails_loud() {
        let t = target("block:root", "", None);
        let err = TemplatePlacement::decide(&t).unwrap_err();
        assert!(
            err.to_string().contains("page root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn non_empty_page_root_still_nests() {
        // A non-empty root needs no parent — children placement targets itself.
        let t = target("block:root", "Home", None);
        let placement = TemplatePlacement::decide(&t).unwrap();
        assert_eq!(placement.target_parent(), "block:root");
    }

    #[test]
    fn command_only_content_is_empty_after_strip() {
        // Bullet whose ENTIRE content is the typed "/journal" → empty → in place.
        let t = target("block:b", "/journal", Some("block:p")).without_typed_command(0, 8);
        assert_eq!(
            TemplatePlacement::decide(&t).unwrap(),
            TemplatePlacement::InPlace {
                parent_id: "block:p".into(),
                replaced_block_id: "block:b".into(),
            }
        );
    }

    #[test]
    fn real_content_survives_command_strip() {
        // "Weekly sync" + "/journal" at byte 11 → real content remains → children.
        let t =
            target("block:b", "Weekly sync/journal", Some("block:p")).without_typed_command(11, 8);
        assert!(matches!(
            TemplatePlacement::decide(&t).unwrap(),
            TemplatePlacement::AsChildren { .. }
        ));
    }

    #[test]
    fn command_strip_is_bounds_clamped() {
        // Stale/short snapshot: prefix_start past the end strips nothing.
        let t = target("block:b", "abc", Some("block:p")).without_typed_command(10, 8);
        assert!(matches!(
            TemplatePlacement::decide(&t).unwrap(),
            TemplatePlacement::AsChildren { .. }
        ));
    }

    #[test]
    fn templates_from_blocks_finds_org_uppercase_marker() {
        use holon_api::Value;
        use holon_api::block::Block;
        use holon_api::entity_uri::EntityUri;

        // The org parser lifts `:TEMPLATE:` as an UPPERCASE "TEMPLATE" property
        // key (block_params.rs). RED before the case-insensitive fix: the
        // picker's lowercase exact-match `get` returned None → an EMPTY picker
        // for every org-authored template. Drives the REAL enumeration logic
        // (`templates_from_blocks`, which `list_templates` runs verbatim), not
        // a fake resolver.
        let mut org = Block::new_text(
            EntityUri::block("tpl-daily"),
            EntityUri::no_parent(),
            "* {{date}}",
        );
        org.properties
            .insert("TEMPLATE".into(), Value::String("daily-journal".into()));
        // A lowercase, programmatically-created template must ALSO be found.
        let mut prog = Block::new_text(
            EntityUri::block("tpl-prog"),
            EntityUri::no_parent(),
            "body",
        );
        prog.properties
            .insert("template".into(), Value::String("prog".into()));
        // A non-template block is skipped.
        let plain = Block::new_text(EntityUri::block("plain"), EntityUri::no_parent(), "hi");

        let out = templates_from_blocks([&org, &prog, &plain].into_iter());
        assert!(
            out.iter()
                .any(|t| t.template_id == "block:tpl-daily" && t.name == "daily-journal"),
            "org uppercase :TEMPLATE: must be found; got {out:?}"
        );
        assert!(
            out.iter()
                .any(|t| t.template_id == "block:tpl-prog" && t.name == "prog"),
            "lowercase marker must also be found; got {out:?}"
        );
        assert_eq!(out.len(), 2, "plain block skipped; got {out:?}");
    }
}
