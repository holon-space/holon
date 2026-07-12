//! Core PBT types: mutations, test variants, and marker traits.

use std::collections::HashMap;

use holon_api::ContentType;
use holon_api::SourceLanguage;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::types::Mutation;

/// Org-dependent reference-model applier for [`Mutation`]. Lives here (not in
/// holon-pbt-core) because it needs `holon_orgmode::OrgBlockExt` via the org
/// round-trip helpers below. Callers `use crate::pbt::types::MutationApply`.
pub trait MutationApply {
    /// Apply this mutation to a vector of blocks (for the reference model).
    fn apply_to(&self, blocks: &mut Vec<Block>);
}

// TODO: Move to some sut_org.rs or similar
/// Apply org-mode properties (task_state, priority, tags, scheduled, deadline)
/// and custom properties from `fields` onto `block`.
///
/// When `is_create` is true, task_state is always set from fields (no
/// clear-to-None path). The caller handles update-specific task_state clearing
/// separately.
fn apply_org_properties(block: &mut Block, fields: &HashMap<String, Value>, is_create: bool) {
    if is_create
        && let Some(task_state) = fields
            .get("task_state")
            .or_else(|| fields.get("TODO"))
            .and_then(|v| v.as_string())
    {
        block.set_task_state(Some(holon_api::TaskState::from_keyword(task_state)));
    }
    if let Some(priority) = fields
        .get("priority")
        .or_else(|| fields.get("PRIORITY"))
        .and_then(|v| v.as_i64())
    {
        block.set_priority(Some(
            holon_api::Priority::from_int(priority as i32)
                .unwrap_or_else(|e| panic!("stored priority {priority} is invalid: {e}")),
        ));
    }
    if let Some(tags) = fields
        .get("tags")
        .or_else(|| fields.get("TAGS"))
        .and_then(|v| v.as_string())
    {
        block.set_tags(holon_api::Tags::from_csv(tags));
    }
    if let Some(scheduled) = fields
        .get("scheduled")
        .or_else(|| fields.get("SCHEDULED"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(scheduled)
    {
        block.set_scheduled(Some(ts));
    }
    if let Some(deadline) = fields
        .get("deadline")
        .or_else(|| fields.get("DEADLINE"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(deadline)
    {
        block.set_deadline(Some(ts));
    }
    // TODO: These are probably duplicated somewhere in non-test code.
    // Let's reuse that
    let extra_keys: &[&str] = if is_create {
        &[
            "content",
            "content_type",
            "source_language",
            "id",
            "parent_id",
            "task_state",
            "TODO",
            "priority",
            "PRIORITY",
            "tags",
            "TAGS",
            "scheduled",
            "SCHEDULED",
            "deadline",
            "DEADLINE",
        ]
    } else {
        &[
            "content",
            "task_state",
            "TODO",
            "priority",
            "PRIORITY",
            "tags",
            "TAGS",
            "scheduled",
            "SCHEDULED",
            "deadline",
            "DEADLINE",
        ]
    };
    for (k, v) in fields.iter() {
        if !extra_keys.contains(&k.as_str()) {
            block.properties.insert(k.clone(), v.clone());
        }
    }
}

// TODO: Move to sut_org.rb
/// Normalize `(content, marks)` to the org round-trip FIXED POINT — the state
/// the SUT converges to after writeback (`render_inline_marks`) and re-ingest
/// (headline trim + `extract_inline_marks`) stop changing the block.
///
/// For Text blocks the first line becomes the org headline, which the parser
/// `.trim()`s (both ends) on re-parse, so leading *and* trailing whitespace
/// on the first line is stripped. Trailing whitespace on the entire string
/// is also stripped. The parser extracts inline org markup (`[[…]]` links,
/// `*bold*`, …) into `block.marks` and stores only the RENDERED LABEL as
/// content (parse-don't-validate at the boundary); the reference must carry
/// those marks — modeling only the stripped content is exactly the oracle gap
/// that let the 2026-07-10 on-disk link-destruction escape (both sides
/// compared `marks = None`). Identity for plain text, so the default ASCII
/// generators are unaffected.
///
/// One pass is NOT idempotent (e.g. `[[x]]` resolves its bare wiki-target to a
/// deterministic entity id on the first parse, so the second writeback emits
/// the resolved `[[block:<hash>][x]]` form), hence the loop. Terminates in
/// practice within a few iterations (render∘extract is idempotent — pinned by
/// holon-org-format's inline_marks_proptest); the cap fails loud.
///
/// Source blocks preserve content verbatim (no headline, no inline markup) and
/// never carry marks.
///
/// Marks are returned canonicalized (`canonicalize_marks`) mirroring every
/// prod read boundary (`marks_from_json`, Loro Peritext reader).
pub fn normalize_content_for_org_roundtrip(
    content: &str,
    content_type: ContentType,
) -> (String, Option<Vec<holon_api::MarkSpan>>) {
    if content_type == ContentType::Source {
        return (content.trim_end().to_string(), None);
    }
    let mut text = content.to_string();
    let mut marks: Vec<holon_api::MarkSpan> = Vec::new();
    for _ in 0..32 {
        // Writeback: what the block looks like on disk (identity while no
        // marks have been extracted yet — the raw input IS the org text).
        let on_disk = holon_orgmode::inline_marks::render_inline_marks(&text, &marks);
        // Re-ingest: headline-title trim, then mark extraction.
        let trimmed_end = on_disk.trim_end();
        let trimmed = match trimmed_end.split_once('\n') {
            Some((first, rest)) => format!("{}\n{}", first.trim(), rest),
            None => trimmed_end.trim_start().to_string(),
        };
        let (rendered, spans) = holon_orgmode::inline_marks::extract_inline_marks(&trimmed);
        if rendered == text && spans == marks {
            holon_api::canonicalize_marks(&mut marks);
            let marks = if marks.is_empty() { None } else { Some(marks) };
            return (text, marks);
        }
        text = rendered;
        marks = spans;
    }
    panic!(
        "normalize_content_for_org_roundtrip: org render/parse did not reach a fixed point after \
         32 iterations for input {content:?} (last iterate: text={text:?}, marks={marks:?})"
    );
}

/// Apply the org HEADLINE-TAG lens to a block: the first content line is the
/// headline title on disk, so a trailing `:tag1:tag2:` group there is org TAG
/// syntax (org has no escape for it) and re-parses into `block.tags`, not
/// content. Mirrors the parser exactly via
/// [`holon_orgmode::parser::split_headline_tags`]; pinned in
/// holon-org-format/tests/properties_prefix_headline_repro.rs.
///
/// Use wherever the reference models a block crossing the org-FILE boundary
/// (external file writes that the SUT parses into its stores, and the ref's
/// on-disk org view). In-memory stores (SQL/Loro) keep the raw content — the
/// reinterpretation happens only at the file parse. Surfaced by extended-gen
/// axis 1 (`:PROPERTIES:` → empty title + tag `PROPERTIES`).
pub fn apply_org_headline_tag_split(block: &mut Block) {
    if block.content_type == ContentType::Source {
        return;
    }
    let (first, rest) = match block.content.split_once('\n') {
        Some((f, r)) => (f, Some(r.to_string())),
        None => (block.content.as_str(), None),
    };
    let (title, tags) = holon_orgmode::parser::split_headline_tags(first);
    if tags.is_empty() {
        return;
    }
    block.content = match rest {
        Some(r) => format!("{title}\n{r}"),
        None => title,
    };
    for t in tags {
        block.tags.insert(t);
    }
}

impl MutationApply for Mutation {
    /// Apply mutation to a vector of blocks (for reference model)
    fn apply_to(&self, blocks: &mut Vec<Block>) {
        match self {
            Mutation::Create {
                id,
                parent_id,
                fields,
                ..
            } => {
                let content_type: ContentType = fields
                    .get("content_type")
                    .and_then(|v| v.as_string())
                    .unwrap_or("text")
                    .parse()
                    .unwrap();

                // Normalize content to match the org round-trip.
                // For text blocks, the first line becomes the org headline —
                // the parser calls `.trim()` on it, so trailing whitespace on
                // the first line is stripped on re-parse. Trailing whitespace
                // on the whole string is also stripped. Source blocks preserve
                // content verbatim (no headline involved).
                let raw = fields
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                let (content, marks) = normalize_content_for_org_roundtrip(raw, content_type);

                let source_language: Option<SourceLanguage> = fields
                    .get("source_language")
                    .and_then(|v| v.as_string())
                    .map(|s| s.parse::<SourceLanguage>().unwrap());

                let mut block = if content_type == ContentType::Source {
                    let mut b = Block::new_text(id.clone(), parent_id.clone(), content);
                    b.content_type = ContentType::Source;
                    b.source_language = source_language;
                    b
                } else {
                    Block::new_text(id.clone(), parent_id.clone(), content)
                };
                block.marks = marks;

                apply_org_properties(&mut block, fields, true);

                // Mirror production: a Create without an explicit sort_key
                // lands on `block.sort_key='a0'` (SQL column default), which
                // sorts *after* every gen_n_keys-assigned sibling (lowercase
                // 'a' > uppercase hex digits used by FractionalIndex). The
                // canonicalizer in `assign_reference_sequences_canonical`
                // sorts siblings by `sequence` then id; if the new block
                // inherits the default `sequence=0`, the id tie-break decides
                // the slot, which is arbitrary. Push it past every existing
                // sibling instead so canonicalization places it last.
                let max_sibling_seq = blocks
                    .iter()
                    .filter(|b| b.parent_id == *parent_id)
                    .map(|b| b.sequence())
                    .max()
                    .unwrap_or(-1);
                block.set_sequence(max_sibling_seq + 1);

                blocks.push(block);
            }
            Mutation::Update { id, fields, .. } => {
                if let Some(block) = blocks.iter_mut().find(|b| b.id == *id) {
                    if let Some(content) = fields.get("content").and_then(|v| v.as_string()) {
                        let (text, marks) =
                            normalize_content_for_org_roundtrip(content, block.content_type);
                        block.content = text;
                        block.marks = marks;
                    }

                    if fields.contains_key("task_state") || fields.contains_key("TODO") {
                        match fields
                            .get("task_state")
                            .or_else(|| fields.get("TODO"))
                            .and_then(|v| v.as_string())
                        {
                            Some(kw) => {
                                block.set_task_state(Some(holon_api::TaskState::from_keyword(kw)))
                            }
                            None => block.set_task_state(None),
                        }
                    }
                    apply_org_properties(block, fields, false);
                }
            }
            Mutation::Delete { id, .. } => {
                let mut to_delete: Vec<EntityUri> = vec![id.clone()];
                let mut i = 0;
                while i < to_delete.len() {
                    let parent_id = &to_delete[i];
                    let children: Vec<EntityUri> = blocks
                        .iter()
                        .filter(|b| b.parent_id == *parent_id)
                        .map(|b| b.id.clone())
                        .collect();
                    to_delete.extend(children);
                    i += 1;
                }
                blocks.retain(|b| !to_delete.contains(&b.id));
            }
            Mutation::Move {
                id, new_parent_id, ..
            } => {
                if let Some(block) = blocks.iter_mut().find(|b| b.id == *id) {
                    block.parent_id = new_parent_id.clone();
                }
            }
            Mutation::RestartApp => {}
        }
    }
}

#[cfg(test)]
mod org_roundtrip_tests {
    use holon_api::ContentType;

    use super::normalize_content_for_org_roundtrip;

    #[test]
    fn empty_link_normalizes_to_clean_fixed_point_no_reversed_brackets() {
        // Ref-model oracle STRENGTHENING (dogfood #4): before the fix the ref
        // delegated to a renderer that emitted `]][[` for an empty link and so
        // CODIFIED the corruption — the SUT and ref agreed on the broken form,
        // hiding the bug. The renderer/extractor now drop the zero-width link,
        // so the ref's expected on-disk form is clean; a SUT that reintroduces
        // `]][[` would now DIVERGE and fail the keystone.
        for input in ["[[]]", "[[][]]", "a[[]]b"] {
            let (content, marks) = normalize_content_for_org_roundtrip(input, ContentType::Text);
            assert!(
                !content.contains("]]["),
                "ref model still codifies reversed brackets for {input:?}: {content:?}"
            );
            assert!(
                marks.is_none(),
                "empty link must carry no marks for {input:?}, got {marks:?}"
            );
        }
    }
}
