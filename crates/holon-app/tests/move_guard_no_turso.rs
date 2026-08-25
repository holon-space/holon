//! D18.c capability pin: the net gate's placement policy classifies its
//! subject with Turso ABSENT (ADR 0032 D10 — guards must evaluate without a
//! projection), through the SAME declaration the renderer evaluates
//! (`block_profile.yaml`'s `is_program`), not a private SQL restatement.
//!
//! The container here is deliberately bare: no DbHandle, no QueryableCache,
//! no matview manager. The guard gets the two backend-blind seams a Loro-only
//! session would wire — a `BlockReader` over an in-memory forest and a
//! `ProfileResolving` whose `rule_sibling` lookup is fed by
//! `LiveEntitySpec::live_data_from_blocks`, the production CDC-free arm.
//!
//! @pbt kind harness
//! @pbt covers move-guard-no-turso — the machinery-containment refusal, its
//! class-matched confirmation, and the unseen-block Confirm all reach a
//! verdict with no Turso service in the container
//! @pbt overlaps move_guard_policy — same policy against a REAL Turso
//! container; this file pins the storage-blind half

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::Injector;
use fluxdi::Provider;
use holon::api::net_guard::CONFIRM_BREAK_PARAM;
use holon::api::net_guard::Confirmation;
use holon::api::net_guard::NetGuard;
use holon::api::net_guard::NetGuardOp;
use holon::api::net_guard::NetVerdict;
use holon::api::net_guard::RefusalClass;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_app::move_guard::MoveGuard;
use holon_filesystem::BlockReader;
use holon_profiles::LiveEntitySpec;
use holon_profiles::ProfileResolver;
use holon_profiles::ProfileResolving;

/// Two pages plus a rule (head + trigger) under an owning heading, and a
/// second rule sitting at TOP level — its machinery's parent is the explicit
/// root sentinel, which the sibling lookup must key like any other parent.
fn forest() -> Vec<Block> {
    let uri = |s: &str| EntityUri::parse(s).expect("fixture uri");
    let mut page_a = Block::new_text(uri("block:page-a"), EntityUri::no_parent(), "Page A");
    page_a.tags.insert("Page");
    let mut page_b = Block::new_text(uri("block:page-b"), EntityUri::no_parent(), "Page B");
    page_b.tags.insert("Page");
    vec![
        page_a,
        page_b,
        Block::new_text(uri("block:heading"), uri("block:page-a"), "Owning heading"),
        Block::new_source(
            uri("block:rule-head"),
            uri("block:heading"),
            "holon_rule",
            "rule: {}",
        ),
        Block::new_source(
            uri("block:trigger"),
            uri("block:heading"),
            "holon_sql",
            "SELECT 1",
        ),
        Block::new_source(
            uri("block:top-head"),
            EntityUri::no_parent(),
            "holon_rule",
            "rule: {}",
        ),
        Block::new_source(
            uri("block:top-trigger"),
            EntityUri::no_parent(),
            "holon_sql",
            "SELECT 1",
        ),
    ]
}

struct ForestReader {
    blocks: Vec<Block>,
}

#[async_trait]
impl BlockReader for ForestReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .blocks
            .iter()
            .filter(|b| b.parent_id == *doc_id)
            .cloned()
            .collect())
    }

    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .get_blocks(doc_id)
            .await?
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.blocks.iter().find(|b| b.id == *id).cloned())
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(Vec::new())
    }
}

/// The renderer's resolver, built exactly the way a no-Turso session builds
/// it: built-in type profiles, entity lookups fed from a block snapshot.
fn resolver_from(blocks: &[Block]) -> Arc<dyn ProfileResolving> {
    let type_registry =
        holon_profiles::create_default_registry().expect("default TypeRegistry builds");
    let type_profiles = holon_profiles::type_profiles_from_registry(&type_registry);
    let mut live_entities = holon_profiles::LiveEntities::new();
    for spec in LiveEntitySpec::ALL.iter().copied() {
        live_entities.insert(
            spec.entity_name(),
            spec.live_data_from_blocks(blocks.iter()),
        );
    }
    let empty_profiles = holon_api::live_data::LiveData::new(
        Vec::new(),
        |_| Ok(String::new()),
        |_| anyhow::bail!("a no-Turso fixture has no user profile source"),
    );
    Arc::new(ProfileResolver::with_type_profiles(
        empty_profiles,
        holon_api::UiInfo::default(),
        live_entities,
        HashMap::new(),
        type_profiles,
    ))
}

/// A container with the two backend-blind seams and NOTHING Turso.
fn guard() -> MoveGuard {
    let blocks = forest();
    let resolver = resolver_from(&blocks);
    let reader: Arc<dyn BlockReader> = Arc::new(ForestReader { blocks });
    let injector = Injector::root();
    injector.provide::<dyn BlockReader>(Provider::root(move |_| reader.clone()));
    injector.provide::<dyn ProfileResolving>(Provider::root(move |_| resolver.clone()));
    let registry = Arc::new(
        holon_capability::registry::shipped_profiles()
            .expect("the shipped capability profiles must parse"),
    );
    MoveGuard::new(injector, registry)
}

async fn verdict(
    guard: &MoveGuard,
    id: &str,
    destination: &str,
    confirm_break: Option<&str>,
) -> NetVerdict {
    let mut params: HashMap<Arc<str>, Value> = HashMap::new();
    params.insert(Arc::from("id"), Value::String(id.to_string()));
    params.insert(
        Arc::from("parent_id"),
        Value::String(destination.to_string()),
    );
    if let Some(class) = confirm_break {
        params.insert(
            Arc::from(CONFIRM_BREAK_PARAM),
            Value::String(class.to_string()),
        );
    }
    let confirmation = Confirmation::parse(&params).expect("confirmation parses");
    guard
        .check(&NetGuardOp {
            entity_name: "block",
            op_name: "move_block",
            params: &params,
            confirmation,
        })
        .await
        .expect("the guard reaches a verdict without Turso")
}

fn assert_machinery_refusal(verdict: NetVerdict) {
    match verdict {
        NetVerdict::Refuse(refusal) => {
            assert_eq!(refusal.class, RefusalClass::MachineryContainment)
        }
        NetVerdict::Confirm => panic!("machinery separation must refuse, got Confirm"),
    }
}

#[tokio::test]
async fn a_triggers_move_out_of_its_rule_refuses_without_turso() {
    let guard = guard();
    assert_machinery_refusal(verdict(&guard, "block:trigger", "block:page-b", None).await);
}

#[tokio::test]
async fn a_rule_heads_move_out_of_its_heading_refuses_without_turso() {
    let guard = guard();
    assert_machinery_refusal(verdict(&guard, "block:rule-head", "block:page-b", None).await);
}

/// Root-sentinel edge: machinery whose parent IS the stored sentinel. The
/// sibling lookup must key `sentinel:no_parent` like any other parent — the
/// old SQL sibling-scan did.
#[tokio::test]
async fn top_level_machinery_under_the_root_sentinel_refuses_without_turso() {
    let guard = guard();
    assert_machinery_refusal(verdict(&guard, "block:top-trigger", "block:page-b", None).await);
}

#[tokio::test]
async fn a_class_matched_confirmation_carries_the_move_without_turso() {
    let guard = guard();
    let v = verdict(
        &guard,
        "block:trigger",
        "block:page-b",
        Some("machinery_containment"),
    )
    .await;
    match v {
        NetVerdict::Confirm => {}
        NetVerdict::Refuse(r) => panic!("confirmed move must pass, got refusal: {}", r.reason),
    }
}

/// A block the reader does not hold cannot be classified — confirmed, not
/// refused on a guess. Same contract as the Turso arm's unseen-projection row.
#[tokio::test]
async fn an_unseen_block_is_confirmed_without_turso() {
    let guard = guard();
    let v = verdict(&guard, "block:ghost", "block:page-b", None).await;
    match v {
        NetVerdict::Confirm => {}
        NetVerdict::Refuse(r) => panic!("unseen block must confirm, got refusal: {}", r.reason),
    }
}
