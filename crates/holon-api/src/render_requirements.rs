//! What a renderer needs from the rows it draws.
//!
//! A query and a renderer meet at a BINDING. The query's SELECT list decides
//! which columns a row carries; the renderer decides which of those it needs.
//! A column the renderer binds through a widget parameter that declares a
//! default degrades to that default when absent — documented, silent. A column
//! it binds anywhere else (a variant condition, a parameter with no default)
//! makes the render WRONG when absent, and that is the loud path.
//!
//! Requirements are per-binding, not per-query: the same query can feed several
//! renderers and the same renderer several queries, so the manifest travels
//! with the subscription rather than with the SQL.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::sync::RwLock;

use crate::render_types::RenderExpr;

/// The columns one renderer needs, split by how it needs them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderRequirements {
    required: BTreeSet<String>,
    optional: BTreeSet<String>,
    /// The renderer hands each row to that row's own entity profile
    /// (`render_entity()` / `live_block()`), whose variants are not knowable
    /// from the template alone. The profile then answers for itself.
    dispatch_to_entity: bool,
}

impl RenderRequirements {
    /// A binding with no renderer attached — a raw watch, an advice stream, a
    /// test subscription. Nothing is required, so no projection width is a gap.
    pub fn none() -> Self {
        Self::default()
    }

    /// A binding whose renderer defers to each row's entity profile.
    pub fn entity_dispatch() -> Self {
        Self {
            dispatch_to_entity: true,
            ..Self::default()
        }
    }

    pub fn from_parts(required: BTreeSet<String>, optional: BTreeSet<String>) -> Self {
        let optional = optional.difference(&required).cloned().collect();
        Self {
            required,
            optional,
            dispatch_to_entity: false,
        }
    }

    pub fn dispatches_to_entity(&self) -> bool {
        self.dispatch_to_entity
    }

    /// Whether an absent `column` makes this renderer's output wrong.
    pub fn requires(&self, column: &str) -> bool {
        self.required.contains(column)
    }

    pub fn required(&self) -> &BTreeSet<String> {
        &self.required
    }

    /// Columns this renderer binds but can draw without.
    pub fn optional(&self) -> &BTreeSet<String> {
        &self.optional
    }

    /// The manifest of a binding whose renderers are all of these. Required
    /// wins: one renderer needing a column makes the binding need it.
    pub fn union(mut self, other: &Self) -> Self {
        self.required.extend(other.required.iter().cloned());
        self.optional.extend(other.optional.iter().cloned());
        self.optional = self.optional.difference(&self.required).cloned().collect();
        self.dispatch_to_entity |= other.dispatch_to_entity;
        self
    }

    /// Narrow to the columns `other` also requires. The enrich seat uses this
    /// to keep the entity profile's own manifest inside what the subscribed
    /// binding actually asked for.
    pub fn intersect_required(&self, other: &Self) -> BTreeSet<String> {
        if other.dispatch_to_entity {
            return self.required.clone();
        }
        self.required
            .intersection(&other.required)
            .cloned()
            .collect()
    }

    /// The required columns `projection` fails to carry.
    pub fn unmet_by<'a, I>(&self, projection: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let carried: BTreeSet<&str> = projection.into_iter().collect();
        self.required
            .iter()
            .filter(|c| !carried.contains(c.as_str()))
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Widget parameter defaults — the DATA that separates required from optional
// ---------------------------------------------------------------------------

/// Ordered parameter names of one widget, each flagged with whether it declares
/// a default. Positional args index into this list.
type WidgetParams = Vec<(String, bool)>;

static WIDGET_PARAM_DEFAULTS: OnceLock<BTreeMap<String, WidgetParams>> = OnceLock::new();

/// One-shot flag so an unregistered table is announced once, not per row.
static UNREGISTERED_ANNOUNCED: OnceLock<RwLock<bool>> = OnceLock::new();

/// Publish the widget parameter table derived from `WidgetMeta`.
///
/// The frontend owns the builder registry, so it registers; every other crate
/// reads. Subsequent calls are ignored (`OnceLock`).
pub fn register_widget_param_defaults<I, P>(widgets: I)
where
    I: IntoIterator<Item = (String, P)>,
    P: IntoIterator<Item = (String, bool)>,
{
    let table: BTreeMap<String, WidgetParams> = widgets
        .into_iter()
        .map(|(name, params)| (name, params.into_iter().collect()))
        .collect();
    let _ = WIDGET_PARAM_DEFAULTS.set(table);
}

/// Whether widget `name`'s parameter `param` declares a default.
///
/// `None` means undecidable: an unknown widget, an unknown parameter, or a
/// table that was never registered. Callers treat undecidable as required —
/// over-reporting a gap is recoverable, missing one is the drift this whole
/// mechanism exists to stop.
fn param_declares_default(name: &str, param: &ParamRef) -> Option<bool> {
    let table = WIDGET_PARAM_DEFAULTS.get()?;
    let params = table.get(name)?;
    let entry = match param {
        ParamRef::Named(n) => params.iter().find(|(p, _)| p == n)?,
        ParamRef::Positional(i) => params.get(*i)?,
    };
    Some(entry.1)
}

fn announce_unregistered_table() {
    if WIDGET_PARAM_DEFAULTS.get().is_some() {
        return;
    }
    let flag = UNREGISTERED_ANNOUNCED.get_or_init(|| RwLock::new(false));
    if *flag.read().unwrap() {
        return;
    }
    let mut announced = flag.write().unwrap();
    if *announced {
        return;
    }
    *announced = true;
    tracing::warn!(
        "render requirements: no widget parameter table registered \
         (`register_widget_param_defaults`), so every bound field is classified REQUIRED. \
         Optional-with-default degradation cannot be recognised in this process."
    );
}

enum ParamRef {
    Named(String),
    Positional(usize),
}

// ---------------------------------------------------------------------------
// Derivation from a render template
// ---------------------------------------------------------------------------

/// The names a render template binds, before computed fields are expanded to
/// the columns behind them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BoundNames {
    pub required: BTreeSet<String>,
    pub optional: BTreeSet<String>,
    /// The template hands rows to the row entity's own profile
    /// (`render_entity()` / `live_block()`), so that profile's manifest is part
    /// of this binding's.
    pub dispatches_to_entity: bool,
}

impl BoundNames {
    fn absorb(&mut self, other: BoundNames) {
        self.required.extend(other.required);
        self.optional.extend(other.optional);
        self.dispatches_to_entity |= other.dispatches_to_entity;
    }
}

/// Collect the `col("…")` bindings of a render template, classified by whether
/// the widget parameter carrying them declares a default.
pub fn bound_names(expr: &RenderExpr) -> BoundNames {
    announce_unregistered_table();
    let mut out = BoundNames::default();
    walk(expr, None, &mut out);
    out.optional = out.optional.difference(&out.required).cloned().collect();
    out
}

/// The manifest of the renderer a binding attaches to `expr`, ready to travel
/// with the subscription. `computed` maps each of the row entity's computed
/// fields to the columns it reads.
pub fn requirements_for_template(
    expr: &RenderExpr,
    computed: &BTreeMap<String, BTreeSet<String>>,
) -> RenderRequirements {
    let names = bound_names(expr);
    let dispatch = names.dispatches_to_entity;
    let mut reqs = expand_through_computed(&names, computed);
    reqs.dispatch_to_entity = dispatch;
    reqs
}

/// `enclosing` is the widget parameter this expression sits in, once one is
/// known. A `col()` deeper inside a nested widget re-binds it, so the
/// classifier always sees the parameter closest to the reference.
fn walk(expr: &RenderExpr, enclosing: Option<(&str, ParamRef)>, out: &mut BoundNames) {
    match expr {
        RenderExpr::ColumnRef { name } => classify(name, enclosing, out),
        RenderExpr::FunctionCall { name, args } => {
            if name == "col" {
                if let Some(RenderExpr::Literal { value }) = args.first().map(|a| &a.value) {
                    if let Some(column) = value.as_string() {
                        classify(column, enclosing, out);
                    }
                }
                return;
            }
            if name == "render_entity" || name == "live_block" {
                out.dispatches_to_entity = true;
            }
            let mut positional = 0usize;
            for arg in args {
                let param = match &arg.name {
                    Some(n) => ParamRef::Named(n.clone()),
                    None => {
                        let at = positional;
                        positional += 1;
                        ParamRef::Positional(at)
                    }
                };
                walk(&arg.value, Some((name, param)), out);
            }
        }
        RenderExpr::Object { fields } => {
            // An object literal under a widget names its own parameters — the
            // `#{header: …, content: …}` form the DSL uses for named args.
            let widget = enclosing.as_ref().map(|(w, _)| *w);
            for (key, value) in fields {
                let param = widget.map(|w| (w, ParamRef::Named(key.clone())));
                walk(value, param, out);
            }
        }
        RenderExpr::Array { items } => {
            for item in items {
                let mut nested = BoundNames::default();
                walk(item, None, &mut nested);
                out.absorb(nested);
            }
        }
        RenderExpr::BinaryOp { left, right, .. } => {
            walk(left, None, out);
            walk(right, None, out);
        }
        RenderExpr::LiveBlock { .. } => out.dispatches_to_entity = true,
        RenderExpr::Literal { .. } => {}
    }
}

fn classify(name: &str, enclosing: Option<(&str, ParamRef)>, out: &mut BoundNames) {
    let optional = enclosing
        .and_then(|(widget, param)| param_declares_default(widget, &param))
        .unwrap_or(false);
    if optional {
        out.optional.insert(name.to_string());
    } else {
        out.required.insert(name.to_string());
    }
}

/// Resolve bound names to the stored columns behind them.
///
/// A name that is a computed field stands for the columns its expression reads,
/// transitively through sibling computed fields; a name that is not stands for
/// itself. The binding's classification carries down: a computed field bound to
/// a defaulting parameter makes every column beneath it optional.
pub fn expand_through_computed(
    names: &BoundNames,
    computed: &BTreeMap<String, BTreeSet<String>>,
) -> RenderRequirements {
    let mut required = BTreeSet::new();
    let mut optional = BTreeSet::new();
    for name in &names.required {
        expand_one(name, computed, &mut required, &mut BTreeSet::new());
    }
    for name in &names.optional {
        expand_one(name, computed, &mut optional, &mut BTreeSet::new());
    }
    RenderRequirements::from_parts(required, optional)
}

fn expand_one(
    name: &str,
    computed: &BTreeMap<String, BTreeSet<String>>,
    out: &mut BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) {
    if !seen.insert(name.to_string()) {
        return;
    }
    match computed.get(name) {
        Some(deps) => {
            for dep in deps {
                expand_one(dep, computed, out, seen);
            }
        }
        None => {
            out.insert(name.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;
    use crate::render_types::Arg;

    fn call(name: &str, args: Vec<Arg>) -> RenderExpr {
        RenderExpr::FunctionCall {
            name: name.to_string(),
            args,
        }
    }

    fn col(name: &str) -> RenderExpr {
        call(
            "col",
            vec![Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::String(name.to_string()),
                },
            }],
        )
    }

    fn register() {
        register_widget_param_defaults(vec![
            (
                "icon".to_string(),
                vec![
                    ("name".to_string(), true),
                    ("size".to_string(), true),
                    ("color".to_string(), true),
                ],
            ),
            (
                "text".to_string(),
                vec![("content".to_string(), false), ("style".to_string(), true)],
            ),
        ]);
    }

    #[test]
    fn a_defaulting_parameter_makes_its_binding_optional() {
        register();
        let names = bound_names(&call(
            "row",
            vec![
                Arg {
                    name: None,
                    value: call(
                        "icon",
                        vec![Arg {
                            name: None,
                            value: col("bullet_shape"),
                        }],
                    ),
                },
                Arg {
                    name: None,
                    value: call(
                        "text",
                        vec![Arg {
                            name: None,
                            value: col("content"),
                        }],
                    ),
                },
            ],
        ));
        assert!(
            names.optional.contains("bullet_shape"),
            "`icon`'s first parameter declares a default, so its binding degrades: {names:?}"
        );
        assert!(
            names.required.contains("content"),
            "`text`'s first parameter declares no default: {names:?}"
        );
    }

    #[test]
    fn a_computed_binding_expands_to_the_columns_beneath_it() {
        let mut computed = BTreeMap::new();
        computed.insert(
            "bullet_shape".to_string(),
            BTreeSet::from(["collapsed".to_string()]),
        );
        computed.insert(
            "is_program".to_string(),
            BTreeSet::from(["is_rule_head".to_string(), "parent_id".to_string()]),
        );
        computed.insert(
            "is_rule_head".to_string(),
            BTreeSet::from(["source_language".to_string()]),
        );

        let names = BoundNames {
            required: BTreeSet::from(["is_program".to_string()]),
            optional: BTreeSet::from(["bullet_shape".to_string()]),
            dispatches_to_entity: false,
        };
        let reqs = expand_through_computed(&names, &computed);
        assert_eq!(
            reqs.required(),
            &BTreeSet::from(["parent_id".to_string(), "source_language".to_string()]),
            "transitive through the sibling computed field"
        );
        assert_eq!(reqs.optional(), &BTreeSet::from(["collapsed".to_string()]));
    }

    #[test]
    fn required_beats_optional_when_two_renderers_bind_the_same_column() {
        let a = RenderRequirements::from_parts(
            BTreeSet::new(),
            BTreeSet::from(["collapsed".to_string()]),
        );
        let b = RenderRequirements::from_parts(
            BTreeSet::from(["collapsed".to_string()]),
            BTreeSet::new(),
        );
        let merged = a.union(&b);
        assert!(merged.requires("collapsed"));
        assert!(!merged.optional().contains("collapsed"));
    }

    #[test]
    fn unmet_by_names_only_the_required_columns_a_projection_drops() {
        let reqs = RenderRequirements::from_parts(
            BTreeSet::from(["content_type".to_string(), "widget_only".to_string()]),
            BTreeSet::from(["collapsed".to_string()]),
        );
        assert_eq!(
            reqs.unmet_by(vec!["id", "content", "content_type"]),
            vec!["widget_only".to_string()],
        );
    }
}
