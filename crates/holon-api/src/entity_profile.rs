//! EntityProfile data model + the `ProfileResolving` capability trait.
//!
//! Each entity (e.g., "block") can have a profile that defines:
//! - Computed fields (Rhai expressions evaluated from row data)
//! - A default render expression + operations
//! - Conditional variants that override rendering based on row data
//!
//! The runtime data model and the per-row resolution machinery live here
//! (storage de-leak Stage 10); profile *sources* — YAML parsing of org
//! profile blocks and the LiveData-backed `ProfileResolver` — stay in
//! `holon::entity_profile`.
//!
//! NOTE: Rhai ASTs are !Send+!Sync, so EntityProfile stores source strings
//! and compiles on-demand during resolution. Compilation is fast for small
//! expressions (<1µs each).

use std::collections::HashMap;
use std::sync::Arc;

use rhai::Engine as RhaiEngine;
use rhai::Scope;

use crate::CompiledExpr;
use crate::EntityName;
use crate::Value;
use crate::predicate::Predicate;
use crate::render_types::OperationDescriptor;
use crate::render_types::RenderExpr;
use crate::render_types::RenderProfile;
use crate::render_types::RenderVariant;

/// A computed field: name + pre-compiled Rhai expression.
pub type CompiledComputedField = (String, CompiledExpr);

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
    pub defaults: std::collections::HashMap<String, Value>,
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

// ---------------------------------------------------------------------------
// Resolution (compiles Rhai on-demand)
// ---------------------------------------------------------------------------

impl EntityProfile {
    /// Resolve a single row to its RenderProfile.
    pub fn resolve(
        &self,
        row: &HashMap<String, Value>,
        engine: &RhaiEngine,
    ) -> Option<Arc<StoredProfile>> {
        self.resolve_with_computed(row, engine).0
    }

    /// Resolve profile AND return computed field values.
    /// Single Rhai evaluation pass — use this when you need computed values in
    /// row data.
    pub fn resolve_with_computed(
        &self,
        row: &HashMap<String, Value>,
        engine: &RhaiEngine,
    ) -> (Option<Arc<StoredProfile>>, HashMap<String, Value>) {
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
    #[allow(clippy::type_complexity)] // returns (matched variants, evaluated field values) tuple
    pub fn resolve_candidates(
        &self,
        row: &HashMap<String, Value>,
        engine: &RhaiEngine,
    ) -> (
        Vec<(&StoredVariant, Arc<StoredProfile>)>,
        HashMap<String, Value>,
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
        // First match wins — conditionless variants (empty condition_source) always
        // match.
        for variant in &self.variants {
            if variant.condition_source.is_empty()
                || eval_bool_source(engine, &variant.condition_source, scope)
            {
                return Some(variant.profile.clone());
            }
        }
        None
    }

    fn extract_computed_values(&self, scope: &Scope<'_>) -> HashMap<String, Value> {
        self.computed_fields
            .iter()
            .filter_map(|(name, _expr)| {
                scope
                    .get_value::<rhai::Dynamic>(name)
                    .map(|d| (name.clone(), dynamic_to_value(&d)))
            })
            .collect()
    }

    fn build_scope(&self, row: &HashMap<String, Value>, engine: &RhaiEngine) -> Scope<'static> {
        let mut scope = Scope::new();

        for (key, value) in row {
            scope.push(key.clone(), value_to_dynamic(value));

            // Flatten `properties` object so inner fields (task_state, priority, etc.)
            // are available as top-level scope variables for profile conditions.
            // (Nested `if let`: holon-api is edition 2021, no let-chains.)
            if key == "properties" {
                if let Value::Object(props) = value {
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

/// Convert a Rhai `Dynamic` back into a holon `Value`.
///
/// `pub` because profile-source machinery in `holon::entity_profile`
/// (entity lookup registration) shares it.
pub fn dynamic_to_value(d: &rhai::Dynamic) -> Value {
    if d.is_unit() {
        Value::Null
    } else if let Some(s) = d.clone().try_cast::<String>() {
        Value::String(s)
    } else if let Some(i) = d.clone().try_cast::<i64>() {
        Value::Integer(i)
    } else if let Some(f) = d.clone().try_cast::<f64>() {
        Value::Float(f)
    } else if let Some(b) = d.clone().try_cast::<bool>() {
        Value::Boolean(b)
    } else {
        Value::String(d.to_string())
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

/// Convert a holon `Value` into a Rhai `Dynamic`.
///
/// `pub` for the same reason as [`dynamic_to_value`].
pub fn value_to_dynamic(value: &Value) -> rhai::Dynamic {
    match value {
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Integer(i) => rhai::Dynamic::from(*i),
        Value::Float(f) => rhai::Dynamic::from(*f),
        Value::Boolean(b) => rhai::Dynamic::from(*b),
        Value::Null => rhai::Dynamic::UNIT,
        Value::DateTime(s) => rhai::Dynamic::from(s.clone()),
        Value::Json(s) => rhai::Dynamic::from(s.clone()),
        Value::Array(arr) => {
            let items: Vec<rhai::Dynamic> = arr.iter().map(value_to_dynamic).collect();
            rhai::Dynamic::from(items)
        }
        Value::Object(obj) => {
            let mut map = rhai::Map::new();
            for (k, v) in obj {
                map.insert(k.clone().into(), value_to_dynamic(v));
            }
            rhai::Dynamic::from(map)
        }
    }
}

// ---------------------------------------------------------------------------
// ProfileResolving capability trait + ProfileCache
// ---------------------------------------------------------------------------

/// Trait for DI — allows testing with mock resolvers.
pub trait ProfileResolving: Send + Sync {
    fn resolve(&self, row: &HashMap<String, Value>) -> Arc<RenderProfile>;

    /// Resolve profile AND return computed field values in one pass.
    fn resolve_with_computed(
        &self,
        row: &HashMap<String, Value>,
    ) -> (Arc<RenderProfile>, HashMap<String, Value>);

    fn resolve_batch(&self, rows: &[HashMap<String, Value>]) -> Vec<Arc<RenderProfile>>;

    /// Resolve ALL matching variant candidates for a row (multi-variant mode).
    ///
    /// Returns a `RenderProfile` with `variants` populated — the frontend picks
    /// the active one based on local UI state.
    fn resolve_with_variants(
        &self,
        row: &HashMap<String, Value>,
    ) -> (Arc<RenderProfile>, HashMap<String, Value>) {
        // Default: fall back to single-variant resolution
        self.resolve_with_computed(row)
    }

    /// Resolve a row that the caller DECLARES must be entity-shaped.
    ///
    /// This is the CONTRACT seam (Martin ruling 2026-07-11). Most render paths
    /// accept either row shape and call [`Self::resolve_with_computed`], where
    /// a value row (no entity `id`) is a legitimate display case rendered
    /// plainly. But an entity TEMPLATE / entity-id-dependent widget (e.g.
    /// click-to-open the entity) genuinely REQUIRES an entity row: handing
    /// it a value row is a contract violation, not a display case. Such
    /// callers route through this method, which returns a loud `Err`
    /// instead of silently rendering a value row — the fail-loud path for a
    /// declared expectation.
    fn resolve_entity_required(
        &self,
        row: &HashMap<String, Value>,
    ) -> anyhow::Result<(Arc<RenderProfile>, HashMap<String, Value>)> {
        if crate::RowIdentity::of_row(row).is_value() {
            anyhow::bail!(
                "widget declared it requires an ENTITY row but received a VALUE row (no \
                 entity-shaped `id`): {row:?}. Entity templates / entity-id click handling cannot \
                 resolve a synthetic value row — project a real `... AS id` or render this query \
                 through a value-row-tolerant widget"
            );
        }
        Ok(self.resolve_with_computed(row))
    }

    /// Get virtual child config for an entity type, if declared in its profile.
    // ALLOW(unused_param): trait shape; default impl ignores name
    fn virtual_child_config(&self, _entity_name: &str) -> Option<VirtualChildConfig> {
        None
    }

    /// Entity-level operations (keyed by id scheme, e.g. `"block"`) — the same
    /// set `materialize` attaches to a row of that entity.
    // ALLOW(unused_param): trait shape; default impl ignores name
    fn operations_for(&self, _entity_name: &str) -> Vec<OperationDescriptor> {
        Vec::new()
    }

    /// Get collection-level variants (tree/table/board view modes).
    ///
    /// Collection profiles are entity-agnostic — any entity with the required
    /// columns (e.g. parent_id for trees) can use them.
    fn resolve_collection_variants(&self) -> Vec<RenderVariant> {
        Vec::new()
    }

    /// Mutable holding the current profile cache snapshot.
    ///
    /// Each rebuild swaps in a fresh `Arc<ProfileCache>`, so consumers can
    /// `.signal_cloned()` it to react to profile YAML edits without waiting
    /// for a structural CDC event.
    ///
    /// Default: a Mutable holding an empty cache that never changes
    /// (for mock resolvers / tests).
    fn profile_signal(&self) -> futures_signals::signal::Mutable<Arc<ProfileCache>> {
        futures_signals::signal::Mutable::new(Arc::new(ProfileCache::empty()))
    }
}

#[derive(Debug)]
pub struct ProfileCache {
    profiles: HashMap<EntityName, EntityProfile>,
}

impl ProfileCache {
    /// Empty cache used by stub/mock resolvers.
    pub fn empty() -> Self {
        Self {
            profiles: HashMap::new(),
        }
    }

    /// Cache over a pre-built profile map (used by `ProfileResolver`'s
    /// rebuild path).
    pub fn new(profiles: HashMap<EntityName, EntityProfile>) -> Self {
        Self { profiles }
    }

    /// Look up the profile for an entity. Generic over the borrowed key form,
    /// mirroring `HashMap::get` (`&EntityName` or `&str`).
    pub fn get<Q>(&self, entity_name: &Q) -> Option<&EntityProfile>
    where
        EntityName: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.profiles.get(entity_name)
    }
}
