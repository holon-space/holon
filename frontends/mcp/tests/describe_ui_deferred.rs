//! `describe_ui` must never report a deferred subtree as empty content.
//!
//! The render interpreter is pure and synchronous, so for a `live_query` it
//! emits a self-describing node whose `content` is a prototype built from NO
//! rows; the platform layer starts the real watcher from the node's props.
//! `describe_ui` has no platform layer, so it used to hand that prototype back
//! as though it were the result — a working widget rendered as broken, which
//! caused a real misdiagnosis (BugFunnel 2026-08-02, PERCEPTION row).
//!
//! Two guarantees, one per increment:
//!  1. NOT expanded ⇒ explicitly marked `unevaluated` (never a silent empty).
//!  2. Expanded ⇒ the query's real rows, and a query FAILURE surfaces as an
//!     error naming the failure rather than falling back to the placeholder.
//!
//! Every expansion case seeds real rows and control-asserts that the very same
//! query returns them through the engine, so a red here means the rows are
//! missing from the RENDER, not from the fixture.
//!
//! @pbt kind harness
//! @pbt covers describe-ui-deferred — deferred subtrees are either resolved or
//! explicitly marked unevaluated, never reported as empty content
//! @pbt overlaps integrations_section_seed — kept: that harness asserts the
//! seeded expression's string shape and the raw SQL result, never the rendered
//! output

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Module;
use fluxdi::Provider;
use holon_api::render_types::RenderExpr;
use holon_frontend::view_model::ViewKind;
use holon_frontend::view_model::ViewModel;
use holon_mcp::describe_ui_expand::DeferredPolicy;
use holon_mcp::describe_ui_expand::EngineResolver;
use holon_mcp::describe_ui_expand::resolve_deferred;

const PROVIDER_QUERY: &str =
    "SELECT provider_name, updated_at FROM sync_states ORDER BY provider_name ASC";

/// Mirrors `crates/holon-app/tests/integrations_section_seed.rs` —
/// `sync_states` is an eager schema root of the `BackendEngine` factory, so the
/// query below has a real table to read.
async fn fresh_engine(db_path: std::path::PathBuf) -> Arc<holon::api::BackendEngine> {
    holon::di::create_backend_engine_with_extras(
        db_path,
        |injector| {
            holon::sync::EventInfraModule
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
            Ok(())
        },
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .map(|(engine, _)| engine)
    .expect("fresh-db lazy DI graph must build")
}

async fn seed_providers(engine: &holon::api::BackendEngine) {
    let db = engine.db_handle();
    for (provider, token, ts) in [
        ("todoist", "tok-b", "2026-07-18 09:00:00"),
        ("claude-history", "tok-a", "2026-07-18 08:00:00"),
    ] {
        db.execute_values(
            &format!(
                "INSERT INTO sync_states (provider_name, sync_token, updated_at) \
                 VALUES ('{provider}', '{token}', '{ts}')"
            ),
            vec![],
        )
        .await
        .expect("insert sync_states fixture row");
    }
}

fn item_template() -> RenderExpr {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    holon_api::render_dsl::parse_render_dsl(r#"list(#{item_template: text(col("provider_name"))})"#)
        .expect("item template parses")
}

/// The node the pure interpreter emits: self-describing props plus a `content`
/// prototype interpreted from NO rows.
fn deferred_live_query_node(
    services: &Arc<dyn holon_frontend::reactive::BuilderServices>,
    query: &str,
) -> ViewModel {
    let render_expr = item_template();
    let prototype = services
        .interpret(&render_expr, &holon_frontend::RenderContext::default())
        .snapshot();
    ViewModel {
        kind: ViewKind::LiveQuery {
            content: Box::new(prototype),
            query: Some(query.to_string()),
            query_lang: Some("holon_sql".to_string()),
            query_context_id: None,
            render_expr: Some(render_expr),
        },
        ..ViewModel::empty()
    }
}

fn services_for(
    engine: Arc<holon::api::BackendEngine>,
) -> Arc<dyn holon_frontend::reactive::BuilderServices> {
    Arc::new(holon_app::HeadlessBuilderServices::new(engine))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// INCREMENT 3. With expansion on, the node shows the query's REAL rows.
/// Control-asserted: the same query returns those rows straight from the
/// engine, so a failure here is a rendering failure, not a missing fixture.
#[test]
fn expanded_live_query_renders_the_querys_real_rows() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = fresh_engine(dir.path().join("fresh.db")).await;
        seed_providers(&engine).await;

        // CONTROL: the fixture really is queryable through the engine.
        let control = engine
            .db_handle()
            .query(PROVIDER_QUERY, HashMap::new())
            .await
            .expect("control query");
        assert_eq!(
            control.len(),
            2,
            "fixture must supply 2 rows before the render is judged; got {control:?}"
        );

        let services = services_for(engine);
        let mut vm = deferred_live_query_node(&services, PROVIDER_QUERY);
        let resolver = EngineResolver {
            services: services.clone(),
        };
        resolve_deferred(&mut vm, DeferredPolicy::Expand(&resolver)).await;

        let rendered = vm.pretty_print(0);
        assert!(
            rendered.contains("claude-history") && rendered.contains("todoist"),
            "expanded live_query must render both seeded providers; got:\n{rendered}"
        );
    });
}

/// INCREMENT 2. With expansion off, the subtree is EXPLICITLY marked — a
/// consumer can tell "no rows" from "not evaluated". The silent prototype (an
/// empty-text placeholder) must be gone.
#[test]
fn unexpanded_live_query_is_marked_unevaluated() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = fresh_engine(dir.path().join("fresh.db")).await;
        seed_providers(&engine).await;

        let services = services_for(engine);
        let mut vm = deferred_live_query_node(&services, PROVIDER_QUERY);
        resolve_deferred(&mut vm, DeferredPolicy::MarkOnly).await;

        let rendered = vm.pretty_print(0);
        assert!(
            rendered.contains("UNEVALUATED[live_query_rows]"),
            "an unexpanded live_query must carry an explicit unevaluated marker; got:\n{rendered}"
        );
        assert!(
            !rendered.contains("list"),
            "the synthetic empty prototype must be REPLACED by the marker, not kept beside it; \
             got:\n{rendered}"
        );
    });
}

/// INCREMENT 2, second mechanism. An `expand_toggle` whose content thunk never
/// fired is marked too — the thunk does not survive into the snapshot, so this
/// pass CANNOT expand it, and saying nothing would be the same silent lie.
#[test]
fn unforced_expand_toggle_content_is_marked_unevaluated() {
    runtime().block_on(async {
        let mut vm = ViewModel {
            kind: ViewKind::ExpandToggle {
                target_id: "block:page-a".to_string(),
                expanded: false,
                content_deferred: true,
                children: holon_frontend::view_model::LazyChildren::fully_materialized(vec![]),
            },
            ..ViewModel::empty()
        };
        resolve_deferred(&mut vm, DeferredPolicy::MarkOnly).await;

        resolve_deferred(&mut vm, DeferredPolicy::MarkOnly).await;

        let rendered = vm.pretty_print(0);
        assert!(
            rendered.contains("UNEVALUATED[expand_toggle_content]"),
            "an unforced expand_toggle thunk must be marked; got:\n{rendered}"
        );
        // Marking is idempotent: `content_deferred` stays set as honest node
        // metadata, so a second pass must not append a second marker.
        assert_eq!(
            rendered
                .matches("UNEVALUATED[expand_toggle_content]")
                .count(),
            1,
            "two passes must not stack markers; got:\n{rendered}"
        );
    });
}

/// A resolver that fans out: every expansion returns `BRANCH` sibling
/// live_query nodes. Depth alone does not bound this — total work is the sum of
/// a geometric series — so the budget must.
struct FanOutResolver {
    calls: std::sync::atomic::AtomicUsize,
}

const BRANCH: usize = 3;

impl holon_mcp::describe_ui_expand::DeferredResolver for FanOutResolver {
    fn expand_live_query<'a>(
        &'a self,
        spec: holon_mcp::describe_ui_expand::LiveQuerySpec<'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<ViewModel>> + Send + 'a>>
    {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let render_expr = spec.render_expr.clone();
        let query = spec.query.to_string();
        Box::pin(async move {
            let children = (0..BRANCH)
                .map(|_| ViewModel {
                    kind: ViewKind::LiveQuery {
                        content: Box::new(ViewModel::empty()),
                        query: Some(query.clone()),
                        query_lang: Some("holon_sql".to_string()),
                        query_context_id: None,
                        render_expr: Some(render_expr.clone()),
                    },
                    ..ViewModel::empty()
                })
                .collect();
            Ok(ViewModel {
                kind: ViewKind::Column {
                    gap: 0.0,
                    children: holon_frontend::view_model::LazyChildren::fully_materialized(
                        children,
                    ),
                },
                ..ViewModel::empty()
            })
        })
    }
}

/// Total expansion WORK is bounded, not just nesting depth: a resolver fanning
/// out per expansion must terminate against the budget, and every subtree the
/// budget refused must say so with the same explicit marker.
#[test]
fn fan_out_expansion_is_bounded_and_exhaustion_is_disclosed() {
    runtime().block_on(async {
        let budget = holon_mcp::describe_ui_expand::ExpansionBudget::new(
            64,
            std::time::Duration::from_secs(30),
        );
        let resolver = FanOutResolver {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut vm = ViewModel {
            kind: ViewKind::LiveQuery {
                content: Box::new(ViewModel::empty()),
                query: Some("SELECT 1".to_string()),
                query_lang: Some("holon_sql".to_string()),
                query_context_id: None,
                render_expr: Some(item_template()),
            },
            ..ViewModel::empty()
        };

        let started = std::time::Instant::now();
        holon_mcp::describe_ui_expand::resolve_deferred_within(
            &mut vm,
            DeferredPolicy::Expand(&resolver),
            &budget,
        )
        .await;
        let elapsed = started.elapsed();

        let calls = resolver.calls.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            calls, 64,
            "the resolver must be called exactly the budgeted number of times, not {calls}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "a bounded pass must finish promptly; took {elapsed:?}"
        );

        let rendered = vm.pretty_print(0);
        assert!(
            rendered.contains("budget of") && rendered.contains("is exhausted"),
            "refused subtrees must name the budget; got:\n{rendered}"
        );
        // Every refusal is an explicit marker, never a silent empty.
        assert_eq!(
            rendered.matches("UNEVALUATED[live_query_rows]").count(),
            BRANCH * 64 - (64 - 1),
            "every live_query the budget refused must carry a marker; got:\n{rendered}"
        );
    });
}

/// A failing query must name the failure. Falling back to the empty prototype
/// would reintroduce exactly the bug this pass exists to fix.
#[test]
fn failed_expansion_surfaces_an_error_not_a_placeholder() {
    runtime().block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let engine = fresh_engine(dir.path().join("fresh.db")).await;

        let services = services_for(engine);
        let mut vm = deferred_live_query_node(&services, "SELECT * FROM table_that_does_not_exist");
        let resolver = EngineResolver {
            services: services.clone(),
        };
        resolve_deferred(&mut vm, DeferredPolicy::Expand(&resolver)).await;

        let rendered = vm.pretty_print(0);
        assert!(
            rendered.contains("live_query expansion FAILED")
                && rendered.contains("table_that_does_not_exist"),
            "a failed expansion must surface an error naming the query; got:\n{rendered}"
        );
    });
}
