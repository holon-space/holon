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

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Start the application (triggers sync, may race with DDL).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StartApp {
    pub wait_for_ready: bool,
    /// Enable the fake external MCP provider (adds concurrent DDL during startup)
    pub enable_fake_mcp: bool,
    /// Enable Loro CRDT layer (false = SQL-only, matching Flutter default)
    pub enable_loro: bool,
}

impl TransitionFactory<ReferenceState> for StartApp {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let instance = StartApp {
            wait_for_ready: true,
            enable_fake_mcp: true,
            enable_loro: state.enable_loro(),
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

impl TransitionRef<ReferenceState> for StartApp {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.action.app_started, Reason::AppAlreadyStarted),
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
        state.action.app_started = true;

        // Freshness mirrors prod `seed_default_layout`: the default layout —
        // and the journals focus + open default drawers that come with it —
        // is only seeded when `block:root-layout` is absent at boot. A
        // pre-startup user `index.org` keeps the well-known root id (the
        // generator pins `:ID: root-layout`), file sync runs before seeding,
        // so the user layout suppresses the entire default-layout seed.
        let fresh = !state
            .domain
            .block_state
            .blocks
            .contains_key(&holon_api::root_layout_block_uri());

        // Default layout boots both sidebars as open drawers — mirror that in ref state.
        if fresh {
            state
                .ui
                .tab
                .drawer_open
                .insert("block:default-left-sidebar".to_string(), true);
            state
                .ui
                .tab
                .drawer_open
                .insert("block:default-right-sidebar".to_string(), true);
        }

        // The production system always seeds default layout blocks on first startup.
        // `default_doc_uri` is the *document* the seed layout belongs to (the
        // no-parent sentinel) — used below as the parent/doc for the index.org
        // layout blocks and to classify them as seeds. The default page block
        // itself has the stable id `block:__default__` (matches prod's
        // `FrontendSession::default_doc_uri()` after the sentinel root-fix);
        // its *document* is still the sentinel so the truth check classifies it
        // as a seed and excludes it from the user-content comparison.
        let default_doc_uri = EntityUri::no_parent();
        let default_doc_id = EntityUri::block("__default__");

        // Fixed-ID document pages (e.g. block:journals) are built regardless
        // of freshness — prod repairs missing page shells idempotently.
        for asset in holon_frontend::DEFAULT_ASSETS {
            if let Some(doc_id) = asset.fixed_doc_id {
                let uri = EntityUri::parse(doc_id).expect("static asset id");
                let name = asset
                    .filename
                    .strip_suffix(".org")
                    .unwrap_or(asset.filename);
                let mut block =
                    Block::new_text(uri.clone(), EntityUri::no_parent(), name.to_string());
                block.set_page(true);
                state.domain.block_state.blocks.insert(uri.clone(), block);
                state
                    .domain
                    .block_state
                    .block_documents
                    .insert(uri.clone(), uri);
            }
        }

        if fresh {
            // Add the seed page block itself (tags ⊇ ["Page"])
            let mut seed_doc_block = Block::new_text(
                default_doc_id.clone(),
                EntityUri::no_parent(),
                "__default__",
            );
            seed_doc_block.set_page(true);
            // No keyword-set seeding here: random init is gone (see
            // `state_machine.rs::init_state`), and the default `__default__`
            // doc is not user-written content. If a user wants a `#+TODO:`
            // header on a doc, they emit a `WriteOrgFile` transition whose
            // content contains it — and that transition's `apply_to_ref`
            // parses the directive from its own `self.content`.
            state
                .domain
                .block_state
                .blocks
                .insert(default_doc_id.clone(), seed_doc_block);
            state
                .domain
                .block_state
                .block_documents
                .insert(default_doc_id.clone(), default_doc_uri.clone());

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
                    .domain
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
                    state
                        .domain
                        .render_expressions
                        .insert(block_id.clone(), expr);
                }
                state.domain.block_state.blocks.insert(block_id, b);
            }

            // Classify seeded default blocks into layout_blocks to protect
            // them from PBT mutation and enable ViewModel construction.
            let default_block_ids: Vec<EntityUri> = state
                .domain
                .block_state
                .blocks
                .keys()
                .filter(|id| {
                    state
                        .domain
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
                    let block = &state.domain.block_state.blocks[block_id];
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
                        state.domain.layout_blocks.query_source_ids.insert(block_id);
                        state.domain.layout_blocks.headline_ids.insert(parent_id);
                    }
                    SeedClassification::Render {
                        block_id,
                        parent_id,
                    } => {
                        state
                            .domain
                            .layout_blocks
                            .render_source_ids
                            .insert(block_id);
                        state.domain.layout_blocks.headline_ids.insert(parent_id);
                    }
                    SeedClassification::EntityProfile { parent_id, .. } => {
                        state.domain.layout_blocks.headline_ids.insert(parent_id);
                    }
                }
            }
        } // end fresh-only default-layout seed

        // Load the seed entity profile from the TypeRegistry's bundled
        // block_profile.yaml (not from org blocks — the seed index.org
        // doesn't contain entity profile blocks).
        let registry = holon::type_registry::create_default_registry()
            .expect("default TypeRegistry must initialize");
        let block_type_def = registry
            .get("block")
            .expect("Block type must be registered");
        state.domain.seed_profile = holon::entity_profile::profile_from_type_def(&block_type_def);

        // FU-10 mirror: production `seed_default_layout` calls
        // `navigation::focus(Main, block:journals)` on fresh DBs ONLY, which
        // inserts a navigation_history row and updates the cursor. Mirror
        // that here so the reference state's `current_focus` and
        // `expected_focus_root_ids(Main)` line up with what the SUT
        // actually has post-StartApp.
        if fresh {
            use crate::pbt::reference_state::OpenPinEntry;
            let journals_uri = EntityUri::block("journals");
            let history = state
                .ui
                .tab
                .navigation_history
                .entry(Region::Main)
                .or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(journals_uri.clone()));
            history.cursor = history.entries.len() - 1;

            let history_id = state.ui.tab.next_history_id;
            state.ui.tab.next_history_id += 1;
            let added_ts_logical = state.ui.user.next_pin_ts;
            state.ui.user.next_pin_ts += 1;
            let pins = state.ui.user.open_pins.entry(Region::Main).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(journals_uri),
                added_ts_logical,
            });
        }
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for StartApp {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        sut.apply_start_app(
            state,
            self.wait_for_ready,
            self.enable_fake_mcp,
            self.enable_loro,
        )
        .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for StartApp {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 200,
            writes: 60,
            ddl: 300,
            tolerance: 80,
        }
    }
}
