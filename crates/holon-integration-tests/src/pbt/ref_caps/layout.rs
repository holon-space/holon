//! `RefLayout` / `RefRenderExpr` / `RefLayoutInteract` / `RefLayoutMutate`.
//!
//! @pbt kind ref
//! @pbt covers layout-render-config — layout-block classification
//!   (headline / query-source / render-source), author-intent `RenderExpr` per
//!   render source, and interactivity/draggability predictions. FIDELITY: the
//!   render-expr↔Rhai mapping is a closed vocabulary (`render_expr_from_rhai`);
//!   a Rhai string outside it round-trips as an untracked render (table()
//!   fallback in `get_block_data`, disclosed via warn).

use std::collections::BTreeSet;

use holon_api::ContentType;
use holon_api::EdgeFieldUpdate;
use holon_api::Region;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefRenderExpr;

use super::super::reference_state::ReferenceState;
use super::cap_id;
use super::from_cap_region;

impl RefLayout for ReferenceState {
    fn layout_block_ids(&self) -> BTreeSet<EntityUri> {
        let ids: BTreeSet<&holon_api::entity_uri::EntityUri> = self
            .domain
            .layout_blocks
            .headline_ids
            .iter()
            .chain(self.domain.layout_blocks.query_source_ids.iter())
            .chain(self.domain.layout_blocks.render_source_ids.iter())
            .collect();
        ids.into_iter().map(cap_id).collect()
    }

    fn profile_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain.profile_block_ids.iter().map(cap_id).collect()
    }

    fn has_blocks_profile(&self) -> bool {
        self.has_blocks_profile()
    }

    fn has_user_index_org(&self) -> bool {
        ReferenceState::has_user_index_org(self)
    }

    fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain.block_state.blocks.keys().map(cap_id).collect()
    }

    fn expected_visible_content_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        let focus_roots = self.rendered_focus_root(from_cap_region(region));
        // A block is legitimately in the region's main panel iff it descends from
        // the current focus root — regardless of how it renders. Per the fork-A
        // program-rendering ruling (block_profile.yaml), source blocks are NOT
        // display-hidden: a rule/program source renders as a `rule_card`, a plain
        // source (e.g. python) renders as the `source`→`query_result` variant, and
        // only the `holon_source` spacer(0) case is invisible. All of these are
        // valid rows for the current root, so none are "stale". The former
        // `content_type != Source` exclusion wrongly reported the visible plain-
        // source row as stale; staleness is a cross-ROOT property, gated purely by
        // `is_descendant_of_any(focus_roots)` here.
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| self.is_descendant_of_any(&b.id, &focus_roots))
            .map(|b| cap_id(&b.id))
            .collect()
    }

    fn has_user_documents(&self) -> bool {
        !self.files.documents.is_empty()
    }

    fn region_entity_focused(&self, region: CapRegion) -> bool {
        self.ui
            .tab
            .focused_entity_id
            .contains_key(&from_cap_region(region))
    }
}

impl RefRenderExpr for ReferenceState {
    fn render_expr_ids(&self) -> Vec<EntityUri> {
        self.domain.render_expressions.keys().cloned().collect()
    }
    fn has_render_expr(&self, id: &EntityUri) -> bool {
        self.domain.render_expressions.contains_key(id)
    }
    fn render_expr_mentions(&self, id: &EntityUri, needle: &str) -> bool {
        self.domain
            .render_expressions
            .get(id)
            .is_some_and(|expr| crate::pbt::value_fn_invariants::rhai_mentions(expr, needle))
    }
}

impl RefLayoutInteract for ReferenceState {
    fn render_source_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .layout_blocks
            .render_source_ids
            .iter()
            .cloned()
            .collect()
    }
    fn query_source_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .layout_blocks
            .query_source_ids
            .iter()
            .cloned()
            .collect()
    }
    fn is_immutable(&self, id: &EntityUri) -> bool {
        self.domain.layout_blocks.is_immutable(id)
    }
    fn block_renders_draggable(&self, id: &EntityUri) -> bool {
        ReferenceState::block_renders_draggable(self, id)
    }
    fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri> {
        ReferenceState::main_rendered_block_ids(self)
    }
    fn region_focused_entity(&self, region: CapRegion) -> Option<EntityUri> {
        self.focused_entity(from_cap_region(region)).cloned()
    }
    fn focused_main_editable(&self) -> Option<EntityUri> {
        ReferenceState::focused_main_editable(self)
    }
    fn block_has_tag(&self, id: &EntityUri, tag: &str) -> bool {
        self.domain
            .block_state
            .blocks
            .get(id)
            .is_some_and(|b| b.tags.contains(tag))
    }
    fn doc_has_editable_text(&self, doc_uri: &EntityUri) -> bool {
        self.domain.block_state.blocks.values().any(|b| {
            b.parent_id == *doc_uri
                && b.content_type == ContentType::Text
                && !b.is_page()
                && !self.domain.layout_blocks.contains(&b.id)
        })
    }

    fn headline_ids(&self) -> Vec<EntityUri> {
        self.domain
            .layout_blocks
            .headline_ids
            .iter()
            .cloned()
            .collect()
    }
}

impl RefLayoutMutate for ReferenceState {
    fn apply_click_focus(&mut self, region: Region, block_id: &EntityUri) {
        use crate::pbt::ui_types::CursorPosition;
        use crate::pbt::ui_types::OpenPinEntry;
        // A real click outside the active editor blurs it (real-editor-only commit
        // via `blur_active_editor`). Same-block clicks don't blur.
        if self
            .ui
            .tab
            .active_editor
            .as_ref()
            .is_some_and(|e| e.block_id != *block_id)
        {
            self.blur_active_editor();
        }
        if self.predicts_navigation_focus(block_id, region) {
            // Sidebar `selectable` → `navigation.focus(region=main)`: mirror the
            // nav-history push (see navigate_focus.rs for rationale).
            let history = self
                .ui
                .tab
                .navigation_history
                .entry(Region::Main)
                .or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(block_id.clone()));
            history.cursor = history.entries.len() - 1;

            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(Region::Main).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(block_id.clone()),
                added_ts_logical,
            });

            self.ui.tab.focused_entity_id.remove(&Region::Main);
            self.ui.tab.focused_cursor.remove(&Region::Main);
            self.ui.tab.focused_block = Some(block_id.clone());
        } else {
            // Editor focus only — the nav cursor is unchanged (ADR 0010: focus is
            // in-memory state, set directly).
            self.ui.tab.focused_block = Some(block_id.clone());
            self.ui
                .tab
                .focused_entity_id
                .insert(region, block_id.clone());
            self.ui
                .tab
                .focused_cursor
                .insert(region, CursorPosition::start());
        }
    }

    fn apply_slash_delete(&mut self, block_id: &EntityUri) {
        use holon_pbt_core::types::Mutation;
        use holon_pbt_core::types::MutationEvent;
        use holon_pbt_core::types::MutationSource;
        // Leaf-reversibility gate (same DeclaredIrreversible rule as join_block):
        // the engine's block `delete` only produces a create-inverse for a leaf;
        // a delete that CASCADES to descendants is declared irreversible. Snapshot
        // only for leaves, else the ref undo stack desyncs from the engine's.
        if self.sorted_children_of(block_id).is_empty() {
            self.push_undo_snapshot();
        }
        self.apply_mutation(&MutationEvent {
            source: MutationSource::UI,
            mutation: Mutation::Delete {
                id: block_id.clone(),
            },
        });
        self.clear_focus_if_deleted(block_id);
    }

    fn set_edge_field_value(&mut self, id: &EntityUri, update: &EdgeFieldUpdate) {
        // A SetEdgeField is its own User-origin undo step: the engine journals a
        // whole-set-restore inverse for the edge write (edge fields are always
        // reversible — no leaf/cascade gate like delete), so the ref records a
        // snapshot to keep its undo stack in correspondence with the SUT's. Omit
        // it and a later UndoLastMutation pops mismatched steps (SUT retracts the
        // edge; ref reaches past it to the prior mutation → block-count skew).
        self.push_undo_snapshot();
        let block = self
            .domain
            .block_state
            .blocks
            .get_mut(id)
            .expect("set_edge_field_value: subject block must exist (precondition)");
        // Direct field assignment (public edge-field columns); `is_page` is
        // computed from `tags` on read, so no cached state to sync.
        match update {
            EdgeFieldUpdate::Tags(tags) => block.tags = tags.clone(),
            EdgeFieldUpdate::Requires(reqs) => block.requires = reqs.clone(),
            EdgeFieldUpdate::AdviceSuppressed(reqs) => block.advice_suppressed = reqs.clone(),
            EdgeFieldUpdate::ContributesTo(reqs) => block.contributes_to = reqs.clone(),
        }
    }

    fn bulk_add_blocks(&mut self, doc_uri: &EntityUri, blocks: &[Block]) {
        for block in blocks {
            let mut block = block.clone();
            // Mirror the org round-trip normalization `Mutation::apply_to` does.
            // Parse order: tag split off the raw headline first, THEN mark
            // extraction (see write_org_file ingest above).
            crate::pbt::types::apply_org_headline_tag_split(&mut block);
            let (content, marks) = crate::pbt::types::normalize_content_for_org_roundtrip(
                &block.content,
                block.content_type,
            );
            block.content = content;
            block.marks = marks;
            let id = block.id.clone();
            self.domain.block_state.blocks.insert(id.clone(), block);
            self.domain
                .block_state
                .block_documents
                .insert(id.clone(), doc_uri.clone());
            // An external bulk add lands as an org-file rewrite the watcher
            // re-ingests — INGEST-origin, so it survives prod's undo and must
            // survive the oracle's snapshot restore too.
            self.files.ingest_origin_blocks.insert(id);
        }
        let mut all_blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::org_utils::assign_reference_sequences_canonical(&mut all_blocks);
        self.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
        self.domain.block_state.next_id += blocks.len();
    }

    fn create_block_under(&mut self, parent: &EntityUri, content: &str) {
        ReferenceState::create_block_under(self, parent, content);
    }

    fn create_block_under_with_id(&mut self, parent: &EntityUri, content: &str, id: EntityUri) {
        ReferenceState::create_block_under_with_id(self, parent, content, id);
    }

    fn birth_block_via_creation_slot(&mut self, parent: &EntityUri, content: &str) {
        ReferenceState::birth_block_under_slot(self, parent, content);
    }

    fn seed_template_definition(
        &mut self,
        parent: &EntityUri,
        content: &str,
        marks: Option<Vec<holon_api::MarkSpan>>,
        id: EntityUri,
    ) {
        ReferenceState::create_block_under_with_id(self, parent, content, id.clone());
        if let Some(b) = self.domain.block_state.blocks.get_mut(&id) {
            b.marks = marks;
        }
        // Re-classify as SEED. `insert_block_under_no_snapshot` derives the
        // document from the parent, which for a definition CHILD is the
        // definition root — a real document, so the child would start being
        // compared as user content. `seed_block_ids` keys on a `no_parent`
        // document, so writing the sentinel here restores the exclusion the
        // definition blocks have always had, while `parent_id` stays truthful.
        self.domain
            .block_state
            .block_documents
            .insert(id, EntityUri::no_parent());
    }

    fn apply_instantiate_template(
        &mut self,
        target_parent: &EntityUri,
        inst_root_id: EntityUri,
        inst_child_id: EntityUri,
        root_content: &str,
        child_content: &str,
        child_marks: Option<Vec<holon_api::MarkSpan>>,
        template_id: &str,
    ) {
        ReferenceState::apply_instantiate_template(
            self,
            target_parent,
            inst_root_id,
            inst_child_id,
            root_content,
            child_content,
            child_marks,
            template_id,
        );
    }

    fn apply_block_to_page(
        &mut self,
        origin: &EntityUri,
        page_id: EntityUri,
        destination_parent: &EntityUri,
    ) {
        ReferenceState::apply_block_to_page(self, origin, page_id, destination_parent);
    }
}

/// The page-identity surface (`RenamePage` / `CreatePageAtFreedPath`). Every
/// method delegates to the `ReferenceState` home above; nothing new lives here.
impl holon_pbt_core::capabilities::RefPageIdentity for ReferenceState {
    fn freed_page_paths(&self) -> Vec<String> {
        ReferenceState::freed_page_paths_ref(self)
    }

    fn page_path_of_ref(&self, id: &EntityUri) -> Option<String> {
        ReferenceState::page_path_of_ref(self, id)
    }

    fn ref_resolve_page_name(&self, hint: &str) -> Option<EntityUri> {
        ReferenceState::ref_resolve_page_name(self, hint)
    }

    fn page_titles(&self) -> Vec<String> {
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.is_page())
            .map(|b| b.content.trim().to_string())
            .collect()
    }

    fn apply_page_rename(&mut self, page_id: &EntityUri, new_title: &str) {
        ReferenceState::apply_page_rename(self, page_id, new_title);
    }

    fn apply_create_page_at_path(&mut self, path: &str) {
        ReferenceState::apply_create_page_at_path(self, path);
    }
}
