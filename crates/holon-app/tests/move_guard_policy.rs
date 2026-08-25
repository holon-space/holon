//! The net gate's placement policy, driven through a REAL container.
//!
//! The guard resolves the shared `BlockOrdering` and the document-home
//! authority, so it only means anything against a built DI graph — and it is
//! consulted from inside the dispatcher, which is where these cases reach it.
//!
//! @pbt kind harness
//! @pbt covers move-guard-machinery — rule machinery may not be separated from
//! the structure that owns it, whatever kind of home the destination has
//! @pbt covers move-guard-destination-kind — a destination whose profile does
//! not declare the moved entity's kind refuses the move
//! @pbt overlaps general_e2e_composed_pbt — the keystone draws `RehomeEntity`
//! into the same gate; this pins the policy's own predicate without a draw

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::Value;
use holon_capability::CapabilityProfile;
use holon_capability::ProfileRegistry;
use holon_loro_wiring::EventInfraModule;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime"),
    )
}

/// The shipped profiles, with `org`'s hosted kinds narrowed to `kinds`.
///
/// `profile_of` maps a durable format to one of two shipped profiles and both
/// declare every kind, so the destination-capability refusal is unreachable
/// through the production registry. Narrowing org here drives the clause
/// against a profile that actually withholds a kind.
fn registry_with_org_kinds(kinds: &str) -> Arc<ProfileRegistry> {
    const ORG: &str = include_str!("../../holon-org-format/profile.yaml");
    const NATIVE: &str = include_str!("../../../assets/default/capability/holon-native.yaml");
    const DECLARED: &str = "hosted_entity_kinds: [block, page, program]";
    assert!(
        ORG.contains(DECLARED),
        "the org profile's kind list must be found to be narrowed"
    );
    let narrowed = ORG.replace(DECLARED, &format!("hosted_entity_kinds: {kinds}"));
    Arc::new(
        ProfileRegistry::new(vec![
            CapabilityProfile::from_yaml(&narrowed).expect("narrowed org profile parses"),
            CapabilityProfile::from_yaml(NATIVE).expect("native profile parses"),
        ])
        .expect("registry builds"),
    )
}

async fn engine(
    db_path: std::path::PathBuf,
    registry: Arc<ProfileRegistry>,
) -> Arc<holon::api::BackendEngine> {
    let (engine, _) = holon::di::create_backend_engine_with_extras(
        db_path,
        move |injector| {
            EventInfraModule
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure EventInfraModule: {e}"))?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
                |resolver| {
                    let db = resolver
                        .resolve::<dyn holon::di::DbHandleProvider>()
                        .handle();
                    Arc::new(holon::core::SqlOperationProvider::new(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            // The guard reads through the backend-blind `BlockReader` seam;
            // this container's arm is the same one `OrgModeModule` registers
            // in production: `CacheBlockReader` over the block cache.
            injector.provide::<dyn holon_filesystem::BlockReader>(Provider::root_async(
                |resolver| async move {
                    let cache = resolver
                        .resolve_async::<holon::core::queryable_cache::QueryableCache<
                            holon_api::block::Block,
                        >>()
                        .await;
                    Arc::new(holon_app::turso_seams::CacheBlockReader::new(cache))
                        as Arc<dyn holon_filesystem::BlockReader>
                },
            ));
            let registry = registry.clone();
            injector.provide::<dyn holon::api::net_guard::NetGuard>(Provider::root_async(
                move |resolver| {
                    let registry = registry.clone();
                    async move {
                        Arc::new(holon_app::move_guard::MoveGuard::new(resolver, registry))
                            as Arc<dyn holon::api::net_guard::NetGuard>
                    }
                },
            ));
            Ok(())
        },
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .expect("container builds");
    engine
}

/// A block seeded through the ops layer — the container's cache reads the
/// `block` matview, so a raw INSERT would be invisible until CDC propagates.
async fn seed(
    engine: &holon::api::BackendEngine,
    id: &str,
    parent: &str,
    language: Option<&str>,
    is_page: bool,
) {
    let mut p: HashMap<Arc<str>, Value> = HashMap::new();
    p.insert(Arc::from("id"), Value::String(id.to_string()));
    p.insert(Arc::from("parent_id"), Value::String(parent.to_string()));
    p.insert(Arc::from("content"), Value::String(id.to_string()));
    p.insert(
        Arc::from("content_type"),
        Value::String(if language.is_some() { "source" } else { "text" }.into()),
    );
    if let Some(language) = language {
        p.insert(
            Arc::from("source_language"),
            Value::String(language.to_string()),
        );
    }
    p.insert(
        Arc::from("sort_key"),
        Value::String(format!("a{}", id.len())),
    );
    engine
        .execute_operation(
            &holon_api::EntityName::new("block"),
            "create",
            p,
            holon_api::operation_engine::OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("seed {id}: {e:#}"));
    if is_page {
        engine
            .db_handle()
            .execute(
                &format!(
                    "INSERT INTO block_tags (block_id, tag) VALUES ('{id}', '{}')",
                    holon_api::PAGE_TAG
                ),
                vec![],
            )
            .await
            .expect("tag page");
    }
}

/// Two pages, each with a heading; the first heading owns a `holon_rule` head.
async fn seed_two_pages_one_rule(engine: &holon::api::BackendEngine) {
    seed(engine, "block:pageA", "sentinel:no_parent", None, true).await;
    seed(engine, "block:owner", "block:pageA", None, false).await;
    seed(
        engine,
        "block:rule",
        "block:owner",
        Some("holon_rule"),
        false,
    )
    .await;
    seed(engine, "block:plain", "block:owner", None, false).await;
    seed(engine, "block:pageB", "sentinel:no_parent", None, true).await;
    seed(engine, "block:elsewhere", "block:pageB", None, false).await;
}

async fn move_block(
    engine: &holon::api::BackendEngine,
    id: &str,
    parent_id: &str,
    confirm_break: Option<&str>,
) -> anyhow::Result<()> {
    let mut p: HashMap<Arc<str>, Value> = HashMap::new();
    p.insert(Arc::from("id"), Value::String(id.to_string()));
    p.insert(Arc::from("parent_id"), Value::String(parent_id.to_string()));
    p.insert(Arc::from("after_block_id"), Value::Null);
    if let Some(class) = confirm_break {
        p.insert(Arc::from("confirm_break"), Value::String(class.to_string()));
    }
    engine
        .execute_operation(
            &holon_api::EntityName::new("block"),
            "move_block",
            p,
            holon_api::operation_engine::OpOrigin::User,
        )
        .await
        .map(|_| ())
}

/// Two sibling headings under one page; the second owns a whole rule — a
/// `holon_rule` head and the `holon_sql` trigger read beside it.
async fn seed_merge_sides_one_rule(engine: &holon::api::BackendEngine) {
    seed(engine, "block:pageA", "sentinel:no_parent", None, true).await;
    seed(engine, "block:canon", "block:pageA", None, false).await;
    seed(engine, "block:dup", "block:pageA", None, false).await;
    seed(
        engine,
        "block:dupRule",
        "block:dup",
        Some("holon_rule"),
        false,
    )
    .await;
    seed(
        engine,
        "block:dupTrig",
        "block:dup",
        Some("holon_sql"),
        false,
    )
    .await;
}

async fn parent_of(engine: &holon::api::BackendEngine, id: &str) -> String {
    let rows = engine
        .db_handle()
        .query(
            &format!("SELECT parent_id FROM block_raw WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .expect("read parent");
    rows.into_iter()
        .next()
        .and_then(|r| {
            r.get("parent_id")
                .and_then(|v| v.as_string().map(str::to_string))
        })
        .expect("block present")
}

/// The counterexample the format-based rule waved through: the destination is a
/// perfectly ordinary org page, so nothing about the HOME is wrong — what is
/// wrong is that the rule head ends up somewhere its owning heading is not.
#[test]
fn machinery_may_not_move_to_a_heading_in_another_page() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_two_pages_one_rule(&engine).await;

        let err = move_block(&engine, "block:rule", "block:elsewhere", None)
            .await
            .expect_err("separating a rule head from its owning heading must be refused");

        let err = format!("{err:#}");
        assert!(
            err.contains("breaks the rule"),
            "the refusal must name what it protects: {err}"
        );
        assert!(
            err.contains("confirm_break") && err.contains("machinery_containment"),
            "an overridable refusal must name the override AND the class a \
             confirmation has to be minted for: {err}"
        );
        assert_eq!(
            parent_of(&engine, "block:rule").await,
            "block:owner",
            "a refused move leaves the tree untouched"
        );
    });
}

/// The same move, confirmed. The gate is a refusal a caller may knowingly
/// overrule, not a prohibition.
#[test]
fn a_confirmed_caller_may_separate_machinery_anyway() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_two_pages_one_rule(&engine).await;

        move_block(
            &engine,
            "block:rule",
            "block:elsewhere",
            Some("machinery_containment"),
        )
        .await
        .expect("a confirmation minted for the machinery refusal carries the move through");

        assert_eq!(parent_of(&engine, "block:rule").await, "block:elsewhere");
    });
}

/// Ordinary content is not machinery, and the policy has nothing to say about
/// where it goes.
#[test]
fn ordinary_content_moves_between_pages_freely() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_two_pages_one_rule(&engine).await;

        move_block(&engine, "block:plain", "block:elsewhere", None)
            .await
            .expect("a plain block may move to another page");

        assert_eq!(parent_of(&engine, "block:plain").await, "block:elsewhere");
    });
}

/// The destination-capability half, driven against a profile that actually
/// withholds the kind. The move is legal by every other measure — the block
/// stays under its own owning heading — so only the kind clause can refuse it.
#[test]
fn a_home_that_declares_no_program_kind_refuses_one() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page]"),
        )
        .await;
        seed_two_pages_one_rule(&engine).await;

        let err = move_block(&engine, "block:rule", "block:owner", None)
            .await
            .expect_err("org declares no `program` kind in this registry");

        let err = format!("{err:#}");
        assert!(
            err.contains("program"),
            "the refusal must name the kind the home withholds: {err}"
        );
        assert!(
            err.contains("confirm_break") && err.contains("destination_capability"),
            "an overridable refusal must name the override AND the class a \
             confirmation has to be minted for: {err}"
        );
    });
}

/// A confirmation answers exactly the refusal class it was minted for. Minted
/// for the destination-capability refusal, it must not carry a move past the
/// machinery-containment refusal — a universal override would be a blanket
/// bypass of every policy the gate ever hosts.
#[test]
fn a_confirmation_for_another_class_answers_nothing() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_two_pages_one_rule(&engine).await;

        let err = move_block(
            &engine,
            "block:rule",
            "block:elsewhere",
            Some("destination_capability"),
        )
        .await
        .expect_err("a wrong-class confirmation must not override the machinery refusal");

        let err = format!("{err:#}");
        assert!(
            err.contains("machinery_containment"),
            "the refusal must name the class a confirmation would have to be \
             minted for: {err}"
        );
        assert_eq!(
            parent_of(&engine, "block:rule").await,
            "block:owner",
            "a wrong-class confirmation leaves the tree untouched"
        );
    });
}

/// `authorization` names a refusal class on purpose NOT confirmable: the
/// parser refuses to mint a confirmation for it, loudly.
#[test]
fn an_authorization_confirmation_cannot_be_minted() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_two_pages_one_rule(&engine).await;

        let err = move_block(
            &engine,
            "block:plain",
            "block:elsewhere",
            Some("authorization"),
        )
        .await
        .expect_err("no confirmation answers an authorization refusal");

        let err = format!("{err:#}");
        assert!(
            err.contains("authorization") && err.contains("not confirmable"),
            "the refusal must say the class is not confirmable: {err}"
        );
        assert_eq!(
            parent_of(&engine, "block:plain").await,
            "block:owner",
            "an unmintable confirmation aborts the dispatch before the provider"
        );
    });
}

/// A merge relocates the duplicate's WHOLE child set under the canonical, so a
/// rule among those children keeps the siblings it is read with. The guard sees
/// one child at a time and cannot tell that from an extraction, so the merge —
/// like the convert — declares it; without the declaration the refusal
/// propagates and the whole merge aborts.
#[test]
fn merging_a_heading_that_owns_a_rule_moves_the_rule_whole() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_merge_sides_one_rule(&engine).await;

        let mut p: HashMap<Arc<str>, Value> = HashMap::new();
        p.insert(Arc::from("canonical"), Value::String("block:canon".into()));
        p.insert(Arc::from("duplicate"), Value::String("block:dup".into()));
        engine
            .execute_operation(
                &holon_api::EntityName::new("block"),
                "merge_blocks",
                p,
                holon_api::operation_engine::OpOrigin::User,
            )
            .await
            .expect("a merge that carries a whole rule across must not be refused");

        assert_eq!(parent_of(&engine, "block:dupRule").await, "block:canon");
        assert_eq!(
            parent_of(&engine, "block:dupTrig").await,
            "block:canon",
            "the head and the trigger it is read with must land together"
        );
    });
}

/// D18.c wiring pin: the guard's classification IS the renderer's — ONE
/// declaration (`block_profile.yaml`'s `is_program`), one evaluator. Two
/// assertions per fixture block: the resolver's computed `is_program` has the
/// extension the guard's old inline SQL had (a head is its own sibling
/// witness; a trigger is program via the sibling clause; text beside a head
/// and a page are NOT program), and the guard's observable verdict matches
/// that same value (machinery refusal iff program). Any future second
/// classifier drifting from the yaml reds here.
#[test]
fn guard_classification_agrees_with_the_renderers_is_program() {
    let rt = runtime();
    rt.clone().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = engine(
            dir.path().join("t.db"),
            registry_with_org_kinds("[block, page, program]"),
        )
        .await;
        seed_merge_sides_one_rule(&engine).await;
        seed(&engine, "block:pageB", "sentinel:no_parent", None, true).await;

        // Order matters: the non-program blocks genuinely move, so they go last.
        let cases = [
            ("block:dupRule", true),
            ("block:dupTrig", true),
            ("block:canon", false),
            ("block:pageA", false),
        ];
        for (id, expect_program) in cases {
            let rows = engine
                .db_handle()
                .query(
                    &format!(
                        "SELECT id, parent_id, content_type, source_language FROM block_raw \
                         WHERE id = '{id}'"
                    ),
                    HashMap::new(),
                )
                .await
                .expect("projection row");
            let raw = rows
                .into_iter()
                .next()
                .unwrap_or_else(|| panic!("{id} must be in the projection"));
            let mut row: HashMap<String, Value> = HashMap::new();
            for key in ["id", "parent_id", "content_type", "source_language"] {
                row.insert(
                    key.to_string(),
                    raw.get(key).cloned().unwrap_or(Value::Null),
                );
            }
            let computed = engine.profile_resolver().resolve_computed_only(
                &row,
                &holon_api::render_requirements::RenderRequirements::none(),
            );
            let is_program = match computed.get("is_program") {
                Some(Value::Boolean(b)) => *b,
                other => panic!("is_program for {id} must be a Boolean, got {other:?}"),
            };
            assert_eq!(
                is_program, expect_program,
                "the yaml's is_program must have the old SQL predicate's extension for {id}"
            );

            let moved = move_block(&engine, id, "block:pageB", None).await;
            match (expect_program, moved) {
                (true, Err(e)) => {
                    let e = format!("{e:#}");
                    assert!(
                        e.contains("machinery_containment"),
                        "{id} must refuse with the machinery class: {e}"
                    );
                }
                (true, Ok(())) => panic!("{id} is program — its move must be refused"),
                (false, Ok(())) => {}
                // Other layers may still refuse (`block:pageA` is a root block
                // move_block rejects outright) — the probe only pins that the
                // GUARD never classed a non-program block as machinery.
                (false, Err(e)) => {
                    let e = format!("{e:#}");
                    assert!(
                        !e.contains("machinery_containment"),
                        "{id} is not program — the guard must not refuse it as machinery: {e}"
                    );
                }
            }
        }
    });
}
