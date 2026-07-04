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

use std::collections::BTreeSet;
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
    /// Required-column set of `condition_source` (type-aware binding). A row
    /// missing any of these makes the condition a structural non-match without
    /// invoking Rhai. Empty when `condition_source` is empty.
    pub condition_required: BTreeSet<String>,
    /// Data-only Rhai condition (None = always true on data side).
    pub data_condition: Option<String>,
    /// Required-column set of `data_condition` (empty when `None`).
    pub data_condition_required: BTreeSet<String>,
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

impl VirtualChildConfig {
    /// Widen the defaults to the entity's DECLARED schema: every declared
    /// column the `virtual_child:` YAML block does not set is seeded `Null` —
    /// the same value a projected row carries for an unset column.
    ///
    /// Without this the synthetic slot row is a NARROWER projection than any
    /// real row of the entity, so every computed field / variant condition over
    /// an unset declared column reports a projection gap the in-process row
    /// cannot possibly have.
    pub fn widened_to_declared(mut self, declared_columns: &BTreeSet<String>) -> Self {
        for col in declared_columns {
            self.defaults.entry(col.clone()).or_insert(Value::Null);
        }
        self
    }
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
    /// The entity's DECLARED schema columns — the persistent field names of its
    /// `TypeDefinition` (the columns a well-formed row of this type always
    /// carries). Used for type-aware binding classification: a required column
    /// MISSING from a row is a real projection gap (LOUD) iff it is declared
    /// here; otherwise it is expected heterogeneity — an optional property or a
    /// UI-state variable — and is silent. Empty for profiles built without a
    /// TypeDefinition (org-source / test fixtures): every miss is then silent.
    pub declared_columns: BTreeSet<String>,
}

// ---------------------------------------------------------------------------
// Resolution (compiles Rhai on-demand)
// ---------------------------------------------------------------------------

impl EntityProfile {
    /// Normalize the creation-slot config against this profile's declared
    /// schema (see [`VirtualChildConfig::widened_to_declared`]). Idempotent —
    /// widening only fills columns the config does not already carry.
    pub fn with_widened_virtual_child(mut self) -> Self {
        self.virtual_child = self
            .virtual_child
            .take()
            .map(|c| c.widened_to_declared(&self.declared_columns));
        self
    }

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

    /// Evaluate ONLY the computed fields for a row — no variant resolution.
    ///
    /// Callers that need the computed-field map but NOT a resolved profile (the
    /// enrichment boundary, `enrich_row`) must use this instead of discarding
    /// the profile from [`Self::resolve_with_computed`]. Running full
    /// variant resolution there evaluated every variant's condition —
    /// including UI-bearing ones like `is_source && is_focused` — against a
    /// raw storage row that carries no such bindings, emitting spurious
    /// eval errors. Computing the fields directly (build scope → extract
    /// computed) skips that entirely.
    pub fn compute_fields_only(
        &self,
        row: &HashMap<String, Value>,
        engine: &RhaiEngine,
    ) -> HashMap<String, Value> {
        let scope = self.build_scope(row, engine);
        self.extract_computed_values(&scope)
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
                Some(dc) => eval_condition(
                    engine,
                    dc,
                    &variant.data_condition_required,
                    &self.declared_columns,
                    &mut scope,
                ),
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
                || eval_condition(
                    engine,
                    &variant.condition_source,
                    &variant.condition_required,
                    &self.declared_columns,
                    scope,
                )
            {
                return Some(variant.profile.clone());
            }
        }
        None
    }

    fn extract_computed_values(&self, scope: &Scope<'_>) -> HashMap<String, Value> {
        // Every computed field appears in the output. A field UNBOUND for this
        // row (type-aware binding skipped it, so it was never pushed to scope)
        // defaults to `Null` — preserving the row's shape for consumers without
        // letting the unbound field poison downstream scope evaluation.
        self.computed_fields
            .iter()
            .map(|(name, _expr)| {
                let value = scope
                    .get_value::<rhai::Dynamic>(name)
                    .map(|d| dynamic_to_value(&d))
                    .unwrap_or(Value::Null);
                (name.clone(), value)
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

        // Evaluate computed fields in topo order via shared evaluator, with
        // type-aware binding against this entity's declared schema.
        let mut computed_ctx = row.clone();
        crate::computed::resolve_computed_fields_with_scope(
            engine,
            &mut scope,
            &self.computed_fields,
            &mut computed_ctx,
            &self.declared_columns,
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

/// Evaluate a profile variant condition against a row scope with **type-aware
/// binding** — the same contract as computed fields.
///
/// `required` is the condition's precompiled required-column set (free vars
/// minus `is_def_var` guards minus `let` locals — see
/// [`holon_expr::required_columns`]). `declared` is the entity's declared
/// schema.
///
/// - If any required column is ABSENT from scope, the condition is *unbound*:
///   the row is structurally the wrong shape (heterogeneous rows expose
///   different columns; an unbound sibling computed field is likewise absent).
///   We return a NON-MATCH **without invoking Rhai** — so no "Variable not
///   found" is raised and, crucially, no `() && …` type-error cascade occurs. A
///   missing column that IS in `declared` is disclosed LOUDLY once (a real
///   projection gap); missing UI-state vars and optional columns are silent.
/// - If every required column is present, we evaluate. A genuine error now
///   (type mismatch on present data, non-bool result) is surfaced at WARN and
///   treated as a non-match — a disclosed degraded signal, never a silent
///   false. WARN not ERROR: one bad condition degrades one variant, it must not
///   abort the render.
fn eval_condition(
    engine: &RhaiEngine,
    source: &str,
    required: &BTreeSet<String>,
    declared: &BTreeSet<String>,
    scope: &mut Scope,
) -> bool {
    let missing: Vec<&String> = required.iter().filter(|c| !scope.contains(c)).collect();
    if !missing.is_empty() {
        for col in &missing {
            if declared.contains(*col) {
                crate::computed::warn_missing_declared_column(source, col);
            }
        }
        return false;
    }
    match engine.eval_with_scope::<bool>(scope, source) {
        Ok(val) => val,
        Err(e) => {
            tracing::warn!(
                condition = source,
                "profile condition failed to evaluate on PRESENT columns (type mismatch / \
                 non-bool result) — treated as non-match, this variant is DEGRADED. Fix the \
                 condition or the row data producing it: {e}"
            );
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

    /// Compute a row's computed-field values WITHOUT resolving its render
    /// profile.
    ///
    /// The enrichment boundary needs only the computed fields; resolving the
    /// profile there evaluates variant conditions against rows that lack the
    /// bindings those conditions reference (e.g. UI-state variables on a raw
    /// storage row), producing spurious eval errors. Real resolvers override
    /// this with the resolution-free path; the default falls back to the
    /// full pass so mock/test resolvers keep working unchanged.
    fn resolve_computed_only(&self, row: &HashMap<String, Value>) -> HashMap<String, Value> {
        self.resolve_with_computed(row).1
    }

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

    /// Collection-level variants resolved through a NAMED profile instead of
    /// the default `collection` profile — the seam a perspective's
    /// `profile_override` drives (a "Kanban perspective" points its panels at
    /// a profile whose collection variants default to `board`).
    ///
    /// Returns `None` when no profile of that name is in the cache, so the
    /// caller can disclose the degraded default-variant behaviour
    /// (fail-visible, not fail-silent).
    // ALLOW(fallback): documents a disclosed, fail-visible degrade (returns None so
    // the caller surfaces it), not a hidden swallow ALLOW(unused_param): trait
    // shape; default impl (mocks) has no cache
    fn resolve_collection_variants_named(&self, _name: &EntityName) -> Option<Vec<RenderVariant>> {
        None
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
    ///
    /// Every profile is normalized on the way in, so a cached profile can
    /// never hand out a creation-slot config narrower than its declared
    /// schema — this is the one funnel every profile source (type-defined,
    /// org-sourced, merged) passes through.
    pub fn new(profiles: HashMap<EntityName, EntityProfile>) -> Self {
        let profiles = profiles
            .into_iter()
            .map(|(name, profile)| (name, profile.with_widened_virtual_child()))
            .collect();
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
