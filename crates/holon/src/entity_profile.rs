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
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use holon_api::EntityName;
use holon_api::render_types::{OperationDescriptor, RenderExpr};
use holon_engine::guard::CompiledExpr;
use rhai::{Engine as RhaiEngine, Scope};

use crate::storage::types::StorageEntity;
use crate::sync::LiveData;

/// Map of entity name → live collection for Rhai lookup functions.
pub type LiveEntities = HashMap<EntityName, Arc<LiveData<StorageEntity>>>;

// ---------------------------------------------------------------------------
// Core types (Send + Sync safe — no Rhai AST stored)
// ---------------------------------------------------------------------------

/// Shared between all rows matching the same entity + variant.
/// Serialized once as JSON to Flutter per unique instance.
#[derive(Debug, Clone)]
pub struct RowProfile {
    pub name: String,
    pub render: RenderExpr,
    pub operations: Vec<OperationDescriptor>,
}

/// A conditional override within an entity profile.
/// Stores condition as source string (Rhai ASTs are !Send).
#[derive(Debug, Clone)]
pub struct RowVariant {
    pub name: String,
    pub condition_source: String,
    pub profile: Arc<RowProfile>,
    pub specificity: usize,
}

/// Complete profile for one entity type.
/// All Rhai expressions stored as source strings for thread safety.
#[derive(Debug, Clone)]
pub struct EntityProfile {
    pub entity_name: EntityName,
    pub default: Arc<RowProfile>,
    pub variants: Vec<RowVariant>,
    pub computed_fields: Vec<ComputedField>,
}

// SAFETY: EntityProfile only stores Strings, Arcs, and Vecs — all Send+Sync.
unsafe impl Send for EntityProfile {}
unsafe impl Sync for EntityProfile {}

/// A field computed from row data via Rhai, available to conditions and render.
#[derive(Debug, Clone)]
pub struct ComputedField {
    pub name: String,
    pub source: String,
}

/// Context for profile resolution (view preferences, UI state).
#[derive(Debug, Clone, Default)]
pub struct ProfileContext {
    pub preferred_variant: Option<String>,
    pub view_width: Option<f64>,
}

// ---------------------------------------------------------------------------
// YAML parsing
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct RawEntityProfile {
    entity_name: String,
    #[serde(default)]
    computed: BTreeMap<String, String>,
    default: RawProfileSpec,
    #[serde(default)]
    variants: Vec<RawVariant>,
}

#[derive(Debug, serde::Deserialize)]
struct RawProfileSpec {
    render: String,
    #[serde(default)]
    operations: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RawVariant {
    name: String,
    condition: String,
    render: String,
    #[serde(default)]
    operations: Vec<String>,
}

/// Parse a render expression from text.
///
/// Parse a render DSL expression into a RenderExpr.
///
/// Uses the Rhai-based render DSL parser. Accepts both Rhai syntax and JSON.
pub fn parse_render_text(text: &str) -> Result<RenderExpr> {
    crate::render_dsl::parse_render_dsl(text)
}

/// Parse a YAML entity profile block into an EntityProfile.
///
/// The `entity_name` is read from the YAML itself (top-level `entity_name` field).
/// Validates all Rhai expressions compile (fails fast on syntax errors),
/// but stores only source strings in the returned EntityProfile.
pub fn parse_entity_profile(
    yaml_content: &str,
    operation_lookup: &dyn Fn(&str, &str) -> Vec<OperationDescriptor>,
) -> Result<EntityProfile> {
    let engine = RhaiEngine::new();
    let raw: RawEntityProfile =
        serde_yaml::from_str(yaml_content).context("Invalid YAML in entity profile")?;
    let entity_name = &raw.entity_name;

    // Parse + validate computed fields, then topo-sort
    let computed_fields = parse_and_sort_computed_fields(&engine, &raw.computed)?;

    // Parse default profile
    let default_render = parse_render_text(&raw.default.render)?;
    let default_ops = if raw.default.operations.is_empty() {
        operation_lookup(entity_name, "")
    } else {
        resolve_operation_names(entity_name, &raw.default.operations, operation_lookup)
    };
    let default = Arc::new(RowProfile {
        name: "default".to_string(),
        render: default_render,
        operations: default_ops,
    });

    // Parse variants
    let mut variants = Vec::new();
    for raw_variant in &raw.variants {
        let condition_src = strip_rhai_prefix(&raw_variant.condition);

        // Validate that the condition compiles
        CompiledExpr::compile(&engine, &condition_src).map_err(|e| {
            anyhow::anyhow!(
                "Failed to compile condition for variant '{}': {}",
                raw_variant.name,
                e
            )
        })?;

        let render = parse_render_text(&raw_variant.render)?;
        let ops = if raw_variant.operations.is_empty() {
            operation_lookup(entity_name, "")
        } else {
            resolve_operation_names(entity_name, &raw_variant.operations, operation_lookup)
        };

        let specificity = condition_src.len();
        let profile = Arc::new(RowProfile {
            name: raw_variant.name.clone(),
            render,
            operations: ops,
        });

        variants.push(RowVariant {
            name: raw_variant.name.clone(),
            condition_source: condition_src,
            profile,
            specificity,
        });
    }

    // Sort by specificity descending (most specific first)
    variants.sort_by(|a, b| b.specificity.cmp(&a.specificity));

    Ok(EntityProfile {
        entity_name: EntityName::new(entity_name),
        default,
        variants,
        computed_fields,
    })
}

/// Strip the `= ` prefix convention from a Rhai expression string.
fn strip_rhai_prefix(s: &str) -> String {
    s.strip_prefix('=')
        .map(|s| s.trim())
        .unwrap_or(s.trim())
        .to_string()
}

fn resolve_operation_names(
    entity_name: &str,
    names: &[String],
    lookup: &dyn Fn(&str, &str) -> Vec<OperationDescriptor>,
) -> Vec<OperationDescriptor> {
    let all_ops = lookup(entity_name, "");
    names
        .iter()
        .filter_map(|name| all_ops.iter().find(|o| o.name == *name).cloned())
        .collect()
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
) -> Result<Vec<ComputedField>> {
    let mut fields = Vec::new();
    for (name, value) in raw {
        let source = strip_rhai_prefix(value);
        // Validate compilation
        CompiledExpr::compile(engine, &source)
            .map_err(|e| anyhow::anyhow!("Failed to compile computed field '{name}': {e}"))?;
        fields.push(RawComputedField {
            name: name.clone(),
            source,
        });
    }
    Ok(topo_sort_computed_fields(fields))
}

fn topo_sort_computed_fields(fields: Vec<RawComputedField>) -> Vec<ComputedField> {
    if fields.is_empty() {
        return vec![];
    }

    let names: HashSet<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();

    for field in &fields {
        let mut field_deps = Vec::new();
        for other in &names {
            if *other != field.name.as_str() && crate::util::expr_references(&field.source, other) {
                field_deps.push(*other);
            }
        }
        deps.insert(field.name.as_str(), field_deps);
    }

    let order = crate::util::topo_sort_kahn(&names, &deps);

    let mut field_map: HashMap<String, RawComputedField> =
        fields.into_iter().map(|f| (f.name.clone(), f)).collect();

    order
        .into_iter()
        .map(|name| {
            let raw = field_map.remove(&name).unwrap();
            ComputedField {
                name: raw.name,
                source: raw.source,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Resolution (compiles Rhai on-demand)
// ---------------------------------------------------------------------------

impl EntityProfile {
    /// Resolve a single row to its RowProfile.
    /// Creates a Rhai engine per call (fast, avoids Send issues).
    pub fn resolve(
        &self,
        row: &HashMap<String, holon_api::Value>,
        context: &ProfileContext,
        live_entities: &LiveEntities,
    ) -> Arc<RowProfile> {
        self.resolve_with_computed(row, context, live_entities).0
    }

    /// Resolve profile AND return computed field values.
    /// Single Rhai evaluation pass — use this when you need computed values in row data.
    pub fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
        context: &ProfileContext,
        live_entities: &LiveEntities,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>) {
        let mut engine = RhaiEngine::new();
        register_entity_lookups(&mut engine, live_entities);
        let mut scope = self.build_scope(row, &engine);

        let profile = self.resolve_from_scope(&engine, &mut scope, context);
        let computed = self.extract_computed_values(&scope);
        (profile, computed)
    }

    fn resolve_from_scope(
        &self,
        engine: &RhaiEngine,
        scope: &mut Scope<'_>,
        context: &ProfileContext,
    ) -> Arc<RowProfile> {
        if let Some(ref preferred) = context.preferred_variant {
            for variant in &self.variants {
                if variant.name == *preferred {
                    if eval_bool_source(engine, &variant.condition_source, scope) {
                        return variant.profile.clone();
                    }
                }
            }
        }

        for variant in &self.variants {
            if eval_bool_source(engine, &variant.condition_source, scope) {
                return variant.profile.clone();
            }
        }

        self.default.clone()
    }

    fn extract_computed_values(&self, scope: &Scope<'_>) -> HashMap<String, holon_api::Value> {
        self.computed_fields
            .iter()
            .filter_map(|field| {
                scope
                    .get_value::<rhai::Dynamic>(&field.name)
                    .map(|d| (field.name.clone(), dynamic_to_value(&d)))
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

        // Evaluate computed fields in topo order
        for field in &self.computed_fields {
            let result = engine
                .eval_with_scope::<rhai::Dynamic>(&mut scope, &field.source)
                .unwrap_or(rhai::Dynamic::UNIT);
            scope.push(field.name.clone(), result);
        }

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
    engine
        .eval_with_scope::<bool>(scope, source)
        .unwrap_or(false)
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
        let name = entity_name.as_str().to_string();
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

/// A row with its resolved profile attached.
#[derive(Debug, Clone)]
pub struct ResolvedRow {
    pub data: HashMap<String, holon_api::Value>,
    pub profile: Arc<RowProfile>,
}

/// Trait for DI — allows testing with mock resolvers.
pub trait ProfileResolving: Send + Sync {
    fn resolve(
        &self,
        row: &HashMap<String, holon_api::Value>,
        context: &ProfileContext,
    ) -> Arc<RowProfile>;

    /// Resolve profile AND return computed field values in one pass.
    fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
        context: &ProfileContext,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>);

    fn resolve_batch(
        &self,
        rows: &[HashMap<String, holon_api::Value>],
        context: &ProfileContext,
    ) -> Vec<Arc<RowProfile>>;
}

struct ProfileCache {
    source_version: u64,
    profiles: HashMap<EntityName, EntityProfile>,
}

/// Concrete profile resolver backed by LiveData (CDC-driven, live-updating).
///
/// Variants are filtered against `UiInfo` — if a variant references widgets
/// the frontend can't render, it's dropped. Cache is rebuilt lazily when
/// LiveData version changes.
pub struct ProfileResolver {
    source: Arc<crate::sync::LiveData<EntityProfile>>,
    cache: RwLock<ProfileCache>,
    fallback: Arc<RowProfile>,
    ui_info: holon_api::UiInfo,
    live_entities: LiveEntities,
}

impl ProfileResolver {
    pub fn new(
        source: Arc<crate::sync::LiveData<EntityProfile>>,
        ui_info: holon_api::UiInfo,
        live_entities: LiveEntities,
    ) -> Self {
        let fallback = Arc::new(RowProfile {
            name: "fallback".to_string(),
            render: RenderExpr::FunctionCall {
                name: "row".to_string(),
                args: vec![holon_api::render_types::Arg {
                    name: None,
                    value: RenderExpr::ColumnRef {
                        name: "content".to_string(),
                    },
                }],
                operations: vec![],
            },
            operations: vec![],
        });

        let initial_version = source.version();
        let initial_cache = Self::build_cache_from_source(&source, &ui_info, initial_version);

        ProfileResolver {
            source,
            cache: RwLock::new(initial_cache),
            fallback,
            ui_info,
            live_entities,
        }
    }

    fn build_cache_from_source(
        source: &crate::sync::LiveData<EntityProfile>,
        ui_info: &holon_api::UiInfo,
        version: u64,
    ) -> ProfileCache {
        let items = source.read();
        let mut profiles = HashMap::new();
        for profile in items.values() {
            let filtered = Self::filter_profile(profile, ui_info);
            profiles.insert(filtered.entity_name.clone(), filtered);
        }
        ProfileCache {
            source_version: version,
            profiles,
        }
    }

    fn filter_profile(profile: &EntityProfile, ui_info: &holon_api::UiInfo) -> EntityProfile {
        if ui_info.is_permissive() {
            return profile.clone();
        }

        let filtered_variants: Vec<RowVariant> = profile
            .variants
            .iter()
            .filter(|v| {
                let names = holon_api::extract_widget_names(&v.profile.render);
                ui_info.supports_all(&names)
            })
            .cloned()
            .collect();

        let default = {
            let names = holon_api::extract_widget_names(&profile.default.render);
            if ui_info.supports_all(&names) {
                profile.default.clone()
            } else {
                // Replace with a safe fallback render
                Arc::new(RowProfile {
                    name: "default".to_string(),
                    render: RenderExpr::FunctionCall {
                        name: "row".to_string(),
                        args: vec![holon_api::render_types::Arg {
                            name: None,
                            value: RenderExpr::ColumnRef {
                                name: "content".to_string(),
                            },
                        }],
                        operations: vec![],
                    },
                    operations: profile.default.operations.clone(),
                })
            }
        };

        EntityProfile {
            entity_name: profile.entity_name.clone(),
            default,
            variants: filtered_variants,
            computed_fields: profile.computed_fields.clone(),
        }
    }

    fn ensure_cache_fresh(&self) {
        let current_version = self.source.version();
        {
            let cache = self.cache.read().unwrap();
            if cache.source_version == current_version {
                return;
            }
        }
        let new_cache = Self::build_cache_from_source(&self.source, &self.ui_info, current_version);
        let mut cache = self.cache.write().unwrap();
        // Double-check: another thread may have updated while we built
        if cache.source_version < current_version {
            *cache = new_cache;
        }
    }
}

impl ProfileResolving for ProfileResolver {
    fn resolve(
        &self,
        row: &HashMap<String, holon_api::Value>,
        context: &ProfileContext,
    ) -> Arc<RowProfile> {
        self.resolve_with_computed(row, context).0
    }

    fn resolve_with_computed(
        &self,
        row: &HashMap<String, holon_api::Value>,
        context: &ProfileContext,
    ) -> (Arc<RowProfile>, HashMap<String, holon_api::Value>) {
        self.ensure_cache_fresh();

        let entity_name = match row.get("entity_name") {
            Some(holon_api::Value::String(s)) => s.as_str(),
            _ => return (self.fallback.clone(), HashMap::new()),
        };

        let cache = self.cache.read().unwrap();
        if let Some(profile) = cache.profiles.get(entity_name) {
            return profile.resolve_with_computed(row, context, &self.live_entities);
        }

        // entity_name may be a matview/view name (e.g. "focus_roots") rather than the
        // actual entity type. Fall back to inferring entity type from the ID scheme.
        if let Some(holon_api::Value::String(id)) = row.get("id") {
            let scheme_entity = id.split_once(':').map(|(scheme, _)| scheme);
            if let Some(profile) = scheme_entity.and_then(|s| cache.profiles.get(s)) {
                return profile.resolve_with_computed(row, context, &self.live_entities);
            }
        }

        (self.fallback.clone(), HashMap::new())
    }

    fn resolve_batch(
        &self,
        rows: &[HashMap<String, holon_api::Value>],
        context: &ProfileContext,
    ) -> Vec<Arc<RowProfile>> {
        self.ensure_cache_fresh();
        rows.iter()
            .map(|row| ProfileResolving::resolve(self, row, context))
            .collect()
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

    #[test]
    fn test_parse_render_text_simple() {
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
        let fields = vec![
            RawComputedField {
                name: "b".to_string(),
                source: "a + 1".to_string(),
            },
            RawComputedField {
                name: "a".to_string(),
                source: "42".to_string(),
            },
        ];
        let sorted = topo_sort_computed_fields(fields);
        assert_eq!(sorted[0].name, "a");
        assert_eq!(sorted[1].name, "b");
    }

    #[test]
    fn test_parse_entity_profile_basic() {
        let yaml = r#"
entity_name: block

computed:
  is_task: "= task_state != ()"

default:
  render: 'row(col("content"))'

variants:
  - name: task
    condition: "= is_task"
    render: 'row(col("content"))'
    operations: []
"#;
        let no_ops = |_entity: &str, _filter: &str| -> Vec<OperationDescriptor> { vec![] };
        let profile = parse_entity_profile(yaml, &no_ops).unwrap();
        assert_eq!(profile.entity_name, "block");
        assert_eq!(profile.computed_fields.len(), 1);
        assert_eq!(profile.computed_fields[0].name, "is_task");
        assert_eq!(profile.variants.len(), 1);
        assert_eq!(profile.variants[0].name, "task");
    }

    fn make_test_profile(yaml: &str) -> EntityProfile {
        let no_ops = |_: &str, _: &str| -> Vec<OperationDescriptor> { vec![] };
        parse_entity_profile(yaml, &no_ops).unwrap()
    }

    #[test]
    fn test_resolve_default() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
default:
  render: 'row(col("content"))'
variants:
  - name: task
    condition: "= task_state != ()"
    render: 'row(col("content"))'
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "content".to_string(),
            holon_api::Value::String("hello".to_string()),
        );
        let resolved = profile.resolve(&row, &ProfileContext::default(), &LiveEntities::new());
        assert_eq!(resolved.name, "default");
    }

    #[test]
    fn test_resolve_variant() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
default:
  render: 'row(col("content"))'
variants:
  - name: task
    condition: "= task_state != ()"
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
        let resolved = profile.resolve(&row, &ProfileContext::default(), &LiveEntities::new());
        assert_eq!(resolved.name, "task");
    }

    #[test]
    fn test_resolve_variant_from_nested_properties() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
default:
  render: 'row(col("content"))'
variants:
  - name: task
    condition: "= task_state != ()"
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
        let resolved = profile.resolve(&row, &ProfileContext::default(), &LiveEntities::new());
        assert_eq!(resolved.name, "task");
    }

    #[test]
    fn test_resolve_preferred_variant() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed: {}
default:
  render: 'row(col("content"))'
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
        let ctx = ProfileContext {
            preferred_variant: Some("detailed".to_string()),
            ..Default::default()
        };
        let resolved = profile.resolve(&row, &ctx, &LiveEntities::new());
        assert_eq!(resolved.name, "detailed");
    }

    #[test]
    fn test_resolve_with_computed_fields() {
        let profile = make_test_profile(
            r#"
entity_name: block
computed:
  is_task: "= task_state != ()"
default:
  render: 'row(col("content"))'
variants:
  - name: task
    condition: "= is_task"
    render: 'row(col("content"))'
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "task_state".to_string(),
            holon_api::Value::String("TODO".to_string()),
        );
        let resolved = profile.resolve(&row, &ProfileContext::default(), &LiveEntities::new());
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
default:
  render: 'row(col("content"))'
variants: []
"#,
        );

        let mut row = HashMap::new();
        row.insert(
            "content".to_string(),
            holon_api::Value::String("world".to_string()),
        );
        let (profile_result, computed) =
            profile.resolve_with_computed(&row, &ProfileContext::default(), &LiveEntities::new());
        assert_eq!(profile_result.name, "default");
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
default:
  render: 'editable_text(col("content"))'
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
}
