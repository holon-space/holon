use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use holon_api::render_eval::{
    column_ref_name, eval_binary_op, eval_to_value, resolve_args, resolve_args_with, OutlineTree,
    ResolvedArgs, ValueFnLookup, CORE_VALUE_FN_LOOKUP,
};
use holon_api::render_types::{OperationWiring, RenderExpr};
use holon_api::widget_spec::DataRow;
use holon_api::{EntityUri, InterpValue, Value};

/// Trait for attaching entity data to a widget node.
/// Both `ViewModel` and `ReactiveViewModel` implement this.
pub trait WithEntity {
    fn attach_entity(&mut self, entity: std::sync::Arc<DataRow>);
}

use crate::reactive::BuilderServices;
use crate::RenderContext;

/// Arguments passed to every builder during interpretation.
///
/// Builders read their own configuration from `args`, access the current data context
/// via `ctx`, call `interpret` to recursively render sub-expressions, and access
/// `services` for profile resolution, block data, etc.
///
/// `services` is separate from `ctx` so that `RenderContext` stays a pure data struct
/// (no lifetimes, no Arc) that frontends can store freely.
pub struct BuilderArgs<'a, W> {
    pub args: &'a ResolvedArgs,
    pub ctx: &'a RenderContext,
    pub services: &'a dyn BuilderServices,
    /// Recursion handle — interpret a `RenderExpr` in a given context.
    /// The closure captures `services` internally, so callers just pass (expr, ctx).
    pub interpret: &'a dyn Fn(&RenderExpr, &RenderContext) -> W,
}

/// A single widget builder that knows how to produce a widget of type `W`.
///
/// Builders are registered by name in the `RenderInterpreter` and dispatched
/// when the interpreter encounters a matching `FunctionCall` (or a synthetic
/// dispatch for leaf `RenderExpr` variants).
pub trait Builder<W>: Send + Sync {
    fn build(&self, ba: BuilderArgs<'_, W>) -> W;
}

/// Blanket impl: any matching function is a Builder.
impl<W, F> Builder<W> for F
where
    F: Fn(BuilderArgs<'_, W>) -> W + Send + Sync,
{
    fn build(&self, ba: BuilderArgs<'_, W>) -> W {
        (self)(ba)
    }
}

// ── Value functions ──────────────────────────────────────────────────────
//
// A *value function* is a function-call in the render DSL that produces a
// plain scalar or a reactive row set — it is NOT a widget. Registered in
// the same `RenderInterpreter` under a disjoint name space: a given name
// is either a widget builder or a value function, never both.
//
// Arg evaluation (`resolve_args_with`) dispatches `FunctionCall` nodes
// into the value-fn registry via a short-lived `ValueFnBinding` that
// carries `&services` and `&ctx` — the slice of interpreter state a
// value fn needs.

/// A registered render-DSL function whose return type is `InterpValue`
/// (a scalar `Value` or a reactive `Rows` provider).
pub trait ValueFn: Send + Sync {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        ctx: &RenderContext,
    ) -> InterpValue;
}

/// Blanket impl so plain `fn`-style registrations work.
impl<F> ValueFn for F
where
    F: Fn(&ResolvedArgs, &dyn BuilderServices, &RenderContext) -> InterpValue + Send + Sync,
{
    fn invoke(
        &self,
        args: &ResolvedArgs,
        services: &dyn BuilderServices,
        ctx: &RenderContext,
    ) -> InterpValue {
        (self)(args, services, ctx)
    }
}

/// Short-lived `ValueFnLookup` that captures the services + ctx a
/// value-fn needs. Constructed fresh at the top of `interpret()` and
/// passed to `resolve_args_with`.
struct ValueFnBinding<'a> {
    fns: &'a HashMap<String, Arc<dyn ValueFn>>,
    services: &'a dyn BuilderServices,
    ctx: &'a RenderContext,
}

impl<'a> ValueFnLookup for ValueFnBinding<'a> {
    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<InterpValue> {
        // User-supplied registry first, then built-in core fns (`concat`,
        // ...). Keeps `concat` working from any DSL context regardless of
        // whether a frontend explicitly registered it.
        self.fns
            .get(name)
            .map(|f| f.invoke(args, self.services, self.ctx))
            .or_else(|| CORE_VALUE_FN_LOOKUP.invoke(name, args))
    }
}

const MAX_QUERY_DEPTH: usize = 10;

/// Post-build hook that tags widgets with accessibility/test IDs.
///
/// Called after every `dispatch()` with the produced widget, builder name,
/// and render context. Frontends use this to attach element IDs from the
/// row data (e.g. `ctx.row().get("id")`) so geometry queries and automated
/// tests can locate widgets by entity ID.
pub type AnnotatorFn<W> = Box<dyn Fn(W, &str, &RenderContext) -> W + Send + Sync>;

/// Generic render interpreter parameterised over the widget type `W`.
///
/// All `RenderExpr` variants are dispatched to registered builders:
/// - `FunctionCall { name, .. }` → builder registered under `name`
/// - `ColumnRef` / `Literal` / `BinaryOp` → dispatched to `"text"` builder
/// - `Array` / `Object` → dispatched to `"column"` builder
/// - `LiveBlock` → dispatched to `"live_block"` builder
///
/// The set of registered builder names is the authoritative list of widgets
/// this frontend supports, accessible via `supported_widgets()`.
pub struct RenderInterpreter<W: 'static> {
    builders: HashMap<String, Box<dyn Builder<W>>>,
    /// Disjoint registry of value functions (e.g. `focus_chain()`,
    /// `ops_of(uri)`). Dispatched during arg evaluation — see
    /// `ValueFnBinding` above.
    value_fns: HashMap<String, Arc<dyn ValueFn>>,
    annotator: Option<AnnotatorFn<W>>,
}

impl<W> std::fmt::Debug for RenderInterpreter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderInterpreter")
            .field("builders", &self.builders.keys().collect::<Vec<_>>())
            .field("value_fns", &self.value_fns.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl<W> RenderInterpreter<W> {
    pub fn new() -> Self {
        Self {
            builders: HashMap::new(),
            value_fns: HashMap::new(),
            annotator: None,
        }
    }

    pub fn register(&mut self, name: impl Into<String>, builder: impl Builder<W> + 'static) {
        let n = name.into();
        if self.value_fns.contains_key(&n) {
            panic!(
                "cannot register widget builder '{n}': a value function is already registered \
                 under that name"
            );
        }
        self.builders.insert(n, Box::new(builder));
    }

    /// Register a value function — a DSL name that evaluates to an
    /// `InterpValue` (scalar or reactive row set) rather than a widget.
    /// Panics on name collision with an existing widget builder.
    pub fn register_value_fn(&mut self, name: impl Into<String>, f: impl ValueFn + 'static) {
        let n = name.into();
        if self.builders.contains_key(&n) {
            panic!(
                "cannot register value function '{n}': a widget builder is already registered \
                 under that name"
            );
        }
        self.value_fns.insert(n, Arc::new(f));
    }

    /// Set a post-build annotator that tags every widget with test/accessibility IDs.
    pub fn set_annotator(
        &mut self,
        f: impl Fn(W, &str, &RenderContext) -> W + Send + Sync + 'static,
    ) {
        self.annotator = Some(Box::new(f));
    }

    /// The set of widget names this interpreter can render.
    /// Feed this into `UiInfo` so the backend knows what widgets to emit.
    pub fn supported_widgets(&self) -> HashSet<String> {
        self.builders.keys().cloned().collect()
    }

    /// All DSL function names (builders + value functions).
    pub fn dsl_names(&self) -> Vec<String> {
        self.builders
            .keys()
            .chain(self.value_fns.keys())
            .cloned()
            .collect()
    }

    /// Parse a render DSL string using this interpreter's registered names.
    pub fn parse_dsl(&self, source: &str) -> anyhow::Result<holon_api::render_types::RenderExpr> {
        let names = self.dsl_names();
        let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        holon::render_dsl::parse_render_dsl_with_names(source, &name_refs)
    }

    #[tracing::instrument(level = "debug", skip_all)]
    pub fn interpret(
        &self,
        expr: &RenderExpr,
        ctx: &RenderContext,
        services: &dyn BuilderServices,
    ) -> W {
        let interpret_fn = |e: &RenderExpr, c: &RenderContext| self.interpret(e, c, services);

        match expr {
            RenderExpr::FunctionCall { name, args } => {
                // Bind the value-fn registry so `resolve_args_with` can
                // dispatch `FunctionCall` arg expressions (e.g.
                // `collection: focus_chain()`) through it.
                let binding = ValueFnBinding {
                    fns: &self.value_fns,
                    services,
                    ctx,
                };
                let resolved = resolve_args_with(args, ctx.row(), &binding);
                self.dispatch(name, &resolved, ctx, services, &interpret_fn)
            }
            RenderExpr::ColumnRef { name } => {
                let value = ctx.row().get(name).cloned().unwrap_or(Value::Null);
                let args = ResolvedArgs::from_positional_value(value);
                self.dispatch("text", &args, ctx, services, &interpret_fn)
            }
            RenderExpr::Literal { value } => {
                let args = ResolvedArgs::from_positional_value(value.clone());
                self.dispatch("text", &args, ctx, services, &interpret_fn)
            }
            RenderExpr::BinaryOp { op, left, right } => {
                let l = eval_to_value(left, ctx.row());
                let r = eval_to_value(right, ctx.row());
                let result = eval_binary_op(op, &l, &r);
                let args = ResolvedArgs::from_positional_value(result);
                self.dispatch("text", &args, ctx, services, &interpret_fn)
            }
            RenderExpr::Array { items } => {
                let args = ResolvedArgs::from_positional_exprs(items.clone());
                self.dispatch("column", &args, ctx, services, &interpret_fn)
            }
            RenderExpr::Object { fields } => {
                let exprs: Vec<_> = fields.iter().map(|(_, e)| e.clone()).collect();
                let args = ResolvedArgs::from_positional_exprs(exprs);
                self.dispatch("column", &args, ctx, services, &interpret_fn)
            }
            RenderExpr::LiveBlock { block_id } => {
                let args = ResolvedArgs::from_positional_value(Value::String(block_id.clone()));
                self.dispatch("live_block", &args, ctx, services, &interpret_fn)
            }
        }
    }

    fn dispatch(
        &self,
        name: &str,
        args: &ResolvedArgs,
        ctx: &RenderContext,
        services: &dyn BuilderServices,
        interpret_fn: &dyn Fn(&RenderExpr, &RenderContext) -> W,
    ) -> W {
        let widget = match self.builders.get(name) {
            Some(builder) => builder.build(BuilderArgs {
                args,
                ctx,
                services,
                interpret: interpret_fn,
            }),
            None => {
                tracing::warn!("No builder registered for: {name}");
                let fallback_args = ResolvedArgs::from_positional_value(Value::String(format!(
                    "[unknown: {name}]"
                )));
                self.builders
                    .get("text")
                    .expect("'text' builder must be registered")
                    .build(BuilderArgs {
                        args: &fallback_args,
                        ctx,
                        services,
                        interpret: interpret_fn,
                    })
            }
        };
        match &self.annotator {
            Some(annotate) => annotate(widget, name, ctx),
            None => widget,
        }
    }
}

// =========================================================================
// Widget classification
// =========================================================================

/// Returns `true` for widgets whose `build` output is fully determined by
/// their props (String, bool, f64, etc.) — no structural children, no
/// collection driver, no side-effect wiring.
///
/// These widgets are eligible for the `resolve_props` fast path, which
/// skips the full `services.interpret()` pipeline when recomputing props
/// on data changes.
pub fn is_props_only_widget(widget_name: &str) -> bool {
    matches!(
        widget_name,
        "text"
            | "badge"
            | "icon"
            | "checkbox"
            | "spacer"
            | "editable_text"
            | "state_toggle"
            | "source_block"
            | "source_editor"
            | "block_operations"
            | "op_button"
            | "table_row"
            | "pref_field"
    )
}

/// Fast-path props extraction for `is_props_only_widget` builders.
///
/// Resolves args from the expression, builds a `BuilderArgs`, and dispatches
/// to the builder's macro-generated `resolve_props_from_args`. For raw
/// builders that lack a macro-generated function, falls back to
/// `services.interpret()` and extracts the resulting props.
pub fn resolve_props(
    widget_name: &str,
    expr: &RenderExpr,
    data: &Arc<DataRow>,
    services: &dyn BuilderServices,
    space: Option<crate::render_context::AvailableSpace>,
) -> HashMap<String, Value> {
    use crate::reactive_view::row_render_context;
    use crate::reactive_view_model::ReactiveViewModel;

    let ctx = row_render_context(data.clone(), None, services, space);

    // Extract args from FunctionCall; other expr variants have no args.
    let args = match expr {
        RenderExpr::FunctionCall { args, .. } => resolve_args(args, ctx.row()),
        _ => ResolvedArgs::from_positional_exprs(vec![]),
    };

    // Dummy interpret closure — props_only builders never recurse.
    let noop_interpret = |_e: &RenderExpr, _c: &RenderContext| -> ReactiveViewModel {
        ReactiveViewModel::from_widget("_unreachable", HashMap::new())
    };

    let ba = BuilderArgs {
        args: &args,
        ctx: &ctx,
        services,
        interpret: &noop_interpret,
    };

    // Try the macro-generated fast path first.
    if let Some(props) = crate::shadow_builders::dispatch_resolve_props(widget_name, &ba) {
        return props;
    }

    // Fallback for raw builders: full interpret, extract props.
    let fresh = services.interpret(expr, &ctx);
    fresh.props.get_cloned()
}

// =========================================================================
// Shared builders — framework-independent, work for any W
// =========================================================================

/// `col` builder: interprets each positional expr and collects into a vertical list.
///
/// Frontends wrap this by providing their own `col` that calls `shared_col_build`
/// and then wraps the resulting `Vec<W>` in their framework's vstack equivalent.
pub fn shared_col_build<W>(ba: &BuilderArgs<'_, W>) -> Vec<W> {
    ba.args
        .positional_exprs
        .iter()
        .map(|expr| (ba.interpret)(expr, ba.ctx))
        .collect()
}

/// `tree` builder: interprets rows as a hierarchical tree using `parent_id` and `sortkey`.
///
/// Uses `OutlineTree` to build parent-child relationships, then walks depth-first.
/// Returns `Vec<(W, usize)>` — each widget paired with its nesting depth.
/// Frontends wrap each `(widget, depth)` in their own indentation container.
pub fn shared_tree_build<W: WithEntity>(ba: &BuilderArgs<'_, W>) -> Vec<(W, usize)> {
    let template = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"));

    let Some(tmpl) = template else {
        return vec![];
    };

    let rows = &ba.ctx.data_rows;
    if rows.is_empty() {
        return vec![((ba.interpret)(tmpl, ba.ctx), 0)];
    }

    let parent_id_col = ba
        .args
        .get_template("parent_id")
        .and_then(column_ref_name)
        .unwrap_or("parent_id");
    let sort_col = ba
        .args
        .get_template("sortkey")
        .or(ba.args.get_template("sort_key"))
        .and_then(column_ref_name)
        .unwrap_or("sort_key");

    let tree = OutlineTree::from_rows(rows, parent_id_col, sort_col);
    tree.walk_depth_first(|resolved_row, depth| {
        let row_ctx = ba.ctx.with_row(Arc::clone(resolved_row));
        let row_ctx = RenderContext {
            depth: row_ctx.depth + depth,
            ..row_ctx
        };
        let mut node = (ba.interpret)(tmpl, &row_ctx);
        // Attach entity data to the outermost node so navigators and
        // tree_item wrappers can find the entity_id directly.
        node.attach_entity(Arc::clone(resolved_row));
        (node, depth)
    })
}

/// `live_block` builder: fetches a block's WidgetSpec and recursively interprets it.
///
/// Works for any `W` because it delegates all widget construction to `interpret`.
/// The only framework-specific part is error rendering, which falls back to `text`.
pub fn shared_live_block_build<W>(ba: &BuilderArgs<'_, W>) -> Result<W, String> {
    let block_id = ba
        .args
        .get_positional_string(0)
        .or_else(|| {
            ba.ctx
                .row()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .map(|s| EntityUri::parse(&s).expect("live_block: invalid entity URI"))
        .expect("live_block: no positional arg and no 'id' column in current row");

    if ba.ctx.query_depth >= MAX_QUERY_DEPTH {
        return Err(format!(
            "[live_block recursion limit reached (depth {})]",
            ba.ctx.query_depth
        ));
    }

    let deeper = ba.ctx.deeper_query();

    let (render_expr, data_rows) = ba.services.get_block_data(&block_id);
    let child_ctx = deeper.with_data_rows(data_rows);
    Ok((ba.interpret)(&render_expr, &child_ctx))
}

/// Result of a live query build, carrying both the interpreted content and metadata
/// needed for reactive subscriptions.
pub struct LiveQueryResult<W> {
    pub content: W,
    pub compiled_sql: String,
    pub query_context_id: Option<String>,
    pub render_expr: holon_api::render_types::RenderExpr,
}

/// `live_query` builder: compiles + executes a query, then interprets the result.
///
/// Returns `Ok(LiveQueryResult)` on success or `Err(message)` for the frontend to render as error text.
pub fn shared_live_query_build<W>(ba: &BuilderArgs<'_, W>) -> Result<LiveQueryResult<W>, String> {
    use holon_api::QueryLanguage;

    if ba.ctx.query_depth >= MAX_QUERY_DEPTH {
        return Err(format!(
            "[query recursion limit reached (depth {})]",
            ba.ctx.query_depth
        ));
    }

    let (query, language) = if let Some(gql) = ba.args.get_string("gql") {
        (gql.to_string(), QueryLanguage::HolonGql)
    } else if let Some(sql) = ba.args.get_string("sql") {
        (sql.to_string(), QueryLanguage::HolonSql)
    } else {
        (
            ba.args.get_string("prql").unwrap_or("").to_string(),
            QueryLanguage::HolonPrql,
        )
    };

    if query.is_empty() {
        return Err("[empty query]".to_string());
    }

    let query = if language != QueryLanguage::HolonPrql {
        ba.services
            .compile_to_sql(&query, language)
            .map_err(|e| format!("Query compile error: {e}"))?
    } else {
        query
    };

    let context_id = ba
        .args
        .get_string("context")
        .map(|s| s.to_string())
        .or_else(|| {
            ba.ctx
                .row()
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        });

    let query_context = context_id.as_ref().map(|id| {
        let uri = holon_api::EntityUri::from_raw(id);
        crate::QueryContext {
            current_block_id: Some(uri.clone()),
            context_parent_id: Some(uri),
            context_path_prefix: None,
        }
    });

    let sql = ba
        .services
        .compile_to_sql(&query, QueryLanguage::HolonPrql)
        .unwrap_or_else(|_| query.clone());

    let compiled_sql = sql.clone();
    let result = ba.services.start_query(sql, query_context);

    let deeper_ctx = ba.ctx.deeper_query();

    // The render expression for interpreting query results comes from the
    // builder args (e.g., item_template), not from the query itself.
    // Default to table() when no template is specified.
    let live_query_render_expr = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
        .cloned()
        .unwrap_or_else(|| holon_api::render_types::RenderExpr::FunctionCall {
            name: "table".to_string(),
            args: vec![],
        });

    // Resolve `virtual_parent: true` → `virtual_parent: "<context_id>"`.
    // The DSL author opts into virtual children by writing `virtual_parent: true`
    // on a collection expression. We resolve the sentinel to the actual parent ID
    // here, so the stored expression survives signal re-interpretation.
    let live_query_render_expr = resolve_virtual_parent(live_query_render_expr, &context_id);

    match result {
        Ok(_stream) => {
            let child_ctx = deeper_ctx.with_data_rows(vec![]);
            let content = (ba.interpret)(&live_query_render_expr, &child_ctx);
            Ok(LiveQueryResult {
                content,
                compiled_sql,
                query_context_id: context_id,
                render_expr: live_query_render_expr,
            })
        }
        Err(e) => Err(format!("Query error: {e}")),
    }
}

/// Resolve `virtual_parent: true` sentinels in a render expression.
///
/// Walks one level deep into a `FunctionCall`'s named args. When it finds
/// `virtual_parent` set to `Literal(Bool(true))`, replaces it with the
/// live_query's context_id. If no context_id is available, removes the arg.
fn resolve_virtual_parent(expr: RenderExpr, context_id: &Option<String>) -> RenderExpr {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            let args = args
                .into_iter()
                .filter_map(|arg| {
                    if arg.name.as_deref() == Some("virtual_parent") {
                        match &arg.value {
                            RenderExpr::Literal {
                                value: Value::Boolean(true),
                            } => context_id.as_ref().map(|id| holon_api::render_types::Arg {
                                name: Some("virtual_parent".to_string()),
                                value: RenderExpr::Literal {
                                    value: Value::String(id.clone()),
                                },
                            }),
                            _ => Some(arg),
                        }
                    } else {
                        Some(arg)
                    }
                })
                .collect();
            RenderExpr::FunctionCall { name, args }
        }
        other => other,
    }
}

/// `render_entity` builder: dispatches based on content_type/source_language in the current row.
///
/// For query-language source blocks, fetches + recurses via live_block.
/// Returns `Ok(W)` or `Err(message)`.
pub fn shared_render_entity_build<W>(ba: &BuilderArgs<'_, W>) -> RenderBlockResult {
    if !ba.ctx.row().contains_key("id") {
        return RenderBlockResult::Empty;
    }

    // Profile/variant resolution — works for any entity type.
    // The profile resolver derives entity type from the row ID's URI scheme
    // (e.g., "block:xyz" → block profiles, "cc-project:xyz" → cc-project profiles).
    // All rendering behavior (source blocks, text editing, query blocks) is defined
    // as variants in entity profile YAML — no hardcoded content_type matching.
    let profile = ba.services.resolve_profile(ba.ctx.row());
    let ops: Vec<OperationWiring> = profile
        .as_ref()
        .map(|p| {
            p.operations
                .iter()
                .cloned()
                .map(|d| d.to_default_wiring())
                .collect()
        })
        .unwrap_or_default();

    match profile {
        Some(ref p) => {
            let active_render = pick_active_variant(p, ba.ctx, ba.services);
            RenderBlockResult::ProfileWidget {
                render: active_render,
                operations: ops,
            }
        }
        None => RenderBlockResult::Empty,
    }
}

/// Pick the active render expression from a profile's variant candidates.
///
/// If the profile has multi-variant candidates, evaluates each candidate's
/// `condition` predicate against the current UI state (focus, view mode).
/// Returns the first matching candidate's render expression, or falls back
/// to the profile's default render.
fn pick_active_variant(
    profile: &holon::entity_profile::RowProfile,
    ctx: &RenderContext,
    services: &dyn BuilderServices,
) -> RenderExpr {
    if profile.variants.is_empty() {
        return profile.render.clone();
    }

    // Get block ID for UI state lookup
    let block_id = ctx
        .row()
        .get("id")
        .and_then(|v| v.as_string())
        .map(|s| EntityUri::from_raw(s));

    let mut ui_state = match block_id {
        Some(ref id) => services.ui_state(id),
        None => HashMap::new(),
    };

    // Merge container-query allocation AFTER services.ui_state so per-subtree
    // refinement shadows any global viewport fallback stored in UiState.
    if let Some(space) = ctx.available_space {
        ui_state.insert(
            "available_width_px".to_string(),
            holon_api::Value::Float(space.width_px as f64),
        );
        ui_state.insert(
            "available_height_px".to_string(),
            holon_api::Value::Float(space.height_px as f64),
        );
        ui_state.insert(
            "available_width_physical_px".to_string(),
            holon_api::Value::Float(space.width_physical_px as f64),
        );
        ui_state.insert(
            "available_height_physical_px".to_string(),
            holon_api::Value::Float(space.height_physical_px as f64),
        );
        ui_state.insert(
            "scale_factor".to_string(),
            holon_api::Value::Float(space.scale_factor as f64),
        );
    }

    // Find first variant whose condition matches
    for variant in &profile.variants {
        if variant.condition.evaluate(&ui_state) {
            return variant.render.clone();
        }
    }

    profile.render.clone()
}

/// Result of `shared_render_entity_build` — all rendering behavior is now driven
/// by entity profile variants defined in YAML.
pub enum RenderBlockResult {
    /// The row has a profile with a render expression + operations — interpret it.
    ProfileWidget {
        render: RenderExpr,
        operations: Vec<OperationWiring>,
    },
    /// No matching profile/variant — frontend renders nothing.
    Empty,
    /// Error message — frontend renders as error text.
    Error(String),
}
