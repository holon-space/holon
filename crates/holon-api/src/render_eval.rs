use std::collections::HashMap;
use std::sync::Arc;

use crate::Value;
use crate::interp_value::InterpValue;
use crate::interp_value::ReactiveRowProvider;
use crate::render_types::Arg;
use crate::render_types::BinaryOperator;
use crate::render_types::RenderExpr;
use crate::types::TaskState;
use crate::widget_spec::DataRow;

// =========================================================================
// Shared builder utilities
// =========================================================================

pub fn column_ref_name(expr: &RenderExpr) -> Option<&str> {
    match expr {
        RenderExpr::ColumnRef { name } => Some(name.as_str()),
        _ => None,
    }
}

pub fn sort_key_column(args: &ResolvedArgs) -> Option<&str> {
    match args.get_template("sort_key") {
        Some(RenderExpr::ColumnRef { name }) => Some(name.as_str()),
        _ => None,
    }
}

/// Convert a sort key value to a string whose lexicographic ordering
/// matches the desired sort order.
///
/// FractionalIndex hex strings (e.g. `"80"`, `"7F80"`, `"A0"`) are passed
/// through as-is — their lexicographic byte order is the correct sort order.
/// Integers are zero-padded to 20 digits, floats are converted via their
/// IEEE 754 bits (with sign-bit flipping for negative values).
pub fn sort_value(v: Option<&Value>) -> String {
    match v {
        Some(Value::Integer(i)) => format!("{:020}", *i as i128),
        Some(Value::Float(f)) => {
            let bits = f.to_bits();
            // Flip sign bit so IEEE 754 bit order matches numeric order.
            let adjusted = if bits & (1 << 63) != 0 {
                !bits
            } else {
                bits | (1 << 63)
            };
            format!("{:020}", adjusted)
        }
        Some(Value::String(s)) => s.clone(),
        _ => "\u{10FFFF}".to_string(),
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

pub fn resolve_states<K: RowKey>(args: &ResolvedArgs, row: &HashMap<K, Value>) -> Vec<String> {
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
            ka.cmp(&kb)
        });

        let mut roots: Vec<usize> = Vec::new();
        let mut children_of: HashMap<String, Vec<usize>> = HashMap::new();

        let ids: std::collections::HashSet<&str> = sorted_rows
            .iter()
            .filter_map(|r| r.get("id").and_then(|v| v.as_string()))
            .collect();

        for (i, row) in sorted_rows.iter().enumerate() {
            let pid = row
                .get(parent_id_col)
                .and_then(|v| v.as_string())
                .unwrap_or("");

            let parent_exists = ids.contains(pid);

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
            .is_some_and(|s| s.eq_ignore_ascii_case("drawer"))
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
            let is_drawer = collapse_to.is_some_and(|s| s.eq_ignore_ascii_case("drawer"));
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
/// the design plan — no silent first-arg fallback). // ALLOW(fallback):
/// historical name in doc comment
pub trait ValueFnLookup {
    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<InterpValue>;
}

/// Built-in value functions available to every caller — `concat` for
/// now, more added later. Frontend registries chain through this as a
/// fallback so user-supplied registrations can still override built-in //
/// ALLOW(fallback): doc describes registry chaining order names (collision
/// check at `register_value_fn` enforces uniqueness against widgets, not
/// against the core list).
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

/// Key bound for row maps accepted by the eval entry points: both the
/// Arc<str>-keyed `StorageEntity` (engine side) and the String-keyed
/// `DataRow` (frontend/FRB side) qualify.
pub trait RowKey: std::borrow::Borrow<str> + std::hash::Hash + Eq {}
impl<T: std::borrow::Borrow<str> + std::hash::Hash + Eq> RowKey for T {}

/// Scalar-only legacy path (preserved behavior for callers that don't
/// have a value-fn registry). Thin wrapper over `eval_to_interp` that
/// drops `Rows` to `Value::Null` with a warning.
pub fn resolve_args<K: RowKey>(args: &[Arg], row: &HashMap<K, Value>) -> ResolvedArgs {
    resolve_args_with(args, row, &CORE_VALUE_FN_LOOKUP)
}

/// Resolve arguments with value-function dispatch.
///
/// Scalar-valued results are placed in `positional` / `named`; row-set
/// results end up in `rows` under their named-arg key. Positional
/// row-sets panic — positional args are scalar by convention, so a row
/// set there is a user error in the DSL worth surfacing at the first
/// evaluation.
pub fn resolve_args_with<K: RowKey>(
    args: &[Arg],
    row: &HashMap<K, Value>,
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
                        "value-function returned Rows in positional position; use a named arg \
                         (e.g. `collection:`) instead"
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
pub fn eval_to_value<K: RowKey>(expr: &RenderExpr, row: &HashMap<K, Value>) -> Value {
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
/// pre-F1 "silently return first arg" fallback is gone — a typo'd //
/// ALLOW(fallback): historical name in doc comment function call now produces a
/// visible `Null` at the consumer.
pub fn eval_to_interp<K: RowKey>(
    expr: &RenderExpr,
    row: &HashMap<K, Value>,
    fns: &dyn ValueFnLookup,
) -> InterpValue {
    use InterpValue::*;
    match expr {
        RenderExpr::Literal { value } => Value(value.clone()),
        RenderExpr::ColumnRef { name } => Value(
            row.get(name.as_str())
                .cloned()
                .unwrap_or(crate::Value::Null),
        ),
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
                // F1: silent first-arg default removed. Unknown name // ALLOW(fallback): historical
                // reference in code comment → Null. Built-in fns (`concat`, ...)
                // are reachable through `CORE_VALUE_FN_LOOKUP` and should be
                // chained into the caller's lookup if a frontend wants to keep
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

/// Arithmetic ops (`+ - * /`). Type-mismatched or div-by-zero operands yield
/// `Null`. Only called by `eval_binary_op` for arithmetic operators.
fn eval_arithmetic(op: &BinaryOperator, left: &Value, right: &Value) -> Value {
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
        other => unreachable!("eval_arithmetic called with non-arithmetic op {other:?}"),
    }
}

/// Ordering comparisons (`> < >= <=`). Non-numeric or mismatched operands
/// yield `Boolean(false)`. Only called by `eval_binary_op` for ordering ops.
fn eval_ordering(op: &BinaryOperator, left: &Value, right: &Value) -> Value {
    match op {
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
        other => unreachable!("eval_ordering called with non-ordering op {other:?}"),
    }
}

/// Boolean ops (`&& ||`). Non-boolean operands yield `Boolean(false)`. Only
/// called by `eval_binary_op` for logical operators.
fn eval_logical(op: &BinaryOperator, left: &Value, right: &Value) -> Value {
    match (op, left, right) {
        (BinaryOperator::And, Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(*a && *b),
        (BinaryOperator::Or, Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(*a || *b),
        (BinaryOperator::And | BinaryOperator::Or, _, _) => Value::Boolean(false),
        (other, _, _) => unreachable!("eval_logical called with non-logical op {other:?}"),
    }
}

pub fn eval_binary_op(op: &BinaryOperator, left: &Value, right: &Value) -> Value {
    match op {
        BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul | BinaryOperator::Div => {
            eval_arithmetic(op, left, right)
        }
        BinaryOperator::Eq => Value::Boolean(left == right),
        BinaryOperator::Neq => Value::Boolean(left != right),
        BinaryOperator::Gt | BinaryOperator::Lt | BinaryOperator::Gte | BinaryOperator::Lte => {
            eval_ordering(op, left, right)
        }
        BinaryOperator::And | BinaryOperator::Or => eval_logical(op, left, right),
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
    fn test_eval_binary_op_type_mismatch_fallbacks() {
        // Arithmetic on incompatible types -> Null.
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Add,
                &Value::Integer(1),
                &Value::Boolean(true)
            ),
            Value::Null
        );
        // Ordering on non-numeric operands -> Boolean(false).
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Gt,
                &Value::String("a".into()),
                &Value::String("b".into())
            ),
            Value::Boolean(false)
        );
        // Logical on non-boolean operands -> Boolean(false).
        assert_eq!(
            eval_binary_op(&BinaryOperator::And, &Value::Integer(1), &Value::Integer(0)),
            Value::Boolean(false)
        );
    }

    #[test]
    fn test_eval_to_value_literal() {
        let row = crate::StorageEntity::new();
        let expr = RenderExpr::Literal {
            value: Value::Integer(42),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::Integer(42));
    }

    #[test]
    fn test_eval_to_value_column_ref() {
        let mut row = crate::StorageEntity::new();
        row.insert("name".into(), Value::String("Alice".into()));
        let expr = RenderExpr::ColumnRef {
            name: "name".to_string(),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::String("Alice".into()));
    }

    #[test]
    fn test_eval_to_value_missing_column() {
        let row = crate::StorageEntity::new();
        let expr = RenderExpr::ColumnRef {
            name: "missing".to_string(),
        };
        assert_eq!(eval_to_value(&expr, &row), Value::Null);
    }

    #[test]
    fn test_eval_to_value_binary_op() {
        let row = crate::StorageEntity::new();
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
        let row = crate::StorageEntity::new();
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
        let row = crate::StorageEntity::new();
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
        let mut row = crate::StorageEntity::new();
        row.insert("col1".into(), Value::String("val1".into()));

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
        assert_eq!(Value::Float(2.5).to_display_string(), "2.5");
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
        assert_eq!(state_display("TODO"), ("TODO", "muted"));
        assert_eq!(state_display("DOING"), ("DOING", "warning"));
        assert_eq!(state_display("DONE"), ("[x]", "success"));
        assert_eq!(state_display(""), ("", "muted"));
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
        let row = crate::StorageEntity::new();
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
        let row = crate::StorageEntity::new();
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
        let row = crate::StorageEntity::new();
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
        let mut row = crate::StorageEntity::new();
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

#[cfg(test)]
mod mutation_gap_tests {
    use super::*;

    fn empty_args() -> ResolvedArgs {
        ResolvedArgs {
            positional: vec![],
            positional_exprs: vec![],
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
        }
    }

    #[test]
    fn sort_value_orders_ints_floats_strings_and_missing() {
        let sv = |v: &Value| sort_value(Some(v));

        assert!(sv(&Value::Integer(2)) < sv(&Value::Integer(10)));
        assert!(sv(&Value::Integer(0)) < sv(&Value::Integer(7)));

        // IEEE bit-flip trick must order negatives < zero < positives.
        assert!(sv(&Value::Float(-2.5)) < sv(&Value::Float(-1.5)));
        assert!(sv(&Value::Float(-1.5)) < sv(&Value::Float(0.0)));
        assert!(sv(&Value::Float(0.0)) < sv(&Value::Float(2.5)));
        assert!(sv(&Value::Float(2.5)) < sv(&Value::Float(10.25)));

        // FractionalIndex hex strings pass through untouched.
        assert_eq!(sv(&Value::String("7F80".to_string())), "7F80");

        // Missing sorts after any string/int representation.
        let missing = sort_value(None);
        assert_eq!(missing, "\u{10FFFF}");
        assert!(sv(&Value::String("zz".to_string())) < missing);
        assert!(sv(&Value::Integer(i64::MAX)) < missing);
    }

    #[test]
    fn cmp_values_total_order() {
        use std::cmp::Ordering::*;
        let int = |i: i64| Value::Integer(i);
        let f = |x: f64| Value::Float(x);
        let s = |x: &str| Value::String(x.to_string());

        assert_eq!(cmp_values(Some(&int(1)), Some(&int(2))), Less);
        assert_eq!(cmp_values(Some(&int(2)), Some(&int(1))), Greater);
        assert_eq!(cmp_values(Some(&f(1.5)), Some(&f(2.5))), Less);
        assert_eq!(cmp_values(Some(&s("a")), Some(&s("b"))), Less);
        assert_eq!(cmp_values(None, None), Equal);
        assert_eq!(cmp_values(None, Some(&int(1))), Greater);
        assert_eq!(cmp_values(Some(&int(1)), None), Less);
    }

    #[test]
    fn binary_op_arithmetic_semantics() {
        let i = |x: i64| Value::Integer(x);
        let f = |x: f64| Value::Float(x);
        let s = |x: &str| Value::String(x.to_string());

        assert_eq!(eval_binary_op(&BinaryOperator::Add, &i(2), &i(3)), i(5));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Add, &f(1.5), &f(2.25)),
            f(3.75)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Add, &s("a"), &s("b")),
            s("ab")
        );

        assert_eq!(eval_binary_op(&BinaryOperator::Sub, &i(5), &i(2)), i(3));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Sub, &f(5.5), &f(2.0)),
            f(3.5)
        );

        assert_eq!(eval_binary_op(&BinaryOperator::Mul, &i(3), &i(4)), i(12));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Mul, &f(1.5), &f(2.0)),
            f(3.0)
        );

        assert_eq!(eval_binary_op(&BinaryOperator::Div, &i(7), &i(2)), i(3));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Div, &f(3.0), &f(2.0)),
            f(1.5)
        );
        // Division by zero yields Null, never panics.
        assert_eq!(
            eval_binary_op(&BinaryOperator::Div, &i(7), &i(0)),
            Value::Null
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Div, &f(1.0), &f(0.0)),
            Value::Null
        );

        // Type mismatch yields Null.
        assert_eq!(
            eval_binary_op(&BinaryOperator::Add, &i(1), &f(1.0)),
            Value::Null
        );
    }

    #[test]
    fn binary_op_ordering_semantics() {
        let i = |x: i64| Value::Integer(x);
        let f = |x: f64| Value::Float(x);
        let b = |x: bool| Value::Boolean(x);

        assert_eq!(eval_binary_op(&BinaryOperator::Gt, &i(3), &i(2)), b(true));
        assert_eq!(eval_binary_op(&BinaryOperator::Gt, &i(2), &i(2)), b(false));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Gt, &f(2.5), &f(2.0)),
            b(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Gt, &f(2.0), &f(2.0)),
            b(false)
        );

        assert_eq!(eval_binary_op(&BinaryOperator::Lt, &i(1), &i(2)), b(true));
        assert_eq!(eval_binary_op(&BinaryOperator::Lt, &i(2), &i(2)), b(false));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Lt, &f(1.0), &f(2.0)),
            b(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Lt, &f(2.0), &f(2.0)),
            b(false)
        );

        assert_eq!(eval_binary_op(&BinaryOperator::Gte, &i(2), &i(2)), b(true));
        assert_eq!(eval_binary_op(&BinaryOperator::Gte, &i(1), &i(2)), b(false));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Gte, &f(2.0), &f(2.0)),
            b(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Gte, &f(1.0), &f(2.0)),
            b(false)
        );

        assert_eq!(eval_binary_op(&BinaryOperator::Lte, &i(2), &i(2)), b(true));
        assert_eq!(eval_binary_op(&BinaryOperator::Lte, &i(3), &i(2)), b(false));
        assert_eq!(
            eval_binary_op(&BinaryOperator::Lte, &f(2.0), &f(2.0)),
            b(true)
        );
        assert_eq!(
            eval_binary_op(&BinaryOperator::Lte, &f(3.0), &f(2.0)),
            b(false)
        );
    }

    #[test]
    fn state_and_color_display_tables() {
        assert_eq!(state_icon(""), "");
        assert_eq!(state_icon("CANCELLED"), "✗");
        assert_eq!(state_icon("DOING"), "◑");
        assert_eq!(state_icon("DONE"), "✓");
        assert_eq!(state_icon("TODO"), "○");

        assert_eq!(state_display(""), ("", "muted"));
        assert_eq!(state_display("TODO"), ("TODO", "muted"));
        assert_eq!(state_display("DOING"), ("DOING", "warning"));
        assert_eq!(state_display("DONE"), ("[x]", "success"));
        assert_eq!(state_display("CANCELLED"), ("CANCELLED", "error"));
        assert_eq!(state_display("WAITING"), ("WAITING", "primary"));

        assert_eq!(resolve_color_name("red"), "#FF0000");
        assert_eq!(resolve_color_name("green"), "#00FF00");
        assert_eq!(resolve_color_name("blue"), "#0000FF");
        assert_eq!(resolve_color_name("yellow"), "#FFFF00");
        assert_eq!(resolve_color_name("white"), "#FFFFFF");
        assert_eq!(resolve_color_name("gray"), "#808080");
        assert_eq!(resolve_color_name("grey"), "#808080");
        assert_eq!(resolve_color_name("muted"), "#808080");
        assert_eq!(resolve_color_name("#ABCDEF"), "#ABCDEF");
        assert_eq!(resolve_color_name("chartreuse"), "#FFFFFF");
    }

    #[test]
    fn resolve_states_template_and_default() {
        let row: HashMap<String, Value> = [(
            "sts".to_string(),
            Value::Array(vec![
                Value::String("A".to_string()),
                Value::String("B".to_string()),
            ]),
        )]
        .into_iter()
        .collect();

        let mut args = empty_args();
        args.templates.insert(
            "states".to_string(),
            RenderExpr::ColumnRef {
                name: "sts".to_string(),
            },
        );
        assert_eq!(
            resolve_states(&args, &row),
            vec!["A".to_string(), "B".to_string()]
        );

        let default = resolve_states(&empty_args(), &row);
        assert_eq!(
            default,
            vec![
                String::new(),
                "TODO".to_string(),
                "DOING".to_string(),
                "DONE".to_string()
            ]
        );
    }

    #[test]
    fn drawer_rows_and_column_refs() {
        let drawer_row: Arc<DataRow> = Arc::new(
            [(
                "collapse_to".to_string(),
                Value::String("Drawer".to_string()),
            )]
            .into_iter()
            .collect(),
        );
        let plain_row: Arc<DataRow> = Arc::new(
            [(
                "collapse_to".to_string(),
                Value::String("inline".to_string()),
            )]
            .into_iter()
            .collect(),
        );
        assert!(has_drawer_rows(&[plain_row.clone(), drawer_row]));
        assert!(!has_drawer_rows(&[plain_row]));
        assert!(!has_drawer_rows(&[]));

        let col = RenderExpr::ColumnRef {
            name: "title".to_string(),
        };
        assert_eq!(column_ref_name(&col), Some("title"));
        assert_eq!(column_ref_name(&RenderExpr::Array { items: vec![] }), None);

        let mut args = empty_args();
        assert_eq!(sort_key_column(&args), None);
        args.templates.insert(
            "sort_key".to_string(),
            RenderExpr::ColumnRef {
                name: "seq".to_string(),
            },
        );
        assert_eq!(sort_key_column(&args), Some("seq"));
    }

    #[test]
    fn resolved_args_getters() {
        let mut args = empty_args();
        args.positional = vec![
            Value::Integer(7),
            Value::String("s".to_string()),
            Value::Float(1.25),
            Value::Boolean(true),
            Value::Null,
        ];
        args.positional_exprs = vec![RenderExpr::ColumnRef {
            name: "c0".to_string(),
        }];
        args.named = [
            ("s".to_string(), Value::String("v".to_string())),
            ("f".to_string(), Value::Float(2.5)),
            ("i".to_string(), Value::Integer(3)),
            ("bt".to_string(), Value::Boolean(true)),
            ("bf".to_string(), Value::Boolean(false)),
        ]
        .into_iter()
        .collect();
        args.templates.insert(
            "tpl".to_string(),
            RenderExpr::ColumnRef {
                name: "x".to_string(),
            },
        );

        assert_eq!(args.get_string("s"), Some("v"));
        assert_eq!(args.get_string("missing"), None);
        assert_eq!(args.get_string_or("s", "d"), "v");
        assert_eq!(args.get_string_or("missing", "d"), "d");

        assert_eq!(args.get_f64("f"), Some(2.5));
        assert_eq!(args.get_f64("i"), Some(3.0));
        assert_eq!(args.get_f64("s"), None);
        assert_eq!(args.get_f64("missing"), None);

        assert_eq!(args.get_bool("bt"), Some(true));
        assert_eq!(args.get_bool("bf"), Some(false));
        assert_eq!(args.get_bool("s"), None);

        assert_eq!(args.get_positional_f64(0), Some(7.0));
        assert_eq!(args.get_positional_f64(2), Some(1.25));
        assert_eq!(args.get_positional_f64(9), None);

        assert_eq!(args.get_positional_string(0), Some("7".to_string()));
        assert_eq!(args.get_positional_string(1), Some("s".to_string()));
        assert_eq!(args.get_positional_string(2), Some("1.25".to_string()));
        assert_eq!(args.get_positional_string(3), Some("true".to_string()));
        assert_eq!(args.get_positional_string(4), None);
        assert_eq!(args.get_positional_string(9), None);

        assert_eq!(args.get_positional_column_name(0), Some("c0"));
        assert_eq!(args.get_positional_column_name(5), None);

        assert!(matches!(
            args.get_template("tpl"),
            Some(RenderExpr::ColumnRef { name }) if name == "x"
        ));
        assert!(args.get_template("nope").is_none());
        assert!(args.get_rows("nope").is_none());
    }
}
