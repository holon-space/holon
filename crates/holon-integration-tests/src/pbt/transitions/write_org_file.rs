//! Transition: write an org file to the temp directory.
//!
//! Mirrors the legacy logic split across `state_machine.rs:326-338` (generator),
//! `state_machine.rs:3077-3101` (precondition),
//! `state_machine.rs:1738-1931` (ref-state apply),
//! `sut.rs:661-670` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use regex::Regex;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_orgmode::OrgBlockExt;

/// Write an org file to the temp directory (before app starts).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WriteOrgFile {
    pub filename: String,
    pub content: String,
}

/// Parse the `#+TODO:` directive from raw org content. Returns the full
/// keyword set (active and done keywords, in order). Returns `None` if
/// no `#+TODO:` line is present.
///
/// Self-contained mirror of the keyword set the production org parser
/// will pick up — used so `apply_to_ref` doesn't need to read randomly-
/// initialised `state.keyword_set` and can instead derive everything
/// from `self.content`.
fn parse_todo_directive(content: &str) -> Option<Vec<String>> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#+TODO:") {
            // `#+TODO: TODO DOING | DONE CANCELLED` — split off the `|` divider
            // and collect every non-empty whitespace-separated token.
            let kws: Vec<String> = rest
                .split_whitespace()
                .filter(|tok| *tok != "|")
                .map(|tok| tok.to_string())
                .collect();
            if kws.is_empty() {
                return None;
            }
            return Some(kws);
        }
    }
    None
}

impl E2ETransitionFactory for WriteOrgFile {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let pre_startup_file_count = state.documents.len();
        let file_weight = if pre_startup_file_count < 3 { 3 } else { 1 };

        // Temporarily disable layout overrides to confirm the asymmetry-fix
        // path on a vanilla seed layout. Restore to `true` after verification.
        let state_for_preconditions = state.clone();
        let strat = crate::pbt::generators::generate_org_file_content_with_keywords(
            None,
            std::env::var("HOLON_PBT_NO_LAYOUT_OVERRIDE").is_err(), // LAYOUT_MUTATIONS_ENABLED
        )
        .prop_filter("WriteOrgFile preconditions", move |(filename, content)| {
            WriteOrgFile {
                filename: filename.clone(),
                content: content.clone(),
            }
            .preconditions(&state_for_preconditions)
            .is_good()
        })
        .prop_map(|(filename, content)| WriteOrgFile { filename, content })
        .boxed();

        Validated::Good((file_weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for WriteOrgFile {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![];

        // Reject if any block IDs in this file already exist under a different document.
        let doc_name = std::path::Path::new(self.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.filename);
        let doc_uri = state
            .doc_uri_by_name(doc_name)
            .unwrap_or_else(|| EntityUri::block("precondition-placeholder"));
        let id_re = Regex::new(r":ID:\s*(\S+)").unwrap();
        let mut any_collision = false;
        for caps in id_re.captures_iter(&self.content) {
            let block_id = caps.get(1).unwrap().as_str();
            let block_entity = EntityUri::block(block_id);
            if let Some(existing_doc) = state.block_state.block_documents.get(&block_entity)
                && *existing_doc != doc_uri
            {
                any_collision = true;
                break;
            }
        }
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
            .documents
            .insert(doc_uri.clone(), self.filename.clone());

        // Remove old content blocks from this document (handles re-writing the same file)
        let old_block_ids: Vec<EntityUri> = state
            .block_state
            .block_documents
            .iter()
            .filter(|(_, uri)| **uri == doc_uri)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &old_block_ids {
            state.block_state.blocks.remove(id);
            state.block_state.block_documents.remove(id);
            state.layout_blocks.remove(id);
        }

        // Parse #+TODO from self.content so the transition is self-contained:
        // fixture replay must reproduce todo-keyword behaviour from the saved
        // transitions alone, without depending on a randomly-initialised
        // state.keyword_set (the previous source of hidden randomness — see
        // devlog/2026-05-19-phase-c-validation-diagnosis.md).
        let parsed_todo_keywords: Option<Vec<String>> = parse_todo_directive(&self.content);

        // Add the page block (tags ⊇ ["Page"]) for this org file.
        let mut doc_block =
            Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name.clone());
        doc_block.set_page(true);
        if let Some(ref kws) = parsed_todo_keywords {
            use holon_api::types::TaskState;
            use holon_orgmode::models::OrgDocumentExt;
            let states: Vec<TaskState> = kws.iter().map(|kw| TaskState::from_keyword(kw)).collect();
            doc_block.set_todo_keywords(Some(states));
        }
        state.block_state.blocks.insert(doc_uri.clone(), doc_block);
        state
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri.clone());

        // Parse block IDs from content and add to reference state
        let id_regex = Regex::new(r":ID:\s*(\S+)").unwrap();
        let headline_regex = Regex::new(r"^\*+\s+(.+)$").unwrap();
        let src_begin_regex = Regex::new(r"(?i)#\+begin_src\s+(\w+)(?:\s.*)?$").unwrap();
        let src_id_regex = Regex::new(r":id\s+(\S+)").unwrap();
        let src_end_regex = Regex::new(r"(?i)#\+end_src").unwrap();

        let mut current_headline: Option<String> = None;
        let mut current_block_id: Option<EntityUri> = None;
        let mut in_source_block = false;
        let mut source_language: Option<String> = None;
        let mut source_content = String::new();
        let mut source_block_id: Option<String> = None;
        let mut source_block_index = 0;
        let mut sequence_counter: i64 = 0;

        for line in self.content.lines() {
            if let Some(caps) = headline_regex.captures(line) {
                current_headline = Some(caps.get(1).unwrap().as_str().trim().to_string());
                source_block_index = 0;
            } else if let Some(caps) = id_regex.captures(line) {
                let block_id = caps.get(1).unwrap().as_str().to_string();
                let raw_headline = current_headline.clone().unwrap_or_default();

                let known_keywords: Vec<String> =
                    parsed_todo_keywords.clone().unwrap_or_else(|| {
                        vec!["TODO".to_string(), "DOING".to_string(), "DONE".to_string()]
                    });
                let (content, task_keyword) = known_keywords
                    .iter()
                    .find_map(|kw| {
                        raw_headline.strip_prefix(kw.as_str()).and_then(|rest| {
                            if rest.is_empty() || rest.starts_with(' ') {
                                Some((rest.trim_start().to_string(), kw.clone()))
                            } else {
                                None
                            }
                        })
                    })
                    .map(|(c, kw)| (c, Some(kw)))
                    .unwrap_or((raw_headline, None));

                let block_uri = EntityUri::block(&block_id);
                let mut block = Block::new_text(block_uri.clone(), doc_uri.clone(), content);
                if let Some(kw) = task_keyword {
                    use holon_api::types::TaskState;
                    block.set_task_state(Some(TaskState::from_keyword(&kw)));
                }
                block.set_sequence(sequence_counter);
                sequence_counter += 1;
                state
                    .block_state
                    .block_documents
                    .insert(block_uri.clone(), doc_uri.clone());
                current_block_id = Some(block_uri.clone());
                state.block_state.blocks.insert(block_uri, block);
            } else if let Some(caps) = src_begin_regex.captures(line) {
                in_source_block = true;
                source_language = Some(caps.get(1).unwrap().as_str().to_string());
                source_content.clear();
                source_block_id = src_id_regex
                    .captures(line)
                    .map(|c| c.get(1).unwrap().as_str().to_string());
            } else if src_end_regex.is_match(line) && in_source_block {
                if let Some(parent_key) = &current_block_id {
                    let parent_block = &state.block_state.blocks[parent_key];
                    let parent_uri = parent_block.id.clone();
                    let src_id = source_block_id.take().unwrap_or_else(|| {
                        format!("{}::src::{}", parent_uri.id(), source_block_index)
                    });
                    let src_uri = EntityUri::block(&src_id);
                    let mut src_block = Block {
                        id: src_uri.clone(),
                        parent_id: parent_uri,
                        content: source_content.trim().to_string(),
                        content_type: holon_api::ContentType::Source,
                        source_language: source_language
                            .as_ref()
                            .map(|s| s.parse::<holon_api::SourceLanguage>().unwrap()),
                        created_at: 0,
                        updated_at: 0,
                        ..Block::default()
                    };
                    if self.filename == "index.org"
                        && let Some(sl) = src_block.source_language.as_ref()
                    {
                        if sl.as_query().is_some() {
                            state.layout_blocks.headline_ids.insert(parent_key.clone());
                            state.layout_blocks.query_source_ids.insert(src_uri.clone());
                        } else if matches!(sl, holon_api::SourceLanguage::Render) {
                            state.layout_blocks.headline_ids.insert(parent_key.clone());
                            state
                                .layout_blocks
                                .render_source_ids
                                .insert(src_uri.clone());
                            if let Some(expr) = super::super::reference_state::render_expr_from_rhai(
                                src_block.content.as_str(),
                            ) {
                                state.render_expressions.insert(src_uri.clone(), expr);
                            }
                        }
                    }
                    src_block.set_sequence(sequence_counter);
                    sequence_counter += 1;
                    state.block_state.blocks.insert(src_uri.clone(), src_block);
                    state
                        .block_state
                        .block_documents
                        .insert(src_uri, doc_uri.clone());
                    source_block_index += 1;
                }
                in_source_block = false;
                source_language = None;
                source_content.clear();
            } else if in_source_block {
                if !source_content.is_empty() {
                    source_content.push('\n');
                }
                source_content.push_str(line);
            }
        }

        // Re-assign sequences using canonical ordering
        let mut all_blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
        crate::org_utils::assign_reference_sequences_canonical(&mut all_blocks);
        state.block_state.blocks = all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();

        state.rebuild_profile_tracking();
        state.pre_startup_file_count += 1;
    }

    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut dyn SutHandle) {
        // Pin the document's identity into the file so production's
        // org_sync_controller picks up the same `block:ref-doc-N` URI the
        // reference state minted, instead of falling back to name-chain
        // resolution and assigning a fresh UUID. Without this the two ID
        // spaces diverge for documents (Page blocks), but agree for content
        // blocks — because content blocks already carry `:ID:` in the body.
        let doc_name = std::path::Path::new(self.filename.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.filename);
        let content = match state.doc_uri_by_name(doc_name) {
            Some(uri) if holon_orgmode::parser::parse_doc_id(&self.content).is_none() => {
                format!("#+ID: {}\n{}", uri.id(), self.content)
            }
            _ => self.content.clone(),
        };
        sut.apply_write_org_file(&self.filename, &content).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}
