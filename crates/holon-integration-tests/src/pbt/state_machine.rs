//! Reference state machine: `VariantRef` wrapper and `ReferenceStateMachine` impl.
//!
//! This contains the transition generation, preconditions, and reference model
//! application logic for the property-based test.

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, Region, SourceLanguage, Value};

use holon_orgmode::models::OrgBlockExt;

use crate::assign_reference_sequences_canonical;

use super::generators::*;
use super::query::WatchSpec;
use super::reference_state::{NavigationHistory, ReferenceState};
use super::transitions::E2ETransition;
use super::types::*;
use crate::LoroCorruptionType;

use loro::{ExportMode, LoroDoc};

/// Simulate Loro CRDT merge for concurrent text updates.
///
/// Creates two Loro peers from a common ancestor, applies one update on each,
/// then merges them. Returns the CRDT-merged content.
fn loro_merge_text(original: &str, update_a: &str, update_b: &str) -> String {
    let ancestor = LoroDoc::new();
    ancestor.set_peer_id(0).unwrap();
    let text = ancestor.get_text("content");
    text.update(original, Default::default()).unwrap();
    ancestor.commit();
    let snapshot = ancestor.export(ExportMode::Snapshot).unwrap();

    let peer_a = LoroDoc::new();
    peer_a.set_peer_id(1).unwrap();
    peer_a.import(&snapshot).unwrap();
    peer_a
        .get_text("content")
        .update(update_a, Default::default())
        .unwrap();
    peer_a.commit();

    let peer_b = LoroDoc::new();
    peer_b.set_peer_id(2).unwrap();
    peer_b.import(&snapshot).unwrap();
    peer_b
        .get_text("content")
        .update(update_b, Default::default())
        .unwrap();
    peer_b.commit();

    let b_updates = peer_b.export(ExportMode::all_updates()).unwrap();
    peer_a.import(&b_updates).unwrap();

    peer_a.get_text("content").to_string()
}

#[derive(Debug, Clone)]
pub struct VariantRef<V: VariantMarker>(pub ReferenceState, PhantomData<V>);

impl<V: VariantMarker> std::ops::Deref for VariantRef<V> {
    type Target = ReferenceState;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<V: VariantMarker> std::ops::DerefMut for VariantRef<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<V: VariantMarker> ReferenceStateMachine for VariantRef<V> {
    type State = Self;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        prop_oneof![
            // ~50% with keyword set (exercises task_state mutations)
            1 => todo_keyword_set_strategy().prop_map(|ks| {
                let mut state = ReferenceState::with_variant(V::variant());
                state.keyword_set = Some(ks);
                VariantRef(state, PhantomData)
            }),
            // ~50% without (exercises no-keyword path)
            1 => Just(VariantRef(
                ReferenceState::with_variant(V::variant()),
                PhantomData,
            )),
        ]
        .boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        // Pre-startup phase: generate various pre-startup transitions
        if !state.app_started {
            let pre_startup_file_count = state.documents.len();
            let dir_count = state.pre_startup_directories.len();

            // Weight towards starting app after some setup
            let file_weight = if pre_startup_file_count < 3 { 3 } else { 1 };
            let dir_weight = if dir_count < 10 { 2 } else { 0 }; // Create up to ~10 directories
            let vcs_weight = if !state.git_initialized && !state.jj_initialized {
                1
            } else {
                0
            };

            // Build strategy list dynamically based on state
            let mut strategies: Vec<(u32, BoxedStrategy<E2ETransition>)> = vec![
                (
                    file_weight,
                    generate_org_file_content_with_keywords(state.keyword_set.clone())
                        .prop_map(|(filename, content)| E2ETransition::WriteOrgFile {
                            filename,
                            content,
                        })
                        .boxed(),
                ),
                (
                    2,
                    // Always wait for OrgSyncController to finish initial file processing.
                    // Without this, UI mutations race with OrgSyncController's initial processing,
                    // creating duplicate Loro documents for the same file path (one from
                    // the mutation, one from OrgSyncController), causing block deletion on sync.
                    Just(E2ETransition::StartApp {
                        wait_for_ready: true,
                        enable_todoist: true,
                        enable_loro: state.variant.enable_loro,
                    })
                    .boxed(),
                ),
            ];

            if dir_weight > 0 {
                strategies.push((
                    dir_weight,
                    generate_directory_path()
                        .prop_map(|path| E2ETransition::CreateDirectory { path })
                        .boxed(),
                ));
            }

            if vcs_weight > 0 && !state.git_initialized {
                strategies.push((vcs_weight, Just(E2ETransition::GitInit).boxed()));
            }

            if vcs_weight > 0 && !state.jj_initialized {
                strategies.push((vcs_weight, Just(E2ETransition::JjGitInit).boxed()));
            }

            // Add CreateStaleLoro transition if there are org files to corrupt (Loro only)
            let org_filenames: Vec<String> = state.documents.values().map(|f| f.clone()).collect();
            if state.variant.enable_loro && !org_filenames.is_empty() {
                strategies.push((
                    1, // Lower weight - only occasionally create stale loro files
                    (
                        prop::sample::select(org_filenames),
                        prop::sample::select(vec![
                            LoroCorruptionType::Empty,
                            LoroCorruptionType::Truncated,
                            LoroCorruptionType::InvalidHeader,
                        ]),
                    )
                        .prop_map(|(org_filename, corruption_type)| {
                            E2ETransition::CreateStaleLoro {
                                org_filename,
                                corruption_type,
                            }
                        })
                        .boxed(),
                ));
            }

            return prop::strategy::Union::new_weighted(strategies).boxed();
        }

        // Post-startup phase: generate normal transitions
        // Exclude seeded default layout blocks (doc:__default__) from mutation targets —
        // these blocks have no org file on disk so external mutations can't write to them.
        let block_ids: Vec<String> = state
            .blocks
            .keys()
            .filter(|id| {
                state
                    .block_documents
                    .get(id.as_str())
                    .map_or(true, |doc| doc != "doc:__default__")
            })
            .cloned()
            .collect();
        let text_block_ids: Vec<String> = state
            .blocks
            .iter()
            .filter(|(id, b)| {
                b.content_type == ContentType::Text
                    && state
                        .block_documents
                        .get(id.as_str())
                        .map_or(true, |doc| doc != "doc:__default__")
            })
            .map(|(id, _)| id.clone())
            .collect();
        let doc_uris: Vec<String> = state.documents.keys().cloned().collect();
        let next_id = state.next_id;
        let next_doc_id = state.next_doc_id;

        let mut strategies: Vec<BoxedStrategy<E2ETransition>> = Vec::new();

        strategies.push(
            Just(E2ETransition::CreateDocument {
                file_name: format!("doc_{}.org", next_doc_id),
            })
            .boxed(),
        );

        // Render + query source blocks have dedicated mutation strategies;
        // exclude them from the generic content-update path so random text
        // doesn't corrupt render DSL or break initial_widget().
        let no_content_update: HashSet<String> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        if !doc_uris.is_empty() {
            strategies.push(
                generate_mutation(
                    next_id,
                    block_ids.clone(),
                    text_block_ids.clone(),
                    doc_uris.clone(),
                    no_content_update.clone(),
                )
                .prop_map(|mutation| {
                    E2ETransition::ApplyMutation(MutationEvent {
                        source: MutationSource::UI,
                        mutation,
                    })
                })
                .boxed(),
            );

            strategies.push(
                generate_mutation(
                    next_id,
                    block_ids.clone(),
                    text_block_ids.clone(),
                    doc_uris.clone(),
                    no_content_update.clone(),
                )
                .prop_map(|mutation| {
                    E2ETransition::ApplyMutation(MutationEvent {
                        source: MutationSource::External,
                        mutation,
                    })
                })
                .boxed(),
            );
        }

        strategies.push(
            (
                generate_test_query(),
                generate_query_language(),
                "[a-z]{1,10}",
            )
                .prop_map(|(query, language, id)| E2ETransition::SetupWatch {
                    query_id: format!("query-{}", id),
                    query,
                    language,
                })
                .boxed(),
        );

        if !state.active_watches.is_empty() {
            let watch_ids: Vec<String> = state.active_watches.keys().cloned().collect();
            strategies.push(
                prop::sample::select(watch_ids)
                    .prop_map(|query_id| E2ETransition::RemoveWatch { query_id })
                    .boxed(),
            );
        }

        strategies.push(
            prop::sample::select(vec![
                "all".to_string(),
                "sidebar".to_string(),
                "main".to_string(),
            ])
            .prop_map(|view_name| E2ETransition::SwitchView { view_name })
            .boxed(),
        );

        let regions = Region::ALL.to_vec();

        if !block_ids.is_empty() {
            let block_ids_clone = block_ids.clone();
            strategies.push(
                (
                    prop::sample::select(regions.clone()),
                    prop::sample::select(block_ids_clone),
                )
                    .prop_map(|(region, block_id)| E2ETransition::NavigateFocus {
                        region,
                        block_id,
                    })
                    .boxed(),
            );
        }

        for region in &regions {
            if state.can_go_back(*region) {
                strategies.push(Just(E2ETransition::NavigateBack { region: *region }).boxed());
            }
        }

        for region in &regions {
            if state.can_go_forward(*region) {
                strategies.push(Just(E2ETransition::NavigateForward { region: *region }).boxed());
            }
        }

        strategies.push(
            prop::sample::select(regions)
                .prop_map(|region| E2ETransition::NavigateHome { region })
                .boxed(),
        );

        // Add layout headline mutations (content, task_state, priority, tags)
        {
            let headline_ids: Vec<String> =
                state.layout_blocks.headline_ids.iter().cloned().collect();
            if !headline_ids.is_empty() {
                strategies.push(
                    generate_layout_headline_mutation(headline_ids, state.keyword_set.clone())
                        .prop_map(|mutation| {
                            E2ETransition::ApplyMutation(MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            })
                        })
                        .boxed(),
                );
            }
        }

        // Add render source mutations (change render DSL expression)
        {
            let render_ids: Vec<String> = state
                .layout_blocks
                .render_source_ids
                .iter()
                .cloned()
                .collect();
            if !render_ids.is_empty() {
                strategies.push(
                    generate_render_source_mutation(render_ids)
                        .prop_map(|mutation| {
                            E2ETransition::ApplyMutation(MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            })
                        })
                        .boxed(),
                );
            }
        }

        // Add profile content mutations (change entity profile YAML)
        {
            let profile_ids: Vec<String> = state.profile_block_ids.iter().cloned().collect();
            if !profile_ids.is_empty() {
                strategies.push(
                    generate_profile_content_mutation(profile_ids)
                        .prop_map(|mutation| {
                            E2ETransition::ApplyMutation(MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            })
                        })
                        .boxed(),
                );
            }
        }

        // Add SimulateRestart - only useful if there are blocks to test with
        if !block_ids.is_empty() {
            strategies.push(Just(E2ETransition::SimulateRestart).boxed());
        }

        // Add BulkExternalAdd - tests sync loop by adding multiple blocks via external file
        // Only if there's at least one document to add blocks to
        if !doc_uris.is_empty() {
            let doc_uris_clone = doc_uris.clone();
            strategies.push(
                (
                    prop::sample::select(doc_uris_clone),
                    prop::collection::vec(
                        "[a-zA-Z][a-zA-Z0-9 ]{0,20}",
                        3..=10, // Add 3-10 blocks at once
                    ),
                )
                    .prop_map(move |(doc_uri, contents)| {
                        let doc_entity_uri = EntityUri::from_raw(&doc_uri);
                        let blocks: Vec<Block> = contents
                            .into_iter()
                            .enumerate()
                            .map(|(i, content)| {
                                Block::new_text(
                                    EntityUri::from_raw(&format!("block:bulk-{}-{}", next_id, i)),
                                    doc_entity_uri.clone(),
                                    doc_entity_uri.clone(),
                                    content,
                                )
                            })
                            .collect();
                        E2ETransition::BulkExternalAdd { doc_uri, blocks }
                    })
                    .boxed(),
            );
        }

        // Add ConcurrentSchemaInit - tests database lock bug from concurrent schema initialization
        // Only useful if there are blocks and active watches (IVM operations running)
        if !block_ids.is_empty() && !state.active_watches.is_empty() {
            strategies.push(Just(E2ETransition::ConcurrentSchemaInit).boxed());
        }

        // DISABLED: ConcurrentMutations — reference model assumes External always wins (LWW),
        // but actual CRDT resolution is timing-dependent. The FIXME in apply_concurrent_mutations
        // notes that External is applied from post-both-mutations state, not pre-merge state.
        // Re-enable once the reference model correctly simulates concurrent resolution.
        if false && !doc_uris.is_empty() {
            // Independent mutations (may target different blocks)
            let ui_next_id = next_id;
            let ext_next_id = next_id + 1;
            strategies.push(
                (
                    generate_mutation(
                        ui_next_id,
                        block_ids.clone(),
                        text_block_ids.clone(),
                        doc_uris.clone(),
                        no_content_update.clone(),
                    ),
                    generate_mutation(
                        ext_next_id,
                        block_ids.clone(),
                        text_block_ids.clone(),
                        doc_uris.clone(),
                        no_content_update.clone(),
                    ),
                )
                    .prop_map(|(ui_mut, ext_mut)| E2ETransition::ConcurrentMutations {
                        ui_mutation: MutationEvent {
                            source: MutationSource::UI,
                            mutation: ui_mut,
                        },
                        external_mutation: MutationEvent {
                            source: MutationSource::External,
                            mutation: ext_mut,
                        },
                    })
                    .boxed(),
            );

            // Same-block concurrent partial edits — the highest-conflict CRDT scenario.
            // UI appends a suffix, External prepends a prefix to the same block.
            // After CRDT merge, the result should contain both:
            //   "{prefix} {original} {suffix}"
            // This makes the merge clearly observable in the test output.
            if !block_ids.is_empty() {
                let blocks_snapshot: Vec<(String, String)> = block_ids
                    .iter()
                    .filter_map(|id| {
                        let block = state.blocks.get(id)?;
                        // Only text blocks — source blocks have different content semantics
                        if block.content_type == ContentType::Text {
                            Some((id.clone(), block.content.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                if !blocks_snapshot.is_empty() {
                    strategies.push(
                        (
                            prop::sample::select(blocks_snapshot),
                            "[A-Z][a-z]{2,8}", // suffix (UI appends)
                            "[A-Z][a-z]{2,8}", // prefix (External prepends)
                        )
                            .prop_map(|((id, original), suffix, prefix)| {
                                let ui_content = format!("{} {}", original, suffix);
                                let ext_content = format!("{} {}", prefix, original);
                                E2ETransition::ConcurrentMutations {
                                    ui_mutation: MutationEvent {
                                        source: MutationSource::UI,
                                        mutation: Mutation::Update {
                                            entity: "block".to_string(),
                                            id: id.clone(),
                                            fields: [(
                                                "content".to_string(),
                                                Value::String(ui_content),
                                            )]
                                            .into_iter()
                                            .collect(),
                                        },
                                    },
                                    external_mutation: MutationEvent {
                                        source: MutationSource::External,
                                        mutation: Mutation::Update {
                                            entity: "block".to_string(),
                                            id,
                                            fields: [(
                                                "content".to_string(),
                                                Value::String(ext_content),
                                            )]
                                            .into_iter()
                                            .collect(),
                                        },
                                    },
                                }
                            })
                            .boxed(),
                    );
                }
            }
        }

        prop::strategy::Union::new(strategies).boxed()
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // WriteOrgFile: always valid (can write files before or after startup)
        // StartApp: only valid when app is not started
        // All other transitions: only valid after startup
        match transition {
            // Pre-startup transitions
            E2ETransition::WriteOrgFile { filename, content } => {
                if state.app_started {
                    return false;
                }
                // Reject if any block IDs in this file already exist under a different document.
                // Org :ID: properties must be globally unique — the system asserts on duplicates.
                let doc_uri = EntityUri::doc(filename).to_string();
                let id_re = regex::Regex::new(r":ID:\s*(\S+)").unwrap();
                for caps in id_re.captures_iter(content) {
                    let block_id = caps.get(1).unwrap().as_str();
                    if let Some(existing_doc) = state.block_documents.get(block_id) {
                        if *existing_doc != doc_uri {
                            return false;
                        }
                    }
                }
                true
            }
            E2ETransition::CreateDirectory { .. } => !state.app_started, // Only before startup
            E2ETransition::GitInit => !state.app_started && !state.git_initialized, // Once only
            E2ETransition::JjGitInit => !state.app_started && !state.jj_initialized, // Once only
            E2ETransition::CreateStaleLoro { org_filename, .. } => {
                // Only valid before startup, and org file must exist
                !state.app_started && state.documents.values().any(|f| f == org_filename)
            }
            E2ETransition::StartApp { .. } => {
                // Require at least one org file before startup. The system always loads
                // assets/default/index.org, but the reference model only tracks blocks
                // from explicitly written files.
                !state.app_started && state.pre_startup_file_count > 0
            }

            // Post-startup transitions (require app to be running)
            E2ETransition::CreateDocument { .. } => state.app_started,
            E2ETransition::ApplyMutation(event) => {
                if !state.app_started {
                    return false;
                }
                match &event.mutation {
                    Mutation::Delete { id, .. } => {
                        state.blocks.contains_key(id) && !state.layout_blocks.contains(id)
                    }
                    Mutation::Update { id, .. } => {
                        state.blocks.contains_key(id) && !state.layout_blocks.is_immutable(id)
                    }
                    Mutation::Move {
                        id, new_parent_id, ..
                    } => {
                        state.blocks.contains_key(id)
                            // Don't move source blocks — Org format determines their parent
                            // by heading position, so moves can't round-trip correctly.
                            && state
                                .blocks
                                .get(id)
                                .map_or(false, |b| b.content_type != ContentType::Source)
                            && state
                                .blocks
                                .get(new_parent_id)
                                .map_or(state.documents.contains_key(new_parent_id), |b| {
                                    b.content_type != ContentType::Source
                                })
                    }
                    Mutation::Create { parent_id, .. } => {
                        state.documents.contains_key(parent_id)
                            || state.blocks.get(parent_id).map_or(false, |b| {
                                // Don't create children under source blocks — Org format
                                // can't represent children inside #+begin_src blocks, so
                                // the Org round-trip would flatten the hierarchy.
                                b.content_type != ContentType::Source
                            })
                    }
                    Mutation::RestartApp => true,
                }
            }
            E2ETransition::SetupWatch { .. } => state.app_started,
            E2ETransition::RemoveWatch { query_id } => {
                state.app_started && state.active_watches.contains_key(query_id)
            }
            E2ETransition::SwitchView { .. } => state.app_started,
            E2ETransition::NavigateFocus { block_id, .. } => {
                state.app_started && state.blocks.contains_key(block_id)
            }
            E2ETransition::NavigateBack { region } => {
                state.app_started && state.can_go_back(*region)
            }
            E2ETransition::NavigateForward { region } => {
                state.app_started && state.can_go_forward(*region)
            }
            E2ETransition::NavigateHome { .. } => state.app_started,
            E2ETransition::SimulateRestart => state.app_started && !state.blocks.is_empty(),
            E2ETransition::BulkExternalAdd { doc_uri, .. } => {
                state.app_started && state.documents.contains_key(doc_uri)
            }
            E2ETransition::ConcurrentSchemaInit => {
                state.app_started && !state.blocks.is_empty() && !state.active_watches.is_empty()
            }
            E2ETransition::ConcurrentMutations {
                ui_mutation,
                external_mutation,
            } => {
                if !state.app_started {
                    return false;
                }
                // Both sub-mutations must pass preconditions individually
                let ui_ok =
                    Self::preconditions(state, &E2ETransition::ApplyMutation(ui_mutation.clone()));
                let ext_ok = Self::preconditions(
                    state,
                    &E2ETransition::ApplyMutation(external_mutation.clone()),
                );
                // Reject if both are creates with the same ID (impossible in practice)
                let same_create_id = matches!(
                    (&ui_mutation.mutation, &external_mutation.mutation),
                    (Mutation::Create { id: id1, .. }, Mutation::Create { id: id2, .. }) if id1 == id2
                );
                ui_ok && ext_ok && !same_create_id
            }
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            // Pre-startup transitions
            E2ETransition::WriteOrgFile { filename, content } => {
                use regex::Regex;

                let doc_uri = EntityUri::doc(filename).to_string();
                state.documents.insert(doc_uri.clone(), filename.clone());

                // Remove old blocks from this document (handles re-writing the same file)
                let old_block_ids: Vec<String> = state
                    .block_documents
                    .iter()
                    .filter(|(_, uri)| *uri == &doc_uri)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &old_block_ids {
                    state.blocks.remove(id);
                    state.block_documents.remove(id);
                    state.layout_blocks.remove(id);
                }

                // Parse block IDs from content and add to reference state
                // This tracks what blocks will exist after the app starts and syncs the file
                let id_regex = Regex::new(r":ID:\s*(\S+)").unwrap();
                let headline_regex = Regex::new(r"^\*+\s+(.+)$").unwrap();
                let src_begin_regex = Regex::new(r"(?i)#\+begin_src\s+(\w+)(?:\s.*)?$").unwrap();
                let src_id_regex = Regex::new(r":id\s+(\S+)").unwrap();
                let src_end_regex = Regex::new(r"(?i)#\+end_src").unwrap();

                let mut current_headline: Option<String> = None;
                let mut current_block_id: Option<String> = None;
                let mut in_source_block = false;
                let mut source_language: Option<String> = None;
                let mut source_content = String::new();
                let mut source_block_id: Option<String> = None;
                let mut source_block_index = 0;
                // Global sequence counter matching production parser (single counter
                // for all blocks in the file, regardless of parent depth)
                let mut sequence_counter: i64 = 0;

                for line in content.lines() {
                    if let Some(caps) = headline_regex.captures(line) {
                        current_headline = Some(caps.get(1).unwrap().as_str().trim().to_string());
                        source_block_index = 0; // Reset source block counter for each headline
                    } else if let Some(caps) = id_regex.captures(line) {
                        let block_id = caps.get(1).unwrap().as_str().to_string();
                        let content = current_headline.clone().unwrap_or_default();
                        let block_uri = EntityUri::block(&block_id);
                        let block_key = block_uri.to_string();
                        let doc_entity_uri = EntityUri::doc(filename);
                        let mut block = Block::new_text(
                            block_uri,
                            doc_entity_uri.clone(),
                            doc_entity_uri,
                            content,
                        );
                        block.set_sequence(sequence_counter);
                        sequence_counter += 1;
                        state.blocks.insert(block_key.clone(), block);
                        state
                            .block_documents
                            .insert(block_key.clone(), doc_uri.clone());
                        current_block_id = Some(block_key);
                    } else if let Some(caps) = src_begin_regex.captures(line) {
                        in_source_block = true;
                        source_language = Some(caps.get(1).unwrap().as_str().to_string());
                        source_content.clear();
                        // Extract :id from header args if present
                        source_block_id = src_id_regex
                            .captures(line)
                            .map(|c| c.get(1).unwrap().as_str().to_string());
                    } else if src_end_regex.is_match(line) && in_source_block {
                        // Create source block as child of current headline block
                        if let Some(parent_key) = &current_block_id {
                            let parent_block = &state.blocks[parent_key];
                            let parent_uri = parent_block.id.clone();
                            let parent_doc_id = parent_block.document_id.clone();
                            let src_id = source_block_id.take().unwrap_or_else(|| {
                                format!("{}::src::{}", parent_uri.id(), source_block_index)
                            });
                            let src_uri = EntityUri::block(&src_id);
                            let src_key = src_uri.to_string();
                            let mut src_block = Block {
                                id: src_uri,
                                parent_id: parent_uri,
                                document_id: parent_doc_id,
                                content: source_content.trim().to_string(),
                                content_type: ContentType::Source,
                                source_language: source_language
                                    .as_ref()
                                    .map(|s| s.parse::<SourceLanguage>().unwrap()),
                                source_name: None,
                                properties: HashMap::new(),
                                created_at: 0,
                                updated_at: 0,
                            };
                            // Classify layout blocks in index.org by source language
                            if filename == "index.org" {
                                if let Some(sl) = src_block.source_language.as_ref() {
                                    if sl.as_query().is_some() {
                                        state.layout_blocks.headline_ids.insert(parent_key.clone());
                                        state
                                            .layout_blocks
                                            .query_source_ids
                                            .insert(src_key.clone());
                                    } else if matches!(sl, SourceLanguage::Render) {
                                        state.layout_blocks.headline_ids.insert(parent_key.clone());
                                        state
                                            .layout_blocks
                                            .render_source_ids
                                            .insert(src_key.clone());
                                    }
                                }
                            }
                            src_block.set_sequence(sequence_counter);
                            sequence_counter += 1;
                            state.blocks.insert(src_key.clone(), src_block);
                            state.block_documents.insert(src_key, doc_uri.clone());
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
                // Re-assign sequences using canonical ordering (source blocks
                // first, then by sequence, then by ID). This matches what OrgRenderer
                // will produce after the production system round-trips the file.
                let mut all_blocks: Vec<Block> = state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.blocks = all_blocks
                    .into_iter()
                    .map(|b| (b.id.to_string(), b))
                    .collect();

                state.rebuild_profile_tracking();
                state.pre_startup_file_count += 1;
            }
            E2ETransition::CreateDirectory { path } => {
                state.pre_startup_directories.push(path.clone());
            }
            E2ETransition::GitInit => {
                state.git_initialized = true;
            }
            E2ETransition::JjGitInit => {
                state.jj_initialized = true;
                state.git_initialized = true; // jj git init also creates .git
            }
            E2ETransition::CreateStaleLoro { .. } => {
                // CreateStaleLoro doesn't change reference state - the blocks from the
                // corresponding org file should still exist after startup. The system
                // should detect the corrupted .loro file and recover from the .org file.
            }
            E2ETransition::StartApp { .. } => {
                state.app_started = true;

                // The production system seeds default layout blocks from
                // assets/default/index.org under doc:__default__ when no
                // ROOT_LAYOUT_BLOCK_ID exists. Track them in the reference model.
                let default_doc_uri = "doc:__default__";
                let default_content = include_str!("../../../../assets/default/index.org");
                let parse_result = holon_orgmode::parse_org_file(
                    std::path::Path::new("index.org"),
                    default_content,
                    default_doc_uri,
                    0,
                    std::path::Path::new(""),
                )
                .expect("default index.org must parse");

                // The production code rewrites top-level parent_ids from
                // doc:index.org to doc:__default__
                let file_doc_uri = parse_result.document.id.clone();
                for block in parse_result.blocks {
                    let block_key = block.id.to_string();
                    let parent_id = if block.parent_id.as_str() == file_doc_uri {
                        EntityUri::from_raw(default_doc_uri)
                    } else {
                        block.parent_id.clone()
                    };
                    let mut b = block;
                    b.parent_id = parent_id;
                    b.document_id = EntityUri::from_raw(default_doc_uri);
                    state
                        .block_documents
                        .insert(block_key.clone(), default_doc_uri.to_string());
                    state.blocks.insert(block_key, b);
                }
            }

            // Post-startup transitions
            E2ETransition::CreateDocument { file_name } => {
                let doc_uri = EntityUri::doc(file_name).to_string();
                state.documents.insert(doc_uri, file_name.clone());
                state.next_doc_id += 1;
            }
            E2ETransition::ApplyMutation(event) => {
                if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                    let doc_uri = if EntityUri::parse(parent_id).is_ok_and(|u| u.is_doc()) {
                        parent_id.clone()
                    } else {
                        state
                            .blocks
                            .get(parent_id)
                            .map(|b| b.document_id.as_str().to_string())
                            .unwrap_or_else(|| parent_id.clone())
                    };
                    state.block_documents.insert(id.clone(), doc_uri);
                }

                let mut blocks: Vec<Block> = state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                // Both UI and External mutations trigger org sync re-render
                // (on_block_changed), which re-writes the org file in canonical
                // order (source blocks first). Re-assign sequences to match.
                assign_reference_sequences_canonical(&mut blocks);
                state.blocks = blocks.into_iter().map(|b| (b.id.to_string(), b)).collect();
                state.rebuild_profile_tracking();
                state.next_id += 1;
            }
            E2ETransition::SetupWatch {
                query_id,
                query,
                language,
            } => {
                state.active_watches.insert(
                    query_id.clone(),
                    WatchSpec {
                        query: query.clone(),
                        language: *language,
                    },
                );
            }
            E2ETransition::RemoveWatch { query_id } => {
                state.active_watches.remove(query_id);
            }
            E2ETransition::SwitchView { view_name } => {
                state.current_view = view_name.clone();
            }
            E2ETransition::NavigateFocus { region, block_id } => {
                let history = state
                    .navigation_history
                    .entry(*region)
                    .or_insert_with(NavigationHistory::new);

                history.entries.truncate(history.cursor + 1);
                history.entries.push(Some(block_id.clone()));
                history.cursor = history.entries.len() - 1;
            }
            E2ETransition::NavigateBack { region } => {
                if let Some(history) = state.navigation_history.get_mut(region) {
                    if history.cursor > 0 {
                        history.cursor -= 1;
                    }
                }
            }
            E2ETransition::NavigateForward { region } => {
                if let Some(history) = state.navigation_history.get_mut(region) {
                    if history.cursor < history.entries.len() - 1 {
                        history.cursor += 1;
                    }
                }
            }
            E2ETransition::NavigateHome { region } => {
                let history = state
                    .navigation_history
                    .entry(*region)
                    .or_insert_with(NavigationHistory::new);

                history.entries.truncate(history.cursor + 1);
                history.entries.push(None);
                history.cursor = history.entries.len() - 1;
            }
            E2ETransition::SimulateRestart => {
                // SimulateRestart doesn't change reference state - blocks should be preserved.
                // The SUT will clear last_projection and trigger file re-processing.
            }
            E2ETransition::BulkExternalAdd { blocks, .. } => {
                // Add all blocks to the reference state
                for block in blocks {
                    state.blocks.insert(block.id.to_string(), block.clone());
                }
                // BulkExternalAdd serializes via serialize_blocks_to_org (canonical order)
                let mut all_blocks: Vec<Block> = state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.blocks = all_blocks
                    .into_iter()
                    .map(|b| (b.id.to_string(), b))
                    .collect();
                state.rebuild_profile_tracking();
                state.next_id += blocks.len();
            }
            E2ETransition::ConcurrentSchemaInit => {
                // ConcurrentSchemaInit doesn't change reference state - it only tests
                // that the database doesn't get locked when schema init runs concurrently.
            }
            E2ETransition::ConcurrentMutations {
                ui_mutation,
                external_mutation,
            } => {
                // Detect same-block concurrent content updates — use Loro CRDT merge
                // to determine the expected merged content rather than naive LWW.
                let same_block_update = match (&ui_mutation.mutation, &external_mutation.mutation) {
                    (
                        Mutation::Update {
                            id: ui_id,
                            fields: ui_fields,
                            ..
                        },
                        Mutation::Update {
                            id: ext_id,
                            fields: ext_fields,
                            ..
                        },
                    ) if ui_id == ext_id => {
                        let ui_content = ui_fields.get("content").and_then(|v| v.as_string());
                        let ext_content = ext_fields.get("content").and_then(|v| v.as_string());
                        match (ui_content, ext_content) {
                            (Some(ui_c), Some(ext_c)) => {
                                Some((ui_id.clone(), ui_c.to_string(), ext_c.to_string()))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };

                if let Some((block_id, ui_content, ext_content)) = same_block_update {
                    if state.variant.enable_loro {
                        // Loro CRDT merge determines expected content
                        let original = state
                            .blocks
                            .get(&block_id)
                            .map(|b| b.content.as_str())
                            .unwrap_or("");
                        let merged = loro_merge_text(original, &ui_content, &ext_content);
                        if let Some(block) = state.blocks.get_mut(&block_id) {
                            block.content = merged;
                        }
                    } else {
                        // Without Loro, external write (org file) wins via LWW
                        if let Some(block) = state.blocks.get_mut(&block_id) {
                            block.content = ext_content;
                        }
                    }
                } else {
                    // Non-overlapping mutations: apply both sequentially
                    for event in [ui_mutation, external_mutation] {
                        if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                            let doc_uri = if EntityUri::parse(parent_id).is_ok_and(|u| u.is_doc()) {
                                parent_id.clone()
                            } else {
                                fn find_doc(
                                    block_id: &str,
                                    state: &ReferenceState,
                                ) -> Option<String> {
                                    let block = state.blocks.get(block_id)?;
                                    if block.parent_id.is_doc() {
                                        Some(block.parent_id.as_raw_str().to_string())
                                    } else {
                                        find_doc(block.parent_id.as_raw_str(), state)
                                    }
                                }
                                find_doc(parent_id, &state).unwrap_or_else(|| parent_id.clone())
                            };
                            state.block_documents.insert(id.clone(), doc_uri);
                        }

                        let mut blocks: Vec<Block> = state.blocks.values().cloned().collect();
                        event.mutation.apply_to(&mut blocks);
                        state.blocks = blocks.into_iter().map(|b| (b.id.to_string(), b)).collect();
                    }
                }
                // External mutation re-writes org file in canonical order
                let mut all_blocks: Vec<Block> = state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.blocks = all_blocks
                    .into_iter()
                    .map(|b| (b.id.to_string(), b))
                    .collect();
                state.rebuild_profile_tracking();
                // UI used next_id, External used next_id+1
                state.next_id += 2;
            }
        }
        state
    }
}
