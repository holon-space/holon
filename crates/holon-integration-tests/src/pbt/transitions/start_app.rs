//! Transition: start the application.
//!
//! @pbt rung external
//!   app boot/lifecycle stimulus. NOTE: unimplemented on the headless
//!   frontend component (panics) and NOT in any composed alphabet yet —
//!   lifecycle is a later increment.
//! @pbt covers app-boot — application startup + seeded sidebar watch
//!
//! Mirrors the legacy logic split across `state_machine.rs:339-351`
//! (generator), `state_machine.rs:3109-3114` (precondition),
//! `state_machine.rs:1947-2123` (ref-state apply),
//! `sut.rs:716-944` (SUT apply), and
//! `transition_budgets.rs:129-134` (expected SQL).

use holon_api::ContentType;
use holon_api::QueryLanguage;
use holon_api::SourceLanguage;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBoot;
use holon_pbt_core::capabilities::RefBootMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::capabilities::SutWatchRegister;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::query::QuerySource;
use crate::pbt::query::QueryTable;
use crate::pbt::query::TestQuery;
use crate::pbt::query::WatchSpec;
use crate::pbt::query::all_block_columns;
use crate::pbt::reference_state::ReferenceState;

/// Query id of the production seeded left-sidebar watch
/// (`assets/default/index.org`). Registered on BOTH ref and SUT after boot so
/// `inv-watch-rows-match-ref` (which intersects the two sides) covers the
/// sidebar's `PageBlocks` query.
pub(crate) const SEEDED_SIDEBAR_WATCH_ID: &str = "seeded-left-sidebar-pages";

/// The seeded left-sidebar watch spec (production `index.org` page query).
pub(crate) fn seeded_sidebar_watch_spec() -> WatchSpec {
    WatchSpec {
        query: TestQuery {
            table: QueryTable::Blocks,
            columns: all_block_columns(),
            predicates: vec![],
            source: QuerySource::PageBlocks,
        },
        language: QueryLanguage::HolonSql,
    }
}

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Start the application (triggers sync, may race with DDL).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StartApp {
    pub wait_for_ready: bool,
    /// Enable the fake external MCP provider (adds concurrent DDL during
    /// startup)
    pub enable_fake_mcp: bool,
    /// Enable Loro CRDT layer (false = SQL-only, matching Flutter default)
    pub enable_loro: bool,
}

impl<R: RefLifecycle + RefBootMut> TransitionFactory<R> for StartApp {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn holon_pbt_core::capabilities::SutAppLifecycle,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
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
                2u32.saturating_add((state.pre_startup_file_count() as u32).saturating_mul(8));
            (weight, strat)
        })
    }
}

impl<R: RefLifecycle + RefBootMut> TransitionRef<R> for StartApp {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started(), Reason::AppAlreadyStarted),
            check(
                state.pre_startup_file_count() > 0,
                Reason::PreStartupFileCountZero,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole boot reference effect (flip app_started, seed the bundled
        // default layout / seed profile / sidebar watch, fresh-boot drawers +
        // journals focus) lives in `RefBootMut::boot_app`.
        state.boot_app();
    }
}

/// Load the bundled `block` entity profile (the TypeRegistry's
/// `block_profile.yaml`) into `domain.seed_profile` — the SAME profile the
/// production boot's ProfileResolver serves `render_entity`. Extracted from
/// `StartApp::apply_to_ref` so pre-started oracles (`build_started_ref`, the
/// keystone's `wide_e2e_ref`) carry it too: without it the ref-side
/// `resolve_profile` returns `None`, `render_entity` interprets to `Empty`,
/// the generator sees ZERO `state_toggle` widgets, and `ToggleState` silently
/// vanishes from the composed alphabet (`NoTogglableStates` on every draw —
/// the change-status coverage hole found in the 2026-07-05 dogfood triage).
pub(crate) fn load_seed_profile_into_ref(state: &mut ReferenceState) {
    let registry =
        holon_profiles::create_default_registry().expect("default TypeRegistry must initialize");
    let block_type_def = registry
        .get("block")
        .expect("Block type must be registered");
    state.domain.seed_profile = holon::entity_profile::profile_from_type_def(&block_type_def);
}

/// Model the bundled default-asset layout the production boot's
/// `seed_default_layout` (`holon-app/src/seed.rs`) creates, into the reference
/// `block_state`: the fixed-ID page shells (`block:journals`, always), the
/// `__default__` seed page + the parsed `assets/default/index.org` layout
/// blocks (fresh only), all seed-classified (`block_documents[id]=no_parent`),
/// and the `layout_blocks.{query_source_ids,render_source_ids,headline_ids}`
/// classification. Extracted from `StartApp::apply_to_ref`
/// (behavior-preserving) so the composed/wide oracle's `build_started_ref` can
/// model the SAME layout the booted `full_headless` SUT carries — making
/// `inv-watch-rows-match-ref` a full-fidelity full-block-set comparison rather
/// than a seed-excluded one. Does NOT touch nav-focus / sidebar-drawer /
/// seed-profile state (those stay with the caller: `StartApp` applies them; the
/// composed oracle aligns nav in `structural_ref_wired`).
pub(crate) fn seed_booted_layout_into_ref(state: &mut ReferenceState, fresh: bool) {
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

    // The `block:journals` page + its display query/render, seeded programmatically
    // by prod's `build_default_layout_blocks` regardless of freshness — prod
    // repairs the page idempotently. Model the SAME blocks here so
    // `inv-blocks-match-ref` sees the identical booted set. The page block
    // documents itself (`block_documents[journals]=journals`, NON-seed: the
    // wide oracle asserts the user-visible first-boot page); its children
    // belong to the journals page document. The auto-create RULE is modeled
    // just after this loop (also seeded by `build_default_layout_blocks`); the
    // boot-FIRED journal day-block is modeled only for the composed frontend
    // arm (see `wide_e2e`), since EVERY Turso frontend boot fires it — the
    // rule-firing machinery runs unconditionally on any non-wasm Turso boot, not
    // behind the `Actor::ActionEngine` label.
    let journals_uri = EntityUri::parse(holon_frontend::JOURNALS_PAGE_ID).expect("journals id");
    for block in holon_frontend::journals_page_blocks() {
        let block_id = block.id.clone();
        let doc = if block_id == journals_uri {
            block_id.clone()
        } else {
            journals_uri.clone()
        };
        // Classify the query/render source children into layout_blocks so PBT
        // mutation transitions leave them alone and the render tracks its expr,
        // mirroring the index.org layout handling below.
        if block.content_type == ContentType::Source {
            if let Some(sl) = block.source_language.as_ref() {
                if sl.as_query().is_some() {
                    state
                        .domain
                        .layout_blocks
                        .query_source_ids
                        .insert(block_id.clone());
                    state
                        .domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                } else if matches!(sl, SourceLanguage::Render) {
                    state
                        .domain
                        .layout_blocks
                        .render_source_ids
                        .insert(block_id.clone());
                    state
                        .domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    if let Ok(expr) = state.harness.interpreter.parse_dsl(&block.content) {
                        state
                            .domain
                            .render_expressions
                            .insert(block_id.clone(), expr);
                    }
                }
            }
        }
        state
            .domain
            .block_state
            .block_documents
            .insert(block_id.clone(), doc);
        state.domain.block_state.blocks.insert(block_id, block);
    }

    // The auto-create RULE (`journals_auto_create_blocks`: `Journal Auto-Create`
    // heading + `holon_sql` trigger + `holon_rule` action), also seeded on every
    // boot by prod's `build_default_layout_blocks`. Modeled as NON-seed children of
    // the journals page (`block_documents=journals`) so the block-comparison
    // invariants expect them in the SUT projection. The trigger/action are program
    // (`is_program`) blocks — never display-evaluated — so they are NOT classified
    // as query/render layout sources; the heading hosts them.
    //
    // Assign a per-parent `sequence` matching the boot seed order so the oracle's
    // `(sibling_order_group, sequence, id)` sort (ADR 0005) reproduces the SUT's
    // fractional-index order — otherwise the trigger/action tie-break by id
    // (`action` < `trigger`) and diverge `inv-{live,loro}-children-match-ref` +
    // `inv-blocks-match-ref/org` under `block:journals::auto-create`.
    let mut seq_by_parent: std::collections::HashMap<EntityUri, i64> =
        std::collections::HashMap::new();
    for mut block in holon_frontend::journals_auto_create_blocks() {
        let block_id = block.id.clone();
        let seq = seq_by_parent.entry(block.parent_id.clone()).or_insert(0);
        block.set_sequence(*seq);
        *seq += 1;
        if block.content_type == ContentType::Text {
            // The `Journal Auto-Create` heading hosts the rule pair.
            state
                .domain
                .layout_blocks
                .headline_ids
                .insert(block_id.clone());
        }
        state
            .domain
            .block_state
            .block_documents
            .insert(block_id.clone(), journals_uri.clone());
        state.domain.block_state.blocks.insert(block_id, block);
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
                && let Ok(expr) = state.harness.interpreter.parse_dsl(&b.content)
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
}

/// The composed `CapMap` registers the watch only when it actually hosts
/// `SutWatchRegister`. If the cap is absent, this is a deliberate no-op:
/// `inv-watch-rows-match-ref` intersects the two sides, so a ref-only entry is
/// simply skipped — no phantom comparison.
#[allow(async_fn_in_trait)]
pub trait SutBootWatches {
    async fn register_seeded_sidebar_watch(&self);
}

#[allow(async_fn_in_trait)]
impl SutBootWatches for holon_pbt_core::composition::CapMap {
    async fn register_seeded_sidebar_watch(&self) {
        if let Some(reg) = self.get::<dyn SutWatchRegister>() {
            let spec = seeded_sidebar_watch_spec();
            reg.register_watch(SEEDED_SIDEBAR_WATCH_ID, &spec.query.to_sql(), spec.language)
                .await;
        }
    }
}

// Hand-written (not `cap_transition!`): StartApp binds TWO SUT caps
// (`SutAppLifecycle + SutBootWatches`) and its `apply_to_sut` READS `state`
// (root-layout id + setup validity), which the single-cap macro can't express.
#[allow(async_fn_in_trait)]
impl<R: RefLifecycle + RefBoot, S: SutAppLifecycle + SutBootWatches> TransitionImpl<R, S>
    for StartApp
{
    async fn apply_to_sut(&self, state: &R, sut: &mut S) {
        // Boundary-precompute the `ref_state`-derived values the cap consumes
        // at action time (the HARD RULE forbids `ref_state` in the cap sig, not
        // derived args). The doc-uri reconcile + seed-count settle run in the
        // `StartApp` arm of `block_tree_post_action`.
        let root_id = state
            .root_layout_block_id()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        let expects_valid_index = state.is_properly_setup();
        sut.start_app(
            root_id,
            expects_valid_index,
            self.wait_for_ready,
            self.enable_fake_mcp,
            self.enable_loro,
        )
        .await;
        sut.register_seeded_sidebar_watch().await;
    }
}

#[cfg(feature = "otel-testing")]
use holon_pbt_core::capabilities::RefSqlCardinality;
#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for StartApp {
    fn expected_sql<R: RefSqlCardinality>(&self, _: &R) -> ExpectedSql {
        ExpectedSql {
            reads: 200,
            writes: 60,
            ddl: 300,
            tolerance: 80,
        }
    }
}
