//! `RefDocuments` / `RefDocumentsMut`.
//!
//! @pbt kind ref
//! @pbt covers doc-identity — file-adapter doc identity + naming
//!   (`doc_uri → filename`, page-block creation on `insert_document`). Delete
//!   cascades through the SAME `Mutation::Delete` + canonical re-sort machinery
//!   `apply_content_mutation` uses (single applier discipline for deletes).

use holon_api::ContentType;
use holon_api::SourceLanguage;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_orgmode::OrgBlockExt;
use holon_orgmode::OrgDocumentExt;
use holon_pbt_core::capabilities::DrawnHome;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefDocumentsMut;

use super::super::reference_state::ReferenceState;
use crate::pbt::types::MutationApply;

impl RefDocuments for ReferenceState {
    fn document_names(&self) -> Vec<String> {
        self.files.documents.values().cloned().collect()
    }
    fn has_document(&self, file_name: &str) -> bool {
        self.files
            .documents
            .values()
            .any(|name| name.as_str() == file_name)
    }
    fn document_count(&self) -> usize {
        self.files.documents.len()
    }
    fn doc_uri_by_name(&self, name: &str) -> Option<EntityUri> {
        ReferenceState::doc_uri_by_name(self, name)
    }
    fn block_document_of(&self, block_id: &EntityUri) -> Option<EntityUri> {
        self.domain
            .block_state
            .block_documents
            .get(block_id)
            .cloned()
    }
    fn has_non_seed_advice_rule(&self) -> bool {
        !crate::pbt::advice_expectation::non_seed_advice_rule_blocks(&self.domain.block_state)
            .is_empty()
    }
    fn document_uris(&self) -> Vec<EntityUri> {
        self.files.documents.keys().cloned().collect()
    }
    fn has_document_uri(&self, uri: &EntityUri) -> bool {
        self.files.documents.contains_key(uri)
    }
    fn file_home_of(&self, block_id: &EntityUri) -> DrawnHome {
        if !self.files.documents.contains_key(block_id)
            && !self.domain.block_state.blocks.contains_key(block_id)
        {
            return DrawnHome::Unmodelled;
        }
        let mut cursor = block_id.clone();
        for _ in 0..=self.domain.block_state.blocks.len() {
            if self.files.documents.contains_key(&cursor) {
                return DrawnHome::File(cursor);
            }
            let Some(block) = self.domain.block_state.blocks.get(&cursor) else {
                return DrawnHome::Storeless;
            };
            if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                return DrawnHome::Storeless;
            }
            cursor = block.parent_id.clone();
        }
        panic!("walking `{block_id}`'s parents outlasted the block count — the draw's tree cycles");
    }
}

impl RefDocumentsMut for ReferenceState {
    fn rehome_to_native_storage(&mut self, block_id: &EntityUri) {
        // A DISTINCT state, never the seed spelling: the block leaves disk and
        // stays in every block comparison. `block_documents` is left alone —
        // writing the sentinel there would mark it seed scaffolding and
        // subtract it from both sides of every `blocks-match-ref` arm.
        self.domain
            .block_state
            .native_homed
            .insert(block_id.clone());
    }

    fn insert_document(&mut self, file_name: &str) {
        let doc_uri = self.next_synthetic_doc_uri();
        self.files
            .documents
            .insert(doc_uri.clone(), file_name.to_string());
        self.files.ingest_origin_blocks.insert(doc_uri.clone());

        let doc_name = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_string();
        let mut doc_block = Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name);
        doc_block.set_page(true);
        // New empty documents don't have #+TODO: headers — keywords only appear
        // after the file is written with content.
        self.domain
            .block_state
            .blocks
            .insert(doc_uri.clone(), doc_block);
        self.domain
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri);
    }

    fn remove_document(&mut self, file_name: &str) {
        let doc_uri = self
            .files
            .documents
            .iter()
            .find(|(_, name)| name.as_str() == file_name)
            .map(|(uri, _)| uri.clone())
            .unwrap_or_else(|| {
                panic!(
                    "RefDocumentsMut::remove_document: '{file_name}' not in files.documents \
                     (precondition hole)"
                )
            });
        self.files.documents.remove(&doc_uri);

        // Cascade-delete the page block + all descendants through the same
        // `Mutation::Delete` machinery `ApplyMutation` uses (BFS over parent_id),
        // then re-canonicalize exactly like apply_mutation does.
        let mutation = holon_pbt_core::types::Mutation::Delete {
            id: doc_uri.clone(),
        };
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        mutation.apply_to(&mut blocks);
        crate::org_utils::assign_reference_sequences_canonical(&mut blocks);
        let surviving: std::collections::BTreeMap<EntityUri, Block> =
            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        // The cascade removes the page block AND every descendant. Focus and the
        // active-editor mirror must be cleared for ALL of them, not just the page
        // (`clear_focus_if_deleted`'s contract: a deleted block closes its editor).
        // Clearing only `doc_uri` left an ActiveEditor pinned to a descendant the
        // ref had just deleted — a self-inconsistent oracle that made
        // `inv-editor-text/mirror` compare that ghost editor's cached text against
        // the SUT's (correctly empty) cell.
        let removed: Vec<EntityUri> = self
            .domain
            .block_state
            .blocks
            .keys()
            .filter(|id| !surviving.contains_key(*id))
            .cloned()
            .collect();
        assert!(
            removed.contains(&doc_uri),
            "RefDocumentsMut::remove_document: cascade did not remove the page block \
             {doc_uri} itself (removed={removed:?})"
        );
        self.domain
            .block_state
            .block_documents
            .retain(|id, _| surviving.contains_key(id));
        self.domain.block_state.blocks = surviving;
        self.rebuild_profile_tracking();

        for id in &removed {
            self.clear_focus_if_deleted(id);
            // The file is gone, so nothing here is file-backed any more — an undo
            // must not resurrect it (`rematerialize_file_ingested`).
            self.files.ingest_origin_blocks.remove(id);
        }
    }

    fn rename_document(&mut self, old_file_name: &str, new_file_name: &str) {
        // File-move spec: a document page's title FOLLOWS its file name. Resolve
        // the doc by its OLD filename, re-point the filename, and retitle the
        // page block to the NEW file stem. Production now matches this via the
        // atomic Rename port: `FileSyncController::on_file_renamed` re-homes the
        // doc and retitles the doc-root to the new stem (no delete window), so
        // the SUT converges with this reference.
        let doc_uri = self
            .files
            .documents
            .iter()
            .find(|(_, name)| name.as_str() == old_file_name)
            .map(|(uri, _)| uri.clone())
            .unwrap_or_else(|| {
                panic!(
                    "RefDocumentsMut::rename_document: '{old_file_name}' not in \
                     files.documents (precondition hole)"
                )
            });
        let new_stem = std::path::Path::new(new_file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(new_file_name)
            .to_string();
        self.files
            .documents
            .insert(doc_uri.clone(), new_file_name.to_string());
        self.domain
            .block_state
            .blocks
            .get_mut(&doc_uri)
            .expect("rename_document: doc page block must exist (precondition)")
            .content = new_stem;
        self.recanon_and_rebuild();
    }

    fn seed_org_file(
        &mut self,
        filename: &str,
        blocks: &[Block],
        todo_keywords: Option<Vec<holon_api::TaskState>>,
    ) {
        let doc_name = std::path::Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename)
            .to_string();
        let doc_uri = self
            .doc_uri_by_name(&doc_name)
            .unwrap_or_else(|| self.next_synthetic_doc_uri());
        self.files
            .documents
            .insert(doc_uri.clone(), filename.to_string());

        // Remove old content blocks from this document (re-writing the same file).
        let old_block_ids: Vec<EntityUri> = self
            .domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, uri)| **uri == doc_uri)
            .map(|(id, _)| id.clone())
            // `block_documents` remembers the file a block CAME from, so a
            // re-homed block still names this document here. Production keeps
            // it in native storage, untouched by any rewrite of that file —
            // sweeping it away would delete from the model a block the store
            // still holds.
            .filter(|id| !self.domain.block_state.is_off_disk(id))
            .collect();
        for id in &old_block_ids {
            self.domain.block_state.blocks.remove(id);
            self.domain.block_state.block_documents.remove(id);
            self.domain.layout_blocks.remove(id);
            self.domain.render_expressions.remove(id);
            // A re-write drops the previous parse; only the ids this pass
            // re-inserts below are still file-backed. The doc root is re-added
            // right after, so it stays in the set.
            self.files.ingest_origin_blocks.remove(id);
        }
        // The page block and every block this pass parses out of the file are
        // INGEST-origin: prod never journals them for undo, so they must survive
        // an oracle snapshot restore (see `rematerialize_file_ingested`).
        self.files.ingest_origin_blocks.insert(doc_uri.clone());

        // Add the page block (tags ⊇ ["Page"]) for this org file.
        let mut doc_block =
            Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name.clone());
        doc_block.set_page(true);
        // Mirror the SUT parser: a `#+TODO:` header lands on the document block as
        // the `todo_keywords` property.
        if let Some(kw) = &todo_keywords {
            doc_block.set_todo_keywords(Some(kw.clone()));
        }
        self.domain
            .block_state
            .blocks
            .insert(doc_uri.clone(), doc_block);
        self.domain
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri.clone());

        // Insert the generated blocks directly — no re-parsing. Top-level headings
        // parented to `GEN_PLACEHOLDER` are remapped to the resolved doc uri; the
        // `ID` renderer hint is stripped; layout classification is derived from each
        // block's `source_language`, mirroring the org parser's index.org handling.
        let placeholder =
            EntityUri::block(crate::pbt::transitions::write_org_file::GEN_PLACEHOLDER);
        let is_index = filename == "index.org";
        for (seq, generated) in blocks.iter().enumerate() {
            let mut block = generated.clone();
            if block.parent_id == placeholder {
                block.parent_id = doc_uri.clone();
            }
            block.properties.remove("ID");
            // File-parse order: the org parser splits trailing `:tag:` groups
            // off the RAW headline line first, then extracts inline marks from
            // the tag-less title — mirror that order or mark offsets computed
            // over a still-tagged line diverge from the SUT's.
            crate::pbt::types::apply_org_headline_tag_split(&mut block);
            let (content, marks) = crate::pbt::types::normalize_parsed_block_for_org_roundtrip(
                &block.content,
                block.marks.as_deref(),
                block.content_type,
            );
            block.content = content;
            block.marks = marks;
            block.set_sequence(seq as i64);
            let block_uri = block.id.clone();

            if is_index
                && block.content_type == ContentType::Source
                && let Some(sl) = block.source_language.as_ref()
            {
                if sl.as_query().is_some() {
                    self.domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    self.domain
                        .layout_blocks
                        .query_source_ids
                        .insert(block_uri.clone());
                } else if matches!(sl, SourceLanguage::Render) {
                    self.domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    self.domain
                        .layout_blocks
                        .render_source_ids
                        .insert(block_uri.clone());
                    if let Ok(expr) = self.harness.interpreter.parse_dsl(block.content.as_str()) {
                        self.domain
                            .render_expressions
                            .insert(block_uri.clone(), expr);
                    }
                }
            }

            self.domain
                .block_state
                .block_documents
                .insert(block_uri.clone(), doc_uri.clone());
            self.files.ingest_origin_blocks.insert(block_uri.clone());
            self.domain.block_state.blocks.insert(block_uri, block);
        }

        // Re-assign sequences using canonical ordering.
        let mut all_blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::org_utils::assign_reference_sequences_canonical(&mut all_blocks);
        self.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();

        self.rebuild_profile_tracking();
        self.files.pre_startup_file_count += 1;
    }
}

#[cfg(test)]
mod rehome_state_tests {
    use holon_pbt_core::capabilities::RefBackend;
    use holon_pbt_core::capabilities::RefBlockTree;

    use super::*;
    use crate::pbt::state_machine::fresh_reference_state;

    /// A re-homed block leaves DISK and stays in every block comparison.
    ///
    /// Spelling "off disk" as the seed sentinel would satisfy the first half
    /// and silently break the second: `seed_block_ids` is subtracted from BOTH
    /// sides of every `blocks-match-ref` arm, so the block's existence,
    /// content, parent and sort_key would be asserted by nothing and the
    /// keystone would stay green through a corrupted or deleted row.
    #[test]
    fn a_rehomed_block_leaves_disk_but_stays_compared() {
        let mut state = fresh_reference_state(holon_pbt_core::Wiring::loro_backend());
        let page = EntityUri::block("page");
        let leaf = EntityUri::block("leaf");

        let mut page_block = Block::new_text(page.clone(), EntityUri::no_parent(), "page");
        page_block.set_page(true);
        state
            .domain
            .block_state
            .blocks
            .insert(page.clone(), page_block);
        state
            .files
            .documents
            .insert(page.clone(), "page.org".to_string());
        state.domain.block_state.blocks.insert(
            leaf.clone(),
            Block::new_text(leaf.clone(), page.clone(), "leaf"),
        );

        assert!(
            state.org_blocks().iter().any(|b| b.id == leaf),
            "precondition: the leaf starts out on disk"
        );

        state.rehome_to_native_storage(&leaf);

        assert!(
            !state.org_blocks().iter().any(|b| b.id == leaf),
            "a re-homed block must leave the on-disk view"
        );
        assert!(
            state.non_seed_blocks().iter().any(|b| b.id == leaf),
            "a re-homed block must STILL be compared — subtracting it from the \
             block oracles is how a corrupted or deleted row goes unnoticed"
        );
        assert!(
            state.all_non_seed_block_ids().contains(&leaf),
            "…and must stay eligible for later transitions, or no holon-native-homed \
             block is ever exercised under any operation"
        );
    }

    /// Off-disk-ness is ANCESTRAL, so a block created under a re-homed one is
    /// off disk too — without any create/split path having to propagate a flag.
    /// Modelling it per-block put the child back on disk in a file whose tree
    /// no longer contains it.
    #[test]
    fn a_child_of_a_rehomed_block_is_off_disk_but_still_compared() {
        let mut state = fresh_reference_state(holon_pbt_core::Wiring::loro_backend());
        let page = EntityUri::block("page");
        let leaf = EntityUri::block("leaf");
        let child = EntityUri::block("child");

        let mut page_block = Block::new_text(page.clone(), EntityUri::no_parent(), "page");
        page_block.set_page(true);
        state
            .domain
            .block_state
            .blocks
            .insert(page.clone(), page_block);
        state
            .files
            .documents
            .insert(page.clone(), "page.org".to_string());
        state.domain.block_state.blocks.insert(
            leaf.clone(),
            Block::new_text(leaf.clone(), page.clone(), "leaf"),
        );
        state.rehome_to_native_storage(&leaf);

        // Created AFTER the re-home, under the re-homed block.
        state.domain.block_state.blocks.insert(
            child.clone(),
            Block::new_text(child.clone(), leaf.clone(), "child"),
        );

        assert!(
            !state.org_blocks().iter().any(|b| b.id == child),
            "a child of a re-homed block must not be expected on disk"
        );
        assert!(
            state.non_seed_blocks().iter().any(|b| b.id == child),
            "…and must still be compared"
        );
    }

    /// Re-writing the file a block CAME from must not delete it from the model:
    /// `block_documents` still names that document, but production keeps the
    /// re-homed block in native storage, untouched by the rewrite.
    #[test]
    fn rewriting_the_old_file_keeps_a_rehomed_block() {
        let mut state = fresh_reference_state(holon_pbt_core::Wiring::loro_backend());
        let leaf = EntityUri::block("leaf");
        state.seed_org_file(
            "doc_1.org",
            &[Block::new_text(
                leaf.clone(),
                EntityUri::no_parent(),
                "leaf",
            )],
            None,
        );
        let doc = state
            .domain
            .block_state
            .block_documents
            .get(&leaf)
            .cloned()
            .expect("the seeded block names its document");
        assert!(
            !doc.is_no_parent(),
            "precondition: it came from a real file"
        );

        state.rehome_to_native_storage(&leaf);
        // The SAME file is written again — the sweep runs over everything
        // `block_documents` still attributes to it.
        state.seed_org_file("doc_1.org", &[], None);

        assert!(
            state.domain.block_state.blocks.contains_key(&leaf),
            "a re-homed block must survive a rewrite of the file it left — production \
             still holds it, so deleting it here reports a phantom 'only in SUT'"
        );
    }
}
