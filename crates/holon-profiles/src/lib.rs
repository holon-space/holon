//! @c4 component
//! @c4 layer Core
//! Pattern: Strategy
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-engine "Petri-net engine" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! EntityProfile system: per-entity, per-row render + operation resolution.
//!
//! Each entity (e.g., "block") can have a profile that defines:
//! - Computed fields (Rhai expressions evaluated from row data)
//! - A default render expression + operations
//! - Conditional variants that override rendering based on row data
//!
//! Profile blocks are org blocks with an `entity_profile_for` property.
//! Block content is YAML using the `= ` prefix convention from petri.rs
//! for Rhai expressions.
//!
//! NOTE: Rhai ASTs are !Send+!Sync, so EntityProfile stores source strings
//! and compiles on-demand during resolution. Compilation is fast for small
//! expressions (<1µs each).

pub mod trust;
pub mod type_registry;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use futures_signals::signal_map::SignalMapExt;
use holon_api::CompiledExpr;
use holon_api::EntityName;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::live_data::LiveData;
use holon_api::predicate::Predicate;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::RenderExpr;
use holon_api::render_types::RenderVariant;
use holon_api::row_id;
use rhai::Engine as RhaiEngine;
pub use trust::OriginClass;
pub use trust::TrustDecision;
pub use trust::TrustPolicy;
pub use trust::TrustPolicyParseError;
pub use trust::TrustRule;
pub use type_registry::TableName;
pub use type_registry::TypeRegistry;
pub use type_registry::create_default_registry;
pub use type_registry::type_profiles_from_registry;

/// Variables that are frontend-local (UI state), not data-dependent.
/// Conditions referencing only these variables are extracted as `Predicate`
/// for instant frontend-side switching without a backend round-trip.
const UI_STATE_VARIABLES: &[&str] = &[
    "is_focused",
    "is_expanded",
    "view_mode",
    // Render-context flag set by tree-builder `rules:` overrides (e.g.
    // `role: "page_title"`); merged into ui_state by `pick_active_variant`.
    // Classifying it as data would drop the variant at resolve time (rows
    // have no `role` column).
    "role",
    // Container-query inputs: refined per subtree during render interpretation.
    "available_width_px",
    "available_height_px",
    "available_width_physical_px",
    "available_height_physical_px",
    // Global viewport default: emitted by UiState::context_for when no // ALLOW(fallback): comment
    // describes default-context emission refinement has reached this block.
    "viewport_width_px",
    "viewport_height_px",
    "viewport_width_physical_px",
    "viewport_height_physical_px",
    "scale_factor",
];

/// Map of entity name → live collection for Rhai lookup functions.
pub type LiveEntities = HashMap<EntityName, Arc<LiveData<StorageEntity>>>;

/// A live entity backing the bundled `block` profile's lookup-dependent
/// computed fields (`has_query_source`, `is_program`): source blocks of a
/// fixed language set, keyed by `parent_id`.
///
/// The single seat for both storage arms — a Turso session feeds it from a CDC
/// matview over [`sql_predicate`](Self::sql_predicate), a Loro-only session
/// from a block snapshot via
/// [`live_data_from_blocks`](Self::live_data_from_blocks), and the PBT oracle
/// mirrors it the same way — so a language added here reaches every one of
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveEntitySpec {
    /// `query_source(id)` — "does block `id` own a query-source child?"
    QuerySource,
    /// `rule_sibling(parent_id)` — "does that block own a rule-head child?"
    /// The retired `action` language counts: its trigger must stay hidden
    /// while it surfaces its deprecation.
    RuleSibling,
}

impl LiveEntitySpec {
    pub const ALL: &'static [LiveEntitySpec] = &[Self::QuerySource, Self::RuleSibling];

    /// The Rhai lookup function's name.
    pub fn entity_name(self) -> EntityName {
        match self {
            Self::QuerySource => EntityName::new("query_source"),
            Self::RuleSibling => EntityName::new("rule_sibling"),
        }
    }

    /// The source languages whose blocks populate this entity — the one
    /// definition both the SQL predicate and [`matches`](Self::matches) derive
    /// from.
    pub fn languages(self) -> Vec<holon_api::SourceLanguage> {
        use holon_api::SourceLanguage;
        match self {
            Self::QuerySource => holon_api::QueryLanguage::ALL
                .iter()
                .copied()
                .map(SourceLanguage::Query)
                .collect(),
            Self::RuleSibling => vec![SourceLanguage::HolonRule, SourceLanguage::LegacyAction],
        }
    }

    pub fn matches(self, language: &holon_api::SourceLanguage) -> bool {
        self.languages().contains(language)
    }

    /// The `WHERE` clause selecting this entity's rows out of a block table —
    /// a plain filtered read, no self-join and no chained matview.
    pub fn sql_predicate(self) -> String {
        let langs: Vec<String> = self.languages().iter().map(|l| format!("'{l}'")).collect();
        format!(
            "content_type = 'source' AND source_language IN ({})",
            langs.join(", ")
        )
    }

    /// Build this entity's collection from an in-memory block set — the
    /// CDC-free counterpart of the matview, projected to the same three
    /// columns and keyed by `parent_id`.
    pub fn live_data_from_blocks<'a>(
        self,
        blocks: impl IntoIterator<Item = &'a holon_api::block::Block>,
    ) -> Arc<LiveData<StorageEntity>> {
        let rows: Vec<StorageEntity> = blocks
            .into_iter()
            .filter(|b| b.content_type == holon_api::ContentType::Source)
            .filter_map(|b| b.source_language.as_ref().map(|lang| (b, lang)))
            .filter(|(_, lang)| self.matches(lang))
            .map(|(b, lang)| {
                HashMap::from([
                    (Arc::from("id"), Value::String(b.id.as_str().to_string())),
                    (
                        Arc::from("parent_id"),
                        Value::String(b.parent_id.as_str().to_string()),
                    ),
                    (
                        Arc::from("source_language"),
                        Value::String(lang.to_string()),
                    ),
                ])
            })
            .collect();

        LiveData::new(
            rows,
            |row| {
                row.get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("source-block row missing 'parent_id'"))
            },
            |row| Ok(row.clone()),
        )
    }
}

// ---------------------------------------------------------------------------
// Core types — moved to holon-api (storage de-leak Stage 10); re-exported so
// `holon::entity_profile::*` paths keep working. holon keeps the profile
// *sources*: YAML/org parsing below and the LiveData-backed ProfileResolver.
// ---------------------------------------------------------------------------

pub use holon_api::entity_profile::CompiledComputedField;
pub use holon_api::entity_profile::EntityProfile;
pub use holon_api::entity_profile::ProfileCache;
pub use holon_api::entity_profile::ProfileResolving;
pub use holon_api::entity_profile::RowVariant;
pub use holon_api::entity_profile::StoredProfile;
pub use holon_api::entity_profile::StoredVariant;
pub use holon_api::entity_profile::VirtualChildConfig;
pub use holon_api::entity_profile::value_to_dynamic;
pub use holon_api::render_types::RenderProfile;

// ---------------------------------------------------------------------------
// YAML parsing
// ---------------------------------------------------------------------------

/// Parse a render expression from text.
///
/// Uses the Rhai-based render DSL parser. Accepts both Rhai syntax and JSON.
pub fn parse_render_text(text: &str) -> Result<RenderExpr> {
    holon_api::render_dsl::parse_render_dsl(text)
}

/// Profile data deserialized from YAML. Also the shared intermediate
/// representation for both bundled YAML and org-embedded profiles — passed
/// to `TypeRegistry::apply_parsed_profile` or converted to `EntityProfile`
/// via `to_entity_profile()`.
///
/// Computed field values have the `= ` prefix stripped and are validated at
/// parse time by `parse_profile_yaml`.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParsedProfile {
    pub entity_name: String,
    #[serde(default)]
    pub computed: BTreeMap<String, String>,
    #[serde(default)]
    pub variants: Vec<holon_api::ProfileVariant>,
    #[serde(default)]
    pub virtual_child: Option<VirtualChildConfig>,
}

/// Parse a YAML entity profile into a `ParsedProfile`.
///
/// Strips `= ` prefix from computed field expressions and validates they
/// compile. Variant conditions are already compiled at the serde boundary.
pub fn parse_profile_yaml(yaml_content: &str) -> Result<ParsedProfile> {
    let mut profile: ParsedProfile =
        serde_yaml::from_str(yaml_content).context("Invalid YAML in entity profile")?;

    let engine = RhaiEngine::new();
    for (name, value) in profile.computed.iter_mut() {
        *value = strip_rhai_prefix(value);
        CompiledExpr::compile(&engine, value.as_str())
            .map_err(|e| anyhow::anyhow!("Failed to compile computed field '{name}': {e}"))?;
    }

    Ok(profile)
}

/// Parse a YAML entity profile block into an EntityProfile.
///
/// Convenience wrapper around `parse_profile_yaml` + conversion to the
/// runtime representation used by `ProfileResolver`.
pub fn parse_entity_profile(yaml_content: &str) -> Result<EntityProfile> {
    let parsed = parse_profile_yaml(yaml_content)?;
    parsed.to_entity_profile()
}

impl ParsedProfile {
    /// Convert to the runtime `EntityProfile` representation.
    pub fn to_entity_profile(self) -> Result<EntityProfile> {
        let engine = RhaiEngine::new();
        let computed_fields = parse_and_sort_computed_fields(&engine, &self.computed)?;
        let variants = profile_variants_to_stored(&self.variants)?;
        Ok(EntityProfile {
            entity_name: EntityName::new(&self.entity_name),
            variants,
            computed_fields,
            virtual_child: self.virtual_child,
            // A YAML/org-source profile parsed standalone has no TypeDefinition,
            // so no declared schema is known here — every missing required column
            // is treated as expected heterogeneity (silent). The block profile's
            // declared columns come from the type-def base it merges into (see
            // `profile_from_type_def` + `build_cache_from_source` merge order).
            declared_columns: BTreeSet::new(),
        })
    }
}

/// Build a render profile for a VALUE-shaped row — a row that carries no
/// entity-shaped `id`, so there is no entity to resolve a profile by.
///
/// Value rows are a LEGITIMATE display case (Martin ruling 2026-07-11), not an
/// error and not "degraded": they arise from aggregate queries
/// (`SELECT date('now') AS name`), rule-trigger results, and future table
/// rows. They render as a plain value row — their columns, shown directly —
/// with no warning marker. Loudness is reserved for a genuine CONTRACT
/// violation (a widget that DECLARES it needs entity rows being fed a value
/// row), which is surfaced at the resolver's entity-expecting seam, not here.
fn value_row_profile(row: &HashMap<String, Value>) -> RenderProfile {
    // Present the row's columns plainly, sorted for determinism. Internal
    // matview bookkeeping columns (leading `_`, e.g. `_rowid`) are hidden —
    // they are not user data. If every column is internal, fall back to
    // showing them so the row is never blank.
    let mut visible: Vec<(&String, &Value)> =
        row.iter().filter(|(k, _)| !k.starts_with('_')).collect();
    if visible.is_empty() {
        visible = row.iter().collect();
    }
    visible.sort_by(|a, b| a.0.cmp(b.0));
    let text = visible
        .iter()
        .map(|(k, v)| format!("{k}: {}", value_display(v)))
        .collect::<Vec<_>>()
        .join(", ");
    RenderProfile {
        name: "value-row".to_string(),
        render: RenderExpr::Literal {
            value: Value::String(text),
        },
        operations: vec![],
        variants: vec![],
    }
}

/// Compact one-line display of a cell value for the degraded-row marker.
fn value_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::DateTime(s) => s.clone(),
        Value::Json(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

/// Strip the `= ` prefix convention from a Rhai expression string.
fn strip_rhai_prefix(s: &str) -> String {
    s.strip_prefix('=')
        .map(|s| s.trim())
        .unwrap_or(s.trim())
        .to_string()
}

/// Split a Rhai condition into data-only and UI-only parts.
///
/// Splits on top-level `&&`. Conjuncts that reference ONLY UI state variables
/// are extracted as a `Predicate`; the rest stays as a data-only Rhai string.
///
/// Returns `(data_condition, ui_condition)`.
fn split_condition(source: &str) -> Result<(Option<String>, Predicate)> {
    let conjuncts: Vec<&str> = source.split("&&").map(|s| s.trim()).collect();

    let mut data_parts = Vec::new();
    let mut ui_predicates = Vec::new();

    for conjunct in &conjuncts {
        let refs_only_ui = !conjunct.is_empty()
            && UI_STATE_VARIABLES
                .iter()
                .any(|var| holon_core::util::expr_references(conjunct, var))
            && !has_non_ui_references(conjunct);

        if refs_only_ui {
            ui_predicates.push(parse_conjunct_to_predicate(conjunct)?);
        } else {
            data_parts.push(*conjunct);
        }
    }

    let data_condition = if data_parts.is_empty() {
        None
    } else {
        Some(data_parts.join(" && "))
    };

    let ui_condition = match ui_predicates.len() {
        0 => Predicate::Always,
        1 => ui_predicates.into_iter().next().unwrap(),
        _ => Predicate::And(ui_predicates),
    };

    Ok((data_condition, ui_condition))
}

/// Check if a conjunct references any variables that are NOT UI state
/// variables.
fn has_non_ui_references(conjunct: &str) -> bool {
    let ident_chars = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut i = 0;
    let bytes = conjunct.as_bytes();
    while i < bytes.len() {
        // Skip quoted strings
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' {
                    i += 1; // skip escaped char
                }
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // skip closing quote
            }
            continue;
        }
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && ident_chars(bytes[i] as char) {
                i += 1;
            }
            let ident = &conjunct[start..i];
            if matches!(
                ident,
                "true" | "false" | "if" | "else" | "let" | "fn" | "return"
            ) {
                continue;
            }
            if !UI_STATE_VARIABLES.contains(&ident) {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Parse a simple conjunct into a Predicate.
///
/// Supported grammar (anything else is rejected — a silently-false predicate
/// would make the variant never activate with no diagnostic):
/// - `is_focused` → `Var("is_focused")`
/// - `!is_focused` → `Not(Var("is_focused"))`
/// - `view_mode == "table"` → `Eq { field: "view_mode", value: "table" }` (also
///   `!=`, `<`, `<=`, `>`, `>=` against a literal)
fn parse_conjunct_to_predicate(conjunct: &str) -> Result<Predicate> {
    let s = conjunct.trim();

    let is_ident = |s: &str| {
        let mut chars = s.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };

    if let Some((op, idx)) = find_comparison_operator(s) {
        let field = s[..idx].trim();
        anyhow::ensure!(
            is_ident(field),
            "unsupported UI condition `{s}`: left side of `{op}` must be a plain variable"
        );
        let field = field.to_string();
        let value = parse_literal_value(s[idx + op.len()..].trim());
        return Ok(match op {
            "==" => Predicate::Eq { field, value },
            "!=" => Predicate::Ne { field, value },
            "<=" => Predicate::Lte { field, value },
            ">=" => Predicate::Gte { field, value },
            "<" => Predicate::Lt { field, value },
            ">" => Predicate::Gt { field, value },
            _ => unreachable!("find_comparison_operator returned unknown operator {op}"),
        });
    }

    if let Some(rest) = s.strip_prefix('!') {
        let var = rest.trim();
        anyhow::ensure!(
            is_ident(var),
            "unsupported UI condition `{s}`: `!` must be followed by a plain variable"
        );
        return Ok(Predicate::Not(Box::new(Predicate::Var(var.to_string()))));
    }

    if is_ident(s) {
        return Ok(Predicate::Var(s.to_string()));
    }

    anyhow::bail!(
        "unsupported UI condition `{s}`: expected `var`, `!var`, or `var <op> literal` (rewrite \
         `a || b` as separate variants)"
    )
}

/// Find the first comparison operator outside string literals.
/// Returns the operator token and its byte index. Two-char operators win
/// over their one-char prefixes at the same position.
fn find_comparison_operator(s: &str) -> Option<(&'static str, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            if s[i..].starts_with(op) {
                return Some((op, i));
            }
        }
        i += 1;
    }
    None
}

/// Parse a literal value from a Rhai expression fragment.
fn parse_literal_value(s: &str) -> Value {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        Value::String(s[1..s.len() - 1].to_string())
    } else if s == "true" {
        Value::Boolean(true)
    } else if s == "false" {
        Value::Boolean(false)
    } else if s == "()" || s == "null" {
        Value::Null
    } else if let Ok(i) = s.parse::<i64>() {
        Value::Integer(i)
    } else if let Ok(f) = s.parse::<f64>() {
        Value::Float(f)
    } else {
        Value::String(s.to_string())
    }
}

/// Convert `ProfileVariant`s into `StoredVariant`s.
///
/// `ProfileVariant` conditions are already pre-compiled (CompiledExpr serde).
/// This function splits conditions into data/ui predicates and parses render
/// expressions.
pub fn profile_variants_to_stored(
    profile_variants: &[holon_api::ProfileVariant],
) -> Result<Vec<StoredVariant>> {
    let mut variants = Vec::new();
    for pv in profile_variants {
        let (condition_src, condition_required, data_condition, ui_condition) =
            if let Some(ref compiled) = pv.condition {
                let src = compiled.source.clone();
                let (dc, uc) = split_condition(&src)
                    .with_context(|| format!("in condition of variant '{}'", pv.name))?;
                // The full condition's required columns come straight off its
                // already-compiled AST. The data-only subset is a different
                // string (UI conjuncts stripped), so derive its required set by
                // compiling it once here (compile-only; it is a subset of the
                // parent that already compiled).
                (src, compiled.required_columns.clone(), dc, uc)
            } else {
                (String::new(), BTreeSet::new(), None, Predicate::Always)
            };

        let data_condition_required = match &data_condition {
            Some(dc) => {
                CompiledExpr::compile(&RhaiEngine::new(), dc.as_str())
                    .map_err(|e| {
                        anyhow::anyhow!("compiling data condition of variant '{}': {e}", pv.name)
                    })?
                    .required_columns
            }
            None => BTreeSet::new(),
        };

        let profile = Arc::new(StoredProfile {
            name: pv.name.clone(),
            render: parse_render_text(&pv.render)?,
        });

        variants.push(StoredVariant {
            name: pv.name.clone(),
            priority: pv.priority,
            condition_source: condition_src,
            condition_required,
            data_condition,
            data_condition_required,
            ui_condition,
            profile,
        });
    }
    variants.sort_by_key(|v| std::cmp::Reverse(v.priority));
    Ok(variants)
}

/// Build an EntityProfile from a TypeDefinition's profile_variants.
/// Returns None if the TypeDefinition has no profile_variants.
pub fn profile_from_type_def(type_def: &holon_api::TypeDefinition) -> Option<EntityProfile> {
    if type_def.profile_variants.is_empty() {
        return None;
    }
    let variants = profile_variants_to_stored(&type_def.profile_variants).unwrap_or_else(|e| {
        panic!(
            "Failed to parse profile variants for entity '{}': {e:#}",
            type_def.name
        )
    });

    let computed_fields: Vec<CompiledComputedField> = type_def
        .computed_fields()
        .into_iter()
        .map(|(name, expr)| (name.to_string(), expr.clone()))
        .collect();

    // Declared schema = the TypeDefinition's persistent field names. This is the
    // O1 contract for type-aware binding: a required column MISSING from a row
    // is a real projection gap (LOUD) iff it is one of these declared columns;
    // otherwise (an optional property flattened from `properties`, or a UI-state
    // variable) it is expected heterogeneity and stays silent.
    //
    // Only PERSISTENT fields — `persistent_fields()` excludes `Computed`-lifetime
    // entries, which are the profile's OWN computed fields (is_program,
    // is_rule_head, …) registered into the TypeDefinition. Those are NOT columns
    // the projection carries; classifying them as declared would turn every
    // unbound-sibling propagation into a false LOUD.
    //
    // NOTE: `#[edge_field]` columns (e.g. block `tags`/`requires`) are marked
    // skip-serialization by the Entity derive, so they are NOT in `fields` and
    // are therefore currently classified as optional (silent). This is a known
    // v1 narrowing (see BugFunnel). The persistent columns present here
    // (e.g. `source_language`, `content_type`) already surface the real
    // projection gaps observed at boot.
    let declared_columns: BTreeSet<String> = type_def
        .persistent_fields()
        .iter()
        .map(|f| f.name.clone())
        .collect();

    Some(EntityProfile {
        entity_name: holon_api::EntityName::new(&type_def.name),
        variants,
        computed_fields,
        virtual_child: None,
        declared_columns,
    })
}

// ---------------------------------------------------------------------------
// Computed field parsing + topo-sort
// ---------------------------------------------------------------------------

struct RawComputedField {
    name: String,
    source: String,
}

fn parse_and_sort_computed_fields(
    engine: &RhaiEngine,
    raw: &BTreeMap<String, String>,
) -> Result<Vec<CompiledComputedField>> {
    let mut fields = Vec::new();
    for (name, value) in raw {
        let source = strip_rhai_prefix(value);
        let compiled = CompiledExpr::compile(engine, &source)
            .map_err(|e| anyhow::anyhow!("Failed to compile computed field '{name}': {e}"))?;
        fields.push((
            RawComputedField {
                name: name.clone(),
                source,
            },
            compiled,
        ));
    }
    Ok(topo_sort_computed_fields(fields))
}

fn topo_sort_computed_fields(
    fields: Vec<(RawComputedField, CompiledExpr)>,
) -> Vec<CompiledComputedField> {
    if fields.is_empty() {
        return vec![];
    }

    let names: HashSet<&str> = fields.iter().map(|(f, _)| f.name.as_str()).collect();
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();

    for (field, _) in &fields {
        let mut field_deps = Vec::new();
        for other in &names {
            if *other != field.name.as_str()
                && holon_core::util::expr_references(&field.source, other)
            {
                field_deps.push(*other);
            }
        }
        deps.insert(field.name.as_str(), field_deps);
    }

    let order = holon_core::util::topo_sort_kahn(&names, &deps);

    let mut field_map: HashMap<String, (RawComputedField, CompiledExpr)> = fields
        .into_iter()
        .map(|(f, c)| (f.name.clone(), (f, c)))
        .collect();

    order
        .into_iter()
        .map(|name| {
            let (_raw, compiled) = field_map.remove(&name).unwrap();
            (name, compiled)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Entity lookup registration for Rhai
// ---------------------------------------------------------------------------

/// Build a Rhai engine carrying the entity-lookup functions.
///
/// The single seat: the production [`ProfileResolver`] and the PBT oracle both
/// evaluate the same profile expressions, so both build their engine here.
pub fn build_lookup_engine(live_entities: &LiveEntities) -> RhaiEngine {
    let mut engine = RhaiEngine::new();
    register_entity_lookups(&mut engine, live_entities);
    engine
}

/// Register per-entity lookup functions on a Rhai engine.
///
/// For each entry in `live_entities`, registers a function named after the
/// entity (e.g. `document("block:<uuid>")`) that returns the entity's
/// properties as a Rhai map.
fn register_entity_lookups(engine: &mut RhaiEngine, live_entities: &LiveEntities) {
    for (entity_name, live_data) in live_entities {
        let data = Arc::clone(live_data);
        // Rhai identifiers use underscores; EntityName normalizes to hyphens for URI
        // schemes.
        let name = entity_name.as_str().replace('-', "_");
        engine.register_fn(&name, move |id: String| -> rhai::Dynamic {
            let items = data.read();
            match items.get(&id) {
                Some(entity) => storage_entity_to_rhai_map(entity),
                None => rhai::Dynamic::UNIT,
            }
        });
    }
}

/// Convert a StorageEntity (HashMap<String, Value>) to a Rhai map.
/// Flattens `properties` sub-object into top-level keys.
fn storage_entity_to_rhai_map(entity: &StorageEntity) -> rhai::Dynamic {
    let mut map = rhai::Map::new();
    for (k, v) in entity {
        if &**k == "properties" {
            if let holon_api::Value::Object(props) = v {
                for (pk, pv) in props {
                    map.insert(pk.clone().into(), value_to_dynamic(pv));
                }
            } else if let holon_api::Value::String(json_str) = v
                && let Ok(parsed) =
                    serde_json::from_str::<HashMap<String, holon_api::Value>>(json_str)
            {
                for (pk, pv) in &parsed {
                    map.insert(pk.clone().into(), value_to_dynamic(pv));
                }
            }
        }
        map.insert(k.as_ref().into(), value_to_dynamic(v));
    }
    rhai::Dynamic::from(map)
}

// ---------------------------------------------------------------------------
// ProfileResolving trait + ProfileResolver
// ---------------------------------------------------------------------------

/// Concrete profile resolver backed by LiveData (CDC-driven, live-updating).
///
/// Variants are filtered against `UiInfo` — if a variant references widgets
/// the frontend can't render, it's dropped.
///
/// Cache is rebuilt reactively in a background task when LiveData changes.
/// `resolve()` reads the cache via `watch::Receiver<Arc<ProfileCache>>` —
/// just an Arc clone, no RwLock contention on the hot path.
pub struct ProfileResolver {
    #[allow(dead_code)] // keeps the LiveData stream alive while the resolver exists
    source: Arc<holon_api::live_data::LiveData<EntityProfile>>,
    cache_signal: futures_signals::signal::Mutable<Arc<ProfileCache>>,
    /// Entity operations from the OperationDispatcher, keyed by entity name.
    /// Injected at DI time — this is the single source of truth for operations.
    entity_operations: Arc<HashMap<EntityName, Vec<OperationDescriptor>>>,
    live_entities: std::sync::RwLock<LiveEntities>,
    /// Cached Rhai engine with entity lookup functions pre-registered.
    /// Rebuilt only when `live_entities` changes via `set_live_entities()`.
    rhai_engine: std::sync::RwLock<Arc<RhaiEngine>>,
}

impl ProfileResolver {
    pub fn new(
        source: Arc<holon_api::live_data::LiveData<EntityProfile>>,
        ui_info: holon_api::UiInfo,
        live_entities: LiveEntities,
        entity_operations: HashMap<EntityName, Vec<OperationDescriptor>>,
    ) -> Self {
        Self::with_type_profiles(
            source,
            ui_info,
            live_entities,
            entity_operations,
            Vec::new(),
        )
    }

    /// Create a ProfileResolver seeded with type-defined profiles.
    ///
    /// Type-defined profiles are seeded first; org-based profiles override
    /// them.
    pub fn with_type_profiles(
        source: Arc<holon_api::live_data::LiveData<EntityProfile>>,
        ui_info: holon_api::UiInfo,
        live_entities: LiveEntities,
        entity_operations: HashMap<EntityName, Vec<OperationDescriptor>>,
        type_profiles: Vec<EntityProfile>,
    ) -> Self {
        let entity_operations = Arc::new(entity_operations);
        let type_profiles = Arc::new(type_profiles);
        let initial_cache = Arc::new(Self::build_cache_from_source(
            &source,
            &ui_info,
            &type_profiles,
        ));
        let cache_signal = futures_signals::signal::Mutable::new(initial_cache);

        let bg_source = Arc::clone(&source);
        let bg_type_profiles = Arc::clone(&type_profiles);
        let signal = source.signal_map();
        let bg_signal = cache_signal.clone();
        tokio::spawn(async move {
            signal
                .for_each(move |_diff| {
                    let new_cache = Arc::new(Self::build_cache_from_source(
                        &bg_source,
                        &ui_info,
                        &bg_type_profiles,
                    ));
                    bg_signal.set(new_cache);
                    async {}
                })
                .await;
        });

        let rhai_engine = Arc::new(Self::build_rhai_engine(&live_entities));

        ProfileResolver {
            source,
            cache_signal,
            entity_operations,
            rhai_engine: std::sync::RwLock::new(rhai_engine),
            live_entities: std::sync::RwLock::new(live_entities),
        }
    }

    /// Build a Rhai engine with entity lookup functions pre-registered.
    fn build_rhai_engine(live_entities: &LiveEntities) -> RhaiEngine {
        build_lookup_engine(live_entities)
    }

    /// Replace the live entities used for Rhai lookup functions.
    ///
    /// Called after `preload_startup_views` to avoid the matviews being
    /// dropped by stale view cleanup during startup.
    pub fn set_live_entities(&self, entities: LiveEntities) {
        tracing::info!(
            "[ProfileResolver] set_live_entities: {} entities registered: {:?}",
            entities.len(),
            entities.keys().map(|k| k.as_str()).collect::<Vec<_>>()
        );
        let new_engine = Arc::new(Self::build_rhai_engine(&entities));
        *self.rhai_engine.write().unwrap() = new_engine;
        *self.live_entities.write().unwrap() = entities;
    }

    /// Look up operations for an entity name. Entity-level (keyed by id
    /// scheme), so this returns exactly the operations the renderer
    /// attaches to a row of that entity (see `materialize`). Exposed via
    /// the `ProfileResolving` trait.
    fn lookup_operations(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.entity_operations
            .get(&EntityName::new(entity_name))
            .cloned()
            .unwrap_or_default()
    }

    /// Combine a StoredProfile with entity operations to produce a
    /// RenderProfile.
    ///
    /// Operations are looked up by the ID scheme (e.g. "block" from
    /// "block:xxx"), not by `entity_name` which may be a view/matview alias
    /// like "focus_roots".
    fn materialize(
        &self,
        stored: &StoredProfile,
        row: &HashMap<String, holon_api::Value>,
    ) -> Arc<RenderProfile> {
        let ops = row_id(row)
            .map(|id| self.lookup_operations(id.scheme()))
            .unwrap_or_default();
        Arc::new(RenderProfile {
            name: stored.name.clone(),
            render: stored.render.clone(),
            operations: ops,
            variants: Vec::new(),
        })
    }

    fn build_cache_from_source(
        source: &holon_api::live_data::LiveData<EntityProfile>,
        ui_info: &holon_api::UiInfo,
        type_profiles: &[EntityProfile],
    ) -> ProfileCache {
        let mut profiles = HashMap::new();

        // Seed with type-defined profiles (baseline layer) // ALLOW(fallback):
        // describes profile layering, not error swallowing
        for profile in type_profiles {
            let name = profile.entity_name.clone();
            profiles.insert(name, profile.clone());
        }

        // Overlay org-based profiles (org wins via merge — higher priority overrides)
        let items = source.read();
        for profile in items.values() {
            let filtered = Self::filter_profile(profile, ui_info);
            let name = filtered.entity_name.clone();
            if let Some(existing) = profiles.get_mut(&name) {
                Self::merge_profile(existing, &filtered);
            } else {
                profiles.insert(name, filtered);
            }
        }
        ProfileCache::new(profiles)
    }

    /// Merge a new profile into an existing one with the same entity name.
    /// Variant lists are combined (not replaced) and re-sorted by priority.
    /// Computed fields from the incoming profile are added (incoming wins on
    /// name conflict).
    fn merge_profile(existing: &mut EntityProfile, incoming: &EntityProfile) {
        tracing::info!(
            "[ProfileResolver::merge_profile] entity='{}', existing_variants={}, \
             incoming_variants={}",
            existing.entity_name,
            existing.variants.len(),
            incoming.variants.len(),
        );
        // Combine variant lists — priority handles resolution order
        existing.variants.extend(incoming.variants.iter().cloned());
        existing
            .variants
            .sort_by_key(|v| std::cmp::Reverse(v.priority));

        // Computed fields: incoming overrides existing by name
        for (name, expr) in &incoming.computed_fields {
            if let Some(pos) = existing.computed_fields.iter().position(|(n, _)| n == name) {
                existing.computed_fields[pos] = (name.clone(), expr.clone());
            } else {
                existing.computed_fields.push((name.clone(), expr.clone()));
            }
        }
    }

    fn filter_profile(profile: &EntityProfile, ui_info: &holon_api::UiInfo) -> EntityProfile {
        if ui_info.is_permissive() {
            return profile.clone();
        }

        tracing::info!(
            "[filter_profile] entity='{}', available_widgets={:?}",
            profile.entity_name,
            ui_info.available_widgets
        );

        let filtered_variants: Vec<RowVariant> = profile
            .variants
            .iter()
            .filter(|v| {
                let names = holon_api::extract_widget_names(&v.profile.render);
                ui_info.supports_all(&names)
            })
            .cloned()
            .collect();

        EntityProfile {
            entity_name: profile.entity_name.clone(),
            variants: filtered_variants,
            computed_fields: profile.computed_fields.clone(),
            virtual_child: profile.virtual_child.clone(),
            declared_columns: profile.declared_columns.clone(),
        }
    }
}

impl ProfileResolving for ProfileResolver {
    fn operations_for(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.lookup_operations(entity_name)
    }

    fn resolve(&self, row: &HashMap<String, holon_api::Value>) -> Arc<RenderProfile> {
        self.resolve_with_computed(row).0
    }

    fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> (Arc<RenderProfile>, HashMap<String, holon_api::Value>) {
        let cache = self.cache_signal.get_cloned();

        // A row either carries an entity-shaped `id` or it does not — BOTH are
        // representable, legal inputs. Id-less rows arise legitimately from
        // rule-trigger / aggregate queries (e.g. `SELECT date('now') AS name`,
        // dogfood 2026-07-10) that are pointed at the enriched watch path.
        // A value row (no entity `id`) is a LEGITIMATE display case — an
        // aggregate / rule-trigger result — NOT an error. It cannot be resolved
        // to an entity profile, so it renders plainly as a value row. Logged at
        // debug (not warn): this is an expected shape, not a degradation. The
        // loud path is `resolve_entity_required`, taken by callers that DECLARE
        // they need an entity row. (Historically this `panic!`ed and killed the
        // render/resolve worker, blanking the page with a silent -32603.)
        let entity_uri = match row_id(row) {
            Ok(uri) => uri,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "profile resolver: value-shaped row (no entity `id`) — rendering as a \
                     plain value row (aggregate / rule-trigger result); entity resolution \
                     not attempted"
                );
                return (Arc::new(value_row_profile(row)), HashMap::new());
            }
        };
        let entity_name_str = entity_uri.scheme();
        let entity_name = EntityName::new(entity_name_str);

        let entity_profile = match cache.get(&entity_name) {
            Some(profile) => profile.clone(),
            None => {
                tracing::trace!(
                    "No profile registered for entity '{entity_name_str}' — using default",
                );
                return (
                    Arc::new(RenderProfile {
                        name: "default".to_string(),
                        render: holon_api::RenderExpr::Literal {
                            value: holon_api::Value::String("".to_string()),
                        },
                        operations: vec![],
                        variants: vec![],
                    }),
                    HashMap::new(),
                );
            }
        };

        let engine = self.rhai_engine.read().unwrap().clone();
        let (stored, computed) = entity_profile.resolve_with_computed(row, &engine);
        let stored = stored.unwrap_or_else(|| {
            let variants: Vec<_> = entity_profile
                .variants
                .iter()
                .map(|v| {
                    format!(
                        "{}(priority={}, cond={:?})",
                        v.name, v.priority, v.condition_source
                    )
                })
                .collect();
            panic!(
                "No variant matched for entity '{entity_name_str}' (row id='{entity_uri}'). \
                 Variants tried: {variants:?}"
            )
        });
        (self.materialize(&stored, row), computed)
    }

    fn resolve_computed_only(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> HashMap<String, holon_api::Value> {
        // Mirror the short-circuits of `resolve_with_computed` (value rows and
        // unregistered entities carry no computed fields) but SKIP variant
        // resolution — the enrichment boundary wants only computed fields, and
        // resolving here evaluated UI-bearing variant conditions against
        // UI-less storage rows (spurious eval errors). See
        // `EntityProfile::compute_fields_only`.
        let entity_uri = match row_id(row) {
            Ok(uri) => uri,
            Err(_) => return HashMap::new(),
        };
        let cache = self.cache_signal.get_cloned();
        let entity_profile = match cache.get(&EntityName::new(entity_uri.scheme())) {
            Some(profile) => profile.clone(),
            None => return HashMap::new(),
        };
        let engine = self.rhai_engine.read().unwrap().clone();
        entity_profile.compute_fields_only(row, &engine)
    }

    fn resolve_batch(&self, rows: &[HashMap<String, holon_api::Value>]) -> Vec<Arc<RenderProfile>> {
        rows.iter()
            .map(|row| ProfileResolving::resolve(self, row))
            .collect()
    }

    fn resolve_with_variants(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> (Arc<RenderProfile>, HashMap<String, holon_api::Value>) {
        let cache = self.cache_signal.get_cloned();
        // A row either carries an entity-shaped `id` or it does not — BOTH are
        // representable, legal inputs. Id-less rows arise legitimately from
        // rule-trigger / aggregate queries (e.g. `SELECT date('now') AS name`,
        // dogfood 2026-07-10) that are pointed at the enriched watch path.
        // A value row (no entity `id`) is a LEGITIMATE display case — an
        // aggregate / rule-trigger result — NOT an error. It cannot be resolved
        // to an entity profile, so it renders plainly as a value row. Logged at
        // debug (not warn): this is an expected shape, not a degradation. The
        // loud path is `resolve_entity_required`, taken by callers that DECLARE
        // they need an entity row. (Historically this `panic!`ed and killed the
        // render/resolve worker, blanking the page with a silent -32603.)
        let entity_uri = match row_id(row) {
            Ok(uri) => uri,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "profile resolver: value-shaped row (no entity `id`) — rendering as a \
                     plain value row (aggregate / rule-trigger result); entity resolution \
                     not attempted"
                );
                return (Arc::new(value_row_profile(row)), HashMap::new());
            }
        };
        let entity_name_str = entity_uri.scheme();
        let entity_name = EntityName::new(entity_name_str);

        let entity_profile = match cache.get(&entity_name) {
            Some(profile) => profile.clone(),
            None => {
                tracing::trace!(
                    "No profile registered for entity '{entity_name_str}' — using default",
                );
                return (
                    Arc::new(RenderProfile {
                        name: "default".to_string(),
                        render: holon_api::RenderExpr::Literal {
                            value: holon_api::Value::String("".to_string()),
                        },
                        operations: vec![],
                        variants: vec![],
                    }),
                    HashMap::new(),
                );
            }
        };

        let engine = self.rhai_engine.read().unwrap().clone();
        let (candidates, computed) = entity_profile.resolve_candidates(row, &engine);

        let ops = row_id(row)
            .map(|id| self.lookup_operations(id.scheme()))
            .unwrap_or_default();

        let render_variants: Vec<RenderVariant> = candidates
            .iter()
            .map(|(variant, stored)| RenderVariant {
                name: stored.name.clone(),
                render: stored.render.clone(),
                operations: ops.clone(),
                condition: variant.ui_condition.clone(),
            })
            .collect();

        let (_, stored) = candidates.first().unwrap_or_else(|| {
            let variants: Vec<_> = entity_profile
                .variants
                .iter()
                .map(|v| {
                    format!(
                        "{}(priority={}, data_cond={:?})",
                        v.name, v.priority, v.data_condition
                    )
                })
                .collect();
            panic!(
                "No variant matched for entity '{entity_name_str}' (row id='{entity_uri}'). \
                 Variants tried: {variants:?}"
            )
        });
        let first_profile = Arc::new(RenderProfile {
            name: stored.name.clone(),
            render: stored.render.clone(),
            operations: ops.clone(),
            variants: render_variants,
        });

        (first_profile, computed)
    }

    fn resolve_collection_variants(&self) -> Vec<RenderVariant> {
        self.resolve_collection_variants_named(&EntityName::new("collection"))
            .unwrap_or_default()
    }

    fn resolve_collection_variants_named(&self, name: &EntityName) -> Option<Vec<RenderVariant>> {
        let cache = self.cache_signal.get_cloned();
        let profile = cache.get(name)?;

        Some(
            profile
                .variants
                .iter()
                .map(|v| RenderVariant {
                    name: v.name.clone(),
                    render: v.profile.render.clone(),
                    operations: Vec::new(),
                    condition: v.ui_condition.clone(),
                })
                .collect(),
        )
    }

    fn virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig> {
        let cache = self.cache_signal.get_cloned();
        cache.get(entity_name).and_then(|p| p.virtual_child.clone())
    }

    fn profile_signal(&self) -> futures_signals::signal::Mutable<Arc<ProfileCache>> {
        self.cache_signal.clone()
    }
}

/// Check if a block is an entity profile block (source_language =
/// holon_entity_profile_yaml).
pub fn is_profile_block_by_source_language(source_language: Option<&str>) -> bool {
    source_language == Some("holon_entity_profile_yaml")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk a parsed render tree and collect every string-literal value (the
    /// text a `text("…")` / label arg carries), so a test can assert none was
    /// corrupted into mojibake on the parse path.
    fn collect_string_literals(expr: &RenderExpr, out: &mut Vec<String>) {
        use holon_api::Value;
        match expr {
            RenderExpr::Literal { value } => {
                if let Value::String(s) = value {
                    out.push(s.clone());
                }
            }
            RenderExpr::FunctionCall { args, .. } => {
                for a in args {
                    collect_string_literals(&a.value, out);
                }
            }
            RenderExpr::Array { items } => {
                for it in items {
                    collect_string_literals(it, out);
                }
            }
            RenderExpr::Object { fields } => {
                for v in fields.values() {
                    collect_string_literals(v, out);
                }
            }
            RenderExpr::BinaryOp { left, right, .. } => {
                collect_string_literals(left, out);
                collect_string_literals(right, out);
            }
            RenderExpr::LiveBlock { .. } | RenderExpr::ColumnRef { .. } => {}
        }
    }

    fn init_render_dsl() {
        holon_api::render_dsl::register_widget_names(&[
            "table",
            "live_block",
            "columns",
            "text",
            "row",
            "icon",
            "spacer",
            "tree",
            "render_entity",
            "list",
            "selectable",
            "chain_ops",
            "state_toggle",
            "drawer",
            "if_space",
            "bottom_dock",
            "op_button",
            "chat_bubble",
            "editable_text",
            "focusable",
            "live_query",
        ]);
    }

    /// Bug-4 (PERCEPTION): the `rule_card` variant used a literal em-dash
    /// (`—`, U+2014) as the "last fired" placeholder. The DSL parse preserves
    /// it intact, but the live GPUI render/capture path surfaced only its
    /// first UTF-8 byte (0xE2 → `â`) — mojibake. The placeholders are now
    /// plain ASCII; guard that the real `rule_card` render carries no byte
    /// that would re-open the mojibake (0xE2 is the lead byte of the
    /// em/en-dash family that regressed).
    #[test]
    fn rule_card_render_has_no_mojibake_bytes() {
        init_render_dsl();
        let profile = parse_entity_profile(BLOCK_PROFILE_YAML).unwrap();
        let rule_card = profile
            .variants
            .iter()
            .find(|v| v.name == "rule_card")
            .expect("block profile defines a rule_card variant");
        let mut lits = Vec::new();
        collect_string_literals(&rule_card.profile.render, &mut lits);
        assert!(
            lits.iter().any(|s| s.contains("last fired")),
            "sanity: rule_card carries the 'last fired' placeholder text node: {lits:?}"
        );
        for s in &lits {
            assert!(
                !s.as_bytes().contains(&0xE2),
                "rule_card text node contains a multibyte char (0xE2 lead byte) that mangles on \
                 the live render path — keep placeholders ASCII: {s:?}"
            );
        }
    }

    #[test]
    fn test_parse_render_text_simple() {
        init_render_dsl();
        let expr = parse_render_text(r#"row(text(#{content: col("content")}))"#).unwrap();
        match &expr {
            RenderExpr::FunctionCall { name, args, .. } => {
                assert_eq!(name, "row");
                assert_eq!(args.len(), 1);
            }
            other => panic!("Expected FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn split_condition_extracts_numeric_ui_lt() {
        // Pure UI-side conjunct.
        let (data, ui) = split_condition("available_width_px < 600").unwrap();
        assert!(data.is_none());
        assert_eq!(
            ui,
            Predicate::Lt {
                field: "available_width_px".into(),
                value: Value::Integer(600),
            }
        );
    }

    #[test]
    fn split_condition_extracts_numeric_ui_gte_lte() {
        let (data, ui) = split_condition("available_height_px >= 800").unwrap();
        assert!(data.is_none());
        assert_eq!(
            ui,
            Predicate::Gte {
                field: "available_height_px".into(),
                value: Value::Integer(800),
            }
        );

        let (data, ui) = split_condition("scale_factor <= 1.5").unwrap();
        assert!(data.is_none());
        assert_eq!(
            ui,
            Predicate::Lte {
                field: "scale_factor".into(),
                value: Value::Float(1.5),
            }
        );
    }

    #[test]
    fn split_condition_mixes_data_and_ui_comparison() {
        // Data-side Eq on a non-UI variable + UI-side Lt → split.
        let (data, ui) =
            split_condition("task_state == \"done\" && available_width_px < 480").unwrap();
        assert_eq!(data.as_deref(), Some("task_state == \"done\""));
        assert_eq!(
            ui,
            Predicate::Lt {
                field: "available_width_px".into(),
                value: Value::Integer(480),
            }
        );
    }

    #[test]
    fn split_condition_combines_ui_var_and_ui_comparison() {
        // Two UI conjuncts → And(Var, Lt).
        let (data, ui) = split_condition("is_focused && available_width_px < 600").unwrap();
        assert!(data.is_none());
        assert_eq!(
            ui,
            Predicate::And(vec![
                Predicate::Var("is_focused".into()),
                Predicate::Lt {
                    field: "available_width_px".into(),
                    value: Value::Integer(600),
                },
            ])
        );
    }

    #[test]
    fn split_condition_extracts_is_expanded_as_ui_predicate() {
        let (data, ui) = split_condition("is_expanded").unwrap();
        assert!(data.is_none());
        assert_eq!(ui, Predicate::Var("is_expanded".into()));
    }

    #[test]
    fn test_expr_references() {
        use holon_core::util::expr_references;
        assert!(expr_references("is_task && priority > 0", "is_task"));
        assert!(expr_references("is_task", "is_task"));
        assert!(!expr_references("is_task_done", "is_task"));
        assert!(!expr_references("my_is_task", "is_task"));
        assert!(expr_references("a + is_task + b", "is_task"));
    }

    #[test]
    fn test_topo_sort_empty() {
        let result = topo_sort_computed_fields(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_topo_sort_with_dependencies() {
        let engine = RhaiEngine::new();
        let fields = vec![
            (
                RawComputedField {
                    name: "b".to_string(),
                    source: "a + 1".to_string(),
                },
                CompiledExpr::compile(&engine, "a + 1").unwrap(),
            ),
            (
                RawComputedField {
                    name: "a".to_string(),
                    source: "42".to_string(),
                },
                CompiledExpr::compile(&engine, "42").unwrap(),
            ),
        ];
        let sorted = topo_sort_computed_fields(fields);
        assert_eq!(sorted[0].0, "a");
        assert_eq!(sorted[1].0, "b");
    }

    #[test]
    fn test_parse_entity_profile_basic() {
        let yaml = r#"
entity_name: block

computed:
  is_task: "= task_state != ()"

variants:
  - name: task
    condition: "= is_task"
    render: 'row(col("content"))'
  - name: default
    render: 'row(col("content"))'
"#;
        let profile = parse_entity_profile(yaml).unwrap();
        assert_eq!(profile.entity_name, "block");
        assert_eq!(profile.computed_fields.len(), 1);
        assert_eq!(profile.computed_fields[0].0, "is_task");
        assert_eq!(profile.variants.len(), 2);
        assert_eq!(profile.variants[0].name, "task");
    }

    fn make_test_profile(yaml: &str) -> EntityProfile {
        parse_entity_profile(yaml).unwrap()
    }

    #[test]
    fn test_resolve_default() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
variants:
  - name: task
    condition: "= task_state != ()"
    render: 'row(col("content"))'
  - name: default
    render: 'row(col("content"))'
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "content".to_string(),
            holon_api::Value::String("hello".to_string()),
        );
        let resolved = profile.resolve(&row, &RhaiEngine::new()).unwrap();
        assert_eq!(resolved.name, "default");
    }

    /// The `bullet_shape` computed field (block_profile.yaml) picks the ringed
    /// `orgmode` glyph for a collapsed block (which always has hidden children)
    /// and the plain `circle` dot otherwise. A missing `collapsed` column
    /// leaves the field unbound, so `icon`'s `"circle"` default takes over
    /// — the dot.
    #[test]
    fn bullet_shape_ring_when_collapsed_else_dot() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed:
  bullet_shape: 'if collapsed != () && collapsed != 0 { "orgmode" } else { "circle" }'
variants:
  - name: default
    render: 'row(col("content"))'
"#,
        );
        let engine = RhaiEngine::new();

        let row = |collapsed: Option<i64>| {
            let mut r = HashMap::new();
            r.insert("id".to_string(), Value::String("block:x".to_string()));
            if let Some(c) = collapsed {
                r.insert("collapsed".to_string(), Value::Integer(c));
            }
            r
        };

        assert_eq!(
            profile
                .compute_fields_only(&row(Some(1)), &engine)
                .get("bullet_shape"),
            Some(&Value::String("orgmode".to_string())),
            "collapsed block → ringed orgmode bullet"
        );
        assert_eq!(
            profile
                .compute_fields_only(&row(Some(0)), &engine)
                .get("bullet_shape"),
            Some(&Value::String("circle".to_string())),
            "expanded/leaf block → plain circle dot"
        );
        // Absent `collapsed`: the field is unbound (not a "orgmode"), so the
        // icon name falls back to its default dot.
        let out = profile.compute_fields_only(&row(None), &engine);
        assert_ne!(
            out.get("bullet_shape"),
            Some(&Value::String("orgmode".to_string())),
            "missing collapsed must never yield the ring"
        );
    }

    #[test]
    fn test_resolve_variant() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
variants:
  - name: task
    condition: "= task_state != ()"
    render: 'row(col("content"))'
  - name: default
    render: 'row(col("content"))'
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "content".to_string(),
            holon_api::Value::String("hello".to_string()),
        );
        row.insert(
            "task_state".to_string(),
            holon_api::Value::String("TODO".to_string()),
        );
        let resolved = profile.resolve(&row, &RhaiEngine::new()).unwrap();
        assert_eq!(resolved.name, "task");
    }

    #[test]
    fn test_resolve_variant_from_nested_properties() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
variants:
  - name: task
    condition: "= task_state != ()"
    render: 'row(col("content"))'
  - name: default
    render: 'row(col("content"))'
"#,
        );

        // task_state nested inside properties (as it comes from `from children`
        // queries)
        let mut props = HashMap::new();
        props.insert(
            "task_state".to_string(),
            holon_api::Value::String("DOING".to_string()),
        );
        let mut row = HashMap::new();
        row.insert(
            "content".to_string(),
            holon_api::Value::String("hello".to_string()),
        );
        row.insert("properties".to_string(), holon_api::Value::Object(props));
        let resolved = profile.resolve(&row, &RhaiEngine::new()).unwrap();
        assert_eq!(resolved.name, "task");
    }

    #[test]
    fn test_resolve_preferred_variant() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
variants:
  - name: compact
    condition: "= true"
    render: 'row(col("content"))'
  - name: detailed
    condition: "= true"
    render: 'row(col("content"))'
"#,
        );

        let row = HashMap::new();
        let resolved = profile.resolve(&row, &RhaiEngine::new()).unwrap();
        // Both variants have equal priority (default 0).
        // Stable sort preserves YAML order → "compact" wins as first match.
        assert_eq!(resolved.name, "compact");
    }

    #[test]
    fn test_resolve_with_computed_fields() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed:
  is_task: "= task_state != ()"
variants:
  - name: task
    condition: "= is_task"
    render: 'row(col("content"))'
  - name: default
    render: 'row(col("content"))'
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "task_state".to_string(),
            holon_api::Value::String("TODO".to_string()),
        );
        let resolved = profile.resolve(&row, &RhaiEngine::new()).unwrap();
        assert_eq!(resolved.name, "task");
    }

    #[test]
    fn test_resolve_with_computed_returns_values() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed:
  greeting: '= "hello " + content'
  upper_len: '= len(content)'
variants:
  - name: default
    render: 'row(col("content"))'
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "content".to_string(),
            holon_api::Value::String("world".to_string()),
        );
        let (profile_result, computed) = profile.resolve_with_computed(&row, &RhaiEngine::new());
        assert_eq!(profile_result.unwrap().name, "default");
        assert_eq!(
            computed.get("greeting"),
            Some(&holon_api::Value::String("hello world".to_string()))
        );
        assert_eq!(
            computed.get("upper_len"),
            Some(&holon_api::Value::Integer(5))
        );
    }

    #[test]
    fn test_entity_profile_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EntityProfile>();
        assert_send_sync::<ProfileResolver>();
    }

    #[test]
    fn test_extract_widget_names() {
        init_render_dsl();
        let expr = parse_render_text(
            r#"row(state_toggle(col("task_state")), spacer(8), editable_text(col("content")))"#,
        )
        .unwrap();
        let names = holon_api::extract_widget_names(&expr);
        assert!(names.contains("row"));
        assert!(names.contains("state_toggle"));
        assert!(names.contains("spacer"));
        assert!(names.contains("editable_text"));
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn test_ui_info_filtering() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
variants:
  - name: tree_view
    condition: "= true"
    render: 'tree(col("id"))'
"#,
        );

        // With permissive UiInfo, variant is kept
        let permissive = holon_api::UiInfo::permissive();
        let filtered = ProfileResolver::filter_profile(&profile, &permissive);
        assert_eq!(filtered.variants.len(), 1);

        // With UiInfo that only has editable_text, tree variant is dropped
        let mut limited_widgets = std::collections::HashSet::new();
        limited_widgets.insert("editable_text".to_string());
        let limited = holon_api::UiInfo {
            available_widgets: limited_widgets,
            screen_size: None,
        };
        let filtered = ProfileResolver::filter_profile(&profile, &limited);
        assert_eq!(filtered.variants.len(), 0);
    }

    #[test]
    fn test_split_condition_pure_ui() {
        let (data, ui) = split_condition("is_focused").unwrap();
        assert!(data.is_none());
        assert_eq!(ui, Predicate::Var("is_focused".into()));
    }

    #[test]
    fn test_split_condition_pure_data() {
        let (data, ui) = split_condition("is_task").unwrap();
        assert_eq!(data.as_deref(), Some("is_task"));
        assert_eq!(ui, Predicate::Always);
    }

    #[test]
    fn test_split_condition_mixed() {
        let (data, ui) = split_condition("is_source && is_focused").unwrap();
        assert_eq!(data.as_deref(), Some("is_source"));
        assert_eq!(ui, Predicate::Var("is_focused".into()));
    }

    #[test]
    fn test_split_condition_ui_eq() {
        let (data, ui) = split_condition(r#"is_source && view_mode == "table""#).unwrap();
        assert_eq!(data.as_deref(), Some("is_source"));
        assert_eq!(
            ui,
            Predicate::Eq {
                field: "view_mode".into(),
                value: holon_api::Value::String("table".into())
            }
        );
    }

    #[test]
    fn test_split_condition_all_data() {
        let (data, ui) = split_condition("task_state != () && priority > 0").unwrap();
        assert_eq!(data.as_deref(), Some("task_state != () && priority > 0"));
        assert_eq!(ui, Predicate::Always);
    }

    // -----------------------------------------------------------------------
    // ADR 0024 WP3 — program marking / rule-card render variant.
    //
    // These resolve the REAL block_profile.yaml (not a hand-built fixture) so a
    // regression in `is_program` / the variant precedence fails here.
    // -----------------------------------------------------------------------

    const BLOCK_PROFILE_YAML: &str =
        include_str!("../../../assets/default/types/block_profile.yaml");

    /// Rhai engine with the DB-backed lookups the block profile's computed
    /// fields call. `rule_sibling(parent_id)` returns a non-unit row iff
    /// `parent_id` names a headline that owns a rule head — i.e. the caller
    /// is that rule's trigger sibling (WP3 clause b). `query_source` is
    /// stubbed empty (no block in these rows has query-source children).
    fn engine_with_rule_parent(rule_parent: &'static str) -> RhaiEngine {
        let mut engine = RhaiEngine::new();
        engine.register_fn("rule_sibling", move |parent_id: String| -> rhai::Dynamic {
            if parent_id == rule_parent {
                rhai::Dynamic::from(rhai::Map::new())
            } else {
                rhai::Dynamic::UNIT
            }
        });
        engine.register_fn("query_source", |_: String| -> rhai::Dynamic {
            rhai::Dynamic::UNIT
        });
        engine
    }

    fn source_row(id: &str, parent: &str, lang: &str) -> HashMap<String, holon_api::Value> {
        use holon_api::Value;
        let mut row = HashMap::new();
        row.insert("id".into(), Value::String(id.into()));
        row.insert("parent_id".into(), Value::String(parent.into()));
        row.insert("content_type".into(), Value::String("source".into()));
        row.insert("source_language".into(), Value::String(lang.into()));
        row.insert("content".into(), Value::String("<source body>".into()));
        row
    }

    fn variant_for(row: &HashMap<String, holon_api::Value>, engine: &RhaiEngine) -> String {
        parse_entity_profile(BLOCK_PROFILE_YAML)
            .unwrap()
            .resolve(row, engine)
            .expect("a variant (or default) must resolve")
            .name
            .clone()
    }

    #[test]
    fn rule_head_renders_rule_card() {
        // A `holon_rule` head is program → routed to the rule card, never a query.
        let engine = engine_with_rule_parent("block:journal-auto-create");
        let row = source_row("block:rule", "block:journal-auto-create", "holon_rule");
        assert_eq!(variant_for(&row, &engine), "rule_card");
    }

    #[test]
    fn trigger_sibling_renders_rule_card_not_query_result() {
        // The `holon_sql` trigger sibling of a rule is program via the discovery
        // join (clause b). Priority 0 wins over the `holon_source` spacer that its
        // language would otherwise match — it renders the card, NOT a query result
        // and NOT a hidden spacer.
        let engine = engine_with_rule_parent("block:journal-auto-create");
        let row = source_row("block:trigger", "block:journal-auto-create", "holon_sql");
        let variant = variant_for(&row, &engine);
        assert_eq!(variant, "rule_card");
        assert_ne!(variant, "source");
    }

    #[test]
    fn normal_holon_query_source_is_not_program() {
        // A holon_prql query source with NO rule sibling stays hidden machinery
        // (the `holon_source` spacer) — it must NOT be diverted to the rule card.
        let engine = engine_with_rule_parent("block:journal-auto-create");
        let row = source_row("block:q", "block:journals", "holon_prql");
        let variant = variant_for(&row, &engine);
        assert_ne!(variant, "rule_card");
        assert_eq!(variant, "holon_source");
    }

    #[test]
    fn plain_source_block_still_renders_query_result() {
        // A non-holon source block (not machinery, not a rule) still renders as a
        // query result — the `source` variant, gated on `!is_program`. No regression.
        let engine = engine_with_rule_parent("block:journal-auto-create");
        let row = source_row("block:py", "block:page", "python");
        assert_eq!(variant_for(&row, &engine), "source");
    }

    #[test]
    fn legacy_action_head_surfaces_deprecation_on_card() {
        // A retired `action`-language head is still program (rule card) and the
        // deprecation is surfaced: `is_legacy_rule` is true so the card's `if_col`
        // renders its loud error line.
        let engine = engine_with_rule_parent("block:journal-auto-create");
        let row = source_row("block:legacy", "block:journal-auto-create", "action");
        let (profile, computed) = parse_entity_profile(BLOCK_PROFILE_YAML)
            .unwrap()
            .resolve_with_computed(&row, &engine);
        assert_eq!(profile.expect("variant resolves").name, "rule_card");
        assert_eq!(
            computed.get("is_program"),
            Some(&holon_api::Value::Boolean(true))
        );
        assert_eq!(
            computed.get("is_legacy_rule"),
            Some(&holon_api::Value::Boolean(true))
        );
    }

    /// Regression (dogfood 2026-07-10, Martin ruling 2026-07-11): a
    /// rule-trigger / aggregate query row that carries NO entity-shaped
    /// `id` (e.g. `SELECT date('now','localtime') AS name`) used to panic
    /// the profile resolver and blank the whole page with a silent -32603.
    /// It must now render as a plain VALUE row (its columns, shown
    /// directly) — NOT a "⚠ unresolved" warning, since a value row is a
    /// legitimate display case.
    #[test]
    fn value_row_renders_plainly_instead_of_panicking() {
        let mut row: HashMap<String, Value> = HashMap::new();
        row.insert("_rowid".to_string(), Value::Integer(1));
        row.insert("name".to_string(), Value::String("2026-07-10".to_string()));

        let profile = value_row_profile(&row);
        assert_eq!(profile.name, "value-row");
        let RenderExpr::Literal {
            value: Value::String(text),
        } = &profile.render
        else {
            panic!(
                "value-row profile must render a string literal, got {:?}",
                profile.render
            );
        };
        // The user column is shown plainly, no warning marker, and the internal
        // `_rowid` bookkeeping column is hidden.
        assert!(
            !text.contains("unresolved"),
            "value row must NOT carry a degraded/unresolved marker: {text}"
        );
        assert!(
            text.contains("name: 2026-07-10"),
            "row data missing: {text}"
        );
        assert!(
            !text.contains("_rowid"),
            "internal column must be hidden: {text}"
        );
    }

    /// The CONTRACT seam: a caller that DECLARES it needs an entity row
    /// (`resolve_entity_required`) must FAIL LOUD when handed a value row,
    /// rather than silently rendering it.
    #[test]
    fn entity_required_resolution_fails_loud_on_value_row() {
        use holon_api::ProfileResolving;

        // Minimal resolver exercising the trait's DEFAULT `resolve_entity_required`
        // (the contract seam). Entity rows resolve to a dummy profile; the value
        // branch bails before ever calling `resolve_with_computed`.
        struct MockResolver;
        impl ProfileResolving for MockResolver {
            fn resolve(&self, _: &HashMap<String, Value>) -> Arc<RenderProfile> {
                Arc::new(RenderProfile {
                    name: "mock".to_string(),
                    render: RenderExpr::Literal { value: Value::Null },
                    operations: vec![],
                    variants: vec![],
                })
            }
            fn resolve_with_computed(
                &self,
                row: &HashMap<String, Value>,
            ) -> (Arc<RenderProfile>, HashMap<String, Value>) {
                (self.resolve(row), HashMap::new())
            }
            fn resolve_batch(&self, rows: &[HashMap<String, Value>]) -> Vec<Arc<RenderProfile>> {
                rows.iter().map(|r| self.resolve(r)).collect()
            }
        }

        let resolver = MockResolver;
        let mut value_row: HashMap<String, Value> = HashMap::new();
        value_row.insert("name".to_string(), Value::String("2026-07-10".to_string()));

        let err = resolver
            .resolve_entity_required(&value_row)
            .expect_err("entity-required resolution must reject a value row");
        let msg = err.to_string();
        assert!(
            msg.contains("requires an ENTITY row") && msg.contains("VALUE row"),
            "contract error must name the violation: {msg}"
        );

        // An entity-shaped row passes the contract gate.
        let mut entity_row: HashMap<String, Value> = HashMap::new();
        entity_row.insert("id".to_string(), Value::String("block:abc".to_string()));
        assert!(
            resolver.resolve_entity_required(&entity_row).is_ok(),
            "entity row must satisfy the entity-required contract"
        );
    }

    // -----------------------------------------------------------------------
    // Type-aware binding — the boot-flood fix. These use the REAL block
    // EntityProfile built from the TypeDefinition (so `declared_columns` is
    // populated), exercising the classification that silences the
    // task_state-class flood while surfacing declared-column projection gaps.
    // -----------------------------------------------------------------------

    /// The block `EntityProfile` with `declared_columns` populated (the prod
    /// shape, not the YAML-only `parse_entity_profile` fixture).
    fn block_profile_with_schema() -> EntityProfile {
        let registry = crate::type_registry::create_default_registry().unwrap();
        crate::type_registry::type_profiles_from_registry(&registry)
            .into_iter()
            .find(|p| p.entity_name.as_str() == "block")
            .expect("block profile must exist in the default registry")
    }

    #[test]
    fn declared_columns_separate_columns_from_optional_properties() {
        let profile = block_profile_with_schema();
        // Declared persistent columns → LOUD when missing.
        for col in ["content_type", "source_language", "content", "parent_id"] {
            assert!(
                profile.declared_columns.contains(col),
                "'{col}' must be a declared block column"
            );
        }
        // task_state is an optional PROPERTY (flattened from `properties`), not a
        // declared column → structurally-absent → silent.
        assert!(
            !profile.declared_columns.contains("task_state"),
            "task_state is a property, must NOT be a declared column (else the \
             dominant boot flood would become loud noise)"
        );
    }

    #[test]
    fn computed_fields_carry_the_required_columns_that_drive_classification() {
        let profile = block_profile_with_schema();
        let required = |name: &str| {
            profile
                .computed_fields
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, e)| e.required_columns.clone())
                .unwrap_or_else(|| panic!("computed field '{name}' missing"))
        };
        assert!(required("is_task").contains("task_state"));
        assert!(required("is_page_row").contains("tags"));
        assert!(required("is_legacy_rule").contains("source_language"));
    }

    /// The synthetic creation-slot row is a PROJECTION of the entity like any
    /// other: it must carry the declared schema, or every computed field over a
    /// column the YAML `virtual_child:` block happens not to set reports a
    /// projection gap it cannot possibly have (the row is built in-process).
    #[test]
    fn creation_slot_defaults_cover_every_declared_column() {
        let profile = block_profile_with_schema();
        let declared = profile.declared_columns.clone();
        let config = profile
            .virtual_child
            .expect("block declares a virtual_child creation slot");
        let missing: Vec<&String> = declared
            .iter()
            .filter(|c| !config.defaults.contains_key(*c))
            .collect();
        assert!(
            missing.is_empty(),
            "creation-slot row would omit declared block columns {missing:?}"
        );
    }

    /// The widening must not depend on WHICH source a profile came from.
    /// `build_cache_from_source` inserts org-sourced profiles directly (no
    /// type-registry seat on that path), so the guarantee has to hold at the
    /// cache funnel itself.
    #[test]
    fn org_sourced_profile_reaching_the_cache_is_widened() {
        let profile = EntityProfile {
            entity_name: EntityName::new("widget"),
            variants: vec![],
            computed_fields: vec![],
            virtual_child: Some(holon_api::entity_profile::VirtualChildConfig {
                defaults: HashMap::from([(
                    "content".to_string(),
                    holon_api::Value::String(String::new()),
                )]),
            }),
            declared_columns: ["collapsed", "source_language"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        let mut row: holon_api::entity::StorageEntity = HashMap::new();
        row.insert(
            "id".into(),
            holon_api::Value::String("block:widget".to_string()),
        );
        let source = LiveData::new(
            vec![row],
            |_| Ok("widget".to_string()),
            move |_| Ok(profile.clone()),
        );

        let cache = ProfileResolver::build_cache_from_source(
            &source,
            &holon_api::UiInfo::permissive(),
            &[],
        );
        let defaults = cache
            .get("widget")
            .expect("org-sourced profile is cached")
            .virtual_child
            .as_ref()
            .expect("virtual_child survives caching")
            .defaults
            .clone();
        for col in ["collapsed", "source_language"] {
            assert!(
                defaults.contains_key(col),
                "org-sourced creation slot omits declared column '{col}'"
            );
        }
    }

    #[test]
    fn heterogeneous_plain_block_row_produces_no_error_and_typed_nulls() {
        // The boot-flood row shape: a plain block missing task_state AND tags.
        // Type-aware binding must yield is_task=Null / is_page_row=Null WITHOUT
        // any Rhai "Variable not found", and the dependent embedded_page
        // condition must NOT type-error (the Seat-B cascade) — it just resolves
        // to a non-embedded variant.
        let profile = block_profile_with_schema();
        let engine = RhaiEngine::new();
        let mut row: HashMap<String, Value> = HashMap::new();
        row.insert("id".into(), Value::String("block:plain".into()));
        row.insert("parent_id".into(), Value::String("block:root".into()));
        row.insert("content".into(), Value::String("hello".into()));
        row.insert("content_type".into(), Value::String("text".into()));
        row.insert("source_language".into(), Value::Null);

        let (resolved, computed) = profile.resolve_with_computed(&row, &engine);
        // Unbound computed fields surface as Null (row shape preserved), not errors.
        assert_eq!(computed.get("is_task"), Some(&Value::Null));
        assert_eq!(computed.get("is_page_row"), Some(&Value::Null));
        // A variant still resolves (no panic, no cascade abort); embedded_page —
        // which ANDs the unbound is_page_row — is NOT selected.
        let name = resolved.expect("a variant must resolve").name.clone();
        assert_ne!(name, "embedded_page");
        assert_ne!(name, "embedded_page_expanded");
    }
}
