use std::collections::HashMap;
use std::sync::Arc;

use crate::interp_value::{InterpValue, ReactiveRowProvider};
use crate::render_types::{Arg, BinaryOperator, RenderExpr};
use crate::types::TaskState;
use crate::widget_spec::DataRow;
use crate::Value;

// =========================================================================
// Shared builder utilities
// =========================================================================

pub fn column_ref_name(expr: &RenderExpr) -> Option<&str> {
    match expr {
        RenderExpr::ColumnRef { name } => Some(name.as_str()),
        _ => None,
    }
}

pub fn sort_key_column<'a>(args: &'a ResolvedArgs) -> Option<&'a str> {
    match args.get_template("sort_key") {
        Some(RenderExpr::ColumnRef { name }) => Some(name.as_str()),
        _ => None,
    }
}

pub fn sort_value(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Integer(i)) => *i as f64,
        Some(Value::Float(f)) => *f,
        Some(Value::String(s)) => s.parse().unwrap_or(f64::MAX),
        _ => f64::MAX,
    }
}

pub fn cmp_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(Value::Integer(a)), Some(Value::Integer(b))) => a.cmp(b),
        (Some(Value::Float(a)), Some(Value::Float(b))) => {
            a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
        }
        (Some(Value::String(a)), Some(Value::String(b))) => a.cmp(b),
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}

pub fn sorted_rows(rows: &[Arc<DataRow>], sort_key: Option<&str>) -> Vec<Arc<DataRow>> {
    let mut sorted: Vec<_> = rows.to_vec();
    if let Some(key) = sort_key {
        sorted.sort_by(|a, b| cmp_values(a.get(key), b.get(key)));
    }
    sorted
}

pub fn resolve_color_name(s: &str) -> &str {
    match s {
        "red" => "#FF0000",
        "green" => "#00FF00",
        "blue" => "#0000FF",
        "yellow" => "#FFFF00",
        "white" => "#FFFFFF",
        "gray" | "grey" | "muted" => "#808080",
        s if s.starts_with('#') => s,
        _ => "#FFFFFF",
    }
}

pub fn resolve_states(args: &ResolvedArgs, row: &HashMap<String, Value>) -> Vec<String> {
    if let Some(states_expr) = args.get_template("states") {
        let val = eval_to_value(states_expr, row);
        if let Value::Array(items) = val {
            return items
                .iter()
                .filter_map(|v| v.as_string().map(|s| s.to_string()))
                .collect();
        }
    }
    vec![
        String::new(),
        "TODO".to_string(),
        "DOING".to_string(),
        "DONE".to_string(),
    ]
}

pub fn cycle_state(current: &str, states: &[String]) -> String {
    if states.is_empty() {
        return String::new();
    }
    let idx = states.iter().position(|s| s == current).unwrap_or(0);
    let next = (idx + 1) % states.len();
    states[next].clone()
}

pub fn state_icon(state: &str) -> &'static str {
    if state.is_empty() {
        ""
    } else if state == "CANCELLED" {
        "✗"
    } else {
        let ts = TaskState::from_keyword(state);
        if ts.is_doing() {
            "◑"
        } else if ts.is_done() {
            "✓"
        } else {
            "○"
        }
    }
}

pub fn state_display(state: &str) -> (&str, &str) {
    match state {
        "" => ("", "muted"),
        "TODO" => ("TODO", "muted"),
        "DOING" => ("DOING", "warning"),
        "DONE" => ("[x]", "success"),
        "CANCELLED" => ("CANCELLED", "error"),
        _ => (state, "primary"),
    }
}

// =========================================================================
// Outline tree data structure
// =========================================================================

pub struct OutlineTree {
    pub roots: Vec<usize>,
    pub children_of: HashMap<String, Vec<usize>>,
    pub sorted_rows: Vec<Arc<DataRow>>,
}

impl OutlineTree {
    pub fn from_rows(rows: &[Arc<DataRow>], parent_id_col: &str, sort_col: &str) -> Self {
        let mut sorted_rows = rows.to_vec();
        sorted_rows.sort_by(|a, b| {
            let ka = sort_value(a.get(sort_col));
            let kb = sort_value(b.get(sort_col));
            ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut roots: Vec<usize> = Vec::new();
        let mut children_of: HashMap<String, Vec<usize>> = HashMap::new();

        for (i, row) in sorted_rows.iter().enumerate() {
            let pid = row
                .get(parent_id_col)
                .and_then(|v| v.as_string())
                .unwrap_or("");

            let parent_exists = sorted_rows.iter().any(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map_or(false, |id| id == pid)
            });

            if !parent_exists {
                roots.push(i);
            } else {
                children_of.entry(pid.to_string()).or_default().push(i);
            }
        }

        Self {
            roots,
            children_of,
            sorted_rows,
        }
    }

    pub fn walk_depth_first<F, W>(&self, mut render_item: F) -> Vec<W>
    where
        F: FnMut(&Arc<DataRow>, usize) -> W,
    {
        let mut result = Vec::new();
        self.walk_level(&self.roots, 0, &mut render_item, &mut result);
        result
    }

    fn walk_level<F, W>(
        &self,
        indices: &[usize],
        depth: usize,
        render_item: &mut F,
        result: &mut Vec<W>,
    ) where
        F: FnMut(&Arc<DataRow>, usize) -> W,
    {
        for &i in indices {
            let row = &self.sorted_rows[i];
            result.push(render_item(row, depth));

            if let Some(own_id) = row.get("id").and_then(|v| v.as_string()) {
                if let Some(child_indices) = self.children_of.get(own_id) {
                    self.walk_level(child_indices, depth + 1, render_item, result);
                }
            }
        }
    }
}

// =========================================================================
// Screen layout partitioning
// =========================================================================

#[derive(Debug, PartialEq)]
pub struct CollapsibleRegion<W> {
    pub block_id: Option<String>,
    pub widget: W,
}

pub struct MainRegion<W> {
    pub block_id: Option<String>,
    pub widget: W,
}

pub struct ScreenLayoutPartition<W> {
    pub left_sidebar: Option<CollapsibleRegion<W>>,
    pub main: Vec<MainRegion<W>>,
    pub right_sidebar: Option<CollapsibleRegion<W>>,
}

/// Check whether any rows have `collapse_to = "drawer"` (case-insensitive).
pub fn has_drawer_rows(rows: &[Arc<DataRow>]) -> bool {
    rows.iter().any(|row| {
        row.get("collapse_to")
            .or(row.get("collapse-to"))
            .and_then(|v| v.as_string())
            .map_or(false, |s| s.eq_ignore_ascii_case("drawer"))
    })
}

pub fn partition_screen_columns<W, F>(
    rows: &[Arc<DataRow>],
    mut render_row: F,
) -> ScreenLayoutPartition<W>
where
    F: FnMut(&DataRow) -> W,
{
    struct Spec<W> {
        is_drawer: bool,
        block_id: Option<String>,
        widget: W,
    }

    let specs: Vec<Spec<W>> = rows
        .iter()
        .map(|row| {
            let collapse_to = row
                .get("collapse_to")
                .or(row.get("collapse-to"))
                .and_then(|v| v.as_string());
            let is_drawer = collapse_to.map_or(false, |s| s.eq_ignore_ascii_case("drawer"));
            let block_id = row
                .get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string());
            Spec {
                is_drawer,
                block_id,
                widget: render_row(row),
            }
        })
        .collect();

    let mut first_drawer_idx = None;
    let mut last_drawer_idx = None;
    for (i, spec) in specs.iter().enumerate() {
        if spec.is_drawer {
            if first_drawer_idx.is_none() {
                first_drawer_idx = Some(i);
            }
            last_drawer_idx = Some(i);
        }
    }

    let mut left_sidebar = None;
    let mut right_sidebar = None;
    let mut main = Vec::new();

    for (i, spec) in specs.into_iter().enumerate() {
        if Some(i) == first_drawer_idx {
            left_sidebar = Some(CollapsibleRegion {
                block_id: spec.block_id,
                widget: spec.widget,
            });
        } else if Some(i) == last_drawer_idx && first_drawer_idx != last_drawer_idx {
            right_sidebar = Some(CollapsibleRegion {
                block_id: spec.block_id,
                widget: spec.widget,
            });
        } else {
            main.push(MainRegion {
                block_id: spec.block_id,
                widget: spec.widget,
            });
        }
    }

    ScreenLayoutPartition {
        left_sidebar,
        main,
        right_sidebar,
    }
}

pub struct ResolvedArgs {
    pub positional: Vec<Value>,
    pub positional_exprs: Vec<RenderExpr>,
    pub named: HashMap<String, Value>,
    /// Reactive row-set args populated by `resolve_args_with` when a
    /// value-function returns `InterpValue::Rows`. Read by streaming
    /// Collection-param widgets (e.g. `list(#{collection: focus_chain()})`).
    ///
    /// Kept as a separate field (rather than folding into `named`) so
    /// existing scalar accessors and builders stay byte-compatible.
    pub rows: HashMap<String, Arc<dyn ReactiveRowProvider>>,
    pub templates: HashMap<String, RenderExpr>,
}

impl ResolvedArgs {
    pub fn from_positional_value(value: Value) -> Self {
        Self {
            positional: vec![value],
            positional_exprs: Vec::new(),
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
        }
    }

    pub fn from_positional_exprs(exprs: Vec<RenderExpr>) -> Self {
        Self {
            positional: Vec::new(),
            positional_exprs: exprs,
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
        }
    }

    pub fn get_string(&self, name: &str) -> Option<&str> {
        self.named.get(name).and_then(|v| v.as_string())
    }

    pub fn get_string_or(&self, name: &str, default: &str) -> String {
        self.get_string(name)
            .map(|s| s.to_string())
            .unwrap_or_else(|| default.to_string())
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        self.named.get(name).and_then(value_to_f64)
    }

    pub fn get_positional_f64(&self, index: usize) -> Option<f64> {
        self.positional.get(index).and_then(value_to_f64)
    }

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.named.get(name).and_then(|v| match v {
            Value::Boolean(b) => Some(*b),
            _ => None,
        })
    }

    /// Get positional arg as string, coercing non-string values.
    pub fn get_positional_string(&self, index: usize) -> Option<String> {
        self.positional.get(index).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Integer(i) => Some(i.to_string()),
            Value::Float(f) => Some(f.to_string()),
            Value::Boolean(b) => Some(b.to_string()),
            Value::Null => None,
            other => Some(format!("{other:?}")),
        })
    }

    /// If positional arg at `index` was a `col("foo")` reference, return "foo".
    pub fn get_positional_column_name(&self, index: usize) -> Option<&str> {
        match self.positional_exprs.get(index) {
            Some(RenderExpr::ColumnRef { name }) => Some(name.as_str()),
            _ => None,
        }
    }

    pub fn get_template(&self, name: &str) -> Option<&RenderExpr> {
        self.templates.get(name)
    }

    /// Reactive row-set named arg (e.g. `collection:` on a streaming
    /// list). Returns `None` if the arg was a scalar `Value` or absent.
    pub fn get_rows(&self, name: &str) -> Option<Arc<dyn ReactiveRowProvider>> {
        self.rows.get(name).cloned()
    }
}

fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Dispatcher for named render-DSL functions that return `InterpValue`.
///
/// Implementations are provided by the render interpreter (widget +
/// value-function registry). Unknown names return `None`; the caller
/// in `eval_to_interp` then resolves the name to `Value::Null` (F1 in
/// the design plan — no silent first-arg fallback).
pub trait ValueFnLookup {
    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<InterpValue>;
}

/// Built-in value functions available to every caller — `concat` for
/// now, more added later. Frontend registries chain through this as a
/// fallback so user-supplied registrations can still override built-in
/// names (collision check at `register_value_fn` enforces uniqueness
/// against widgets, not against the core list).
///
/// Replaces the previous `EmptyValueFnLookup` + inline `if name ==
/// "concat"` shim that lived in `eval_to_interp`.
pub struct CoreValueFnLookup;

impl ValueFnLookup for CoreValueFnLookup {
    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<InterpValue> {
        match name {
            "concat" => Some(InterpValue::Value(concat_invoke(args))),
            _ => None,
        }
    }
}

/// Singleton core lookup — built-in value fns, no widget registry.
/// Used by `eval_to_value` / `resolve_args` (the no-frontend path).
pub static CORE_VALUE_FN_LOOKUP: CoreValueFnLookup = CoreValueFnLookup;

/// `concat(a, b, c, ...)` — joins the display-string forms of every
/// positional arg. Promoted from `legacy_concat` in Task #12 so the
/// DSL has no magic-name special cases.
fn concat_invoke(resolved: &ResolvedArgs) -> Value {
    let parts: Vec<String> = resolved
        .positional
        .iter()
        .map(|v| v.to_display_string())
        .collect();
    Value::String(parts.join(""))
}

/// Scalar-only legacy path (preserved behavior for callers that don't
/// have a value-fn registry). Thin wrapper over `eval_to_interp` that
/// drops `Rows` to `Value::Null` with a warning.
pub fn resolve_args(args: &[Arg], row: &HashMap<String, Value>) -> ResolvedArgs {
    resolve_args_with(args, row, &CORE_VALUE_FN_LOOKUP)
}

/// Resolve arguments with value-function dispatch.
///
/// Scalar-valued results are placed in `positional` / `named`; row-set
/// results end up in `rows` under their named-arg key. Positional
/// row-sets panic — positional args are scalar by convention, so a row
/// set there is a user error in the DSL worth surfacing at the first
/// evaluation.
pub fn resolve_args_with(
    args: &[Arg],
    row: &HashMap<String, Value>,
    fns: &dyn ValueFnLookup,
) -> ResolvedArgs {
    let mut positional = Vec::new();
    let mut positional_exprs = Vec::new();
    let mut named = HashMap::new();
    let mut rows = HashMap::new();
    let mut templates = HashMap::new();

    for arg in args {
        match &arg.name {
            Some(name) if is_template_arg(name) => {
                templates.insert(name.clone(), arg.value.clone());
            }
            Some(name) => match eval_to_interp(&arg.value, row, fns) {
                InterpValue::Value(v) => {
                    named.insert(name.clone(), v);
                }
                InterpValue::Rows(p) => {
                    rows.insert(name.clone(), p);
                }
            },
            None => {
                positional_exprs.push(arg.value.clone());
                match eval_to_interp(&arg.value, row, fns) {
                    InterpValue::Value(v) => positional.push(v),
                    InterpValue::Rows(_) => panic!(
                        "value-function returned Rows in positional position; \
                         use a named arg (e.g. `collection:`) instead"
                    ),
                }
            }
        }
    }

    ResolvedArgs {
        positional,
        positional_exprs,
        named,
        rows,
        templates,
    }
}

pub fn is_template_arg(name: &str) -> bool {
    matches!(
        name,
        "item_template"
            | "item"
            | "header"
            | "header_template"
            | "child_template"
            | "action"
            | "parent_id"
            | "sortkey"
            | "sort_key"
            | "context"
            | "states"
    ) || name.starts_with("mode_")
}

/// Legacy scalar eval — preserves every call site that was already
/// `eval_to_value`. Thin wrapper over `eval_to_interp` with the empty
/// lookup: row-sets become `Value::Null` + a warning, since a scalar
/// caller cannot meaningfully consume one.
pub fn eval_to_value(expr: &RenderExpr, row: &HashMap<String, Value>) -> Value {
    match eval_to_interp(expr, row, &CORE_VALUE_FN_LOOKUP) {
        InterpValue::Value(v) => v,
        InterpValue::Rows(_) => {
            tracing::warn!("eval_to_value: FunctionCall returned Rows in scalar context; dropping");
            Value::Null
        }
    }
}

/// Evaluate a `RenderExpr` into an `InterpValue`.
///
/// Drives argument evaluation for `resolve_args_with`. Dispatches
/// `FunctionCall`s through the provided registry; unknown names
/// (other than the legacy `concat` shim) produce `Value::Null`. The
/// pre-F1 "silently return first arg" fallback is gone — a typo'd
/// function call now produces a visible `Null` at the consumer.
pub fn eval_to_interp(
    expr: &RenderExpr,
    row: &HashMap<String, Value>,
    fns: &dyn ValueFnLookup,
) -> InterpValue {
    use InterpValue::*;
    match expr {
        RenderExpr::Literal { value } => Value(value.clone()),
        RenderExpr::ColumnRef { name } => {
            Value(row.get(name).cloned().unwrap_or(crate::Value::Null))
        }
        RenderExpr::BinaryOp { op, left, right } => {
            let l = eval_to_value(left, row);
            let r = eval_to_value(right, row);
            Value(eval_binary_op(op, &l, &r))
        }
        RenderExpr::FunctionCall { name, args, .. } => {
            // Evaluate args against the same registry so value-fn calls
            // nested under other value-fn calls resolve correctly.
            let resolved = resolve_args_with(args, row, fns);
            match fns.invoke(name, &resolved) {
                Some(v) => v,
                // F1: silent first-arg fallback removed. Unknown name
                // → Null. Built-in fns (`concat`, ...) are reachable
                // through `CORE_VALUE_FN_LOOKUP` and should be chained
                // into the caller's lookup if a frontend wants to keep
                // them; the api-level entry points already do that.
                None => Value(crate::Value::Null),
            }
        }
        RenderExpr::Array { items } => Value(crate::Value::Array(
            items.iter().map(|i| eval_to_value(i, row)).collect(),
        )),
        RenderExpr::Object { fields } => Value(crate::Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), eval_to_value(v, row)))
                .collect(),
        )),
        RenderExpr::LiveBlock { block_id } => {
            Value(crate::Value::String(format!("[LiveBlock: {}]", block_id)))
        }
    }
}

pub fn eval_binary_op(op: &BinaryOperator, left: &Value, right: &Value) -> Value {
    match op {
        BinaryOperator::Add => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a + b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
            (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
            _ => Value::Null,
        },
        BinaryOperator::Sub => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a - b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a - b),
            _ => Value::Null,
        },
        BinaryOperator::Mul => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Integer(a * b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a * b),
            _ => Value::Null,
        },
        BinaryOperator::Div => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) if *b != 0 => Value::Integer(a / b),
            (Value::Float(a), Value::Float(b)) if *b != 0.0 => Value::Float(a / b),
            _ => Value::Null,
        },
        BinaryOperator::Eq => Value::Boolean(left == right),
        BinaryOperator::Neq => Value::Boolean(left != right),
        BinaryOperator::Gt => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a > b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a > b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Lt => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a < b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a < b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Gte => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a >= b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a >= b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Lte => match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => Value::Boolean(a <= b),
            (Value::Float(a), Value::Float(b)) => Value::Boolean(a <= b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::And => match (left, right) {
            (Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(*a && *b),
            _ => Value::Boolean(false),
        },
        BinaryOperator::Or => match (left, right) {
            (Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(*a || *b),
            _ => Value::Boolean(false),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_types::Arg;

    #[test]
    fn test_eval_binary_op_arithmetic() {
        assert_eq!(
            eval_binary_op(&BinaryOperator::Add, &Value::Integer(2), &Value::Integer(3)),
            Value::Integer(5)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Sub, &Value::Float(5.0), &Value::Float(2.0)),
            Value::Float(3.0)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Mul, &Value::Integer(3), &Value::Integer(4)),
            Value::Integer(12)
        );
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Div,
                &Value::Integer(10),
                &Value::Integer(3)
            ),
            Value::Integer(3)
        );
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Div,
                &Value::Integer(10),
                &Value::Integer(0)
            ),
            Value::Null
        );
    }

    #[test]
    fn test_eval_binary_op_string_concat() {
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Add,
                &Value::String("hello ".into()),
                &Value::String("world".into())
            ),
            Value::String("hello world".into())
        );
    }

    #[test]
    fn test_eval_binary_op_comparison() {
        assert_eq!(
            eval_binary_op(&BinaryOperator::Eq, &Value::Integer(1), &Value::Integer(1)),
            Value::Boolean(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Neq, &Value::Integer(1), &Value::Integer(2)),
            Value::Boolean(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Gt, &Value::Integer(3), &Value::Integer(2)),
            Value::Boolean(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Lt, &Value::Float(1.0), &Value::Float(2.0)),
            Value::Boolean(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Gte, &Value::Integer(3), &Value::Integer(3)),
            Value::Boolean(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Lte, &Value::Integer(2), &Value::Integer(3)),
            Value::Boolean(true)
        );
    }

    #[test]
    fn test_eval_binary_op_logical() {
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::And,
                &Value::Boolean(true),
                &Value::Boolean(false)
            ),
            Value::Boolean(false)
        );
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Or,
                &Value::Boolean(false),
                &Value::Boolean(true)
            ),
            Value::Boolean(true)
        );
    }

    #[test]
    fn test_eval_to_value_literal() {
        let row = HashMap::new();
        let expr = RenderExpr::Literal {
            value: Value::Integer(42),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::Integer(42));
    }

    #[test]
    fn test_eval_to_value_column_ref() {
        let mut row = HashMap::new();
        row.insert("name".to_string(), Value::String("Alice".into()));
        let expr = RenderExpr::ColumnRef {
            name: "name".to_string(),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::String("Alice".into()));
    }

    #[test]
    fn test_eval_to_value_missing_column() {
        let row = HashMap::new();
        let expr = RenderExpr::ColumnRef {
            name: "missing".to_string(),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::Null);
    }

    #[test]
    fn test_eval_to_value_binary_op() {
        let row = HashMap::new();
        let expr = RenderExpr::BinaryOp {
            op: BinaryOperator::Add,
            left: Box::new(RenderExpr::Literal {
                value: Value::Integer(1),
            }),
            right: Box::new(RenderExpr::Literal {
                value: Value::Integer(2),
            }),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::Integer(3));
    }

    #[test]
    fn test_eval_to_value_concat() {
        let row = HashMap::new();
        let expr = RenderExpr::FunctionCall {
            name: "concat".to_string(),
            args: vec![
                Arg {
                    name: None,
                    value: RenderExpr::Literal {
                        value: Value::String("hello".into()),
                    },
                },
                Arg {
                    name: None,
                    value: RenderExpr::Literal {
                        value: Value::String(" world".into()),
                    },
                },
            ],
        };
        assert_eq!(
            eval_to_value(&expr, &row),
            Value::String("hello world".into())
        );
    }

    #[test]
    fn test_eval_to_value_array() {
        let row = HashMap::new();
        let expr = RenderExpr::Array {
            items: vec![
                RenderExpr::Literal {
                    value: Value::Integer(1),
                },
                RenderExpr::Literal {
                    value: Value::Integer(2),
                },
            ],
        };
        assert_eq!(
            eval_to_value(&expr, &row),
            Value::Array(vec![Value::Integer(1), Value::Integer(2)])
        );
    }

    #[test]
    fn test_resolve_args_named_and_positional() {
        let mut row = HashMap::new();
        row.insert("col1".to_string(), Value::String("val1".into()));

        let args = vec![
            Arg {
                name: None,
                value: RenderExpr::ColumnRef {
                    name: "col1".to_string(),
                },
            },
            Arg {
                name: Some("title".to_string()),
                value: RenderExpr::Literal {
                    value: Value::String("My Title".into()),
                },
            },
            Arg {
                name: Some("item_template".to_string()),
                value: RenderExpr::Literal { value: Value::Null },
            },
        ];

        let resolved = resolve_args(&args, &row);
        assert_eq!(resolved.positional.len(), 1);
        assert_eq!(resolved.positional[0], Value::String("val1".into()));
        assert_eq!(
            resolved.named.get("title"),
            Some(&Value::String("My Title".into()))
        );
        assert!(resolved.templates.contains_key("item_template"));
        assert_eq!(resolved.get_positional_column_name(0), Some("col1"));
    }

    #[test]
    fn test_is_template_arg() {
        assert!(is_template_arg("item_template"));
        assert!(is_template_arg("item"));
        assert!(is_template_arg("header"));
        assert!(is_template_arg("states"));
        assert!(!is_template_arg("title"));
        assert!(!is_template_arg("width"));
    }

    #[test]
    fn test_to_display_string() {
        assert_eq!(Value::String("hello".into()).to_display_string(), "hello");
        assert_eq!(Value::Integer(42).to_display_string(), "42");
        assert_eq!(Value::Float(3.14).to_display_string(), "3.14");
        assert_eq!(Value::Boolean(true).to_display_string(), "true");
        assert_eq!(Value::Null.to_display_string(), "");
        assert_eq!(
            Value::Array(vec![Value::Integer(1), Value::Integer(2)]).to_display_string(),
            "1, 2"
        );
    }

    #[test]
    fn test_sorted_rows() {
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(HashMap::from([
                ("name".into(), Value::String("b".into())),
                ("sort".into(), Value::Integer(2)),
            ])),
            Arc::new(HashMap::from([
                ("name".into(), Value::String("a".into())),
                ("sort".into(), Value::Integer(1)),
            ])),
            Arc::new(HashMap::from([
                ("name".into(), Value::String("c".into())),
                ("sort".into(), Value::Integer(3)),
            ])),
        ];
        let sorted = sorted_rows(&rows, Some("sort"));
        assert_eq!(sorted[0].get("name"), Some(&Value::String("a".into())));
        assert_eq!(sorted[2].get("name"), Some(&Value::String("c".into())));

        let unsorted = sorted_rows(&rows, None);
        assert_eq!(unsorted[0].get("name"), Some(&Value::String("b".into())));
    }

    #[test]
    fn test_outline_tree() {
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(HashMap::from([
                ("id".into(), Value::String("1".into())),
                ("parent_id".into(), Value::String("root".into())),
                ("sort_key".into(), Value::Integer(1)),
            ])),
            Arc::new(HashMap::from([
                ("id".into(), Value::String("2".into())),
                ("parent_id".into(), Value::String("1".into())),
                ("sort_key".into(), Value::Integer(1)),
            ])),
            Arc::new(HashMap::from([
                ("id".into(), Value::String("3".into())),
                ("parent_id".into(), Value::String("root".into())),
                ("sort_key".into(), Value::Integer(2)),
            ])),
        ];

        let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key");
        assert_eq!(tree.roots.len(), 2);

        let items: Vec<(String, usize)> = tree.walk_depth_first(|row, depth| {
            let id = row.get("id").unwrap().as_string().unwrap().to_string();
            (id, depth)
        });
        assert_eq!(
            items,
            vec![
                ("1".to_string(), 0),
                ("2".to_string(), 1),
                ("3".to_string(), 0),
            ]
        );
    }

    #[test]
    fn test_partition_screen_columns() {
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(HashMap::from([
                ("name".into(), Value::String("left".into())),
                ("collapse_to".into(), Value::String("drawer".into())),
            ])),
            Arc::new(HashMap::from([(
                "name".into(),
                Value::String("main".into()),
            )])),
            Arc::new(HashMap::from([
                ("name".into(), Value::String("right".into())),
                ("collapse_to".into(), Value::String("drawer".into())),
            ])),
        ];
        let p = partition_screen_columns(&rows, |row| {
            row.get("name").unwrap().as_string().unwrap().to_string()
        });
        assert_eq!(
            p.left_sidebar.as_ref().map(|r| r.widget.as_str()),
            Some("left")
        );
        assert_eq!(
            p.right_sidebar.as_ref().map(|r| r.widget.as_str()),
            Some("right")
        );
        assert_eq!(p.main.len(), 1);
        assert_eq!(p.main[0].widget, "main");
    }

    #[test]
    fn test_cycle_state() {
        let states = vec!["".into(), "TODO".into(), "DOING".into(), "DONE".into()];
        assert_eq!(cycle_state("", &states), "TODO");
        assert_eq!(cycle_state("TODO", &states), "DOING");
        assert_eq!(cycle_state("DONE", &states), "");
    }

    #[test]
    fn test_state_display() {
        assert_eq!(state_display("TODO"), ("TODO", "warning"));
        assert_eq!(state_display("DONE"), ("[x]", "success"));
        assert_eq!(state_display(""), ("[ ]", "muted"));
        assert_eq!(state_display("CUSTOM"), ("CUSTOM", "primary"));
    }

    #[test]
    fn test_resolve_color_name() {
        assert_eq!(resolve_color_name("red"), "#FF0000");
        assert_eq!(resolve_color_name("#ABC123"), "#ABC123");
        assert_eq!(resolve_color_name("unknown"), "#FFFFFF");
    }

    // ── F1 regression — unknown FunctionCall returns Value::Null ───────
    //
    // Pre-F1, unknown function calls silently returned their first arg,
    // masking DSL typos. This must fail loud now.

    #[test]
    fn f1_unknown_fn_returns_null_not_first_arg() {
        let row = HashMap::new();
        let expr = RenderExpr::FunctionCall {
            name: "definitely_not_registered".to_string(),
            args: vec![Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::Integer(7),
                },
            }],
        };
        assert_eq!(eval_to_value(&expr, &row), Value::Null);
    }

    #[test]
    fn core_concat_still_works() {
        // concat is reachable through `CORE_VALUE_FN_LOOKUP`. The
        // pre-Task-#12 inline shim is gone — this test guards the
        // proper registration path so existing DSL `concat(...)` calls
        // keep producing identical output.
        let row = HashMap::new();
        let expr = RenderExpr::FunctionCall {
            name: "concat".to_string(),
            args: vec![
                Arg {
                    name: None,
                    value: RenderExpr::Literal {
                        value: Value::String("ab".into()),
                    },
                },
                Arg {
                    name: None,
                    value: RenderExpr::Literal {
                        value: Value::String("cd".into()),
                    },
                },
            ],
        };
        assert_eq!(eval_to_value(&expr, &row), Value::String("abcd".into()));
    }

    // ── Value-fn dispatch via resolve_args_with / eval_to_interp ───────

    struct MockValueFnLookup;
    impl ValueFnLookup for MockValueFnLookup {
        fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<InterpValue> {
            match name {
                "echo" => args.positional.first().cloned().map(InterpValue::Value),
                _ => None,
            }
        }
    }

    #[test]
    fn registered_value_fn_dispatches() {
        let row = HashMap::new();
        let expr = RenderExpr::FunctionCall {
            name: "echo".to_string(),
            args: vec![Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::Integer(99),
                },
            }],
        };
        match eval_to_interp(&expr, &row, &MockValueFnLookup) {
            InterpValue::Value(v) => assert_eq!(v, Value::Integer(99)),
            InterpValue::Rows(_) => panic!("expected Value"),
        }
    }

    #[test]
    fn resolve_args_with_empty_lookup_matches_legacy() {
        // Verifies resolve_args() and resolve_args_with(…, &EMPTY) are
        // observationally identical — the byte-compat promise.
        let mut row = HashMap::new();
        row.insert("n".into(), Value::Integer(3));

        let args = vec![
            Arg {
                name: None,
                value: RenderExpr::ColumnRef { name: "n".into() },
            },
            Arg {
                name: Some("title".into()),
                value: RenderExpr::Literal {
                    value: Value::String("hi".into()),
                },
            },
        ];

        let legacy = resolve_args(&args, &row);
        let with_empty = resolve_args_with(&args, &row, &CORE_VALUE_FN_LOOKUP);

        assert_eq!(legacy.positional, with_empty.positional);
        assert_eq!(legacy.named, with_empty.named);
        assert!(legacy.rows.is_empty() && with_empty.rows.is_empty());
    }
}
