//! Transition: write an org file to the temp directory.
//!
//! Mirrors the legacy logic split across `state_machine.rs:326-338` (generator),
//! `state_machine.rs:3077-3101` (precondition),
//! `state_machine.rs:1738-1931` (ref-state apply),
//! `sut.rs:661-670` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::types::{apply_org_headline_tag_split, normalize_content_for_org_roundtrip};
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

use holon_api::block::Block;
use holon_api::{ContentType, EntityUri, SourceLanguage};
use holon_orgmode::OrgBlockExt;
use holon_orgmode::OrgDocumentExt;
use holon_orgmode::OrgRenderer;

/// Seed a document's blocks before the app starts.
///
/// The generator produces `Block` instances directly (it always did, then
/// threw them away by rendering to org text). This transition carries those
/// blocks and decides how to materialise them against the SUT: serialise to
/// org text and write a file for a Turso/org wiring, or write them straight
/// into the Loro doc for a no-Turso wiring. The reference-state effect is the
/// same either way — the blocks are inserted as-is, with no re-parsing.
///
/// The generated blocks are parented to a `gen-placeholder` document uri; the
/// real per-document uri is resolved in `apply_to_ref` (and the placeholder is
/// also what the org renderer uses as the file id on the SUT side, so the
/// emitted text is byte-identical to the previous text-first generator).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WriteOrgFile {
    pub filename: String,
    pub blocks: Vec<Block>,
    /// Custom `#+TODO:` keyword set for this file (extended-gen axis 5).
    /// `None` = no header, parser defaults apply. `#[serde(default)]` keeps
    /// pre-axis-5 capture JSONs loadable.
    #[serde(default)]
    pub keyword_set: Option<crate::pbt::generators::TodoKeywordSet>,
}

/// The placeholder document uri the generator parents top-level blocks to.
/// Top-level headings carry this as their `parent_id`; `apply_to_ref` remaps
/// it to the resolved per-document uri, and the SUT-side renderer uses it as
/// the file id so the emitted org text matches the prior generator output.
const GEN_PLACEHOLDER: &str = "gen-placeholder";

impl WriteOrgFile {
    /// Build a `WriteOrgFile` from raw org text. Used by the Gherkin step
    /// matcher, where authors write org content directly in a docstring. Parses
    /// the text with the production org parser, then reparents top-level blocks
    /// onto the `GEN_PLACEHOLDER` document uri so they flow through the same
    /// seeding path as generator-produced blocks.
    pub fn from_org_text(filename: String, content: &str) -> anyhow::Result<Self> {
        let placeholder = EntityUri::block(GEN_PLACEHOLDER);
        let parsed = holon_orgmode::parse_org_file(
            std::path::Path::new(&filename),
            content,
            &placeholder,
            std::path::Path::new("."),
        )?;
        let doc_id = parsed.document.id.clone();
        let blocks = parsed
            .blocks
            .into_iter()
            .map(|mut b| {
                if b.parent_id == doc_id {
                    b.parent_id = placeholder.clone();
                }
                b
            })
            .collect();
        Ok(Self {
            filename,
            blocks,
            keyword_set: None,
        })
    }
}

impl TransitionFactory<ReferenceState> for WriteOrgFile {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pre_startup_file_count = state.files.documents.len();
        let file_weight = if pre_startup_file_count < 3 { 3 } else { 1 };

        // Layout overrides (custom `index.org` query layouts) are OFF by
        // default: a vanilla seed layout renders blocks interactively, so the
        // edit/split/cursor transitions are reachable. Opt back in with
        // `HOLON_PBT_LAYOUT_OVERRIDE=1` to exercise custom-layout paths.
        let state_for_preconditions = state.clone();
        let allow_index_override = std::env::var("HOLON_PBT_LAYOUT_OVERRIDE").is_ok();
        // Axis 5 (promoted 2026-06-10): ~half the files carry a custom
        // `#+TODO:` keyword set, emitted as the org header on the SUT side
        // and adopted by the reference doc block.
        let strat = proptest::option::of(crate::pbt::generators::todo_keyword_set_strategy())
            .prop_flat_map(move |keyword_set| {
                crate::pbt::generators::generate_org_file_content_with_keywords(
                    keyword_set.clone(),
                    allow_index_override,
                )
                .prop_map(move |(filename, blocks)| WriteOrgFile {
                    filename,
                    blocks,
                    keyword_set: keyword_set.clone(),
                })
            })
            .prop_filter("WriteOrgFile preconditions", move |t| {
                t.preconditions(&state_for_preconditions).is_good()
            })
            .boxed();

        Validated::Good((file_weight, strat))
    }
}

impl TransitionRef<ReferenceState> for WriteOrgFile {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![];

        // Reject if any heading block in this file already exists under a
        // different document. Mirrors the previous `:ID:`-drawer collision
        // check: only text/heading blocks carry an `:ID:` drawer, so source
        // blocks (`{id}::src::N`) are excluded.
        let doc_name = std::path::Path::new(self.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.filename);
        let doc_uri = state
            .doc_uri_by_name(doc_name)
            .unwrap_or_else(|| EntityUri::block("precondition-placeholder"));
        let any_collision = self
            .blocks
            .iter()
            .filter(|b| b.content_type != holon_api::ContentType::Source)
            .any(|b| {
                state
                    .domain
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .is_some_and(|existing_doc| *existing_doc != doc_uri)
            });
        checks.push(check(!any_collision, Reason::BlockIdAlreadyExists));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let doc_name = std::path::Path::new(self.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.filename)
            .to_string();
        let doc_uri = state
            .doc_uri_by_name(&doc_name)
            .unwrap_or_else(|| state.next_synthetic_doc_uri());
        state
            .files
            .documents
            .insert(doc_uri.clone(), self.filename.clone());

        // Remove old content blocks from this document (handles re-writing the same file)
        let old_block_ids: Vec<EntityUri> = state
            .domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, uri)| **uri == doc_uri)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &old_block_ids {
            state.domain.block_state.blocks.remove(id);
            state.domain.block_state.block_documents.remove(id);
            state.domain.layout_blocks.remove(id);
            // A same-id rewrite (index.org layout swap) re-inserts the parsed
            // expr below; leaving the stale entry made `root_render_expr()`
            // serve the PREVIOUS layout's template when the new one failed
            // the `render_expr_from_rhai` exact-match lookup.
            state.domain.render_expressions.remove(id);
        }

        // Add the page block (tags ⊇ ["Page"]) for this org file.
        let mut doc_block =
            Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name.clone());
        doc_block.set_page(true);
        // Mirror the SUT parser: a `#+TODO:` header lands on the document
        // block as the `todo_keywords` property (parser.rs:164).
        if let Some(ks) = &self.keyword_set {
            doc_block.set_todo_keywords(Some(ks.0.clone()));
        }
        state
            .domain
            .block_state
            .blocks
            .insert(doc_uri.clone(), doc_block);
        state
            .domain
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri.clone());

        // Insert the generated blocks directly into the reference model — no
        // re-parsing. The generator already built these `Block`s (it used to
        // render them to org text and throw them away); we keep them.
        //
        // Top-level headings are parented to `GEN_PLACEHOLDER`; remap those to
        // the resolved document uri. Source/child blocks keep their real parent
        // (the heading uri). The `ID` property is a renderer hint (it makes the
        // org renderer emit the `:ID:` drawer on the SUT side) — it is not part
        // of the parsed block on either side, so strip it from the reference
        // model to match what the SUT's org parser produces.
        //
        // Layout classification (query/render source ids, render expressions)
        // is derived from each block's `source_language`, mirroring the org
        // parser's index.org handling.
        let placeholder = EntityUri::block(GEN_PLACEHOLDER);
        let is_index = self.filename == "index.org";
        for (seq, generated) in self.blocks.iter().enumerate() {
            let mut block = generated.clone();
            if block.parent_id == placeholder {
                block.parent_id = doc_uri.clone();
            }
            block.properties.remove("ID");
            // The SUT renders these blocks to org text and re-parses them; the
            // org parser `.trim()`s headlines and `.trim_end()`s content. The
            // reference takes the generated blocks verbatim, so a generator-
            // produced trailing space (the headline strategy permits one)
            // survives here while the SUT strips it — `inv-displayed-text`
            // then diverges by that space. Normalize to mirror the round-trip,
            // matching `BulkExternalAdd` and `Mutation::apply_to`.
            block.content = normalize_content_for_org_roundtrip(&block.content, block.content_type);
            // A trailing `:tag:` group on the title line re-parses as org TAGS.
            apply_org_headline_tag_split(&mut block);
            // Carry the file/generation order into `sequence` so the canonical
            // re-sequencing below recovers the same sibling order the SUT gets
            // from parsing the rendered org (the renderer is a stable sort by
            // `sibling_order_group`, preserving this order within each group).
            // Mirrors the old text-parse path's `set_sequence(sequence_counter)`.
            block.set_sequence(seq as i64);
            let block_uri = block.id.clone();

            if is_index
                && block.content_type == ContentType::Source
                && let Some(sl) = block.source_language.as_ref()
            {
                if sl.as_query().is_some() {
                    state
                        .domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    state
                        .domain
                        .layout_blocks
                        .query_source_ids
                        .insert(block_uri.clone());
                } else if matches!(sl, SourceLanguage::Render) {
                    state
                        .domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    state
                        .domain
                        .layout_blocks
                        .render_source_ids
                        .insert(block_uri.clone());
                    // Real DSL parse (same path StartApp's seed classification
                    // uses) — the exact-match `render_expr_from_rhai` lookup
                    // silently dropped generator templates not in the
                    // `valid_render_expressions` list (e.g. the GQL/SQL index
                    // variants' static `row(text("…"))` templates), leaving
                    // `root_render_expr()` stale or empty after a swap.
                    if let Ok(expr) = state.interpreter.parse_dsl(block.content.as_str()) {
                        state
                            .domain
                            .render_expressions
                            .insert(block_uri.clone(), expr);
                    }
                }
            }

            state
                .domain
                .block_state
                .block_documents
                .insert(block_uri.clone(), doc_uri.clone());
            state.domain.block_state.blocks.insert(block_uri, block);
        }

        // Re-assign sequences using canonical ordering
        let mut all_blocks: Vec<Block> =
            state.domain.block_state.blocks.values().cloned().collect();
        crate::org_utils::assign_reference_sequences_canonical(&mut all_blocks);
        state.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();

        state.rebuild_profile_tracking();
        state.pre_startup_file_count += 1;
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for WriteOrgFile {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        let doc_name = std::path::Path::new(self.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.filename);

        // Serialise the generated blocks to org text. The blocks are parented
        // to `GEN_PLACEHOLDER`, so the renderer's file id must match for them
        // to land at the top level — this reproduces the exact text the
        // previous text-first generator emitted.
        let rendered = OrgRenderer::render_entitys(
            &self.blocks,
            std::path::Path::new(self.filename.as_str()),
            &EntityUri::block(GEN_PLACEHOLDER),
        );

        // Pin the document's identity into the file so production's
        // file_sync_controller picks up the same `block:ref-doc-N` URI the
        // reference state minted, instead of falling back to name-chain
        // resolution and assigning a fresh UUID. Without this the two ID
        // spaces diverge for documents (Page blocks), but agree for content
        // blocks — because content blocks already carry `:ID:` in the body.
        let content = match state.doc_uri_by_name(doc_name) {
            Some(uri) if holon_orgmode::parser::parse_doc_id(&rendered).is_none() => {
                format!("#+ID: {}\n{}", uri.id(), rendered)
            }
            _ => rendered,
        };
        // Axis 5: custom keywords (STARTED/NEXT/…) are not in the parser's
        // default set — without the `#+TODO:` header they'd re-parse as
        // headline content instead of task states.
        let content = match &self.keyword_set {
            Some(ks) => format!("{}\n{}", ks.to_org_header(), content),
            None => content,
        };
        sut.apply_write_org_file(&self.filename, &content).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for WriteOrgFile {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

#[cfg(test)]
mod keyword_set_round_trip_tests {
    use super::*;
    use crate::pbt::generators::{
        generate_org_file_content_with_keywords, todo_keyword_set_strategy,
    };

    proptest::proptest! {
        /// Axis-5 parity guard: a generated keyword set serialized the way
        /// `apply_to_sut` does (`#+TODO:` header + `render_entitys`) must
        /// re-parse to the same per-block task states and the same document
        /// keyword set the reference model adopts. Without the header,
        /// custom keywords (STARTED/NEXT/…) re-parse as headline content —
        /// this pins the divergence shut independent of slice sampling.
        #[test]
        fn keyword_set_survives_sut_serialize_parse(
            (ks, (filename, blocks)) in todo_keyword_set_strategy().prop_flat_map(|ks| {
                (Just(ks.clone()), generate_org_file_content_with_keywords(Some(ks), false))
            })
        ) {
            let placeholder = EntityUri::block(GEN_PLACEHOLDER);
            let rendered = OrgRenderer::render_entitys(
                &blocks,
                std::path::Path::new(filename.as_str()),
                &placeholder,
            );
            let content = format!("{}\n{}", ks.to_org_header(), rendered);

            let parsed = holon_orgmode::parse_org_file(
                std::path::Path::new(&filename),
                &content,
                &placeholder,
                std::path::Path::new("."),
            )
            .expect("generated org content must parse");

            // Document adopts the keyword set (what apply_to_ref mirrors).
            prop_assert_eq!(
                parsed.document.todo_keywords(),
                Some(ks.0.clone()),
                "doc todo_keywords must round-trip"
            );

            // Each generated block's task state survives the round-trip —
            // keyword AND category (the parser categorizes via the doc's
            // done-list, the generator via TaskState::from_keyword).
            for generated in &blocks {
                let reparsed = parsed
                    .blocks
                    .iter()
                    .find(|b| b.id == generated.id)
                    .unwrap_or_else(|| panic!("block {} lost in round-trip", generated.id));
                prop_assert_eq!(
                    reparsed.task_state(),
                    generated.task_state(),
                    "task_state diverged for block {} (content {:?})",
                    generated.id,
                    generated.content
                );
            }
        }
    }
}
