//! Transition: start the application.
//!
//! Mirrors the legacy logic split across `state_machine.rs:339-351` (generator),
//! `state_machine.rs:3109-3114` (precondition),
//! `state_machine.rs:1947-2123` (ref-state apply),
//! `sut.rs:716-944` (SUT apply), and
//! `transition_budgets.rs:129-134` (expected SQL).

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, Region, SourceLanguage};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Start the application (triggers sync, may race with DDL).
#[derive(Clone, Debug)]
pub struct StartApp {
    pub wait_for_ready: bool,
    /// Enable Todoist fake mode (adds concurrent DDL during startup)
    pub enable_todoist: bool,
    /// Enable Loro CRDT layer (false = SQL-only, matching Flutter default)
    pub enable_loro: bool,
}

impl E2ETransitionFactory for StartApp {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let instance = StartApp {
            wait_for_ready: true,
            enable_todoist: true,
            enable_loro: state.variant.enable_loro,
        };
        instance.preconditions(state).map(|()| {
            let strat = Just(instance).boxed();
            // Scale weight with how much pre-startup work has already
            // accumulated so an unlucky proptest seed can't starve us out
            // of the 50-step pre-startup budget. Each org file written
            // raises StartApp's odds (capped to keep early steps able to
            // still pick CreateDirectory/WriteOrgFile/JjGitInit).
            let weight =
                2u32.saturating_add((state.pre_startup_file_count as u32).saturating_mul(8));
            (weight, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for StartApp {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started, Reason::AppAlreadyStarted),
            check(
                state.pre_startup_file_count > 0,
                Reason::PreStartupFileCountZero,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.app_started = true;

        // The production system always seeds default layout blocks on first startup.
        let default_doc_uri = EntityUri::no_parent();
        {
            // Add the seed page block itself (tags ⊇ ["Page"])
            let mut seed_doc_block = Block::new_text(
                default_doc_uri.clone(),
                EntityUri::no_parent(),
                "__default__",
            );
            seed_doc_block.set_page(true);
            if let Some(ref ks) = state.keyword_set {
                use holon_orgmode::models::OrgDocumentExt;
                seed_doc_block.set_todo_keywords(Some(ks.0.clone()));
            }
            state
                .block_state
                .blocks
                .insert(default_doc_uri.clone(), seed_doc_block);
            state
                .block_state
                .block_documents
                .insert(default_doc_uri.clone(), default_doc_uri.clone());

            // Seed fixed-ID document blocks from DEFAULT_ASSETS
            for asset in holon_frontend::DEFAULT_ASSETS {
                if let Some(doc_id) = asset.fixed_doc_id {
                    let uri = EntityUri::from_raw(doc_id);
                    let name = asset
                        .filename
                        .strip_suffix(".org")
                        .unwrap_or(asset.filename);
                    let mut block =
                        Block::new_text(uri.clone(), EntityUri::no_parent(), name.to_string());
                    block.set_page(true);
                    state.block_state.blocks.insert(uri.clone(), block);
                    state.block_state.block_documents.insert(uri.clone(), uri);
                }
            }

            let default_content = include_str!("../../../../../assets/default/index.org");
            let parse_result = holon_orgmode::parse_org_file(
                std::path::Path::new("index.org"),
                default_content,
                &default_doc_uri,
                std::path::Path::new(""),
            )
            .expect("default index.org must parse");

            let file_doc_uri = parse_result.document.id.clone();
            for block in parse_result.blocks {
                let parent_id = if block.parent_id == file_doc_uri {
                    default_doc_uri.clone()
                } else {
                    block.parent_id.clone()
                };
                let mut b = block;
                b.parent_id = parent_id;
                let block_id = b.id.clone();
                state
                    .block_state
                    .block_documents
                    .insert(block_id.clone(), default_doc_uri.clone());
                // Track render expressions for default layout render source blocks
                if b.content_type == ContentType::Source
                    && b.source_language
                        .as_ref()
                        .is_some_and(|sl| matches!(sl, SourceLanguage::Render))
                    && let Ok(expr) = state.interpreter.parse_dsl(&b.content)
                {
                    state.render_expressions.insert(block_id.clone(), expr);
                }
                state.block_state.blocks.insert(block_id, b);
            }

            // Classify seeded default blocks into layout_blocks to protect
            // them from PBT mutation and enable ViewModel construction.
            let default_block_ids: Vec<EntityUri> = state
                .block_state
                .blocks
                .keys()
                .filter(|id| {
                    state
                        .block_state
                        .block_documents
                        .get(*id)
                        .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel())
                })
                .cloned()
                .collect();

            // Collect classification info before mutating state
            enum SeedClassification {
                Query {
                    block_id: EntityUri,
                    parent_id: EntityUri,
                },
                Render {
                    block_id: EntityUri,
                    parent_id: EntityUri,
                },
                EntityProfile {
                    parent_id: EntityUri,
                },
            }
            let classifications: Vec<SeedClassification> = default_block_ids
                .iter()
                .filter_map(|block_id| {
                    let block = &state.block_state.blocks[block_id];
                    if block.content_type != ContentType::Source {
                        return None;
                    }
                    let sl = block.source_language.as_ref()?;
                    if sl.as_query().is_some() {
                        Some(SeedClassification::Query {
                            block_id: block_id.clone(),
                            parent_id: block.parent_id.clone(),
                        })
                    } else if matches!(sl, SourceLanguage::Render) {
                        Some(SeedClassification::Render {
                            block_id: block_id.clone(),
                            parent_id: block.parent_id.clone(),
                        })
                    } else if sl.to_string() == "holon_entity_profile_yaml" {
                        Some(SeedClassification::EntityProfile {
                            parent_id: block.parent_id.clone(),
                        })
                    } else {
                        None
                    }
                })
                .collect();

            for class in classifications {
                match class {
                    SeedClassification::Query {
                        block_id,
                        parent_id,
                    } => {
                        state.layout_blocks.query_source_ids.insert(block_id);
                        state.layout_blocks.headline_ids.insert(parent_id);
                    }
                    SeedClassification::Render {
                        block_id,
                        parent_id,
                    } => {
                        state.layout_blocks.render_source_ids.insert(block_id);
                        state.layout_blocks.headline_ids.insert(parent_id);
                    }
                    SeedClassification::EntityProfile { parent_id, .. } => {
                        state.layout_blocks.headline_ids.insert(parent_id);
                    }
                }
            }
        } // end seed block scope

        // Load the seed entity profile from the TypeRegistry's bundled
        // block_profile.yaml (not from org blocks — the seed index.org
        // doesn't contain entity profile blocks).
        let registry = holon::type_registry::create_default_registry()
            .expect("default TypeRegistry must initialize");
        let block_type_def = registry
            .get("block")
            .expect("Block type must be registered");
        state.seed_profile = holon::entity_profile::profile_from_type_def(&block_type_def);

        // FU-10 mirror: production `seed_default_layout` calls
        // `navigation::focus(Main, block:journals)` on fresh DBs, which
        // inserts a navigation_history row and updates the cursor. Mirror
        // that here so the reference state's `current_focus` and
        // `expected_focus_root_ids(Main)` line up with what the SUT
        // actually has post-StartApp.
        use crate::pbt::reference_state::{NavigationHistory, OpenPinEntry};
        let journals_uri = EntityUri::block("journals");
        let history = state
            .navigation_history
            .entry(Region::Main)
            .or_insert_with(NavigationHistory::new);
        history.entries.truncate(history.cursor + 1);
        history.entries.push(Some(journals_uri.clone()));
        history.cursor = history.entries.len() - 1;

        let history_id = state.next_history_id;
        state.next_history_id += 1;
        let added_ts_logical = state.next_pin_ts;
        state.next_pin_ts += 1;
        let pins = state.open_pins.entry(Region::Main).or_default();
        pins.clear();
        pins.push(OpenPinEntry {
            history_id,
            block_id: Some(journals_uri),
            added_ts_logical,
        });
    }

    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_start_app(
            state,
            self.wait_for_ready,
            self.enable_todoist,
            self.enable_loro,
        )
        .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 200,
            writes: 60,
            ddl: 300,
            tolerance: 80,
        }
    }
}
