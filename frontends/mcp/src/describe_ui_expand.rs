//! Resolution of the subtrees the pure render interpreter leaves deferred.
//!
//! `interpret` is synchronous, so for a `live_query` it validates the query and
//! emits a SELF-DESCRIBING node whose `content` is a structural prototype built
//! from no rows; the platform layer starts the real watcher from the node's
//! props. `describe_ui` has no platform layer, so without this pass it reports
//! that prototype as if it were the result — a working widget rendered as
//! broken (BugFunnel 2026-08-02).
//!
//! This pass runs over the finished snapshot: it either resolves a deferred
//! subtree for real or replaces it with an explicit [`ViewKind::Unevaluated`]
//! marker. It never leaves the silent prototype in place.

use std::future::Future;
use std::pin::Pin;

use holon_api::render_types::RenderExpr;
use holon_frontend::view_model::DeferredMechanism;
use holon_frontend::view_model::ViewKind;
use holon_frontend::view_model::ViewModel;

/// Nested `live_query` expansion is bounded: a template whose rows contain the
/// same query would otherwise recurse forever. Subtrees past the cap are marked
/// unevaluated rather than silently dropped.
const MAX_EXPANSION_DEPTH: usize = 8;

/// Depth alone does not bound WORK — total expansions are `Σ bᵏ` in the
/// branching factor, and for the ordinary per-row nested query
/// (`live_query(item_template: live_query(context: col("id")))`) that factor is
/// the outer query's ROW COUNT. 256 comfortably covers the realistic shapes (a
/// 100-row outer query with one inner query per row is 101) while refusing the
/// combinatorial ones.
const MAX_TOTAL_EXPANSIONS: usize = 256;

/// Matches the `await_ready` timeout `describe_ui` already applies, so the
/// tool's worst case stays bounded at roughly twice one readiness wait.
const EXPANSION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Bounds the TOTAL work one `resolve_deferred` pass may do — a running count
/// across the whole tree, not per branch — plus a wall clock for the case where
/// few queries are each slow.
pub struct ExpansionBudget {
    remaining: std::sync::atomic::AtomicUsize,
    deadline: std::time::Instant,
}

impl ExpansionBudget {
    pub fn new(max_expansions: usize, wall_clock: std::time::Duration) -> Self {
        Self {
            remaining: std::sync::atomic::AtomicUsize::new(max_expansions),
            deadline: std::time::Instant::now() + wall_clock,
        }
    }

    /// Claim one expansion, or explain why the budget refused it. The message
    /// becomes the `Unevaluated` reason, so exhaustion is always disclosed.
    fn claim(&self) -> Result<(), String> {
        if std::time::Instant::now() > self.deadline {
            return Err(format!(
                "the {}s expansion deadline for this describe_ui call elapsed",
                EXPANSION_DEADLINE.as_secs()
            ));
        }
        self.remaining
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_sub(1),
            )
            .map(|_| ())
            .map_err(|_| {
                format!(
                    "this describe_ui call's budget of {MAX_TOTAL_EXPANSIONS} total live_query \
                     expansions is exhausted"
                )
            })
    }
}

impl Default for ExpansionBudget {
    fn default() -> Self {
        Self::new(MAX_TOTAL_EXPANSIONS, EXPANSION_DEADLINE)
    }
}

/// A `live_query` node's self-description — everything needed to resolve its
/// rows without re-entering the interpreter's query plumbing.
pub struct LiveQuerySpec<'a> {
    pub query: &'a str,
    pub query_lang: &'a str,
    pub query_context_id: Option<&'a str>,
    pub render_expr: &'a RenderExpr,
}

/// Resolves a deferred subtree against a live backend. A trait so the traversal
/// is testable without one.
pub trait DeferredResolver: Send + Sync {
    /// Run the query ONCE and interpret `render_expr` against its rows. Must
    /// use a snapshot query path — `describe_ui` must not leave a watcher
    /// running behind it.
    fn expand_live_query<'a>(
        &'a self,
        spec: LiveQuerySpec<'a>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ViewModel>> + Send + 'a>>;
}

/// What `describe_ui` does with subtrees the interpreter deferred.
#[derive(Clone, Copy)]
pub enum DeferredPolicy<'a> {
    /// Resolve them against the backend.
    Expand(&'a dyn DeferredResolver),
    /// Leave them unresolved — but mark every one.
    MarkOnly,
}

/// The production resolver: the engine's one-shot query path plus the same
/// interpretation the live watcher does — `render_expr` interpreted ONCE with
/// every row bound into the context (mirrors `watch_query_live`), minus the
/// subscription.
pub struct EngineResolver {
    pub services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
}

impl DeferredResolver for EngineResolver {
    fn expand_live_query<'a>(
        &'a self,
        spec: LiveQuerySpec<'a>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ViewModel>> + Send + 'a>> {
        Box::pin(async move {
            let engine = self.services.query_engine().ok_or_else(|| {
                anyhow::anyhow!("this session has no query engine (no-Turso frontend)")
            })?;
            let language: holon_api::QueryLanguage = spec.query_lang.parse().map_err(|e| {
                anyhow::anyhow!("unsupported query_lang '{}': {e}", spec.query_lang)
            })?;
            let context = spec.query_context_id.map(|id| {
                // ALLOW(entity_uri_from_raw): live_query node prop, same as the
                // gpui builder's own reconstruction
                let uri = holon_api::EntityUri::from_raw(id);
                holon_frontend::QueryContext {
                    current_block_id: Some(uri.clone()),
                    context_parent_id: Some(uri),
                    context_path_prefix: None,
                }
            });

            let rows = engine
                .execute_query(spec.query, language, Default::default(), context)
                .await?;
            // `interpret` is synchronous and a nested live_query inside
            // `render_expr` reaches the engine's `watch_query`, which blocks on
            // a scoped thread — that must not run on a tokio worker.
            let services = self.services.clone();
            let render_expr = spec.render_expr.clone();
            tokio::task::spawn_blocking(move || {
                let render_ctx = holon_frontend::RenderContext {
                    data_rows: rows.into_iter().map(std::sync::Arc::new).collect(),
                    ..Default::default()
                };
                services.interpret(&render_expr, &render_ctx).snapshot()
            })
            .await
            .map_err(|e| anyhow::anyhow!("render_expr interpretation panicked: {e}"))
        })
    }
}

fn unevaluated(mechanism: DeferredMechanism, reason: impl Into<String>) -> ViewModel {
    ViewModel {
        kind: ViewKind::Unevaluated {
            mechanism,
            reason: reason.into(),
        },
        ..ViewModel::empty()
    }
}

fn ends_with_marker(items: &[ViewModel], mechanism: DeferredMechanism) -> bool {
    matches!(
        items.last().map(|vm| &vm.kind),
        Some(ViewKind::Unevaluated { mechanism: m, .. }) if *m == mechanism
    )
}

fn error_node(message: String) -> ViewModel {
    ViewModel {
        kind: ViewKind::Error { message },
        ..ViewModel::empty()
    }
}

/// Rewrite every deferred subtree of `vm` in place, under the default budget.
pub async fn resolve_deferred(vm: &mut ViewModel, policy: DeferredPolicy<'_>) {
    resolve_deferred_within(vm, policy, &ExpansionBudget::default()).await
}

/// [`resolve_deferred`] with an explicit budget.
pub async fn resolve_deferred_within(
    vm: &mut ViewModel,
    policy: DeferredPolicy<'_>,
    budget: &ExpansionBudget,
) {
    walk(vm, policy, 0, budget).await
}

/// `query_depth` counts nested live_query EXPANSIONS, not tree depth — a
/// legitimate query sitting deep under layout wrappers must still expand.
fn walk<'a>(
    vm: &'a mut ViewModel,
    policy: DeferredPolicy<'a>,
    query_depth: usize,
    budget: &'a ExpansionBudget,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        match &mut vm.kind {
            ViewKind::LiveQuery {
                content,
                query,
                query_lang,
                query_context_id,
                render_expr,
            } => {
                **content = resolve_live_query(
                    query.as_deref(),
                    query_lang.as_deref(),
                    query_context_id.as_deref(),
                    render_expr.as_ref(),
                    policy,
                    query_depth,
                    budget,
                )
                .await;
                walk(content, policy, query_depth + 1, budget).await;
                return;
            }
            // The content thunk lives on the `ReactiveViewModel`, which the
            // snapshot has already discarded — this pass can only mark it.
            // `content_deferred` stays set: it is honest metadata about the
            // node, so re-running the pass must not append a second marker.
            ViewKind::ExpandToggle {
                content_deferred: true,
                children,
                ..
            } if !ends_with_marker(&children.items, DeferredMechanism::ExpandToggleContent) => {
                children.items.push(unevaluated(
                    DeferredMechanism::ExpandToggleContent,
                    "content thunk not forced: the expand_toggle gate is closed and the thunk \
                     does not survive into the snapshot",
                ));
            }
            _ => {}
        }

        for child in vm.children_mut() {
            walk(child, policy, query_depth, budget).await;
        }
    })
}

async fn resolve_live_query(
    query: Option<&str>,
    query_lang: Option<&str>,
    query_context_id: Option<&str>,
    render_expr: Option<&RenderExpr>,
    policy: DeferredPolicy<'_>,
    query_depth: usize,
    budget: &ExpansionBudget,
) -> ViewModel {
    let resolver = match policy {
        DeferredPolicy::MarkOnly => {
            return unevaluated(
                DeferredMechanism::LiveQueryRows,
                "rows not evaluated: describe_ui was called with expand_deferred=false",
            );
        }
        DeferredPolicy::Expand(r) => r,
    };

    if query_depth >= MAX_EXPANSION_DEPTH {
        return unevaluated(
            DeferredMechanism::LiveQueryRows,
            format!(
                "rows not evaluated: nested live_query expansion exceeds the \
                 {MAX_EXPANSION_DEPTH}-level cap"
            ),
        );
    }

    let (Some(query), Some(query_lang), Some(render_expr)) = (query, query_lang, render_expr)
    else {
        return unevaluated(
            DeferredMechanism::LiveQueryRows,
            "rows not evaluated: the node carries no query/query_lang/render_expr, so it cannot \
             describe its own result",
        );
    };

    // Claimed only once every precondition holds, so a refused node never
    // consumes budget another node could have used.
    if let Err(reason) = budget.claim() {
        return unevaluated(
            DeferredMechanism::LiveQueryRows,
            format!("rows not evaluated: {reason}"),
        );
    }

    let spec = LiveQuerySpec {
        query,
        query_lang,
        query_context_id,
        render_expr,
    };
    // The caller walks whatever this returns, so a nested live_query inside the
    // rows is resolved there — one query level deeper.
    match resolver.expand_live_query(spec).await {
        Ok(resolved) => resolved,
        Err(e) => error_node(format!(
            "live_query expansion FAILED ({query_lang}): {e:#} — query: {query}"
        )),
    }
}
