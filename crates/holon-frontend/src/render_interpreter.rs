use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::InterpValue;
use holon_api::Value;
use holon_api::render_eval::CORE_VALUE_FN_LOOKUP;
use holon_api::render_eval::OutlineTree;
use holon_api::render_eval::ResolvedArgs;
use holon_api::render_eval::ValueFnLookup;
use holon_api::render_eval::column_ref_name;
use holon_api::render_eval::eval_binary_op;
use holon_api::render_eval::eval_to_value;
use holon_api::render_eval::resolve_args;
use holon_api::render_eval::resolve_args_for_widget;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;

/// Trait for attaching entity data to a widget node.
/// Both `ViewModel` and `ReactiveViewModel` implement this.
pub trait WithEntity {
    fn attach_entity(&mut self, entity: std::sync::Arc<DataRow>);
}

use crate::RenderContext;
use crate::reactive::BuilderServices;

/// Arguments passed to every builder during interpretation.
///
/// Builders read their own configuration from `args`, access the current data
/// context via `ctx`, call `interpret` to recursively render sub-expressions,
/// and access `services` for profile resolution, block data, etc.
///
/// `services` is separate from `ctx` so that `RenderContext` stays a pure data
/// struct (no lifetimes, no Arc) that frontends can store freely.
pub struct BuilderArgs<'a, W> {
    pub args: &'a ResolvedArgs,
    pub ctx: &'a RenderContext,
    pub services: &'a dyn BuilderServices,
    /// Recursion handle — interpret a `RenderExpr` in a given context.
    /// The closure captures `services` internally, so callers just pass (expr,
    /// ctx).
    pub interpret: &'a dyn Fn(&RenderExpr, &RenderContext) -> W,
    /// What the immediate parent container offered this widget (its parent's
    /// `ctx.offering(..)`). Lives here rather than on `ctx` so it cannot leak:
    /// `ctx` — the value builders clone for their own children — has already
    /// had it stripped.
    pub parent_capability: crate::render_context::ContainerCapability,
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
// Arg evaluation (`resolve_args_for_widget`) dispatches `FunctionCall` nodes
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
/// passed to `resolve_args_for_widget`.
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
    /// Declared params per widget, keyed by DSL name. Drives per-widget
    /// template-vs-scalar arg classification; a widget absent here (or one
    /// declaring no params) is judged by the global `is_template_arg`
    /// allowlist instead.
    widget_metas: HashMap<String, &'static holon_api::WidgetMeta>,
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

impl<W> Default for RenderInterpreter<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W> RenderInterpreter<W> {
    pub fn new() -> Self {
        Self {
            builders: HashMap::new(),
            value_fns: HashMap::new(),
            widget_metas: HashMap::new(),
            annotator: None,
        }
    }

    /// Bind the macro-generated `WIDGET_META` of every registered builder so
    /// arg classification can consult the widget's own param list.
    pub fn set_widget_metas(&mut self, metas: Vec<&'static holon_api::WidgetMeta>) {
        self.widget_metas = metas.into_iter().map(|m| (m.name.to_string(), m)).collect();
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

    /// Set a post-build annotator that tags every widget with
    /// test/accessibility IDs.
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
        holon_api::render_dsl::parse_render_dsl_with_names(source, &name_refs)
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
                // Bind the value-fn registry so `resolve_args_for_widget` can
                // dispatch `FunctionCall` arg expressions (e.g.
                // `collection: focus_chain()`) through it.
                let binding = ValueFnBinding {
                    fns: &self.value_fns,
                    services,
                    ctx,
                };
                let resolved = resolve_args_for_widget(
                    args,
                    ctx.row(),
                    &binding,
                    self.widget_metas.get(name.as_str()).copied(),
                );
                // `live_block(id, #{role: "page_title", ...})` — second
                // positional Object arg becomes ctx.flags for the resolved
                // block's variant dispatch. AST stays shape-stable; flags
                // are consumed by `pick_active_variant`, never serialized.
                if name == "live_block" {
                    if let Some(holon_api::Value::Object(map)) = resolved.positional.get(1) {
                        let child_ctx = ctx.with_flags(map.clone());
                        return self.dispatch(name, &resolved, &child_ctx, services, &interpret_fn);
                    }
                }
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
                let exprs: Vec<_> = fields.values().cloned().collect();
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
        // One-level scoping of the parent's offer: the builder learns it through
        // `parent_capability`, while the `ctx` it clones for its own children
        // carries `ContainerCapability::None`. Without this strip an `accordion`
        // wrapped in a `row` inside a `column` would inherit the column's offer.
        let parent_capability = ctx.parent_capability;
        let stripped;
        let ctx = if parent_capability == crate::render_context::ContainerCapability::None {
            ctx
        } else {
            stripped = ctx.offering(crate::render_context::ContainerCapability::None);
            &stripped
        };
        let widget = match self.builders.get(name) {
            Some(builder) => builder.build(BuilderArgs {
                args,
                ctx,
                services,
                interpret: interpret_fn,
                parent_capability,
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
                        parent_capability,
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
            | "rendered_text"
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
    // ALLOW(unused_param): closure conforms to existing builder fn-pointer shape;
    // both args are required by signature
    let noop_interpret = |_e: &RenderExpr, _c: &RenderContext| -> ReactiveViewModel {
        ReactiveViewModel::from_widget("_unreachable", HashMap::new())
    };

    let ba = BuilderArgs {
        args: &args,
        ctx: &ctx,
        services,
        interpret: &noop_interpret,
        parent_capability: ctx.parent_capability,
    };

    // Try the macro-generated fast path first.
    if let Some(props) = crate::shadow_builders::dispatch_resolve_props(widget_name, &ba) {
        return props;
    }

    // ALLOW(fallback): two-level dispatch — fast path then full interpret; both
    // succeed deterministically Fallback for raw builders: full interpret,
    // extract props.
    let fresh = services.interpret(expr, &ctx);
    fresh.props.get_cloned()
}

// =========================================================================
// Shared builders — framework-independent, work for any W
// =========================================================================

/// `col` builder: interprets each positional expr and collects into a vertical
/// list.
///
/// Frontends wrap this by providing their own `col` that calls
/// `shared_col_build` and then wraps the resulting `Vec<W>` in their
/// framework's vstack equivalent.
pub fn shared_col_build<W>(ba: &BuilderArgs<'_, W>) -> Vec<W> {
    ba.args
        .positional_exprs
        .iter()
        .map(|expr| (ba.interpret)(expr, ba.ctx))
        .collect()
}

/// The hierarchy inputs a tree/outline build needs, lifted out of the caller's
/// args before the build runs.
///
/// Passing these explicitly is what lets a widget declare them as typed
/// `Expr` params: the helper never asks the untyped arg bag for a name, so
/// templateness is decided by the calling widget, not by a global allowlist.
#[derive(Clone, Copy)]
pub struct TreeInputs<'a> {
    /// Interpreted once per row to produce that row's node.
    pub item_template: &'a RenderExpr,
    /// Row column holding each row's parent id.
    pub parent_id_col: &'a str,
    /// Row column each sibling bucket is sorted by.
    pub sort_col: &'a str,
}

impl<'a> TreeInputs<'a> {
    /// Build from the widget's `item_template` / `parent_id` / `sortkey`
    /// expressions. The two column args are authored as `col("x")`; anything
    /// else (absent, or a non-column expression) means the conventional
    /// column name.
    pub fn new(
        item_template: &'a RenderExpr,
        parent_id: Option<&'a RenderExpr>,
        sortkey: Option<&'a RenderExpr>,
    ) -> Self {
        Self {
            item_template,
            parent_id_col: parent_id.and_then(column_ref_name).unwrap_or("parent_id"),
            sort_col: sortkey.and_then(column_ref_name).unwrap_or("sort_key"),
        }
    }
}

/// The id of the entity a collection renders INSIDE: the explicit
/// `virtual_parent` string (resolved from the `Bool(true)` sentinel by
/// `resolve_virtual_parent`), else the context's `context_entity` — bound by
/// `live_block` / `watch_live` / `live_query` when they mount a block's
/// resolved render over that block's data rows.
///
/// Deliberately NOT `ctx.row()`: a mounted render context has no bound row,
/// so `row()` reads `data_rows.first()` — an arbitrary result row, which
/// would crown a random row as context root.
///
/// The tree builders compare each row id against it to inject the
/// `is_context_root` positional key (Integer 1/0), so a profile rule can
/// distinguish a page's OWN root row from a parentless flat-query result row
/// (`eq("is_context_root", 1)` — the tree_view page_title rule). A flat query
/// over foreign blocks has no row matching the context id, so none of its
/// rows is a context root.
pub fn collection_context_root_id<W>(ba: &BuilderArgs<'_, W>) -> Option<String> {
    ba.args
        .get_string("virtual_parent")
        .map(|s| s.to_string())
        .or_else(|| ba.ctx.context_entity.clone())
}

/// `tree` builder: interprets rows as a hierarchical tree using `parent_id` and
/// `sortkey`.
///
/// Uses `OutlineTree` to build parent-child relationships, then walks
/// depth-first. Returns `Vec<(W, usize, HashMap<String, Value>)>` — each widget
/// paired with its nesting depth and the per-row rule-override map (empty when
/// no rules matched). Frontends wrap each `(widget, depth, overrides)` in their
/// own indentation container, threading overrides into tree_item chrome props
/// (show_bullet, show_chevron, ...).
pub fn shared_tree_build<W: WithEntity>(
    ba: &BuilderArgs<'_, W>,
    inputs: &TreeInputs<'_>,
) -> Vec<(W, usize, HashMap<String, holon_api::Value>)> {
    let TreeInputs {
        item_template: tmpl,
        parent_id_col,
        sort_col,
    } = *inputs;

    let rows = &ba.ctx.data_rows;
    if rows.is_empty() {
        return vec![((ba.interpret)(tmpl, ba.ctx), 0, HashMap::new())];
    }

    // Optional `rules:` arg — see `crate::row_pipeline::parse_rules_arg`.
    // Tree's positional context injects `level` and `depth` (synonyms) so
    // predicates can match `eq("level", 0)` for root rows or `gt("depth", 1)`
    // for deeply-nested rows.
    let rules = crate::row_pipeline::parse_rules_arg(ba.args.named.get("rules"));

    // RULING C1': roots may sort by a per-level ROOT key the render declares
    // through the SAME rules mechanism as the level-0 role/bullet overrides (a
    // `sortkey` inside a level-0 rule), so a tree honors its backing query's
    // top-level `ORDER BY` (which the CDC pipeline's `HashMap` accumulator
    // drops the row order of) for roots while child buckets keep `sort_col`.
    // `None` = no declared root key = pre-C1' behavior.
    let root_sort_key = crate::row_pipeline::extract_root_sort_key(&rules);
    let context_root_id = collection_context_root_id(ba);
    let tree = OutlineTree::from_rows(rows, parent_id_col, sort_col, root_sort_key.as_deref());
    tree.walk_depth_first(|resolved_row, depth| {
        // Tree adjusts `ctx.depth` before the pipeline applies, so child
        // builders see the cumulative depth (parent's + tree's own).
        // `with_row` is done here (not via `apply_full_row_pipeline`) because
        // tree intentionally skips profile/ops wiring at the tree-row level —
        // tree items typically wrap content via `live_block`/`render_entity`
        // which resolve their own profile downstream.
        let row_ctx = RenderContext {
            depth: ba.ctx.depth + depth,
            ..ba.ctx.with_row(Arc::clone(resolved_row))
        };
        let is_context_root = context_root_id.as_deref().is_some_and(|cid| {
            resolved_row
                .get("id")
                .and_then(|v| v.as_string())
                .is_some_and(|rid| rid == cid)
        });
        let positional = HashMap::from([
            ("level".to_string(), holon_api::Value::Integer(depth as i64)),
            ("depth".to_string(), holon_api::Value::Integer(depth as i64)),
            (
                "is_context_root".to_string(),
                holon_api::Value::Integer(is_context_root as i64),
            ),
        ]);
        let (node, overrides) = crate::row_pipeline::apply_rules_and_interpret_with_ctx(
            row_ctx,
            tmpl,
            &rules,
            resolved_row,
            positional,
            |expr, ctx| (ba.interpret)(expr, ctx),
        );
        (node, depth, overrides)
    })
}

/// `live_block` builder: fetches a block's WidgetSpec and recursively
/// interprets it.
///
/// Works for any `W` because it delegates all widget construction to
/// `interpret`. The only framework-specific part is error rendering, which
/// falls back to `text`.
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
    // Flags from the live_block call site (e.g. `live_block(id, #{role:
    // "page_title"})`) sit on `ba.ctx` and propagate to the resolved
    // render via deeper.with_data_rows. The first render_entity inside
    // `render_expr` reads them in pick_active_variant and then clears
    // them on the variant body's ctx (see shadow_builders/render_entity.rs).
    // That render_entity boundary is the load-bearing scope; live_block
    // doesn't itself clear flags, so a non-render_entity `render_expr`
    // (e.g. `column(row(...))`) propagates flags through the whole
    // subtree — harmless because no consumer reads them on those
    // widgets.
    let child_ctx = deeper
        .with_data_rows(data_rows)
        .with_context_entity(block_id.to_string());
    Ok((ba.interpret)(&render_expr, &child_ctx))
}

/// Result of a live query build, carrying both the interpreted content and
/// metadata needed for reactive subscriptions.
pub struct LiveQueryResult<W> {
    pub content: W,
    /// Source query text (PRQL/GQL/SQL) — compilation to SQL happens behind
    /// the query capability when the platform layer subscribes.
    pub query: String,
    pub query_lang: holon_api::QueryLanguage,
    pub query_context_id: Option<String>,
    pub render_expr: holon_api::render_types::RenderExpr,
}

/// `live_query` builder: compiles + executes a query, then interprets the
/// result.
///
/// Returns `Ok(LiveQueryResult)` on success or `Err(message)` for the frontend
/// to render as error text.
///
/// `item_template` is the expression each result row is rendered through,
/// supplied by the calling widget (`None` → `table()`). Passing it in rather
/// than reading it off the arg bag keeps the name-to-templateness decision
/// with the widget that declares the param.
pub fn shared_live_query_build<W>(
    ba: &BuilderArgs<'_, W>,
    item_template: Option<&RenderExpr>,
) -> Result<LiveQueryResult<W>, String> {
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
        // ALLOW(entity_uri_from_raw): context_id from render-spec arg or matview row
        // 'id' field
        let uri = holon_api::EntityUri::from_raw(id);
        crate::QueryContext {
            current_block_id: Some(uri.clone()),
            context_parent_id: Some(uri),
            // Validation-only context (the watch is started and immediately
            // dropped); descendants scoping is irrelevant here, so unfiltered.
            path_context: crate::PathContext::Unfiltered,
        }
    });

    // Validate-by-doing: start (and immediately drop) a watch. Compilation
    // errors and missing-live-query capability both surface as an error
    // render node, exactly as the old compile + start_query pair did. The
    // platform layer starts the *real* watcher from the node props.
    let result = ba
        .services
        .watch_query(&query, language, query_context.clone());

    let deeper_ctx = ba.ctx.deeper_query();

    // The render expression for interpreting query results comes from the
    // caller's item template, not from the query itself. Default to table()
    // when no template is specified.
    //
    // The template is interpreted ONCE against the whole delivered row set, so
    // only a collection widget iterates rows — a scalar template binds the
    // first row and drops the rest. Refuse that here rather than render a
    // plausible single row.
    if let Some(expr) = item_template {
        let name = match expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => name.as_str(),
            other => {
                return Err(format!(
                    "[live_query item_template must be a collection widget \
                     (list/tree/table/board/columns/outline); got {other:?} — a bare per-row \
                     template binds only the first row]"
                ));
            }
        };
        if !crate::collection_layout::is_layout(name) {
            return Err(format!(
                "[live_query item_template must be a collection widget \
                 (list/tree/table/board/columns/outline); got {name} — a bare per-row template \
                 binds only the first row. Wrap it: list(#{{item_template: {name}(…)}})]"
            ));
        }
    }

    let live_query_render_expr = item_template.cloned().unwrap_or_else(|| {
        holon_api::render_types::RenderExpr::FunctionCall {
            name: "table".to_string(),
            args: vec![],
        }
    });

    // Resolve `virtual_parent: true` → `virtual_parent: "<context_id>"`.
    // The DSL author opts into virtual children by writing `virtual_parent: true`
    // on a collection expression. We resolve the sentinel to the actual parent ID
    // here, so the stored expression survives signal re-interpretation.
    let live_query_render_expr = resolve_virtual_parent(live_query_render_expr, &context_id);

    match result {
        Ok(_stream) => {
            let mut child_ctx = deeper_ctx.with_data_rows(vec![]);
            if let Some(id) = &context_id {
                child_ctx = child_ctx.with_context_entity(id.clone());
            }
            let content = (ba.interpret)(&live_query_render_expr, &child_ctx);
            Ok(LiveQueryResult {
                content,
                query,
                query_lang: language,
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

/// `render_entity` builder: dispatches based on content_type/source_language in
/// the current row.
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
    profile: &holon_api::RenderProfile,
    ctx: &RenderContext,
    services: &dyn BuilderServices,
) -> RenderExpr {
    if profile.variants.is_empty() {
        return profile.render.clone();
    }

    // Get block ID for UI state lookup
    // Point-free form would drop the archlint baseline entry for this
    // `EntityUri::from_raw` call site.
    #[allow(clippy::redundant_closure)]
    let block_id = ctx
        .row()
        .get("id")
        .and_then(|v| v.as_string())
        // ALLOW(entity_uri_from_raw): block_id from matview row 'id' field
        .map(|s| EntityUri::from_raw(s));

    let mut ui_state = match block_id {
        Some(ref id) => services.ui_state(id),
        None => HashMap::new(),
    };

    // Merge container-query allocation AFTER services.ui_state so per-subtree
    // ALLOW(fallback): describing the global-viewport merge in UiState
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

    // Merge render-context flags so variant conditions can dispatch on them.
    // Well-known flags: `role`, `view_mode`, `embed_depth`. Set by
    // `live_block(id, #{role: ...})` or tree builder rule evaluation.
    for (k, v) in &ctx.flags {
        ui_state.insert(k.clone(), v.clone());
    }

    // Find first variant whose condition matches. Emit one tracing line per
    // resolution so PBT runs (and live diagnostics) can show which variant
    // won for each entity and why — turns "block X rendered as the wrong
    // widget" from a guessing game into a one-line read.
    for (idx, variant) in profile.variants.iter().enumerate() {
        if variant.condition.evaluate(&ui_state) {
            tracing::trace!(
                target: "profile",
                "[profile] {block_id_repr} variant_idx={idx} condition={cond:?} → MATCH",
                block_id_repr = block_id
                    .as_ref()
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "<no-id>".into()),
                cond = &variant.condition,
            );
            return variant.render.clone();
        }
    }
    tracing::trace!(
        target: "profile",
        "[profile] {block_id_repr} no variant matched → default render",
        block_id_repr = block_id
            .as_ref()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "<no-id>".into()),
    );

    profile.render.clone()
}

/// Result of `shared_render_entity_build` — all rendering behavior is now
/// driven by entity profile variants defined in YAML.
pub enum RenderBlockResult {
    /// The row has a profile with a render expression + operations — interpret
    /// it.
    ProfileWidget {
        render: RenderExpr,
        operations: Vec<OperationWiring>,
    },
    /// No matching profile/variant — frontend renders nothing.
    Empty,
    /// Error message — frontend renders as error text.
    Error(String),
}
