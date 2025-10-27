use std::collections::{HashMap, HashSet};

use holon_api::render_eval::{
    column_ref_name, eval_binary_op, eval_to_value, resolve_args, OutlineTree, ResolvedArgs,
};
use holon_api::render_types::RenderExpr;
use holon_api::Value;

use crate::RenderContext;

/// Arguments passed to every builder during interpretation.
///
/// Builders read their own configuration from `args`, access the current data context
/// via `ctx`, and call `interpret` to recursively render sub-expressions (templates,
/// children, block refs, etc.).
pub struct BuilderArgs<'a, W, Ext: Clone = ()> {
    pub args: &'a ResolvedArgs,
    pub ctx: &'a RenderContext<Ext>,
    /// Recursion handle — interpret a `RenderExpr` in a given context.
    pub interpret: &'a dyn Fn(&RenderExpr, &RenderContext<Ext>) -> W,
}

/// A single widget builder that knows how to produce a widget of type `W`.
///
/// Builders are registered by name in the `RenderInterpreter` and dispatched
/// when the interpreter encounters a matching `FunctionCall` (or a synthetic
/// dispatch for leaf `RenderExpr` variants).
pub trait Builder<W, Ext: Clone = ()>: Send + Sync {
    fn build(&self, ba: BuilderArgs<'_, W, Ext>) -> W;
}

/// Blanket impl: any matching function is a Builder.
impl<W, Ext: Clone, F> Builder<W, Ext> for F
where
    F: Fn(BuilderArgs<'_, W, Ext>) -> W + Send + Sync,
{
    fn build(&self, ba: BuilderArgs<'_, W, Ext>) -> W {
        (self)(ba)
    }
}

const MAX_QUERY_DEPTH: usize = 10;

/// Post-build hook that tags widgets with accessibility/test IDs.
///
/// Called after every `dispatch()` with the produced widget, builder name,
/// and render context. Frontends use this to attach element IDs from the
/// row data (e.g. `ctx.row().get("id")`) so geometry queries and automated
/// tests can locate widgets by entity ID.
pub type AnnotatorFn<W, Ext> = Box<dyn Fn(W, &str, &RenderContext<Ext>) -> W + Send + Sync>;

/// Generic render interpreter parameterised over the widget type `W`.
///
/// All `RenderExpr` variants are dispatched to registered builders:
/// - `FunctionCall { name, .. }` → builder registered under `name`
/// - `ColumnRef` / `Literal` / `BinaryOp` → dispatched to `"text"` builder
/// - `Array` / `Object` → dispatched to `"col"` builder
/// - `BlockRef` → dispatched to `"block_ref"` builder
///
/// The set of registered builder names is the authoritative list of widgets
/// this frontend supports, accessible via `supported_widgets()`.
pub struct RenderInterpreter<W: 'static, Ext: Clone + 'static = ()> {
    builders: HashMap<String, Box<dyn Builder<W, Ext>>>,
    annotator: Option<AnnotatorFn<W, Ext>>,
}

impl<W, Ext: Clone> RenderInterpreter<W, Ext> {
    pub fn new() -> Self {
        Self {
            builders: HashMap::new(),
            annotator: None,
        }
    }

    pub fn register(&mut self, name: impl Into<String>, builder: impl Builder<W, Ext> + 'static) {
        self.builders.insert(name.into(), Box::new(builder));
    }

    /// Set a post-build annotator that tags every widget with test/accessibility IDs.
    pub fn set_annotator(
        &mut self,
        f: impl Fn(W, &str, &RenderContext<Ext>) -> W + Send + Sync + 'static,
    ) {
        self.annotator = Some(Box::new(f));
    }

    /// The set of widget names this interpreter can render.
    /// Feed this into `UiInfo` so the backend knows what widgets to emit.
    pub fn supported_widgets(&self) -> HashSet<String> {
        self.builders.keys().cloned().collect()
    }

    pub fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext<Ext>) -> W {
        let interpret_fn = |e: &RenderExpr, c: &RenderContext<Ext>| self.interpret(e, c);

        match expr {
            RenderExpr::FunctionCall {
                name,
                args,
                operations,
            } => {
                let resolved = resolve_args(args, ctx.row());
                let child_ctx = ctx.with_operations(operations.clone());
                self.dispatch(name, &resolved, &child_ctx, &interpret_fn)
            }
            RenderExpr::ColumnRef { name } => {
                let value = ctx.row().get(name).cloned().unwrap_or(Value::Null);
                let args = ResolvedArgs::from_positional_value(value);
                self.dispatch("text", &args, ctx, &interpret_fn)
            }
            RenderExpr::Literal { value } => {
                let args = ResolvedArgs::from_positional_value(value.clone());
                self.dispatch("text", &args, ctx, &interpret_fn)
            }
            RenderExpr::BinaryOp { op, left, right } => {
                let l = eval_to_value(left, ctx.row());
                let r = eval_to_value(right, ctx.row());
                let result = eval_binary_op(op, &l, &r);
                let args = ResolvedArgs::from_positional_value(result);
                self.dispatch("text", &args, ctx, &interpret_fn)
            }
            RenderExpr::Array { items } => {
                let args = ResolvedArgs::from_positional_exprs(items.clone());
                self.dispatch("col", &args, ctx, &interpret_fn)
            }
            RenderExpr::Object { fields } => {
                let exprs: Vec<_> = fields.iter().map(|(_, e)| e.clone()).collect();
                let args = ResolvedArgs::from_positional_exprs(exprs);
                self.dispatch("col", &args, ctx, &interpret_fn)
            }
            RenderExpr::BlockRef { block_id } => {
                let args = ResolvedArgs::from_positional_value(Value::String(block_id.clone()));
                self.dispatch("block_ref", &args, ctx, &interpret_fn)
            }
        }
    }

    fn dispatch(
        &self,
        name: &str,
        args: &ResolvedArgs,
        ctx: &RenderContext<Ext>,
        interpret_fn: &dyn Fn(&RenderExpr, &RenderContext<Ext>) -> W,
    ) -> W {
        let widget = match self.builders.get(name) {
            Some(builder) => builder.build(BuilderArgs {
                args,
                ctx,
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
// Shared builders — framework-independent, work for any W
// =========================================================================

/// `col` builder: interprets each positional expr and collects into a vertical list.
///
/// Frontends wrap this by providing their own `col` that calls `shared_col_build`
/// and then wraps the resulting `Vec<W>` in their framework's vstack equivalent.
pub fn shared_col_build<W, Ext: Clone>(ba: &BuilderArgs<'_, W, Ext>) -> Vec<W> {
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
pub fn shared_tree_build<W, Ext: Clone>(ba: &BuilderArgs<'_, W, Ext>) -> Vec<(W, usize)> {
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
    tree.walk_depth_first(|row, depth| {
        let row_ctx = ba.ctx.with_row(row.clone());
        let row_ctx = RenderContext {
            depth: row_ctx.depth + depth,
            ..row_ctx
        };
        ((ba.interpret)(tmpl, &row_ctx), depth)
    })
}

/// `block_ref` builder: fetches a block's WidgetSpec and recursively interprets it.
///
/// Works for any `W` because it delegates all widget construction to `interpret`.
/// The only framework-specific part is error rendering, which falls back to `text`.
pub fn shared_block_ref_build<W, Ext: Clone>(ba: &BuilderArgs<'_, W, Ext>) -> Result<W, String> {
    let block_id = ba
        .args
        .get_positional_string(0)
        .expect("block_ref requires a block_id positional arg");

    if ba.ctx.query_depth >= MAX_QUERY_DEPTH {
        return Err(format!(
            "[block_ref recursion limit reached (depth {})]",
            ba.ctx.query_depth
        ));
    }

    let deeper = ba.ctx.deeper_query();
    let session = ba.ctx.session.clone();
    let handle = ba.ctx.runtime_handle.clone();
    let bid = block_id.to_string();

    let result = std::thread::scope(|s| {
        s.spawn(|| handle.block_on(session.engine().blocks().render_block(&bid, None, false)))
            .join()
            .unwrap()
    });

    match result {
        Ok((widget_spec, _stream)) => {
            let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
            let child_ctx = deeper.with_data_rows(data_rows);
            Ok((ba.interpret)(&widget_spec.render_expr, &child_ctx))
        }
        Err(e) => {
            tracing::warn!("render_block({bid}) failed: {e}");
            Err(format!("render_block error: {e}"))
        }
    }
}

/// `live_query` builder: compiles + executes a query, then interprets the result.
///
/// Returns `Ok(W)` on success or `Err(message)` for the frontend to render as error text.
pub fn shared_live_query_build<W, Ext: Clone>(ba: &BuilderArgs<'_, W, Ext>) -> Result<W, String> {
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
        ba.ctx
            .session
            .engine()
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

    let query_context = context_id.map(|id| {
        let uri = holon_api::EntityUri::from_raw(&id);
        crate::QueryContext {
            current_block_id: Some(uri.clone()),
            context_parent_id: Some(uri),
            context_path_prefix: None,
            profile_context: None,
        }
    });

    let sql = ba
        .ctx
        .session
        .engine()
        .compile_to_sql(&query, QueryLanguage::HolonPrql)
        .unwrap_or_else(|_| query.clone());

    let session = ba.ctx.session.clone();
    let handle = ba.ctx.runtime_handle.clone();
    let result = std::thread::scope(|s| {
        s.spawn(|| handle.block_on(session.query_and_watch(sql, HashMap::new(), query_context)))
            .join()
            .unwrap()
    });

    let deeper_ctx = ba.ctx.deeper_query();

    match result {
        Ok((widget_spec, _stream)) => {
            let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
            let child_ctx = deeper_ctx.with_data_rows(data_rows);
            Ok((ba.interpret)(&widget_spec.render_expr, &child_ctx))
        }
        Err(e) => Err(format!("Query error: {e}")),
    }
}

/// `render_block` builder: dispatches based on content_type/source_language in the current row.
///
/// For query-language source blocks, fetches + recurses via block_ref.
/// Returns `Ok(W)` or `Err(message)`.
pub fn shared_render_block_build<W, Ext: Clone>(
    ba: &BuilderArgs<'_, W, Ext>,
) -> RenderBlockResult<W> {
    use holon_api::QueryLanguage;

    let content_type = ba
        .ctx
        .row()
        .get("content_type")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let source_language = ba
        .ctx
        .row()
        .get("source_language")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let content = ba
        .ctx
        .row()
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let is_query_lang = source_language.parse::<QueryLanguage>().is_ok();

    match (content_type.as_str(), is_query_lang) {
        ("source", true) => {
            let block_id = match ba.ctx.row().get("id").and_then(|v| v.as_string()) {
                Some(id) => id.to_string(),
                None => return RenderBlockResult::Error("[render_block: no id]".into()),
            };
            let ref_args = ResolvedArgs::from_positional_value(Value::String(block_id));
            match shared_block_ref_build(&BuilderArgs {
                args: &ref_args,
                ctx: ba.ctx,
                interpret: ba.interpret,
            }) {
                Ok(w) => RenderBlockResult::Widget(w),
                Err(msg) => RenderBlockResult::Error(msg),
            }
        }
        ("source", false) => RenderBlockResult::SourceBlock {
            language: source_language,
            content,
        },
        _ => {
            if content.is_empty() {
                RenderBlockResult::Empty
            } else {
                RenderBlockResult::TextContent(content)
            }
        }
    }
}

/// Result of `shared_render_block_build` — the frontend matches on this to create
/// framework-specific widgets for the non-recursive cases.
pub enum RenderBlockResult<W> {
    /// Successfully rendered via block_ref recursion.
    Widget(W),
    /// A non-query source block — frontend renders language label + content.
    SourceBlock { language: String, content: String },
    /// Plain text content.
    TextContent(String),
    /// Empty content — frontend renders nothing.
    Empty,
    /// Error message — frontend renders as error text.
    Error(String),
}
