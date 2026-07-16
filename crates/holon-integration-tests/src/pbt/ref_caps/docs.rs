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
}

impl RefDocumentsMut for ReferenceState {
    fn insert_document(&mut self, file_name: &str) {
        let doc_uri = self.next_synthetic_doc_uri();
        self.files
            .documents
            .insert(doc_uri.clone(), file_name.to_string());

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
        self.domain
            .block_state
            .block_documents
            .retain(|id, _| surviving.contains_key(id));
        self.domain.block_state.blocks = surviving;
        self.rebuild_profile_tracking();

        self.clear_focus_if_deleted(&doc_uri);
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
            .collect();
        for id in &old_block_ids {
            self.domain.block_state.blocks.remove(id);
            self.domain.block_state.block_documents.remove(id);
            self.domain.layout_blocks.remove(id);
            self.domain.render_expressions.remove(id);
        }

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
            let (content, marks) = crate::pbt::types::normalize_content_for_org_roundtrip(
                &block.content,
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
