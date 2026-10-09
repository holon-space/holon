use std::collections::HashMap;
use std::sync::Arc;

use crate::Value;
use crate::computation::ArithOp;
use crate::computation::CmpOp;
use crate::computation::ComputeError;
use crate::computation::arith_apply;
use crate::computation::as_bool;
use crate::computation::concat_apply;
use crate::interp_value::InterpValue;
use crate::interp_value::ReactiveRowProvider;
use crate::render_types::Arg;
use crate::render_types::BinaryOperator;
use crate::render_types::RenderExpr;
use crate::theme_token::ThemeToken;
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
    // Both spellings are accepted template args (see `is_template_arg`);
    // profiles/index.org write `sortkey:`, so ignoring it here silently
    // fell back to `data_row_sort_key`.
    //
    // Two authoring forms are honored: `sortkey: col("name")` and the plain
    // string form `sortkey: "name"` / `sortkey: "-name"`. The leading `-`
    // means DESCENDING (`sorted_rows` strips it and reverses); the journal
    // feed uses `"-content"` for newest-first. The string form is returned
    // verbatim (with any `-`) so the direction survives to the sort.
    match args
        .get_template("sortkey")
        .or_else(|| args.get_template("sort_key"))
    {
        Some(RenderExpr::ColumnRef { name }) => Some(name.as_str()),
        Some(RenderExpr::Literal {
            value: Value::String(s),
        }) => Some(s.as_str()),
        _ => None,
    }
}

/// Split a sort-key spec into `(column, descending)`. A leading `-` marks a
/// descending sort (e.g. `"-content"` → `("content", true)`); otherwise
/// ascending.
pub fn parse_sort_key(spec: &str) -> (&str, bool) {
    match spec.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (spec, false),
    }
}

/// Invert the lexicographic ordering of a `sort_value` key so an
/// ASCENDING string sort (the streaming `MutableTree`'s only order) yields a
/// DESCENDING result. Each Unicode scalar is complemented against the scalar
/// range; the streaming path uses this for `-`-prefixed sort keys so its order
/// matches the static path's `ord.reverse()`. Exact for the fixed-width keys
/// `sort_value` emits (ISO dates, zero-padded numbers); it does not attempt to
/// correct prefix-length effects for ragged strings.
pub fn reverse_order_key(key: &str) -> String {
    key.chars()
        .map(|c| char::from_u32(0x0010_FFFF - c as u32).unwrap_or('\u{FFFD}'))
        .collect()
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
    if let Some(spec) = sort_key {
        let (col, descending) = parse_sort_key(spec);
        sorted.sort_by(|a, b| {
            let ord = cmp_values(a.get(col), b.get(col));
            if descending { ord.reverse() } else { ord }
        });
    }
    sorted
}

/// The state cycle a `states:` arg names. A `Null` (a document without
/// `#+TODO:`) or an empty list takes the builtin cycle; any other value that is
/// not a list of keywords is refused.
pub fn resolve_states<K: RowKey>(
    args: &ResolvedArgs,
    row: &HashMap<K, Value>,
) -> Result<Vec<String>, ComputeError> {
    let builtin = || {
        vec![
            String::new(),
            "TODO".to_string(),
            "DOING".to_string(),
            "DONE".to_string(),
        ]
    };
    let Some(states_expr) = args.get_template("states") else {
        return Ok(builtin());
    };
    let not_keywords = |value: Value| ComputeError::WrongType {
        context: "states".to_string(),
        expected: "a list of state keywords",
        value,
    };
    match eval_plain_value(states_expr, row)? {
        Value::Null => Ok(builtin()),
        Value::Array(items) if items.is_empty() => Ok(builtin()),
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_string()
                    .map(str::to_string)
                    .ok_or_else(|| not_keywords(Value::Array(items.clone())))
            })
            .collect(),
        other => Err(not_keywords(other)),
    }
}

pub fn cycle_state(current: &str, states: &[String]) -> String {
    if states.is_empty() {
        return String::new();
    }
    // LogSeq dialect flow (ForeignVaultCompat §4): a block already in a
    // LogSeq keyword stays in the LogSeq ring — LATER -> NOW -> DONE — rather
    // than snapping into the native TODO/DOING/DONE ring (which the default
    // `states` list carries and which does not contain LATER/NOW). This keeps
    // an imported LogSeq task's keyword faithful across a cycle. Once it
    // reaches DONE it re-enters the native ring (DONE -> "" -> TODO).
    match current {
        "LATER" => return "NOW".to_string(),
        "NOW" => return "DONE".to_string(),
        _ => {}
    }
    let idx = states.iter().position(|s| s == current).unwrap_or(0);
    let next = (idx + 1) % states.len();
    states[next].clone()
}

/// Font size (px) for a semantic text `style` keyword (`text(.., #{style:
/// "h1"})`).
///
/// Single source of truth for the heading type scale, shared by every frontend
/// (the value flows through the `text` widget's `size` prop, so gpui/dioxus
/// render identically and the PBT widget-tree snapshot can assert on it).
/// Returns `None` for an unrecognized keyword so the caller can fail loud
/// instead of silently rendering a heading at body size — the exact silent
/// drop that let the page-title-not-a-heading regression ship.
pub fn text_style_font_size(style: &str) -> Option<f32> {
    match style {
        "h1" => Some(28.0),
        "h2" => Some(22.0),
        "h3" => Some(18.0),
        "body" => Some(15.0),
        _ => None,
    }
}

/// Everything a `style` keyword does to a `text()` render: the type scale and
/// the weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyleTreatment {
    pub size: f32,
    pub bold: bool,
}

/// Resolve a semantic text `style` keyword into its full treatment.
///
/// Every frontend calls THIS, not `text_style_font_size`, so size and weight
/// cannot drift apart per platform: a keyword the scale does not know yields
/// `None` and changes nothing at all, rather than one frontend bolding an
/// unrecognized `h7` that another leaves at body weight.
pub fn text_style_treatment(style: &str) -> Option<TextStyleTreatment> {
    Some(TextStyleTreatment {
        size: text_style_font_size(style)?,
        bold: style.starts_with('h'),
    })
}

/// Resolve what a `text(..)` widget shows: the real `content`, or a disclosed
/// placeholder when `content` is empty. `empty_placeholder` is the `#{empty:
/// ..}` named arg (`text(col("content"), #{empty: "(untitled)"})`). Returns the
/// string to display plus whether it is the placeholder, so a caller can style
/// the placeholder as degraded (muted/italic) — a genuinely empty value must
/// never render as a blank row (fail-loud, never fake). Single source shared by
/// every frontend, so the "no blank row" guarantee has one implementation.
pub fn text_display<'a>(content: &'a str, empty_placeholder: Option<&'a str>) -> (&'a str, bool) {
    match empty_placeholder {
        Some(p) if content.is_empty() => (p, true),
        _ => (content, false),
    }
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

/// The glyph and the theme colour a task state is drawn with.
///
/// The colour is a [`ThemeToken`], not a name, so it is total by construction:
/// a caller cannot receive a colour string that no resolver knows, which is how
/// `CANCELLED` and every keyword outside the built-in list came to be drawn in
/// body foreground after the vocabulary gave them `error` and `primary`.
///
/// A keyword outside the list takes `Primary`, which is this vocabulary's
/// declared default for a state it does not recognise. That is a decision, not
/// an absence: a foreign vault's own keyword is a state, and drawing it as one
/// says more than drawing it as unremarkable text.
pub fn state_display(state: &str) -> (&str, ThemeToken) {
    match state {
        "" => ("", ThemeToken::Muted),
        "TODO" => ("TODO", ThemeToken::Muted),
        "DOING" => ("DOING", ThemeToken::Warning),
        "DONE" => ("[x]", ThemeToken::Success),
        "CANCELLED" => ("CANCELLED", ThemeToken::Error),
        // LogSeq dialect (ForeignVaultCompat §4): LATER is TODO-family
        // (not started), NOW is DOING-family (in progress). Rendered with
        // the same colours as their native counterparts; the label keeps
        // the source keyword for round-trip fidelity.
        "LATER" => ("LATER", ThemeToken::Muted),
        "NOW" => ("NOW", ThemeToken::Warning),
        _ => (state, ThemeToken::Primary),
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
    /// Build the sibling-bucketed tree with **per-level sort keys** (RULING
    /// C1'). Bucketing under a parent is structural (a row is a child iff its
    /// `parent_id_col` names a present `id`); only the *within-bucket* order is
    /// a sort concern, and it differs by level:
    ///
    /// - **CHILD buckets** keep `sort_col` (document order) — the streaming
    ///   `sort_value` order the global sort below applies.
    /// - **ROOTS** sort by `root_sort_key` when the render declares one (a spec
    ///   like `"-added_ts"`, `-` = descending), so a tree can honor its backing
    ///   query's top-level `ORDER BY` even though query row order does not
    ///   survive the CDC pipeline (`CdcAccumulator` is a `HashMap`). A `None`
    ///   root key leaves roots in `sort_col` order — **byte-identical** to the
    ///   pre-C1' behavior for every render that declares no root key.
    pub fn from_rows(
        rows: &[Arc<DataRow>],
        parent_id_col: &str,
        sort_col: &str,
        root_sort_key: Option<&str>,
    ) -> Self {
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
            // ALLOW(raw_row_id_column): key — membership set for the parent/child join; compared as
            // text, never converted
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

        // Per-level sort keys: child buckets already carry `sort_col` order
        // (the global sort above); ROOTS re-sort by the declared root key when
        // present. The re-sort is STABLE over the `sort_col`-ordered `roots`
        // vec, so roots that tie on the root key keep `sort_col` order (the
        // deterministic tie-break). `None` skips it → roots stay in `sort_col`
        // order (pre-C1', byte-identical).
        if let Some(spec) = root_sort_key {
            let (col, descending) = parse_sort_key(spec);
            roots.sort_by(|&a, &b| {
                let ord = cmp_values(sorted_rows[a].get(col), sorted_rows[b].get(col));
                if descending { ord.reverse() } else { ord }
            });
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

            // ALLOW(raw_row_id_column): key — index key into `children_of`; compared as
            // text, never converted
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
                // ALLOW(raw_row_id_column): label — the partition region's id; its one
                // production consumer (waterui `columns`) reads only the widget
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
    /// The widget these args were resolved for, when the caller knew it.
    /// Its declared params answer "is this arg a template?" per-widget;
    /// `None` (value functions, synthetic arg bags) falls back to the
    /// global `is_template_arg` allowlist.
    pub widget: Option<&'static crate::WidgetMeta>,
}

impl ResolvedArgs {
    pub fn from_positional_value(value: Value) -> Self {
        Self {
            positional: vec![value],
            positional_exprs: Vec::new(),
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
            widget: None,
        }
    }

    pub fn from_positional_exprs(exprs: Vec<RenderExpr>) -> Self {
        Self {
            positional: Vec::new(),
            positional_exprs: exprs,
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
            widget: None,
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

    /// Read a named bool arg, failing loud when it is present with a
    /// non-boolean value. `Ok(None)` = absent, `Ok(Some(b))` = a real boolean,
    /// `Err` = present but not a boolean literal (a config bug the caller must
    /// surface, never silently coerce).
    pub fn get_bool_strict(&self, name: &str) -> Result<Option<bool>, String> {
        match self.named.get(name) {
            None => Ok(None),
            Some(Value::Boolean(b)) => Ok(Some(*b)),
            Some(other) => Err(format!("arg `{name}` must be a boolean, got {other:?}")),
        }
    }

    /// Read a named numeric arg, failing loud when it is present with a
    /// non-numeric value. `Ok(None)` = absent, `Ok(Some(f))` = a real number,
    /// `Err` = present but not numeric (a config bug the caller must surface,
    /// never silently coerce to a default). Mirrors [`Self::get_bool_strict`].
    pub fn get_f64_strict(&self, name: &str) -> Result<Option<f64>, String> {
        match self.named.get(name) {
            None => Ok(None),
            Some(v) => match value_to_f64(v) {
                Some(f) => Ok(Some(f)),
                None => Err(format!("arg `{name}` must be a number, got {v:?}")),
            },
        }
    }

    /// The value a typed param was given: positional slot `slot` when it holds
    /// one, else the named arg. `Null` (a nested widget call, a missing
    /// column) counts as not given.
    fn param_value(&self, slot: Option<usize>, name: &str) -> Option<&Value> {
        slot.and_then(|i| self.positional.get(i))
            .filter(|v| !v.is_null())
            .or_else(|| self.named.get(name).filter(|v| !v.is_null()))
    }

    /// A typed `String` param. A scalar is drawn as its text; a value with no
    /// text form is refused, never replaced by the default.
    pub fn param_string(&self, slot: Option<usize>, name: &str) -> Result<Option<String>, String> {
        self.param_value(slot, name)
            .map(|v| match v {
                Value::String(s) | Value::DateTime(s) | Value::Json(s) => Ok(s.clone()),
                Value::Integer(i) => Ok(i.to_string()),
                Value::Float(f) => Ok(f.to_string()),
                Value::Boolean(b) => Ok(b.to_string()),
                other => Err(format!("arg `{name}` must be text, got {other:?}")),
            })
            .transpose()
    }

    /// A typed number param; anything but a number is refused.
    pub fn param_f64(&self, slot: Option<usize>, name: &str) -> Result<Option<f64>, String> {
        self.param_value(slot, name)
            .map(|v| {
                value_to_f64(v).ok_or_else(|| format!("arg `{name}` must be a number, got {v:?}"))
            })
            .transpose()
    }

    /// A typed bool param; anything but a boolean is refused.
    pub fn param_bool(&self, name: &str) -> Result<Option<bool>, String> {
        self.param_value(None, name)
            .map(|v| match v {
                Value::Boolean(b) => Ok(*b),
                other => Err(format!("arg `{name}` must be a boolean, got {other:?}")),
            })
            .transpose()
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

    /// The unevaluated expression passed under `name`, or `None` when the arg
    /// was absent.
    ///
    /// Asking for a name that this widget does not classify as a template is a
    /// programming error, not an absent arg: such an arg was already evaluated
    /// to a scalar during resolution, so the lookup could never succeed and the
    /// feature behind it would be silently dead. Declare the param as `Expr` /
    /// `Collection` on the widget (or, for `raw fn` widgets, add it to
    /// `is_template_arg`).
    pub fn get_template(&self, name: &str) -> Option<&RenderExpr> {
        assert!(
            is_template_arg_for(self.widget, name),
            "widget `{}`: get_template({name:?}) but `{name}` is not a \
             declared Expr/Collection param and is not on the \
             is_template_arg allowlist — it was resolved as a scalar",
            self.widget.map_or("<unknown>", |m| m.name),
        );
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
/// value-function registry). A call to a name of neither kind fails the
/// enclosing evaluation, as does a value function refusing its arguments.
pub trait ValueFnLookup {
    /// `None` for a name this lookup cannot call.
    fn call_kind(&self, name: &str) -> Option<CallKind>;

    /// Answers `Some` for every name whose `call_kind` is `ValueFn`.
    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<Result<InterpValue, ComputeError>>;

    /// The error for `call`, whose `name` has no `call_kind`.
    fn unknown_function(&self, name: &str, call: String) -> ComputeError;
}

/// What a called name in the render DSL refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    ValueFn,
    /// Its arguments are evaluated where the widget is built, not where the
    /// call is passed as an argument.
    Widget,
}

/// The value functions every evaluation can call, a render's or not.
pub const CORE_VALUE_FN_NAMES: &[&str] = &["concat"];

/// The core value function named `name`. A render's lookup chains through
/// this beneath its registered value functions.
pub fn core_value_fn(name: &str) -> Option<fn(&ResolvedArgs) -> Value> {
    match name {
        "concat" => Some(concat_invoke),
        _ => None,
    }
}

/// The names a value evaluated outside any render can call: the core value
/// functions, and no widget.
struct PlainValueFns;

impl ValueFnLookup for PlainValueFns {
    fn call_kind(&self, name: &str) -> Option<CallKind> {
        core_value_fn(name).map(|_| CallKind::ValueFn)
    }

    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<Result<InterpValue, ComputeError>> {
        core_value_fn(name).map(|f| Ok(InterpValue::Value(f(args))))
    }

    fn unknown_function(&self, name: &str, call: String) -> ComputeError {
        ComputeError::NotAPlainValueFunction {
            name: name.to_string(),
            call,
        }
    }
}

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

/// Everything a render expression's leaves can read while it is evaluated.
pub struct EvalEnv<'a, K: RowKey> {
    pub row: &'a HashMap<K, Value>,
}

impl<'a, K: RowKey> EvalEnv<'a, K> {
    pub fn of_row(row: &'a HashMap<K, Value>) -> Self {
        EvalEnv { row }
    }
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
    env: &EvalEnv<'_, K>,
    fns: &dyn ValueFnLookup,
) -> Result<ResolvedArgs, ComputeError> {
    resolve_args_for_widget(args, env, fns, None)
}

/// `resolve_args_with` for a call site that knows which widget it is
/// resolving args for: the widget's declared params decide templateness,
/// so a migrated widget needs no entry in `is_template_arg`.
pub fn resolve_args_for_widget<K: RowKey>(
    args: &[Arg],
    env: &EvalEnv<'_, K>,
    fns: &dyn ValueFnLookup,
    widget: Option<&'static crate::WidgetMeta>,
) -> Result<ResolvedArgs, ComputeError> {
    let mut positional = Vec::new();
    let mut positional_exprs = Vec::new();
    let mut named = HashMap::new();
    let mut rows = HashMap::new();
    let mut templates = HashMap::new();

    for arg in args {
        match &arg.name {
            Some(name) if is_template_arg_for(widget, name) => {
                templates.insert(name.clone(), arg.value.clone());
            }
            Some(name) => match eval_to_interp(&arg.value, env, fns)? {
                InterpValue::Value(v) => {
                    named.insert(name.clone(), v);
                }
                InterpValue::Rows(p) => {
                    rows.insert(name.clone(), p);
                }
            },
            None => {
                positional_exprs.push(arg.value.clone());
                match eval_to_interp(&arg.value, env, fns)? {
                    InterpValue::Value(v) => positional.push(v),
                    InterpValue::Rows(_) => panic!(
                        "value-function returned Rows in positional position; use a named arg \
                         (e.g. `collection:`) instead"
                    ),
                }
            }
        }
    }

    Ok(ResolvedArgs {
        positional,
        positional_exprs,
        named,
        rows,
        templates,
        widget,
    })
}

/// Per-widget answer to "must this named arg stay an unevaluated template?".
///
/// A widget that declares the param decides for itself; everything else —
/// `raw fn` widgets with no declared params, value functions, undeclared args
/// — is judged by the global allowlist below. Migrating a widget to typed
/// params is therefore what retires its allowlist entries.
pub fn is_template_arg_for(widget: Option<&crate::WidgetMeta>, name: &str) -> bool {
    match widget.and_then(|m| m.classifies_as_template(name)) {
        Some(declared) => declared,
        None => is_template_arg(name),
    }
}

/// Global allowlist for widgets that have not been migrated to typed
/// params. Templateness is really per-widget, so every name here is a widget's
/// question answered for the whole DSL — shrink this list, don't grow it.
pub fn is_template_arg(name: &str) -> bool {
    matches!(
        name,
        "item_template"
            | "item"
            | "header"
            | "header_template"
            | "child_template"
            | "action"
            | "shift_action"
            | "cmd_action"
            | "ctrl_action"
            | "alt_action"
            | "parent_id"
            | "sortkey"
            | "sort_key"
            | "context"
            | "states"
            | "columns"
    ) || name.starts_with("mode_")
}

/// Evaluate a value outside any render — a rule action's param, a filter
/// predicate, a `states:` list: only the core value functions can be called.
/// A render's arguments are evaluated with the lookup its `BuilderServices`
/// hands out instead.
pub fn eval_plain_value<K: RowKey>(
    expr: &RenderExpr,
    row: &HashMap<K, Value>,
) -> Result<Value, ComputeError> {
    match eval_to_interp(expr, &EvalEnv::of_row(row), &PlainValueFns)? {
        InterpValue::Value(v) => Ok(v),
        InterpValue::Rows(_) => unreachable!("a core value function returned rows"),
    }
}

/// Evaluate a `RenderExpr` into an `InterpValue`.
///
/// Drives argument evaluation for `resolve_args_with`. Dispatches
/// `FunctionCall`s through the provided registry; a widget call stays `Null`
/// here, and a call to a name `fns` does not know is an error.
pub fn eval_to_interp<K: RowKey>(
    expr: &RenderExpr,
    env: &EvalEnv<'_, K>,
    fns: &dyn ValueFnLookup,
) -> Result<InterpValue, ComputeError> {
    use InterpValue::*;
    Ok(match expr {
        RenderExpr::Literal { value } => Value(value.clone()),
        RenderExpr::ColumnRef { name } => Value(
            env.row
                .get(name.as_str())
                .cloned()
                .unwrap_or(crate::Value::Null),
        ),
        RenderExpr::BinaryOp {
            op: op @ (BinaryOperator::And | BinaryOperator::Or),
            left,
            right,
        } => {
            let context = format!("`{}` left operand", op.to_rhai());
            let l = as_bool(&eval_operand(left, env, fns)?, &context)?;
            if l == (*op == BinaryOperator::Or) {
                return Ok(Value(crate::Value::Boolean(l)));
            }
            let r = eval_operand(right, env, fns)?;
            Value(eval_binary_op(op, &crate::Value::Boolean(l), &r)?)
        }
        RenderExpr::BinaryOp { op, left, right } => {
            let l = eval_operand(left, env, fns)?;
            let r = eval_operand(right, env, fns)?;
            Value(eval_binary_op(op, &l, &r)?)
        }
        RenderExpr::Not { operand } => Value(crate::Value::Boolean(!as_bool(
            &eval_operand(operand, env, fns)?,
            "`!` operand",
        )?)),
        RenderExpr::If {
            condition,
            then,
            otherwise,
        } => {
            let condition = eval_operand(condition, env, fns)?;
            return eval_to_interp(choose_branch(&condition, then, otherwise)?, env, fns);
        }
        RenderExpr::FunctionCall { name, args } => match fns.call_kind(name) {
            Some(CallKind::Widget) => Value(crate::Value::Null),
            Some(CallKind::ValueFn) => {
                let resolved = resolve_args_with(args, env, fns)?;
                fns.invoke(name, &resolved).unwrap_or_else(|| {
                    panic!("`{name}` is a value fn by `call_kind`, but `invoke` does not know it")
                })?
            }
            None => return Err(fns.unknown_function(name, expr.to_rhai())),
        },
        RenderExpr::Array { items } => Value(crate::Value::Array(
            items
                .iter()
                .map(|i| eval_operand(i, env, fns))
                .collect::<Result<_, _>>()?,
        )),
        RenderExpr::Object { fields } => Value(crate::Value::Object(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), eval_operand(v, env, fns)?)))
                .collect::<Result<_, ComputeError>>()?,
        )),
        RenderExpr::LiveBlock { block_id } => {
            Value(crate::Value::String(format!("[LiveBlock: {}]", block_id)))
        }
    })
}

/// One operator over two evaluated operands, with the semantics of the typed
/// computations ([`crate::computation`]): a missing arithmetic or text operand
/// gives a missing result, a wrongly typed one is an error. `&&` and `||` see
/// both operands here; [`eval_to_interp`] short-circuits them before this.
pub fn eval_binary_op(
    op: &BinaryOperator,
    left: &Value,
    right: &Value,
) -> Result<Value, ComputeError> {
    let arith = |op| arith_apply(op, left, right);
    let cmp = |op: CmpOp| Ok(Value::Boolean(op.apply(left, right)?));
    match op {
        BinaryOperator::Add => arith(ArithOp::Add),
        BinaryOperator::Sub => arith(ArithOp::Sub),
        BinaryOperator::Mul => arith(ArithOp::Mul),
        BinaryOperator::Div => arith(ArithOp::Div),
        BinaryOperator::Concat => concat_apply(left, right),
        BinaryOperator::Eq => cmp(CmpOp::Eq),
        BinaryOperator::Neq => cmp(CmpOp::Ne),
        BinaryOperator::Gt => cmp(CmpOp::Gt),
        BinaryOperator::Lt => cmp(CmpOp::Lt),
        BinaryOperator::Gte => cmp(CmpOp::Ge),
        BinaryOperator::Lte => cmp(CmpOp::Le),
        BinaryOperator::And => Ok(Value::Boolean(
            as_bool(left, "`&&` left operand")? && as_bool(right, "`&&` right operand")?,
        )),
        BinaryOperator::Or => Ok(Value::Boolean(
            as_bool(left, "`||` left operand")? || as_bool(right, "`||` right operand")?,
        )),
    }
}

/// The branch an `if` takes for an evaluated condition. A missing condition
/// is not true, so it takes `otherwise`.
pub fn choose_branch<'a>(
    condition: &Value,
    then: &'a RenderExpr,
    otherwise: &'a RenderExpr,
) -> Result<&'a RenderExpr, ComputeError> {
    let taken = match condition {
        Value::Null => false,
        other => as_bool(other, "`if` condition")?,
    };
    Ok(if taken { then } else { otherwise })
}

/// An operand of an operator, `!` or `if`: a scalar, never a row set.
fn eval_operand<K: RowKey>(
    expr: &RenderExpr,
    env: &EvalEnv<'_, K>,
    fns: &dyn ValueFnLookup,
) -> Result<Value, ComputeError> {
    match eval_to_interp(expr, env, fns)? {
        InterpValue::Value(v) => Ok(v),
        InterpValue::Rows(_) => Err(ComputeError::WrongType {
            context: format!("the operand `{}`", expr.to_rhai()),
            expected: "a scalar value, not a row set",
            value: Value::Null,
        }),
    }
}

#[cfg(test)]
mod tests {
    fn eval_binary_op(op: &BinaryOperator, l: &Value, r: &Value) -> Value {
        super::eval_binary_op(op, l, r).unwrap()
    }

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
    }

    #[test]
    fn test_eval_binary_op_string_concat() {
        assert_eq!(
            eval_binary_op(
                &BinaryOperator::Concat,
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
    fn test_eval_binary_op_missing_in_missing_out() {
        for op in [
            BinaryOperator::Add,
            BinaryOperator::Mul,
            BinaryOperator::Concat,
        ] {
            assert_eq!(
                eval_binary_op(&op, &Value::Null, &Value::Integer(1)),
                Value::Null,
                "{op:?}"
            );
        }
    }

    #[test]
    fn test_eval_binary_op_wrongly_typed_operands_are_errors() {
        let cases = [
            (BinaryOperator::Add, Value::Integer(1), Value::Boolean(true)),
            (
                BinaryOperator::Gt,
                Value::String("a".into()),
                Value::String("b".into()),
            ),
            (BinaryOperator::And, Value::Integer(1), Value::Integer(0)),
        ];
        for (op, l, r) in cases {
            assert!(
                super::eval_binary_op(&op, &l, &r).is_err(),
                "{op:?} on {l:?}, {r:?} must be an error"
            );
        }
    }

    #[test]
    fn test_if_with_a_missing_condition_takes_otherwise() {
        let then = RenderExpr::Literal {
            value: Value::String("then".into()),
        };
        let otherwise = RenderExpr::Literal {
            value: Value::String("otherwise".into()),
        };
        assert_eq!(
            choose_branch(&Value::Null, &then, &otherwise).unwrap(),
            &otherwise
        );
        assert_eq!(
            choose_branch(&Value::Boolean(true), &then, &otherwise).unwrap(),
            &then
        );
        assert!(choose_branch(&Value::Integer(1), &then, &otherwise).is_err());
    }

    #[test]
    fn test_eval_plain_value_literal() {
        let row = crate::StorageEntity::new();
        let expr = RenderExpr::Literal {
            value: Value::Integer(42),
        };
        assert_eq!(eval_plain_value(&expr, &row).unwrap(), Value::Integer(42));
    }

    #[test]
    fn test_eval_plain_value_column_ref() {
        let mut row = crate::StorageEntity::new();
        row.insert("name".into(), Value::String("Alice".into()));
        let expr = RenderExpr::ColumnRef {
            name: "name".to_string(),
        };
        assert_eq!(
            eval_plain_value(&expr, &row).unwrap(),
            Value::String("Alice".into())
        );
    }

    #[test]
    fn test_eval_plain_value_missing_column() {
        let row = crate::StorageEntity::new();
        let expr = RenderExpr::ColumnRef {
            name: "missing".to_string(),
        };
        assert_eq!(eval_plain_value(&expr, &row).unwrap(), Value::Null);
    }

    #[test]
    fn test_eval_plain_value_binary_op() {
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
        assert_eq!(eval_plain_value(&expr, &row).unwrap(), Value::Integer(3));
    }

    #[test]
    fn test_eval_plain_value_concat() {
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
            eval_plain_value(&expr, &row).unwrap(),
            Value::String("hello world".into())
        );
    }

    #[test]
    fn test_eval_plain_value_array() {
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
            eval_plain_value(&expr, &row).unwrap(),
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

        let resolved = resolve_args_with(&args, &EvalEnv::of_row(&row), &PlainValueFns).unwrap();
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

        let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", None);
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

    // ── RULING C1' — per-level tree sort keys ──────────────────────────
    //
    // Helper: DFS the tree, collecting `id`s in render order.
    fn tree_ids(tree: &OutlineTree) -> Vec<String> {
        tree.walk_depth_first(|row, _| row.get("id").unwrap().as_string().unwrap().to_string())
    }

    fn row(pairs: &[(&str, Value)]) -> Arc<DataRow> {
        Arc::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    /// The C1' RED→GREEN core: two ROOT pins whose `added_ts` (pin recency) is
    /// REVERSED vs `sort_key` (ingest order). With a declared root key
    /// `"-added_ts"`, roots must render most-recently-pinned-first
    /// (`[apple, zebra]`), while `apple`'s CHILDREN keep `sort_key` ASC — the
    /// per-level split. Pre-C1' (root key ignored) roots would follow
    /// `sort_key` → `[zebra, apple]`, so this reds until the per-level re-sort
    /// lands.
    #[test]
    fn from_rows_roots_sort_by_root_key_children_keep_sort_key() {
        let s = |x: &str| Value::String(x.into());
        let i = |n: i64| Value::Integer(n);
        let rows: Vec<Arc<DataRow>> = vec![
            // zebra: ingested 1st (sort_key=1), pinned 1st (added_ts=10)
            row(&[
                ("id", s("zebra")),
                ("parent_id", s("root")),
                ("sort_key", i(1)),
                ("added_ts", i(10)),
            ]),
            // apple: ingested 2nd (sort_key=2), pinned last (added_ts=20)
            row(&[
                ("id", s("apple")),
                ("parent_id", s("root")),
                ("sort_key", i(2)),
                ("added_ts", i(20)),
            ]),
            // apple's children, supplied OUT of sort_key order in the input.
            row(&[
                ("id", s("apple-c2")),
                ("parent_id", s("apple")),
                ("sort_key", i(2)),
                ("added_ts", i(99)),
            ]),
            row(&[
                ("id", s("apple-c1")),
                ("parent_id", s("apple")),
                ("sort_key", i(1)),
                ("added_ts", i(5)),
            ]),
        ];
        let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", Some("-added_ts"));
        assert_eq!(
            tree_ids(&tree),
            vec![
                "apple".to_string(),    // root, added_ts DESC → first
                "apple-c1".to_string(), // child, sort_key ASC
                "apple-c2".to_string(),
                "zebra".to_string(), // root, added_ts DESC → last
            ],
            "roots must sort by the declared root key (added_ts DESC) while child \
             buckets keep sort_key (document) order",
        );
    }

    /// No root key declared → roots stay in `sort_key` order, byte-identical to
    /// the pre-C1' behavior. Locks the "no global flip" guarantee.
    #[test]
    fn from_rows_none_root_key_keeps_sort_key_order_exactly() {
        let s = |x: &str| Value::String(x.into());
        let i = |n: i64| Value::Integer(n);
        let rows: Vec<Arc<DataRow>> = vec![
            row(&[
                ("id", s("zebra")),
                ("parent_id", s("root")),
                ("sort_key", i(1)),
                ("added_ts", i(10)),
            ]),
            row(&[
                ("id", s("apple")),
                ("parent_id", s("root")),
                ("sort_key", i(2)),
                ("added_ts", i(20)),
            ]),
        ];
        let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", None);
        assert_eq!(
            tree_ids(&tree),
            vec!["zebra".to_string(), "apple".to_string()],
            "with no root key, roots keep sort_key order (pre-C1' behavior)",
        );
    }

    /// Roots that TIE on the root key fall back to `sort_key` order
    /// deterministically (the re-sort is stable over the `sort_col`-ordered
    /// bucket). `zebra` (sort_key=1) precedes `apple` (sort_key=2) despite both
    /// having `added_ts=10`.
    #[test]
    fn from_rows_root_key_ties_fall_back_to_sort_key_deterministically() {
        let s = |x: &str| Value::String(x.into());
        let i = |n: i64| Value::Integer(n);
        let rows: Vec<Arc<DataRow>> = vec![
            row(&[
                ("id", s("apple")),
                ("parent_id", s("root")),
                ("sort_key", i(2)),
                ("added_ts", i(10)),
            ]),
            row(&[
                ("id", s("zebra")),
                ("parent_id", s("root")),
                ("sort_key", i(1)),
                ("added_ts", i(10)),
            ]),
        ];
        let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", Some("-added_ts"));
        assert_eq!(
            tree_ids(&tree),
            vec!["zebra".to_string(), "apple".to_string()],
            "root-key ties break deterministically on sort_key order",
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
        assert_eq!(state_display("TODO"), ("TODO", ThemeToken::Muted));
        assert_eq!(state_display("DOING"), ("DOING", ThemeToken::Warning));
        assert_eq!(state_display("DONE"), ("[x]", ThemeToken::Success));
        assert_eq!(state_display(""), ("", ThemeToken::Muted));
        assert_eq!(state_display("CUSTOM"), ("CUSTOM", ThemeToken::Primary));
    }

    #[test]
    fn an_unknown_call_in_a_plain_value_is_refused_naming_it() {
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
        assert_eq!(
            eval_plain_value(&expr, &row),
            Err(ComputeError::NotAPlainValueFunction {
                name: "definitely_not_registered".to_string(),
                call: "definitely_not_registered(7)".to_string(),
            })
        );
    }

    #[test]
    fn core_concat_still_works() {
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
        assert_eq!(
            eval_plain_value(&expr, &row).unwrap(),
            Value::String("abcd".into())
        );
    }

    // ── Value-fn dispatch via resolve_args_with / eval_to_interp ───────

    struct MockValueFnLookup;
    impl ValueFnLookup for MockValueFnLookup {
        fn call_kind(&self, name: &str) -> Option<CallKind> {
            (name == "echo").then_some(CallKind::ValueFn)
        }

        fn invoke(
            &self,
            name: &str,
            args: &ResolvedArgs,
        ) -> Option<Result<InterpValue, ComputeError>> {
            match name {
                "echo" => args
                    .positional
                    .first()
                    .cloned()
                    .map(|v| Ok(InterpValue::Value(v))),
                _ => None,
            }
        }

        fn unknown_function(&self, name: &str, call: String) -> ComputeError {
            ComputeError::UnknownFunction {
                name: name.to_string(),
                call,
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
        match eval_to_interp(&expr, &EvalEnv::of_row(&row), &MockValueFnLookup).unwrap() {
            InterpValue::Value(v) => assert_eq!(v, Value::Integer(99)),
            InterpValue::Rows(_) => panic!("expected Value"),
        }
    }
}

#[cfg(test)]
mod mutation_gap_tests {
    fn eval_binary_op(op: &BinaryOperator, l: &Value, r: &Value) -> Value {
        super::eval_binary_op(op, l, r).unwrap()
    }

    use super::*;

    fn empty_args() -> ResolvedArgs {
        ResolvedArgs {
            positional: vec![],
            positional_exprs: vec![],
            named: HashMap::new(),
            rows: HashMap::new(),
            templates: HashMap::new(),
            widget: None,
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
    fn non_finite_float_arithmetic_result_is_refused() {
        let f = |x: f64| Value::Float(x);
        for (op, a, b, symbol) in [
            (BinaryOperator::Mul, 1e308, 10.0, "*"),
            (BinaryOperator::Add, 1.7e308, 1.7e308, "+"),
            (BinaryOperator::Sub, -1.7e308, 1.7e308, "-"),
            (BinaryOperator::Div, 1e308, 1e-308, "/"),
        ] {
            let err = super::eval_binary_op(&op, &f(a), &f(b))
                .expect_err("overflow to infinity must be refused");
            let message = err.to_string();
            assert!(
                message.contains("non-finite float") && message.contains(symbol),
                "{a:?} {symbol} {b:?}: error must name value and operator, got {message}"
            );
        }
    }

    #[test]
    fn integer_overflow_is_refused_naming_operator_and_operands() {
        let i = |x: i64| Value::Integer(x);
        for (op, a, b, expected) in [
            (
                BinaryOperator::Add,
                i64::MAX,
                1,
                format!("{} + 1", i64::MAX),
            ),
            (
                BinaryOperator::Sub,
                i64::MIN,
                1,
                format!("{} - 1", i64::MIN),
            ),
            (
                BinaryOperator::Mul,
                i64::MAX,
                2,
                format!("{} * 2", i64::MAX),
            ),
            (
                BinaryOperator::Div,
                i64::MIN,
                -1,
                format!("{} / -1", i64::MIN),
            ),
        ] {
            let err = super::eval_binary_op(&op, &i(a), &i(b))
                .expect_err("integer overflow must be refused");
            let message = err.to_string();
            assert!(
                message.contains("integer overflow") && message.contains(&expected),
                "{expected}: error must name operator and operands, got {message}"
            );
        }
    }

    #[test]
    fn division_by_zero_is_refused_naming_the_operands() {
        let i = |x: i64| Value::Integer(x);
        let f = |x: f64| Value::Float(x);
        for (l, r, expected) in [
            (i(7), i(0), "integer division by zero: 7 / 0"),
            (f(1.0), f(0.0), "non-finite float inf from 1.0 / 0.0"),
        ] {
            let err = super::eval_binary_op(&BinaryOperator::Div, &l, &r)
                .expect_err("division by zero must be refused");
            assert!(err.to_string().contains(expected), "{expected}: got {err}");
        }
    }

    #[test]
    fn nested_overflow_reaches_the_caller_of_eval_plain_value() {
        let expr = RenderExpr::Array {
            items: vec![RenderExpr::BinaryOp {
                op: BinaryOperator::Mul,
                left: Box::new(RenderExpr::Literal {
                    value: Value::Float(1e308),
                }),
                right: Box::new(RenderExpr::Literal {
                    value: Value::Float(10.0),
                }),
            }],
        };
        let err = eval_plain_value(&expr, &HashMap::<String, Value>::new())
            .expect_err("a nested non-finite result must not become Null");
        assert!(err.to_string().contains("non-finite float inf"), "{err}");
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
            eval_binary_op(&BinaryOperator::Concat, &s("a"), &s("b")),
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
        assert_eq!(eval_binary_op(&BinaryOperator::Add, &i(1), &f(1.0)), f(2.0));
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

        assert_eq!(state_display(""), ("", ThemeToken::Muted));
        assert_eq!(state_display("TODO"), ("TODO", ThemeToken::Muted));
        assert_eq!(state_display("DOING"), ("DOING", ThemeToken::Warning));
        assert_eq!(state_display("DONE"), ("[x]", ThemeToken::Success));
        assert_eq!(state_display("CANCELLED"), ("CANCELLED", ThemeToken::Error));
        assert_eq!(state_display("WAITING"), ("WAITING", ThemeToken::Primary));
    }

    #[test]
    fn text_style_font_size_maps_headings_and_fails_loud_on_unknown() {
        // The heading type scale is monotonically decreasing h1 > h2 > h3, and
        // every heading is strictly larger than the 14px body default so a
        // `#{style: "h1"}` title can never render at body size.
        let h1 = text_style_font_size("h1").expect("h1 is a known style");
        let h2 = text_style_font_size("h2").expect("h2 is a known style");
        let h3 = text_style_font_size("h3").expect("h3 is a known style");
        assert!(h1 > h2 && h2 > h3, "heading scale must decrease h1>h2>h3");
        assert!(h3 > 14.0, "even h3 must exceed the 14px body default");
        assert_eq!(h1, 28.0, "h1 pins the page-title heading scale");
        // Unknown keyword returns None (caller fails loud) rather than silently
        // resolving to body size — the exact swallow that hid the title bug.
        assert_eq!(text_style_font_size("h7"), None);
        assert_eq!(text_style_font_size(""), None);
    }

    #[test]
    fn text_style_treatment_bolds_only_recognized_headings() {
        // A heading carries both halves of the treatment.
        let h1 = text_style_treatment("h1").expect("h1 is a known style");
        assert_eq!(h1.size, text_style_font_size("h1").unwrap());
        assert!(h1.bold, "a heading is bold");
        // `body` is in the scale but is not a heading.
        assert!(!text_style_treatment("body").expect("body is known").bold);
        // An unrecognized keyword yields NO treatment — not a bold body-size
        // one. `h7` looks like a heading and is not in the scale; resolving
        // size and weight separately let one frontend bold it while another
        // did not.
        assert_eq!(text_style_treatment("h7"), None);
        assert_eq!(text_style_treatment(""), None);
    }

    #[test]
    fn text_display_shows_disclosed_placeholder_only_for_empty_content() {
        // Non-empty content always wins — the placeholder never masks real text.
        assert_eq!(text_display("hello", Some("(untitled)")), ("hello", false));
        // Empty content with a placeholder discloses the stand-in (styled muted
        // by the caller) so an empty Page can never render as a blank row.
        assert_eq!(text_display("", Some("(untitled)")), ("(untitled)", true));
        // No placeholder configured → empty stays empty (never fabricated).
        assert_eq!(text_display("", None), ("", false));
        assert_eq!(text_display("hello", None), ("hello", false));
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
            resolve_states(&args, &row).unwrap(),
            vec!["A".to_string(), "B".to_string()]
        );

        let default = resolve_states(&empty_args(), &row).unwrap();
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

    /// When a document has no `#+TODO:` keywords, `todo_states` evaluates
    /// to `Value::Null`. `resolve_states` must fall back to the default
    /// cycle `["", "TODO", "DOING", "DONE"]` — otherwise a task block in a
    /// journal day page (which never declares `#+TODO:`) would cycle from
    /// TODO straight to empty, skipping DOING.
    #[test]
    fn resolve_states_null_value_falls_back_to_default() {
        let row: HashMap<String, Value> = [("sts".to_string(), Value::Null)].into_iter().collect();
        let mut args = empty_args();
        args.templates.insert(
            "states".to_string(),
            RenderExpr::ColumnRef {
                name: "sts".to_string(),
            },
        );
        assert_eq!(
            resolve_states(&args, &row).unwrap(),
            vec![
                String::new(),
                "TODO".to_string(),
                "DOING".to_string(),
                "DONE".to_string()
            ]
        );
    }

    /// When a document explicitly sets an empty `#+TODO:` list (or the
    /// stored JSON is `[]`), `resolve_states` receives an empty Array and
    /// must still fall back to the default cycle — otherwise `cycle_state`
    /// would return `""` for every click.
    #[test]
    fn resolve_states_empty_array_falls_back_to_default() {
        let row: HashMap<String, Value> = [("sts".to_string(), Value::Array(vec![]))]
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
            resolve_states(&args, &row).unwrap(),
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
            "item_template".to_string(),
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
            args.get_template("item_template"),
            Some(RenderExpr::ColumnRef { name }) if name == "x"
        ));
        assert!(args.get_rows("nope").is_none());
    }

    /// The backstop: a name no widget declares and the allowlist does not
    /// carry was resolved as a scalar, so `get_template` can only ever answer
    /// `None`. That silent `None` is what kept `expand_toggle`'s `content`
    /// dead, so it is a panic now rather than an absent-arg answer.
    #[test]
    #[should_panic(expected = "not a")]
    fn get_template_for_an_unclassified_name_panics() {
        let row: HashMap<String, Value> = HashMap::new();
        let args = resolve_args_with(&[], &EvalEnv::of_row(&row), &PlainValueFns).unwrap();
        let _ = args.get_template("nope");
    }

    /// A widget's own declared params answer templateness — the allowlist is
    /// consulted only for names the widget says nothing about.
    #[test]
    fn declared_params_override_the_global_allowlist() {
        static META: crate::WidgetMeta = crate::WidgetMeta {
            name: "probe",
            category: crate::WidgetCategory::Special,
            params: &[
                crate::StaticParam {
                    name: "content",
                    type_hint: "Expr",
                    default: None,
                },
                crate::StaticParam {
                    name: "header",
                    type_hint: "String",
                    default: None,
                },
            ],
            doc: "",
        };
        // Undeclared by `probe`, so the allowlist still rules.
        assert!(is_template_arg_for(Some(&META), "item_template"));
        // Declared as Expr though the allowlist has never heard of it.
        assert!(is_template_arg_for(Some(&META), "content"));
        assert!(!is_template_arg("content"));
        // Declared as a scalar though the allowlist calls it a template.
        assert!(!is_template_arg_for(Some(&META), "header"));
        assert!(is_template_arg("header"));
    }
}
