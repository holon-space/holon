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

/// Builder for weighted proptest strategies with env-var overrides.
///
/// Each strategy gets a label. The default weight is 1. At build time,
/// `PBT_WEIGHT_<LABEL>` env vars override weights (0 disables the strategy).
/// This lets you focus a test run on specific transitions:
///
/// ```sh
/// # Only generate ToggleState and render source mutations:
/// PBT_WEIGHT_TOGGLE_STATE=10 PBT_WEIGHT_RENDER_SOURCE_MUTATION=10 \
///   PBT_WEIGHT_DEFAULT=0 cargo test general_e2e_pbt
///
/// # Boost ToggleState without disabling others:
/// PBT_WEIGHT_TOGGLE_STATE=20 cargo test general_e2e_pbt
/// ```
///
/// `PBT_WEIGHT_DEFAULT` overrides the baseline for all strategies that don't
/// have a specific `PBT_WEIGHT_<LABEL>` set. Defaults to 1 if unset.
struct WeightedStrategies<T: std::fmt::Debug> {
    entries: Vec<(String, u32, BoxedStrategy<T>)>,
}

impl<T: std::fmt::Debug + 'static> WeightedStrategies<T> {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a strategy with default weight (1, or PBT_WEIGHT_DEFAULT).
    fn add(&mut self, label: &str, strategy: BoxedStrategy<T>) {
        self.entries.push((label.to_string(), 1, strategy));
    }

    /// Add a strategy with a specific default weight.
    fn add_weighted(&mut self, label: &str, weight: u32, strategy: BoxedStrategy<T>) {
        self.entries.push((label.to_string(), weight, strategy));
    }

    /// Resolve env-var overrides and build a weighted union.
    fn build(self) -> BoxedStrategy<T> {
        let default_weight: Option<u32> = std::env::var("PBT_WEIGHT_DEFAULT")
            .ok()
            .and_then(|v| v.parse().ok());

        let weighted: Vec<(u32, BoxedStrategy<T>)> = self
            .entries
            .into_iter()
            .filter_map(|(label, base_weight, strategy)| {
                let env_key = format!("PBT_WEIGHT_{}", label.to_uppercase());
                let weight = std::env::var(&env_key)
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| default_weight.unwrap_or(base_weight));
                if weight == 0 {
                    None
                } else {
                    Some((weight, strategy))
                }
            })
            .collect();

        assert!(
            !weighted.is_empty(),
            "All PBT strategies have weight 0 — check PBT_WEIGHT_* env vars"
        );
        prop::strategy::Union::new_weighted(weighted).boxed()
    }
}

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
        let default_doc = EntityUri::doc("__default__");
        let block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .filter(|(_, b)| {
                state
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .map_or(true, |doc| *doc != default_doc)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let text_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .filter(|(_, b)| {
                b.content_type == ContentType::Text
                    && state
                        .block_state
                        .block_documents
                        .get(&b.id)
                        .map_or(true, |doc| *doc != default_doc)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let doc_uris: Vec<EntityUri> = state.documents.keys().map(|u| u.clone()).collect();
        let next_id = state.block_state.next_id;
        let next_doc_id = state.next_doc_id;

        let mut strategies = WeightedStrategies::new();

        strategies.add(
            "create_document",
            Just(E2ETransition::CreateDocument {
                file_name: format!("doc_{}.org", next_doc_id),
            })
            .boxed(),
        );

        // Render + query source blocks have dedicated mutation strategies;
        // exclude them from the generic content-update path so random text
        // doesn't corrupt render DSL or break initial_widget().
        let no_content_update: HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        if !doc_uris.is_empty() {
            strategies.add(
                "ui_mutation",
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

            strategies.add(
                "external_mutation",
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

        strategies.add(
            "setup_watch",
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
            strategies.add(
                "remove_watch",
                prop::sample::select(watch_ids)
                    .prop_map(|query_id| E2ETransition::RemoveWatch { query_id })
                    .boxed(),
            );
        }

        strategies.add(
            "switch_view",
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
            strategies.add(
                "navigate_focus",
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
                strategies.add(
                    "navigate_back",
                    Just(E2ETransition::NavigateBack { region: *region }).boxed(),
                );
            }
        }

        for region in &regions {
            if state.can_go_forward(*region) {
                strategies.add(
                    "navigate_forward",
                    Just(E2ETransition::NavigateForward { region: *region }).boxed(),
                );
            }
        }

        strategies.add(
            "navigate_home",
            prop::sample::select(regions)
                .prop_map(|region| E2ETransition::NavigateHome { region })
                .boxed(),
        );

        // Layout headline mutations (content, task_state, priority, tags)
        {
            let headline_ids: Vec<EntityUri> =
                state.layout_blocks.headline_ids.iter().cloned().collect();
            if !headline_ids.is_empty() {
                strategies.add(
                    "layout_headline_mutation",
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

        // Render source mutations (change render DSL expression)
        {
            let render_ids: Vec<EntityUri> = state
                .layout_blocks
                .render_source_ids
                .iter()
                .cloned()
                .collect();
            if !render_ids.is_empty() {
                strategies.add(
                    "render_source_mutation",
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

        // Profile content mutations (change entity profile YAML)
        {
            let profile_ids: Vec<EntityUri> = state.profile_block_ids.iter().cloned().collect();
            if !profile_ids.is_empty() {
                strategies.add(
                    "profile_mutation",
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

        if !block_ids.is_empty() {
            strategies.add(
                "simulate_restart",
                Just(E2ETransition::SimulateRestart).boxed(),
            );
        }

        // BulkExternalAdd - tests sync loop by adding multiple blocks via external file
        if !doc_uris.is_empty() {
            let doc_uris_clone = doc_uris.clone();
            strategies.add(
                "bulk_external_add",
                (
                    prop::sample::select(doc_uris_clone),
                    prop::collection::vec("[a-zA-Z][a-zA-Z0-9 ]{0,20}", 3..=10),
                )
                    .prop_map(move |(doc_entity_uri, contents)| {
                        let blocks: Vec<Block> = contents
                            .into_iter()
                            .enumerate()
                            .map(|(i, content)| {
                                Block::new_text(
                                    EntityUri::block(&format!("bulk-{}-{}", next_id, i)),
                                    doc_entity_uri.clone(),
                                    doc_entity_uri.clone(),
                                    content,
                                )
                            })
                            .collect();
                        E2ETransition::BulkExternalAdd {
                            doc_uri: doc_entity_uri,
                            blocks,
                        }
                    })
                    .boxed(),
            );
        }

        // ConcurrentSchemaInit - tests database lock bug from concurrent schema initialization
        if !block_ids.is_empty() && !state.active_watches.is_empty() {
            strategies.add(
                "concurrent_schema_init",
                Just(E2ETransition::ConcurrentSchemaInit).boxed(),
            );
        }

        // Edit via display tree — exercises the GPUI blur handler path
        let editable_block_ids: Vec<EntityUri> = text_block_ids
            .iter()
            .filter(|id| !no_content_update.contains(id))
            .cloned()
            .collect();
        if !editable_block_ids.is_empty() {
            let ids = editable_block_ids.clone();
            strategies.add(
                "edit_via_display_tree",
                (prop::sample::select(ids), "[a-z ]{3,20}")
                    .prop_map(
                        |(block_id, new_content)| E2ETransition::EditViaDisplayTree {
                            block_id,
                            new_content,
                        },
                    )
                    .boxed(),
            );
        }

        if !editable_block_ids.is_empty() {
            let ids = editable_block_ids.clone();
            strategies.add(
                "edit_via_view_model",
                (prop::sample::select(ids), "[a-z ]{3,20}")
                    .prop_map(|(block_id, new_content)| E2ETransition::EditViaViewModel {
                        block_id,
                        new_content,
                    })
                    .boxed(),
            );
        }

        // ToggleState: set task_state via the StateToggle widget click path.
        // Only generated when the active render expression includes state_toggle.
        {
            let has_state_toggle = state
                .root_render_expr()
                .map(|expr| expr.to_rhai().contains("state_toggle"))
                .unwrap_or(false);
            if has_state_toggle && !editable_block_ids.is_empty() {
                let ids = editable_block_ids.clone();
                // Use the document's keyword set so states round-trip through org files.
                // Unknown keywords won't be recognized by the parser on re-read.
                let mut valid_states: Vec<String> = vec![String::new()]; // "" = no state
                if let Some(ref ks) = state.keyword_set {
                    valid_states.extend(ks.all_keywords());
                } else {
                    valid_states.extend(["TODO", "DOING", "DONE"].iter().map(|s| s.to_string()));
                }
                strategies.add(
                    "toggle_state",
                    (
                        prop::sample::select(ids),
                        prop::sample::select(valid_states),
                    )
                        .prop_map(|(block_id, new_state)| E2ETransition::ToggleState {
                            block_id,
                            new_state,
                        })
                        .boxed(),
                );
            }
        }

        // TriggerSlashCommand: pick a deletable text block and trigger the
        // slash command menu → select "delete" → execute flow.
        let deletable_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == ContentType::Text
                    && !state.layout_blocks.contains(&b.id)
                    && !b.id.as_str().contains("default-")
                    && state.block_state.blocks.len() > 2
            })
            .map(|b| b.id.clone())
            .collect();
        if !deletable_block_ids.is_empty() {
            strategies.add(
                "trigger_slash_command",
                prop::sample::select(deletable_block_ids)
                    .prop_map(|block_id| E2ETransition::TriggerSlashCommand { block_id })
                    .boxed(),
            );
        }

        // DISABLED: ConcurrentMutations — reference model assumes External always wins (LWW),
        // but actual CRDT resolution is timing-dependent.
        if false && !doc_uris.is_empty() {
            let ui_next_id = next_id;
            let ext_next_id = next_id + 1;
            strategies.add(
                "concurrent_mutations",
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

            if !block_ids.is_empty() {
                let blocks_snapshot: Vec<(EntityUri, String)> = block_ids
                    .iter()
                    .filter_map(|id| {
                        let block = state.block_state.blocks.get(id)?;
                        if block.content_type == ContentType::Text {
                            Some((id.clone(), block.content.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                if !blocks_snapshot.is_empty() {
                    strategies.add(
                        "concurrent_same_block",
                        (
                            prop::sample::select(blocks_snapshot),
                            "[A-Z][a-z]{2,8}",
                            "[A-Z][a-z]{2,8}",
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

        if !state.undo_stack.is_empty() {
            strategies.add_weighted("undo", 2, Just(E2ETransition::UndoLastMutation).boxed());
        }
        if !state.redo_stack.is_empty() {
            strategies.add_weighted("redo", 2, Just(E2ETransition::Redo).boxed());
        }

        strategies.build()
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
                let doc_uri = EntityUri::file(filename);
                let id_re = regex::Regex::new(r":ID:\s*(\S+)").unwrap();
                for caps in id_re.captures_iter(content) {
                    let block_id = caps.get(1).unwrap().as_str();
                    let block_entity = EntityUri::block(block_id);
                    if let Some(existing_doc) = state.block_state.block_documents.get(&block_entity)
                    {
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
                        state.block_state.blocks.contains_key(id)
                            && !state.layout_blocks.contains(id)
                    }
                    Mutation::Update { id, .. } => {
                        state.block_state.blocks.contains_key(id)
                            && !state.layout_blocks.is_immutable(id)
                    }
                    Mutation::Move {
                        id, new_parent_id, ..
                    } => {
                        state.block_state.blocks.contains_key(id)
                            // Don't move source blocks — Org format determines their parent
                            // by heading position, so moves can't round-trip correctly.
                            && state
                                .block_state.blocks
                                .get(id)
                                .map_or(false, |b| b.content_type != ContentType::Source)
                            && state
                                .block_state.blocks
                                .get(new_parent_id)
                                .map_or(state.documents.contains_key(new_parent_id), |b| {
                                    b.content_type != ContentType::Source
                                })
                    }
                    Mutation::Create { parent_id, .. } => {
                        state.documents.contains_key(parent_id)
                            || state.block_state.blocks.get(parent_id).map_or(false, |b| {
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
                state.app_started && state.block_state.blocks.contains_key(block_id)
            }
            E2ETransition::NavigateBack { region } => {
                state.app_started && state.can_go_back(*region)
            }
            E2ETransition::NavigateForward { region } => {
                state.app_started && state.can_go_forward(*region)
            }
            E2ETransition::NavigateHome { .. } => state.app_started,
            E2ETransition::SimulateRestart => {
                state.app_started && !state.block_state.blocks.is_empty()
            }
            E2ETransition::BulkExternalAdd { doc_uri, .. } => {
                state.app_started && state.documents.contains_key(doc_uri)
            }
            E2ETransition::ConcurrentSchemaInit => {
                state.app_started
                    && !state.block_state.blocks.is_empty()
                    && !state.active_watches.is_empty()
            }
            E2ETransition::EditViaDisplayTree { block_id, .. } => {
                state.app_started
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .map_or(false, |b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    // Custom entity profiles override the render expression to
                    // row(col("content")) which has no EditableText node.
                    && !state.has_blocks_profile()
            }
            E2ETransition::EditViaViewModel { block_id, .. } => {
                state.app_started
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .map_or(false, |b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    // Custom entity profiles override the render expression to
                    // row(col("content")) which has no EditableText node.
                    && !state.has_blocks_profile()
            }
            E2ETransition::ToggleState { block_id, .. } => {
                state.app_started
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .map_or(false, |b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state
                        .root_render_expr()
                        .map(|expr| expr.to_rhai().contains("state_toggle"))
                        .unwrap_or(false)
                    // Custom entity profiles override the render expression to
                    // row(col("content")) which has no StateToggle node.
                    && !state.has_blocks_profile()
            }
            E2ETransition::TriggerSlashCommand { block_id } => {
                state.app_started
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .map_or(false, |b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && !block_id.as_str().contains("default-")
                    && state.block_state.blocks.len() > 2
                    // Custom entity profiles override the render expression to
                    // row(col("content")) which has no EditableText node.
                    && !state.has_blocks_profile()
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
            E2ETransition::UndoLastMutation => state.app_started && !state.undo_stack.is_empty(),
            E2ETransition::Redo => state.app_started && !state.redo_stack.is_empty(),
        }
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            // Pre-startup transitions
            E2ETransition::WriteOrgFile { filename, content } => {
                use regex::Regex;

                let doc_uri = EntityUri::file(filename);
                state.documents.insert(doc_uri.clone(), filename.clone());

                // Remove old blocks from this document (handles re-writing the same file)
                let old_block_ids: Vec<EntityUri> = state
                    .block_state
                    .block_documents
                    .iter()
                    .filter(|(_, uri)| **uri == doc_uri)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &old_block_ids {
                    state.block_state.blocks.remove(&id);
                    state.block_state.block_documents.remove(id);
                    state.layout_blocks.remove(&id);
                }

                // Parse block IDs from content and add to reference state
                // This tracks what blocks will exist after the app starts and syncs the file
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
                        let doc_entity_uri = EntityUri::file(filename);
                        let mut block = Block::new_text(
                            block_uri.clone(),
                            doc_entity_uri.clone(),
                            doc_entity_uri,
                            content,
                        );
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
                        // Extract :id from header args if present
                        source_block_id = src_id_regex
                            .captures(line)
                            .map(|c| c.get(1).unwrap().as_str().to_string());
                    } else if src_end_regex.is_match(line) && in_source_block {
                        // Create source block as child of current headline block
                        if let Some(parent_key) = &current_block_id {
                            let parent_block = &state.block_state.blocks[parent_key];
                            let parent_uri = parent_block.id.clone();
                            let parent_doc_id = parent_block.document_id.clone();
                            let src_id = source_block_id.take().unwrap_or_else(|| {
                                format!("{}::src::{}", parent_uri.id(), source_block_index)
                            });
                            let src_uri = EntityUri::block(&src_id);
                            let mut src_block = Block {
                                id: src_uri.clone(),
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
                                            .insert(src_uri.clone());
                                    } else if matches!(sl, SourceLanguage::Render) {
                                        state.layout_blocks.headline_ids.insert(parent_key.clone());
                                        state
                                            .layout_blocks
                                            .render_source_ids
                                            .insert(src_uri.clone());
                                        if let Some(expr) =
                                            super::reference_state::render_expr_from_rhai(
                                                src_block.content.as_str(),
                                            )
                                        {
                                            state.render_expressions.insert(src_uri.clone(), expr);
                                        }
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
                // Re-assign sequences using canonical ordering (source blocks
                // first, then by sequence, then by ID). This matches what OrgRenderer
                // will produce after the production system round-trips the file.
                let mut all_blocks: Vec<Block> =
                    state.block_state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.block_state.blocks =
                    all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();

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
                let default_doc_uri = EntityUri::doc("__default__");
                let default_content = include_str!("../../../../assets/default/index.org");
                let parse_result = holon_orgmode::parse_org_file(
                    std::path::Path::new("index.org"),
                    default_content,
                    &default_doc_uri,
                    0,
                    std::path::Path::new(""),
                )
                .expect("default index.org must parse");

                // The production code rewrites top-level parent_ids from
                // doc:index.org to doc:__default__
                let file_doc_uri = parse_result.document.id.clone();
                for block in parse_result.blocks {
                    let parent_id = if block.parent_id == file_doc_uri {
                        default_doc_uri.clone()
                    } else {
                        block.parent_id.clone()
                    };
                    let mut b = block;
                    b.parent_id = parent_id;
                    b.document_id = default_doc_uri.clone();
                    let block_id = b.id.clone();
                    state
                        .block_state
                        .block_documents
                        .insert(block_id.clone(), default_doc_uri.clone());
                    // Track render expressions for default layout render source blocks
                    if b.content_type == ContentType::Source
                        && b.source_language
                            .as_ref()
                            .map_or(false, |sl| matches!(sl, SourceLanguage::Render))
                    {
                        if let Some(expr) =
                            super::reference_state::render_expr_from_rhai(b.content.as_str())
                        {
                            state.render_expressions.insert(block_id.clone(), expr);
                        }
                    }
                    state.block_state.blocks.insert(block_id, b);
                }
            }

            // Post-startup transitions
            E2ETransition::CreateDocument { file_name } => {
                let doc_uri = EntityUri::file(file_name);
                state.documents.insert(doc_uri, file_name.clone());
                state.next_doc_id += 1;
            }
            E2ETransition::ApplyMutation(event) => {
                if event.source == MutationSource::UI {
                    state.push_undo_snapshot();
                }
                if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                    let doc_uri = if parent_id.is_document() {
                        parent_id.clone()
                    } else {
                        state
                            .block_state
                            .blocks
                            .get(parent_id)
                            .map(|b| b.document_id.clone())
                            .unwrap_or_else(|| parent_id.clone())
                    };
                    state
                        .block_state
                        .block_documents
                        .insert(id.clone(), doc_uri);
                }

                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                // Both UI and External mutations trigger org sync re-render
                // (on_block_changed), which re-writes the org file in canonical
                // order (source blocks first). Re-assign sequences to match.
                assign_reference_sequences_canonical(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();

                // Update tracked render expressions if a render source block was mutated
                if let Mutation::Update { id, fields, .. } = &event.mutation {
                    if state.layout_blocks.render_source_ids.contains(id)
                        && fields.contains_key("content")
                    {
                        if let Some(block) = state.block_state.blocks.get(id) {
                            if let Some(expr) = super::reference_state::render_expr_from_rhai(
                                block.content.as_str(),
                            ) {
                                state.render_expressions.insert(id.clone(), expr);
                            }
                        }
                    }
                }

                state.block_state.next_id += 1;
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
                    state
                        .block_state
                        .blocks
                        .insert(block.id.clone(), block.clone());
                }
                // BulkExternalAdd serializes via serialize_blocks_to_org (canonical order)
                let mut all_blocks: Vec<Block> =
                    state.block_state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.block_state.blocks =
                    all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                state.block_state.next_id += blocks.len();
            }
            E2ETransition::ConcurrentSchemaInit => {
                // ConcurrentSchemaInit doesn't change reference state - it only tests
                // that the database doesn't get locked when schema init runs concurrently.
            }
            E2ETransition::EditViaDisplayTree {
                block_id,
                new_content,
            } => {
                state.push_undo_snapshot();
                // Same as a UI Mutation::Update on content
                let event = MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Update {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                        fields: [("content".to_string(), Value::String(new_content.clone()))]
                            .into(),
                    },
                };
                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                assign_reference_sequences_canonical(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                state.block_state.next_id += 1;
            }

            E2ETransition::EditViaViewModel {
                block_id,
                new_content,
            } => {
                state.push_undo_snapshot();
                // Same as EditViaDisplayTree — a content update via UI mutation
                let event = MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Update {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                        fields: [("content".to_string(), Value::String(new_content.clone()))]
                            .into(),
                    },
                };
                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                assign_reference_sequences_canonical(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                state.block_state.next_id += 1;
            }

            E2ETransition::ToggleState {
                block_id,
                new_state,
            } => {
                state.push_undo_snapshot();
                // The real frontend sends set_field(task_state, value) with the
                // string value directly — even "" for "no state". The backend
                // stores it in properties as-is.
                let event = MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Update {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                        fields: [("task_state".to_string(), Value::String(new_state.clone()))]
                            .into(),
                    },
                };
                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                assign_reference_sequences_canonical(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                state.block_state.next_id += 1;
            }

            E2ETransition::TriggerSlashCommand { block_id } => {
                state.push_undo_snapshot();
                // The slash command selects "delete" — same as a UI Delete mutation.
                let event = MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Delete {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                    },
                };
                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                assign_reference_sequences_canonical(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                state.block_state.next_id += 1;
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
                            .block_state
                            .blocks
                            .get(&block_id)
                            .map(|b| b.content.as_str())
                            .unwrap_or("");
                        let merged = loro_merge_text(original, &ui_content, &ext_content);
                        if let Some(block) = state.block_state.blocks.get_mut(&block_id) {
                            block.content = merged;
                        }
                    } else {
                        // Without Loro, external write (org file) wins via LWW
                        if let Some(block) = state.block_state.blocks.get_mut(&block_id) {
                            block.content = ext_content;
                        }
                    }
                } else {
                    // Non-overlapping mutations: apply both sequentially
                    for event in [ui_mutation, external_mutation] {
                        if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                            let doc_uri = if parent_id.is_document() {
                                parent_id.clone()
                            } else {
                                fn find_doc(
                                    block_id: &EntityUri,
                                    state: &ReferenceState,
                                ) -> Option<EntityUri> {
                                    let block = state.block_state.blocks.get(block_id)?;
                                    if block.parent_id.is_document() {
                                        Some(block.parent_id.clone())
                                    } else {
                                        find_doc(&block.parent_id, state)
                                    }
                                }
                                find_doc(parent_id, &state).unwrap_or_else(|| parent_id.clone())
                            };
                            state
                                .block_state
                                .block_documents
                                .insert(id.clone(), doc_uri);
                        }

                        let mut blocks: Vec<Block> =
                            state.block_state.blocks.values().cloned().collect();
                        event.mutation.apply_to(&mut blocks);
                        state.block_state.blocks =
                            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                    }
                }
                // External mutation re-writes org file in canonical order
                let mut all_blocks: Vec<Block> =
                    state.block_state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.block_state.blocks =
                    all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                // UI used next_id, External used next_id+1
                state.block_state.next_id += 2;
            }

            E2ETransition::UndoLastMutation => {
                state.pop_undo_to_redo();
            }
            E2ETransition::Redo => {
                state.pop_redo_to_undo();
            }
        }
        state
    }
}
