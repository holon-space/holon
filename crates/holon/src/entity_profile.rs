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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use holon_api::CompiledExpr;
use holon_api::predicate::Predicate;
use holon_api::render_types::{OperationDescriptor, RenderExpr, RenderVariant};
use holon_api::{EntityName, Value, row_id};
use rhai::{Engine as RhaiEngine, Scope};

use crate::storage::types::StorageEntity;
use crate::sync::LiveData;

/// Variables that are frontend-local (UI state), not data-dependent.
/// Conditions referencing only these variables are extracted as `Predicate`
/// for instant frontend-side switching without a backend round-trip.
const UI_STATE_VARIABLES: &[&str] = &[
    "is_focused",
    "is_expanded",
    "view_mode",
    // Container-query inputs: refined per subtree during render interpretation.
    "available_width_px",
    "available_height_px",
    "available_width_physical_px",
    "available_height_physical_px",
    // Global viewport fallback: emitted by UiState::context_for when no
    // refinement has reached this block.
    "viewport_width_px",
    "viewport_height_px",
    "viewport_width_physical_px",
    "viewport_height_physical_px",
    "scale_factor",
];

/// Map of entity name → live collection for Rhai lookup functions.
pub type LiveEntities = HashMap<EntityName, Arc<LiveData<StorageEntity>>>;

// ---------------------------------------------------------------------------
// Core types (Send + Sync safe — no Rhai AST stored)
// ---------------------------------------------------------------------------

/// Resolved profile for a single row: how to render it + what operations apply.
///
/// `operations` is injected by `ProfileResolver` at resolve time from the
/// entity's registered operations — NOT stored in profile YAML.
#[derive(Debug, Clone)]
pub struct RowProfile {
    pub name: String,
    pub render: RenderExpr,
    pub operations: Vec<OperationDescriptor>,
    /// All matching variant candidates (for multi-variant frontend selection).
    /// Empty when resolved via the legacy single-variant path.
    pub variants: Vec<RenderVariant>,
}

/// Stored profile spec — render expression only, no operations.
/// Operations are injected by `ProfileResolver` at resolve time.
#[derive(Debug, Clone)]
pub struct StoredProfile {
    pub name: String,
    pub render: RenderExpr,
}

/// A conditional override within an entity profile.
///
/// Stores condition as source string (Rhai ASTs are !Send).
/// The condition is split into a data part (Rhai, backend-evaluated)
/// and a UI part (`Predicate`, frontend-evaluated).
#[derive(Debug, Clone)]
pub struct StoredVariant {
    pub name: String,
    /// Merge/resolution priority. Higher priority variants are checked first.
    /// Seeded defaults use -1, omitted defaults to 0, users can set higher.
    pub priority: i32,
    /// Original full Rhai condition source (empty = always matches).
    pub condition_source: String,
    /// Data-only Rhai condition (None = always true on data side).
    pub data_condition: Option<String>,
    /// Frontend-evaluable UI condition extracted from the full condition.
    pub ui_condition: Predicate,
    pub profile: Arc<StoredProfile>,
}

/// Backward-compat alias.
pub type RowVariant = StoredVariant;

/// Virtual child configuration: default field values for the always-present
/// editable placeholder appended to collections. The driver creates a
/// synthetic DataRow from these defaults (plus a `virtual:` ID and parent_id),
/// then renders it through the normal entity profile via `render_entity()`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VirtualChildConfig {
    pub defaults: std::collections::HashMap<String, holon_api::Value>,
}

/// Complete profile for one entity type.
/// Computed field expressions are pre-compiled at parse time.
#[derive(Debug, Clone)]
pub struct EntityProfile {
    pub entity_name: EntityName,
    /// All variants (including the conditionless "default"). Sorted by priority
    /// descending at resolution time — highest priority checked first.
    pub variants: Vec<StoredVariant>,
    /// Pre-compiled computed fields in topological order.
    pub computed_fields: Vec<CompiledComputedField>,
    /// When set, collections displaying this entity type's children append a
    /// virtual editable placeholder at the end. Typing into it materializes
    /// a real entity.
    pub virtual_child: Option<VirtualChildConfig>,
}

use crate::type_registry::CompiledComputedField;

// ---------------------------------------------------------------------------
// YAML parsing
// ---------------------------------------------------------------------------

/// Parse a render expression from text.
///
/// Uses the Rhai-based render DSL parser. Accepts both Rhai syntax and JSON.
pub fn parse_render_text(text: &str) -> Result<RenderExpr> {
    crate::render_dsl::parse_render_dsl(text)
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
        })
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
fn split_condition(source: &str) -> (Option<String>, Predicate) {
    let conjuncts: Vec<&str> = source.split("&&").map(|s| s.trim()).collect();

    let mut data_parts = Vec::new();
    let mut ui_predicates = Vec::new();

    for conjunct in &conjuncts {
        let refs_only_ui = !conjunct.is_empty()
            && UI_STATE_VARIABLES
                .iter()
                .any(|var| crate::util::expr_references(conjunct, var))
            && !has_non_ui_references(conjunct);

        if refs_only_ui {
            ui_predicates.push(parse_conjunct_to_predicate(conjunct));
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

    (data_condition, ui_condition)
}

/// Check if a conjunct references any variables that are NOT UI state variables.
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
/// Handles patterns like:
/// - `is_focused` → `Var("is_focused")`
/// - `view_mode == "table"` → `Eq { field: "view_mode", value: "table" }`
/// - `!is_focused` → `Not(Var("is_focused"))`
fn parse_conjunct_to_predicate(conjunct: &str) -> Predicate {
    let s = conjunct.trim();

    // Equality: `field == "value"`
    if let Some(idx) = s.find("==") {
        let field = s[..idx].trim().to_string();
        let value_str = s[idx + 2..].trim();
        let value = parse_literal_value(value_str);
        return Predicate::Eq { field, value };
    }

    // Inequality: `field != "value"`
    if let Some(idx) = s.find("!=") {
        let field = s[..idx].trim().to_string();
        let value_str = s[idx + 2..].trim();
        let value = parse_literal_value(value_str);
        return Predicate::Ne { field, value };
    }

    // Numeric comparisons. Order matters: `<=` and `>=` must be checked
    // before `<` and `>` so the longer operator wins.
    if let Some(idx) = s.find("<=") {
        let field = s[..idx].trim().to_string();
        let value = parse_literal_value(s[idx + 2..].trim());
        return Predicate::Lte { field, value };
    }
    if let Some(idx) = s.find(">=") {
        let field = s[..idx].trim().to_string();
        let value = parse_literal_value(s[idx + 2..].trim());
        return Predicate::Gte { field, value };
    }
    if let Some(idx) = s.find('<') {
        let field = s[..idx].trim().to_string();
        let value = parse_literal_value(s[idx + 1..].trim());
        return Predicate::Lt { field, value };
    }
    if let Some(idx) = s.find('>') {
        let field = s[..idx].trim().to_string();
        let value = parse_literal_value(s[idx + 1..].trim());
        return Predicate::Gt { field, value };
    }

    // Negation: `!var`
    if let Some(rest) = s.strip_prefix('!') {
        return Predicate::Not(Box::new(Predicate::Var(rest.trim().to_string())));
    }

    // Simple variable truthiness
    Predicate::Var(s.to_string())
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
/// This function splits conditions into data/ui predicates and parses render expressions.
pub fn profile_variants_to_stored(
    profile_variants: &[holon_api::ProfileVariant],
) -> Result<Vec<StoredVariant>> {
    let mut variants = Vec::new();
    for pv in profile_variants {
        let (condition_src, data_condition, ui_condition) = if let Some(ref compiled) = pv.condition
        {
            let src = compiled.source.clone();
            let (dc, uc) = split_condition(&src);
            (src, dc, uc)
        } else {
            (String::new(), None, Predicate::Always)
        };

        let profile = Arc::new(StoredProfile {
            name: pv.name.clone(),
            render: parse_render_text(&pv.render)?,
        });

        variants.push(StoredVariant {
            name: pv.name.clone(),
            priority: pv.priority,
            condition_source: condition_src,
            data_condition,
            ui_condition,
            profile,
        });
    }
    variants.sort_by(|a, b| b.priority.cmp(&a.priority));
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

    Some(EntityProfile {
        entity_name: holon_api::EntityName::new(&type_def.name),
        variants,
        computed_fields,
        virtual_child: None,
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
            if *other != field.name.as_str() && crate::util::expr_references(&field.source, other) {
                field_deps.push(*other);
            }
        }
        deps.insert(field.name.as_str(), field_deps);
    }

    let order = crate::util::topo_sort_kahn(&names, &deps);

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
// Resolution (compiles Rhai on-demand)
// ---------------------------------------------------------------------------

impl EntityProfile {
    /// Resolve a single row to its RowProfile.
    pub fn resolve(
        &self,
        row: &HashMap<String, holon_api::Value>,
        engine: &RhaiEngine,
    ) -> Option<Arc<StoredProfile>> {
        self.resolve_with_computed(row, engine).0
    }

    /// Resolve profile AND return computed field values.
    /// Single Rhai evaluation pass — use this when you need computed values in row data.
    pub fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
        engine: &RhaiEngine,
    ) -> (
        Option<Arc<StoredProfile>>,
        HashMap<String, holon_api::Value>,
    ) {
        let mut scope = self.build_scope(row, engine);

        let profile = self.resolve_from_scope(engine, &mut scope);
        let computed = self.extract_computed_values(&scope);
        (profile, computed)
    }

    /// Resolve ALL matching candidates for a row (multi-variant mode).
    ///
    /// Evaluates each variant's `data_condition` via Rhai. Returns all variants
    /// whose data conditions match, each carrying its `ui_condition` predicate
    /// for frontend-side selection. The default profile is appended as last
    /// candidate with `Predicate::Always`.
    pub fn resolve_candidates(
        &self,
        row: &HashMap<String, holon_api::Value>,
        engine: &RhaiEngine,
    ) -> (
        Vec<(&StoredVariant, Arc<StoredProfile>)>,
        HashMap<String, holon_api::Value>,
    ) {
        let mut scope = self.build_scope(row, engine);

        let mut candidates = Vec::new();
        for variant in &self.variants {
            let data_matches = match &variant.data_condition {
                None => true, // No data condition = always matches on data side
                Some(dc) => eval_bool_source(engine, dc, &mut scope),
            };
            if data_matches {
                candidates.push((variant, variant.profile.clone()));
            }
        }

        let computed = self.extract_computed_values(&scope);
        (candidates, computed)
    }

    /// Resolve collection-level variants for this entity.
    ///
    /// Returns all collection variants (each carries a `ui_condition` for
    /// frontend-side view-mode switching). The collection default is appended
    /// with `Predicate::Always`.

    fn resolve_from_scope(
        &self,
        engine: &RhaiEngine,
        scope: &mut Scope<'_>,
    ) -> Option<Arc<StoredProfile>> {
        // Variants are sorted by priority desc.
        // First match wins — conditionless variants (empty condition_source) always match.
        for variant in &self.variants {
            if variant.condition_source.is_empty()
                || eval_bool_source(engine, &variant.condition_source, scope)
            {
                return Some(variant.profile.clone());
            }
        }
        None
    }

    fn extract_computed_values(&self, scope: &Scope<'_>) -> HashMap<String, holon_api::Value> {
        self.computed_fields
            .iter()
            .filter_map(|(name, _expr)| {
                scope
                    .get_value::<rhai::Dynamic>(name)
                    .map(|d| (name.clone(), dynamic_to_value(&d)))
            })
            .collect()
    }

    fn build_scope(
        &self,
        row: &HashMap<String, holon_api::Value>,
        engine: &RhaiEngine,
    ) -> Scope<'static> {
        let mut scope = Scope::new();

        for (key, value) in row {
            scope.push(key.clone(), value_to_dynamic(value));

            // Flatten `properties` object so inner fields (task_state, priority, etc.)
            // are available as top-level scope variables for profile conditions.
            if key == "properties" {
                if let holon_api::Value::Object(props) = value {
                    for (prop_key, prop_value) in props {
                        if !row.contains_key(prop_key) {
                            scope.push(prop_key.clone(), value_to_dynamic(prop_value));
                        }
                    }
                }
            }
        }

        // Evaluate computed fields in topo order via shared evaluator
        let mut computed_ctx = row.clone();
        crate::computed::resolve_computed_fields_with_scope(
            engine,
            &mut scope,
            &self.computed_fields,
            &mut computed_ctx,
        );

        scope
    }
}

fn dynamic_to_value(d: &rhai::Dynamic) -> holon_api::Value {
    if d.is_unit() {
        holon_api::Value::Null
    } else if let Some(s) = d.clone().try_cast::<String>() {
        holon_api::Value::String(s)
    } else if let Some(i) = d.clone().try_cast::<i64>() {
        holon_api::Value::Integer(i)
    } else if let Some(f) = d.clone().try_cast::<f64>() {
        holon_api::Value::Float(f)
    } else if let Some(b) = d.clone().try_cast::<bool>() {
        holon_api::Value::Boolean(b)
    } else {
        holon_api::Value::String(d.to_string())
    }
}

fn eval_bool_source(engine: &RhaiEngine, source: &str, scope: &mut Scope) -> bool {
    match engine.eval_with_scope::<bool>(scope, source) {
        Ok(val) => val,
        Err(e) => {
            let msg = format!("{e}");
            if msg.contains("Variable not found") || msg.contains("Output type incorrect") {
                tracing::trace!("[eval_bool_source] '{source}': {e}");
            } else {
                tracing::warn!("[eval_bool_source] '{source}' failed: {e}");
            }
            false
        }
    }
}

fn value_to_dynamic(value: &holon_api::Value) -> rhai::Dynamic {
    match value {
        holon_api::Value::String(s) => rhai::Dynamic::from(s.clone()),
        holon_api::Value::Integer(i) => rhai::Dynamic::from(*i),
        holon_api::Value::Float(f) => rhai::Dynamic::from(*f),
        holon_api::Value::Boolean(b) => rhai::Dynamic::from(*b),
        holon_api::Value::Null => rhai::Dynamic::UNIT,
        holon_api::Value::DateTime(s) => rhai::Dynamic::from(s.clone()),
        holon_api::Value::Json(s) => rhai::Dynamic::from(s.clone()),
        holon_api::Value::Array(arr) => {
            let items: Vec<rhai::Dynamic> = arr.iter().map(value_to_dynamic).collect();
            rhai::Dynamic::from(items)
        }
        holon_api::Value::Object(obj) => {
            let mut map = rhai::Map::new();
            for (k, v) in obj {
                map.insert(k.clone().into(), value_to_dynamic(v));
            }
            rhai::Dynamic::from(map)
        }
    }
}

// ---------------------------------------------------------------------------
// Entity lookup registration for Rhai
// ---------------------------------------------------------------------------

/// Register per-entity lookup functions on a Rhai engine.
///
/// For each entry in `live_entities`, registers a function named after the entity
/// (e.g. `document("doc:index.org")`) that returns the entity's properties as a Rhai map.
fn register_entity_lookups(engine: &mut RhaiEngine, live_entities: &LiveEntities) {
    for (entity_name, live_data) in live_entities {
        let data = Arc::clone(live_data);
        // Rhai identifiers use underscores; EntityName normalizes to hyphens for URI schemes.
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
        if k == "properties" {
            if let holon_api::Value::Object(props) = v {
                for (pk, pv) in props {
                    map.insert(pk.clone().into(), value_to_dynamic(pv));
                }
            } else if let holon_api::Value::String(json_str) = v {
                if let Ok(parsed) =
                    serde_json::from_str::<HashMap<String, holon_api::Value>>(json_str)
                {
                    for (pk, pv) in &parsed {
                        map.insert(pk.clone().into(), value_to_dynamic(pv));
                    }
                }
            }
        }
        map.insert(k.clone().into(), value_to_dynamic(v));
    }
    rhai::Dynamic::from(map)
}

// ---------------------------------------------------------------------------
// ProfileResolving trait + ProfileResolver
// ---------------------------------------------------------------------------

/// Trait for DI — allows testing with mock resolvers.
pub trait ProfileResolving: Send + Sync {
    fn resolve(&self, row: &HashMap<String, holon_api::Value>) -> Arc<RowProfile>;

    /// Resolve profile AND return computed field values in one pass.
    fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>);

    fn resolve_batch(&self, rows: &[HashMap<String, holon_api::Value>]) -> Vec<Arc<RowProfile>>;

    /// Resolve ALL matching variant candidates for a row (multi-variant mode).
    ///
    /// Returns a `RenderProfile` with `variants` populated — the frontend picks
    /// the active one based on local UI state.
    fn resolve_with_variants(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>) {
        // Default: fall back to single-variant resolution
        self.resolve_with_computed(row)
    }

    /// Get virtual child config for an entity type, if declared in its profile.
    fn virtual_child_config(&self, _entity_name: &str) -> Option<VirtualChildConfig> {
        None
    }

    /// Get collection-level variants (tree/table/board view modes).
    ///
    /// Collection profiles are entity-agnostic — any entity with the required
    /// columns (e.g. parent_id for trees) can use them.
    fn resolve_collection_variants(&self) -> Vec<RenderVariant> {
        Vec::new()
    }

    /// Get a watch receiver that fires when the underlying profile data changes.
    ///
    /// UiWatcher uses this to re-render when profile blocks are edited,
    /// without waiting for a structural CDC event.
    ///
    /// Default: returns a receiver that never fires (for mock resolvers / tests).
    fn subscribe_version(&self) -> tokio::sync::watch::Receiver<u64> {
        let (_tx, rx) = tokio::sync::watch::channel(0u64);
        rx
    }
}

struct ProfileCache {
    profiles: HashMap<EntityName, EntityProfile>,
}

/// Concrete profile resolver backed by LiveData (CDC-driven, live-updating).
///
/// Variants are filtered against `UiInfo` — if a variant references widgets
/// the frontend can't render, it's dropped.
///
/// Cache is rebuilt reactively in a background task when LiveData changes.
/// `resolve()` reads the cache via `watch::Receiver<Arc<ProfileCache>>` —
/// just an Arc clone, no RwLock contention on the hot path.
pub struct ProfileResolver {
    source: Arc<crate::sync::LiveData<EntityProfile>>,
    cache_rx: tokio::sync::watch::Receiver<Arc<ProfileCache>>,
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
        source: Arc<crate::sync::LiveData<EntityProfile>>,
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
    /// Type-defined profiles are seeded first; org-based profiles override them.
    pub fn with_type_profiles(
        source: Arc<crate::sync::LiveData<EntityProfile>>,
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
        let (cache_tx, cache_rx) = tokio::sync::watch::channel(initial_cache);

        let bg_source = Arc::clone(&source);
        let bg_type_profiles = Arc::clone(&type_profiles);
        let mut version_rx = source.subscribe_version();
        crate::util::spawn_actor(async move {
            version_rx.borrow_and_update();
            while version_rx.changed().await.is_ok() {
                let new_cache = Arc::new(Self::build_cache_from_source(
                    &bg_source,
                    &ui_info,
                    &bg_type_profiles,
                ));
                if cache_tx.send(new_cache).is_err() {
                    break;
                }
            }
        });

        let rhai_engine = Arc::new(Self::build_rhai_engine(&live_entities));

        ProfileResolver {
            source,
            cache_rx,
            entity_operations,
            rhai_engine: std::sync::RwLock::new(rhai_engine),
            live_entities: std::sync::RwLock::new(live_entities),
        }
    }

    /// Build a Rhai engine with entity lookup functions pre-registered.
    fn build_rhai_engine(live_entities: &LiveEntities) -> RhaiEngine {
        let mut engine = RhaiEngine::new();
        register_entity_lookups(&mut engine, live_entities);
        engine
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

    /// Look up operations for an entity name.
    fn operations_for(&self, entity_name: &str) -> Vec<OperationDescriptor> {
        self.entity_operations
            .get(&EntityName::new(entity_name))
            .cloned()
            .unwrap_or_default()
    }

    /// Combine a StoredProfile with entity operations to produce a RowProfile.
    ///
    /// Operations are looked up by the ID scheme (e.g. "block" from "block:xxx"),
    /// not by `entity_name` which may be a view/matview alias like "focus_roots".
    fn materialize(
        &self,
        stored: &StoredProfile,
        row: &HashMap<String, holon_api::Value>,
    ) -> Arc<RowProfile> {
        let ops = row_id(row)
            .map(|id| self.operations_for(&id.scheme()))
            .unwrap_or_default();
        Arc::new(RowProfile {
            name: stored.name.clone(),
            render: stored.render.clone(),
            operations: ops,
            variants: Vec::new(),
        })
    }

    fn build_cache_from_source(
        source: &crate::sync::LiveData<EntityProfile>,
        ui_info: &holon_api::UiInfo,
        type_profiles: &[EntityProfile],
    ) -> ProfileCache {
        let mut profiles = HashMap::new();

        // Seed with type-defined profiles (fallback layer)
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
        ProfileCache { profiles }
    }

    /// Merge a new profile into an existing one with the same entity name.
    /// Variant lists are combined (not replaced) and re-sorted by priority.
    /// Computed fields from the incoming profile are added (incoming wins on name conflict).
    fn merge_profile(existing: &mut EntityProfile, incoming: &EntityProfile) {
        tracing::info!(
            "[ProfileResolver::merge_profile] entity='{}', existing_variants={}, incoming_variants={}",
            existing.entity_name,
            existing.variants.len(),
            incoming.variants.len(),
        );
        // Combine variant lists — priority handles resolution order
        existing.variants.extend(incoming.variants.iter().cloned());
        existing
            .variants
            .sort_by(|a, b| b.priority.cmp(&a.priority));

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
        }
    }
}

impl ProfileResolving for ProfileResolver {
    fn resolve(&self, row: &HashMap<String, holon_api::Value>) -> Arc<RowProfile> {
        self.resolve_with_computed(row).0
    }

    fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>) {
        let cache = self.cache_rx.borrow().clone();

        let entity_uri = row_id(row).expect("No id found");
        let entity_name_str = entity_uri.scheme();
        let entity_name = EntityName::new(entity_name_str);

        let entity_profile = cache.profiles.get(&entity_name).unwrap_or_else(|| {
            panic!(
                "No profile registered for entity '{entity_name_str}' (row id='{entity_uri}'). \
                 Known profiles: {:?}",
                cache
                    .profiles
                    .keys()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
            )
        });

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

    fn resolve_batch(&self, rows: &[HashMap<String, holon_api::Value>]) -> Vec<Arc<RowProfile>> {
        rows.iter()
            .map(|row| ProfileResolving::resolve(self, row))
            .collect()
    }

    fn resolve_with_variants(
        &self,
        row: &HashMap<String, holon_api::Value>,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>) {
        let cache = self.cache_rx.borrow().clone();
        let entity_uri = row_id(row).expect("No id found");
        let entity_name_str = entity_uri.scheme();
        let entity_name = EntityName::new(entity_name_str);

        let entity_profile = cache.profiles.get(&entity_name).unwrap_or_else(|| {
            panic!(
                "No profile registered for entity '{entity_name_str}' (row id='{entity_uri}'). \
                 Known profiles: {:?}",
                cache
                    .profiles
                    .keys()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
            )
        });

        let engine = self.rhai_engine.read().unwrap().clone();
        let (candidates, computed) = entity_profile.resolve_candidates(row, &engine);

        let ops = row_id(row)
            .map(|id| self.operations_for(&id.scheme()))
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
        let first_profile = Arc::new(RowProfile {
            name: stored.name.clone(),
            render: stored.render.clone(),
            operations: ops.clone(),
            variants: render_variants,
        });

        (first_profile, computed)
    }

    fn resolve_collection_variants(&self) -> Vec<RenderVariant> {
        let cache = self.cache_rx.borrow().clone();
        let collection_name = EntityName::new("collection");
        let Some(collection_profile) = cache.profiles.get(&collection_name) else {
            return Vec::new();
        };

        collection_profile
            .variants
            .iter()
            .map(|v| RenderVariant {
                name: v.name.clone(),
                render: v.profile.render.clone(),
                operations: Vec::new(),
                condition: v.ui_condition.clone(),
            })
            .collect()
    }

    fn virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig> {
        let cache = self.cache_rx.borrow().clone();
        cache
            .profiles
            .get(entity_name)
            .and_then(|p| p.virtual_child.clone())
    }

    fn subscribe_version(&self) -> tokio::sync::watch::Receiver<u64> {
        self.source.subscribe_version()
    }
}

/// Check if a block is an entity profile block (source_language = holon_entity_profile_yaml).
pub fn is_profile_block_by_source_language(source_language: Option<&str>) -> bool {
    source_language == Some("holon_entity_profile_yaml")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn init_render_dsl() {
        crate::render_dsl::register_widget_names(&[
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
        let (data, ui) = split_condition("available_width_px < 600");
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
        let (data, ui) = split_condition("available_height_px >= 800");
        assert!(data.is_none());
        assert_eq!(
            ui,
            Predicate::Gte {
                field: "available_height_px".into(),
                value: Value::Integer(800),
            }
        );

        let (data, ui) = split_condition("scale_factor <= 1.5");
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
        let (data, ui) = split_condition("task_state == \"done\" && available_width_px < 480");
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
        let (data, ui) = split_condition("is_focused && available_width_px < 600");
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
        let (data, ui) = split_condition("is_expanded");
        assert!(data.is_none());
        assert_eq!(ui, Predicate::Var("is_expanded".into()));
    }

    #[test]
    fn test_expr_references() {
        use crate::util::expr_references;
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

        // task_state nested inside properties (as it comes from `from children` queries)
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
        let (data, ui) = split_condition("is_focused");
        assert!(data.is_none());
        assert_eq!(ui, Predicate::Var("is_focused".into()));
    }

    #[test]
    fn test_split_condition_pure_data() {
        let (data, ui) = split_condition("is_task");
        assert_eq!(data.as_deref(), Some("is_task"));
        assert_eq!(ui, Predicate::Always);
    }

    #[test]
    fn test_split_condition_mixed() {
        let (data, ui) = split_condition("is_source && is_focused");
        assert_eq!(data.as_deref(), Some("is_source"));
        assert_eq!(ui, Predicate::Var("is_focused".into()));
    }

    #[test]
    fn test_split_condition_ui_eq() {
        let (data, ui) = split_condition(r#"is_source && view_mode == "table""#);
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
        let (data, ui) = split_condition("task_state != () && priority > 0");
        assert_eq!(data.as_deref(), Some("task_state != () && priority > 0"));
        assert_eq!(ui, Predicate::Always);
    }
}
