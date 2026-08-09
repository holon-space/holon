//! `RefBlockTree` / `RefBlockTreeMut` / `RefApplyMutationMut` / `RefBackend`.
//!
//! @pbt kind ref
//! @pbt covers block-tree-truth — models the canonical block tree (parentage,
//!   sibling order, layout/seed/page classification) the block-correspondence
//!   invariants compare against.
//! @pbt covers org-disk-view — `RefBackend::org_blocks` predicts on-disk org
//!   form; REUSES the real `holon_orgmode::parser::split_headline_tags` (via
//!   `apply_org_headline_tag_split`) fed from ref content — legit reuse, blind
//!   only to a bug INSIDE that shared parser fn (its own tier), not a
//! hand-mirror. @pbt covers apply-mutation —
//! `RefApplyMutationMut::apply_content_mutation`   is a SECOND block-tree
//! applier (`Mutation::apply_to` + canonical re-sort),   distinct from
//! `reference_state.rs`'s structural helpers; the two sequence   disciplines
//! can disagree on post-op sibling order (see honesty-drift finding).

use std::collections::BTreeSet;

use holon_api::ContentType;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefHistoryExpectation;

use super::super::reference_state::ReferenceState;
use super::cap_id;
use super::cap_id_set;
use super::from_cap_region;
use super::parse_id;
use super::parse_id_must;
use crate::pbt::types::MutationApply;

// ─── RefBlockTree ─────────────────────────────────────────────────────

impl RefBlockTree for ReferenceState {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        let uri = parse_id(id)?;
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .map(|b| b.content.as_str())
    }

    fn is_text_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.content_type == holon_api::ContentType::Text)
    }

    fn main_editable_descendants(&self) -> Vec<EntityUri> {
        ReferenceState::main_editable_descendants(self)
            .iter()
            .map(cap_id)
            .collect()
    }

    fn focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        cap_id_set(self.rendered_focus_root(from_cap_region(region)))
    }

    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        ReferenceState::previous_sibling(self, &uri)
            .as_ref()
            .map(cap_id)
    }

    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        ReferenceState::next_sibling(self, &uri)
            .as_ref()
            .map(cap_id)
    }

    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        let b = self.domain.block_state.blocks.get(&uri)?;
        if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
            None
        } else {
            Some(cap_id(&b.parent_id))
        }
    }

    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        ReferenceState::grandparent(self, &uri).as_ref().map(cap_id)
    }

    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let Some(uri) = parse_id(parent) else {
            return vec![];
        };
        ReferenceState::sorted_children_of(self, &uri)
            .into_iter()
            .map(|b| cap_id(&b.id))
            .collect()
    }

    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        let ancestor_uris: BTreeSet<EntityUri> = ancestors.iter().filter_map(parse_id).collect();
        ReferenceState::is_descendant_of_any(self, &uri, &ancestor_uris)
    }

    fn main_panel_renders(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        ReferenceState::main_panel_renders(self, &uri)
    }

    fn is_layout_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.contains(&uri)
    }

    fn is_focusable(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.is_focusable(&uri)
    }

    fn is_no_content_update(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.render_source_ids.contains(&uri)
            || self.domain.layout_blocks.query_source_ids.contains(&uri)
            || self.domain.profile_block_ids.contains(&uri)
    }

    fn is_page_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.is_page())
    }

    fn is_source_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.content_type == ContentType::Source)
    }

    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .block_state
            .blocks
            .keys()
            .filter(|uri| {
                let is_seed = self
                    .domain
                    .block_state
                    .block_documents
                    .get(uri)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed
            })
            .map(cap_id)
            .collect()
    }
}

// ─── RefBlockTreeMut ──────────────────────────────────────────────────

impl RefBlockTreeMut for ReferenceState {
    fn push_undo_snapshot(&mut self) {
        ReferenceState::push_undo_snapshot(self);
    }

    fn set_block_content(&mut self, id: &EntityUri, text: &str) {
        let uri = parse_id_must(id);
        if let Some(b) = self.domain.block_state.blocks.get_mut(&uri) {
            // Editor-commit write path: normalize exactly like prod's
            // `SqlOperationProvider::trimmed_content`, mirroring the
            // inherent `commit_active_editor_if_changed`. The generic
            // pbt-core commit helper writes through here, so both commit
            // paths now share one normalization. Marks are re-derived from
            // the committed text (replacing any previous mark set) — the org
            // writeback→re-ingest fixed point the SUT converges to.
            let (content, marks) =
                super::super::types::normalize_content_for_org_roundtrip(text, b.content_type);
            b.content = content;
            b.marks = marks;
        }
    }

    fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        let uri = parse_id_must(id);
        self.domain
            .block_state
            .blocks
            .get(&uri)?
            .properties
            .get("task_state")
            .and_then(|v| v.as_string().map(str::to_owned))
    }

    fn promote_block_task_keyword(
        &mut self,
        id: &EntityUri,
        keyword: &str,
        stripped: &str,
    ) -> bool {
        self.set_block_content(id, stripped);
        let uri = parse_id_must(id);
        let Some(block) = self.domain.block_state.blocks.get_mut(&uri) else {
            return false;
        };
        block.properties.insert(
            "task_state".to_string(),
            holon_api::Value::String(keyword.to_string()),
        );
        block.properties.insert(
            "task_state_category".to_string(),
            holon_api::Value::String(
                holon_api::TaskState::category_str_for_keyword(keyword).to_string(),
            ),
        );
        true
    }

    fn split_block(&mut self, id: &EntityUri, position: usize) -> EntityUri {
        let uri = parse_id_must(id);
        let new_uri = ReferenceState::split_block(self, &uri, position);
        cap_id(&new_uri)
    }

    fn remint_block(&mut self, old_id: &EntityUri) -> EntityUri {
        let uri = parse_id_must(old_id);
        let new_uri = ReferenceState::remint_block(self, &uri);
        cap_id(&new_uri)
    }

    fn join_block(&mut self, id: &EntityUri) -> usize {
        let uri = parse_id_must(id);
        ReferenceState::join_block(self, &uri)
    }

    fn indent(&mut self, id: &EntityUri) {
        // Mirror `transitions/indent.rs::apply_to_ref`:
        //   prev = previous_sibling
        //   after = sorted_children_of(prev).last().id
        //   move_block(id, prev, after)
        let uri = parse_id_must(id);
        let prev = ReferenceState::previous_sibling(self, &uri)
            .expect("indent: previous sibling required");
        let after = ReferenceState::sorted_children_of(self, &prev)
            .last()
            .map(|b| b.id.clone());
        ReferenceState::move_block(self, &uri, prev, after.as_ref());
    }

    fn outdent(&mut self, id: &EntityUri) {
        let uri = parse_id_must(id);
        ReferenceState::outdent_block(self, &uri);
    }

    fn move_block(&mut self, id: &EntityUri, new_parent: EntityUri, after: Option<&EntityUri>) {
        let uri = parse_id_must(id);
        let parent_uri = parse_id_must(&new_parent);
        let after_uri = after.map(parse_id_must);
        ReferenceState::move_block(self, &uri, parent_uri, after_uri.as_ref());
    }

    fn swap_siblings(&mut self, a: &EntityUri, b: &EntityUri) {
        let a_uri = parse_id_must(a);
        let b_uri = parse_id_must(b);
        ReferenceState::swap_sequence(self, &a_uri, &b_uri);
    }

    fn undo_last_and_reset_cursors(&mut self) {
        self.pop_undo_to_redo();
        // Undo may restore different content — reset all cursors.
        for region in self
            .ui
            .tab
            .focused_entity_id
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.ui
                .tab
                .focused_cursor
                .insert(region, super::super::ui_types::CursorPosition::start());
        }
    }

    fn redo_last_and_reset_cursors(&mut self) {
        self.pop_redo_to_undo();
        for region in self
            .ui
            .tab
            .focused_entity_id
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.ui
                .tab
                .focused_cursor
                .insert(region, super::super::ui_types::CursorPosition::start());
        }
    }
}

impl RefApplyMutationMut for ReferenceState {
    fn apply_content_mutation(
        &mut self,
        mutation: &holon_pbt_core::types::Mutation,
        crosses_org_boundary: bool,
    ) {
        use holon_pbt_core::types::Mutation;

        if let Mutation::Create { id, parent_id, .. } = mutation {
            let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                parent_id.clone()
            } else {
                // The new block belongs to its parent's document. But when the
                // parent is itself a top-level page (its own `block_documents`
                // entry is `no_parent`/`sentinel`), the page IS the document —
                // the child lives in the page's org file, not in the page's
                // (sentinel) document. Inheriting the sentinel would misclassify
                // the child as a seed block and drop it from the `/org` view.
                match self.domain.block_state.block_documents.get(parent_id) {
                    Some(doc) if !doc.is_no_parent() && !doc.is_sentinel() => doc.clone(),
                    _ => parent_id.clone(),
                }
            };
            self.domain
                .block_state
                .block_documents
                .insert(id.clone(), doc_uri);
        }

        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        mutation.apply_to(&mut blocks);
        crate::org_utils::assign_reference_sequences_canonical(&mut blocks);
        let surviving: std::collections::BTreeMap<EntityUri, Block> =
            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        // A mutation that REMOVES a block (`Delete`, which cascades over
        // descendants) ends that block's file-backing: prod's redo must leave it
        // dead, so `rematerialize_file_ingested` must never resurrect it from the
        // pre-undo state. Keyed on the before/after difference rather than on
        // `Mutation::Delete`'s own id so the cascade is covered too.
        self.files
            .ingest_origin_blocks
            .retain(|id| surviving.contains_key(id));
        self.domain.block_state.blocks = surviving;

        // External (org-file) mutation: the created/updated block is written to
        // disk and RE-INGESTED, so a trailing `:tag:` group on its headline (e.g.
        // `:PROPERTIES:`) re-parses into `block.tags`, not content — mirror that
        // FILE-parse reinterpretation exactly as `bulk_add_blocks` / the
        // CreateDocument ingest do. `Mutation::apply_to` above already applied the
        // inline-mark round-trip normalization; the tag split is the remaining
        // org-file-boundary lens. A `UI` mutation stays in-store (echo-suppressed),
        // so it keeps raw content and this is skipped.
        if crosses_org_boundary {
            let affected = match mutation {
                Mutation::Create { id, .. } | Mutation::Update { id, .. } => Some(id.clone()),
                _ => None,
            };
            if let Some(id) = affected
                && let Some(block) = self.domain.block_state.blocks.get_mut(&id)
            {
                super::super::types::apply_org_headline_tag_split(block);
            }
        }
        self.rebuild_profile_tracking();

        if let Mutation::Update { id, fields, .. } = mutation
            && self.domain.layout_blocks.render_source_ids.contains(id)
            && fields.contains_key("content")
            && let Some(block) = self.domain.block_state.blocks.get(id)
            && let Some(expr) =
                super::super::reference_state::render_expr_from_rhai(block.content.as_str())
        {
            self.domain.render_expressions.insert(id.clone(), expr);
        }

        self.domain.block_state.next_id += 1;

        if let Mutation::Update { id, fields, .. } = mutation
            && fields.contains_key("content")
        {
            self.reset_cursor_if_focused(id);
            // Prod's data subscription refreshes an idle (clean) active editor
            // from its live content cell — the exact source `editor_live_text`
            // reads. Mirror that so `inv-editor-text/mirror` sees the refreshed
            // content, not a stale pre-update buffer (dirty editors keep their
            // pending user text; the split-with-pending-edit contract).
            self.refresh_clean_active_editor(id);
        }
    }
}

// ─── RefBackend ───────────────────────────────────────────────────────

impl RefBackend for ReferenceState {
    /// Every reference block whose document is NOT a seed document. The runner
    /// has already remapped `id`/`parent_id` into SUT ID space via
    /// `with_resolved_doc_uris`, so these clone directly into the comparison.
    fn non_seed_blocks(&self) -> Vec<holon_api::Block> {
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = self
                    .domain
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed
            })
            .cloned()
            .collect()
    }

    /// Resolved `block_documents` keys whose document is a seed document.
    fn seed_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| cap_id(id))
            .collect()
    }

    /// Reference blocks as they should appear on disk in org files. The runner
    /// already remapped `id`/`parent_id` into SUT ID space via
    /// `with_resolved_doc_uris` (so `#+ID:`-resolved doc parents are
    /// `block:<uuid>` and split-N placeholders are real UUIDs). The remaining
    /// org-specific step: a
    /// document parent the controller hasn't resolved yet is still a synthetic
    /// doc URI — a key in `self.documents` — and the org parser writes it on
    /// disk as `file:<filename>`. Remap those so the comparison matches.
    fn org_blocks(&self) -> Vec<holon_api::Block> {
        // Blocks always carry a `block:` parent — a top-level org block's
        // parent is its document block (`block:<doc-id>`), which is exactly
        // what `parse_org_file_blocks` reconstructs from the file's `:ID:`
        // drawer. (`EntityUri::file` parents are a future concern, not used
        // for block parentage today.)
        let seed = self.seed_block_ids();
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| !seed.contains(&b.id))
            .filter(|b| !b.is_page())
            .cloned()
            .map(|mut b| {
                // On disk the first content line is the headline title, so a
                // trailing `:tag:` group re-parses as org TAGS (the in-memory
                // stores keep the raw content — e.g. after an editor split
                // that lands exactly before a tag group). The disk view must
                // look through that lens.
                crate::pbt::types::apply_org_headline_tag_split(&mut b);
                b
            })
            .collect()
    }
}

/// C2 provenance-oracle expectation. The values are stamped onto the resolved
/// ref by the harness `run_report` (from the id-reconcile map); this cap just
/// surfaces them to the `history_*` correspondences.
impl RefHistoryExpectation for ReferenceState {
    fn ever_created_ids(&self) -> BTreeSet<EntityUri> {
        self.history_ever_created.clone()
    }

    fn min_recorded_op_groups(&self) -> usize {
        self.history_min_op_groups
    }
}

/// Undo→redo burned-id oracle. Stamped onto the resolved ref by the harness
/// `run_report` from the reconcile's retired pairs; this cap just surfaces it
/// to `inv-undo-redo-reference-heal`.
impl holon_pbt_core::capabilities::RefUndoRedoBurned for ReferenceState {
    fn burned_block_ids(&self) -> BTreeSet<EntityUri> {
        self.undo_redo_burned_ids.clone()
    }
}
