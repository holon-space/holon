//! TypeRegistry: runtime singleton mapping entity name → TypeDefinition.
//!
//! THE authority for all entity schema metadata in Holon. Populated at startup
//! from built-in types (Block), MCP sidecar configs, entity profile
//! computed fields, and (future) user-defined YAML type definitions.
//!
//! Computed field expressions are compiled at registration time (parse
//! boundary). If an expression doesn't compile, `register()` returns an error
//! immediately — no invalid expressions can exist in the registry.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::RwLock;

use anyhow::Context;
use anyhow::Result;
use holon_api::ComputedSpec;
use holon_api::FieldLifetime;
use holon_api::TypeDefinition;
/// A compiled computed field: name + pre-compiled Rhai AST.
/// Stored in topological order (dependencies before dependents).
pub use holon_api::entity_profile::CompiledComputedField;
use holon_api::link_parser::LinkScheme;
use holon_api::link_parser::LinkSchemeRegistry;
use holon_api::link_parser::LinkTargetClassifier;
use holon_core::util::expr_references;
use holon_core::util::topo_sort_kahn;
use rhai::Engine as RhaiEngine;

use crate::ComputedFieldDecl;
use crate::EntityProfile;
use crate::ParsedProfile;
use crate::VirtualChildConfig;
use crate::parse_profile_yaml;

/// Runtime registry of all entity type definitions.
///
/// Thread-safe via interior `RwLock`. Injected as `Arc<TypeRegistry>` via DI.
///
/// Stores `TypeDefinition`s with computed fields already compiled (in
/// `FieldLifetime::Computed`) and topo-sorted for correct evaluation order.
pub struct TypeRegistry {
    types: RwLock<HashMap<String, TypeDefinition>>,
    /// Per-entity creation defaults declared in profile YAML (the
    /// `virtual_child:` block). Held alongside `types` because
    /// `TypeDefinition` lives in `holon-api` and shouldn't depend on
    /// profile-side types like `VirtualChildConfig`. `apply_parsed_profile`
    /// inserts here; `profile_from_type_def` callers read here.
    virtual_children: RwLock<HashMap<String, VirtualChildConfig>>,
}

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// The key every [`TypeRegistry`] entry is stored under: the SQL table name,
/// UNDERSCORED.
///
/// A URI scheme is HYPHENATED, so the two spellings differ for every
/// multi-word entity and a raw-string key silently missed them. Only two
/// constructors exist — [`TableName::of`] (from the entity's own
/// `EntityName::table_name()`) and [`TableName::from_scheme`] (which applies
/// the fold) — so a hyphen-keyed registration is now unrepresentable rather
/// than merely discouraged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableName(String);

impl TableName {
    /// The table name an entity registers under.
    pub fn of(entity: &holon_api::EntityName) -> Self {
        Self(entity.table_name())
    }

    /// The table name a URI scheme maps to, folding `-` to `_`.
    ///
    /// Total and idempotent: `EntityName::new` normalizes `_`→`-` before
    /// `table_name()` maps back, so every spelling of an entity — bare,
    /// hyphenated, or prefixed — collapses to the same key this recovers.
    pub fn from_scheme(scheme: &str) -> Self {
        Self(scheme.replace('-', "_"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TableName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The registry IS the closed set of entity schemes a `[[…]]` target may
/// resolve to. [`TableName::from_scheme`] carries the hyphen→underscore fold,
/// so the join cannot be written wrong here.
impl LinkSchemeRegistry for TypeRegistry {
    fn is_registered_entity_scheme(&self, scheme: &LinkScheme<'_>) -> bool {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .contains_key(TableName::from_scheme(scheme.as_str()).as_str())
    }
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self {
            types: RwLock::new(HashMap::new()),
            virtual_children: RwLock::new(HashMap::new()),
        }
    }

    /// Look up creation defaults for an entity type. Used by
    /// `BuilderServices::virtual_child_config` to seed the trailing-slot
    /// data row in tree's `creation_slot` path.
    pub fn virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig> {
        self.virtual_children
            .read()
            .expect("TypeRegistry poisoned")
            .get(entity_name)
            .cloned()
    }

    /// Register a type definition. Topo-sorts computed fields for correct
    /// evaluation order.
    ///
    /// Expressions are already compiled (at deserialization boundary via
    /// `CompiledExpr` serde). This method validates the topo-sort and
    /// stores the reordered definition.
    /// The key is minted through [`TableName`], never taken raw, so a
    /// hyphenated entity name cannot create a key no scheme lookup will find.
    pub fn register(&self, mut type_def: TypeDefinition) -> Result<()> {
        check_computed_types_match_columns(&type_def)?;
        topo_sort_fields(&mut type_def);
        let key = TableName::from_scheme(&type_def.name);
        self.types
            .write()
            .expect("TypeRegistry poisoned")
            .insert(key.as_str().to_string(), type_def);
        Ok(())
    }

    /// Add computed fields to an existing type definition.
    /// Compiles expressions and recomputes the topo-sorted order.
    ///
    /// Each expression is parsed against the entity's DECLARED column types, so
    /// `+` over two TEXT columns concatenates where `+` over two numeric ones
    /// adds. A `computed_persisted` declaration that does not lower to SQL is
    /// refused here — the tier promises a planted matview column, and there is
    /// no such column to plant.
    fn add_computed_fields(
        &self,
        entity_name: &str,
        fields: Vec<(String, ComputedFieldDecl)>,
    ) -> Result<()> {
        let engine = RhaiEngine::new();
        let mut types = self.types.write().expect("TypeRegistry poisoned");
        let Some(type_def) = types.get_mut(TableName::from_scheme(entity_name).as_str()) else {
            anyhow::bail!(
                "TypeRegistry: cannot add computed fields to unknown entity '{entity_name}'"
            );
        };

        let declared = type_def.field_types();
        for (name, decl) in fields {
            let spec = ComputedSpec::parse(&name, &decl.expr, decl.tier, &declared, &engine)
                .map_err(|e| {
                    anyhow::anyhow!("computed field '{name}' on entity '{entity_name}': {e}")
                })?;
            if let Some(existing) = type_def.fields.iter_mut().find(|f| f.name == name) {
                existing.lifetime = FieldLifetime::Computed { spec };
            } else {
                type_def.fields.push(holon_api::FieldSchema {
                    name,
                    sql_type: "TEXT".to_string(),
                    lifetime: FieldLifetime::Computed { spec },
                    ..Default::default()
                });
            }
        }

        let rhai_only: Vec<&str> = type_def
            .computed_specs()
            .into_iter()
            .filter(|(_, spec)| spec.is_rhai_only())
            .map(|(name, _)| name)
            .collect();
        if !rhai_only.is_empty() {
            tracing::warn!(
                entity = entity_name,
                fields = %rhai_only.join(", "),
                "computed fields are outside the SQL-compilable subset and are served by Rhai \
                 alone; they cannot be planted as matview columns"
            );
        }

        // Re-sort fields to maintain topological order
        topo_sort_fields(type_def);
        Ok(())
    }

    /// Add profile variants to an existing type definition.
    fn add_profile_variants(
        &self,
        entity_name: &str,
        variants: Vec<holon_api::ProfileVariant>,
    ) -> Result<()> {
        let mut types = self.types.write().expect("TypeRegistry poisoned");
        let Some(type_def) = types.get_mut(TableName::from_scheme(entity_name).as_str()) else {
            anyhow::bail!(
                "TypeRegistry: cannot add profile variants to unknown entity '{entity_name}'"
            );
        };
        type_def.profile_variants.extend(variants);
        type_def
            .profile_variants
            .sort_by_key(|v| std::cmp::Reverse(v.priority));
        Ok(())
    }

    /// Apply a parsed profile (from YAML) to the corresponding TypeDefinition.
    ///
    /// Adds computed fields and profile variants. Uses the same `ParsedProfile`
    /// produced by both bundled and org-embedded YAML parsing.
    pub fn apply_parsed_profile(&self, profile: ParsedProfile) -> Result<()> {
        let entity_name = profile.entity_name;
        let computed: Vec<(String, ComputedFieldDecl)> = profile.computed.into_iter().collect();
        if !computed.is_empty() {
            self.add_computed_fields(&entity_name, computed)?;
        }
        if !profile.variants.is_empty() {
            self.add_profile_variants(&entity_name, profile.variants)?;
        }
        if let Some(vc) = profile.virtual_child {
            self.virtual_children
                .write()
                .expect("TypeRegistry poisoned")
                .insert(entity_name.clone(), vc);
        }
        Ok(())
    }

    /// Get a type definition by name.
    ///
    /// Accepts either spelling — table name or URI scheme — since both fold to
    /// the one key entries are stored under.
    pub fn get(&self, name: &str) -> Option<TypeDefinition> {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .get(TableName::from_scheme(name).as_str())
            .cloned()
    }

    /// A [`LinkTargetClassifier`] backed by this registry — the one every
    /// production parse boundary should hold, so `[[<entity>:<id>]]` resolves
    /// for exactly the entities that exist.
    pub fn link_target_classifier(self: &Arc<Self>) -> LinkTargetClassifier {
        LinkTargetClassifier::with_registry(self.clone() as Arc<dyn LinkSchemeRegistry>)
    }

    /// Get all registered type definitions.
    pub fn all(&self) -> Vec<TypeDefinition> {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Get computed fields for an entity, already compiled and topologically
    /// sorted.
    pub fn compiled_fields_for(&self, entity_name: &str) -> Vec<CompiledComputedField> {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .get(TableName::from_scheme(entity_name).as_str())
            .map(|td| {
                td.fields
                    .iter()
                    .filter_map(|f| match &f.lifetime {
                        FieldLifetime::Computed { spec } => {
                            Some((f.name.clone(), spec.expr().clone()))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if an entity is registered. Accepts either spelling.
    pub fn contains(&self, name: &str) -> bool {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .contains_key(TableName::from_scheme(name).as_str())
    }
}

/// Refuse a computed field whose declared column types disagree with the type
/// that owns them.
///
/// The types decide the operator, so a spec carrying `numeric` for a TEXT
/// column lowers to `+` where `eval` raises — the two seats would disagree
/// silently. On the profile path the registry derives the types itself; this
/// guards the path where a `TypeDefinition` arrives already carrying specs.
fn check_computed_types_match_columns(type_def: &TypeDefinition) -> Result<()> {
    let declared = type_def.field_types();
    for (name, spec) in type_def.computed_specs() {
        for referenced in spec.computation().referenced_fields() {
            let (Some(owned), Some(carried)) =
                (declared.kind(&referenced), spec.types().kind(&referenced))
            else {
                continue;
            };
            if owned != carried {
                anyhow::bail!(
                    "computed field '{name}' on entity '{}' declares column '{referenced}' as \
                     {carried:?}, but the type declares it as {owned:?}; the two seats would \
                     disagree on the operator",
                    type_def.name
                );
            }
        }
    }
    Ok(())
}

/// Reorder computed fields in a TypeDefinition so dependencies come before
/// dependents. Non-computed fields keep their original order; computed fields
/// are topo-sorted and placed after all non-computed fields.
fn topo_sort_fields(type_def: &mut TypeDefinition) {
    use std::collections::HashSet;

    let computed_sources: Vec<(String, String)> = type_def
        .fields
        .iter()
        .filter_map(|f| match &f.lifetime {
            FieldLifetime::Computed { spec } => Some((f.name.clone(), spec.expr().source.clone())),
            _ => None,
        })
        .collect();

    if computed_sources.len() <= 1 {
        return;
    }

    let names: HashSet<&str> = computed_sources.iter().map(|(n, _)| n.as_str()).collect();
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, expr) in &computed_sources {
        let mut name_deps = Vec::new();
        for other in &names {
            if *other != name.as_str() && expr_references(expr, other) {
                name_deps.push(*other);
            }
        }
        deps.insert(name.as_str(), name_deps);
    }

    let sorted_names = topo_sort_kahn(&names, &deps);

    let all_fields = std::mem::take(&mut type_def.fields);
    let (non_computed, computed): (Vec<_>, Vec<_>) = all_fields
        .into_iter()
        .partition(|f| !matches!(f.lifetime, FieldLifetime::Computed { .. }));

    let computed_map: HashMap<String, _> =
        computed.into_iter().map(|f| (f.name.clone(), f)).collect();

    type_def.fields = non_computed;
    for name in sorted_names {
        if let Some(field) = computed_map.get(&name) {
            type_def.fields.push(field.clone());
        }
    }
}

/// Bundled entity profile YAMLs — same format as org-embedded profiles.
const BLOCK_PROFILE_YAML: &str = include_str!("../../../assets/default/types/block_profile.yaml");
const PERSON_PROFILE_YAML: &str = include_str!("../../../assets/default/types/person_profile.yaml");
const COLLECTION_PROFILE_YAML: &str =
    include_str!("../../../assets/default/types/collection_profile.yaml");
const INTEGRATION_PROFILE_YAML: &str =
    include_str!("../../../assets/default/types/integration_profile.yaml");

/// Bundled YAML type definitions from `assets/default/types/`.
const BUNDLED_TYPES: &[(&str, &str)] = &[
    (
        "person",
        include_str!("../../../assets/default/types/person.yaml"),
    ),
    (
        "organization",
        include_str!("../../../assets/default/types/organization.yaml"),
    ),
];

/// Create a TypeRegistry pre-populated with built-in types and bundled YAML
/// types.
pub fn create_default_registry() -> Result<Arc<TypeRegistry>> {
    use holon_api::block::Block;

    let registry = TypeRegistry::new();
    registry
        .register(Block::type_definition())
        .context("Failed to register Block type")?;

    for (name, yaml) in BUNDLED_TYPES {
        let type_def: TypeDefinition = serde_yaml::from_str(yaml)
            .with_context(|| format!("Failed to parse bundled type '{name}'"))?;
        registry
            .register(type_def)
            .with_context(|| format!("Failed to register bundled type '{name}'"))?;
    }

    // Bundled entity profiles — same format as org-embedded profiles.
    // Each augments an existing TypeDefinition with computed fields + render
    // variants.
    for (yaml, create_type) in [
        (BLOCK_PROFILE_YAML, false),      // Block already registered above
        (PERSON_PROFILE_YAML, false),     // Person already registered above
        (COLLECTION_PROFILE_YAML, true),  // standalone, needs its own TypeDefinition
        (INTEGRATION_PROFILE_YAML, true), // standalone: its rows live in `integration_state`
    ] {
        let profile = parse_profile_yaml(yaml)
            .with_context(|| "Failed to parse bundled profile YAML".to_string())?;
        // Fail LOUD at boot if a bundled computed field calls a lookup the engine
        // never registers — otherwise it errors at eval and silently degrades to
        // () at WARN, inverting every condition it feeds.
        crate::validate_lookups_registered(&profile).with_context(|| {
            format!(
                "bundled profile '{}' references an unregistered lookup function",
                profile.entity_name
            )
        })?;
        if create_type {
            registry
                .register(TypeDefinition::new(&profile.entity_name, vec![]))
                .with_context(|| {
                    format!(
                        "Failed to register type '{}' for profile",
                        profile.entity_name
                    )
                })?;
        }
        registry
            .apply_parsed_profile(profile)
            .context("Failed to apply entity profile")?;
    }

    Ok(Arc::new(registry))
}

/// Build the set of type-defined profiles from a [`TypeRegistry`].
///
/// Bridge function — `profile_from_type_def` can't see the registry's
/// `virtual_children` map, so it's attached here. Single source of truth
/// shared by the Turso DI path and the no-Turso bundled-only resolver.
pub fn type_profiles_from_registry(type_registry: &TypeRegistry) -> Vec<EntityProfile> {
    type_registry
        .all()
        .iter()
        .filter_map(|td| {
            crate::profile_from_type_def(td).map(|mut p| {
                p.virtual_child = type_registry.virtual_child_config(&td.name);
                p.with_widened_virtual_child()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use holon_api::FieldSchema;

    use super::*;

    /// `TableName` is the ONE key form, so both spellings of an entity — the
    /// hyphenated URI scheme and the underscored table name — must fold to the
    /// same key. This is the join that silently dropped every multi-word
    /// sidecar entity's links.
    #[test]
    fn table_name_folds_both_spellings_to_one_key() {
        assert_eq!(
            TableName::from_scheme("cc-session").as_str(),
            TableName::from_scheme("cc_session").as_str(),
            "a scheme and its table name must key the same entry"
        );
        assert_eq!(TableName::from_scheme("cc-session").as_str(), "cc_session");
        // Idempotent, and single-word names are unaffected (which is exactly
        // why a `person` fixture could not detect the broken join).
        assert_eq!(TableName::from_scheme("person").as_str(), "person");
        assert_eq!(
            TableName::of(&holon_api::EntityName::new("cc_session")).as_str(),
            TableName::from_scheme("cc-session").as_str(),
            "EntityName::table_name() and the scheme fold must agree"
        );
    }

    /// Registration keys through `TableName`, so an entity registered under a
    /// HYPHENATED name is still found by its scheme lookup — a hyphen-keyed
    /// entry cannot exist.
    #[test]
    fn a_hyphenated_registration_is_still_found_by_scheme() {
        let registry = TypeRegistry::new();
        let mut td = TypeDefinition::new("t-widget", vec![]);
        td.primary_key = "id".to_string();
        registry.register(td).expect("register");

        assert!(
            registry.contains("t-widget"),
            "the hyphenated spelling must resolve"
        );
        assert!(
            registry.contains("t_widget"),
            "the underscored spelling must resolve to the SAME entry"
        );
    }

    #[test]
    fn register_and_retrieve() {
        let registry = TypeRegistry::new();
        let td = TypeDefinition::new(
            "person",
            vec![
                FieldSchema::new("id", "TEXT").primary_key(),
                FieldSchema::new("email", "TEXT"),
            ],
        );
        registry.register(td).unwrap();

        let retrieved = registry.get("person").unwrap();
        assert_eq!(retrieved.name, "person");
        assert_eq!(retrieved.fields.len(), 2);
    }

    fn live(expr: &str) -> ComputedSpec {
        ComputedSpec::parse(
            "f",
            expr,
            holon_api::ComputedTier::ComputedLive,
            &holon_api::computation::FieldTypes::new(),
            &RhaiEngine::new(),
        )
        .unwrap()
    }

    fn decl(expr: &str) -> ComputedFieldDecl {
        ComputedFieldDecl {
            expr: expr.to_string(),
            tier: holon_api::ComputedTier::ComputedLive,
        }
    }

    #[test]
    fn computed_fields_compiled_and_topo_sorted() {
        let registry = TypeRegistry::new();
        let td = TypeDefinition {
            name: "task".to_string(),
            fields: vec![
                FieldSchema::new("priority", "INTEGER"),
                FieldSchema {
                    name: "weight".to_string(),
                    sql_type: "REAL".to_string(),
                    lifetime: FieldLifetime::Computed {
                        spec: live("priority_score * 2.0"),
                    },
                    ..Default::default()
                },
                FieldSchema {
                    name: "priority_score".to_string(),
                    sql_type: "REAL".to_string(),
                    lifetime: FieldLifetime::Computed {
                        spec: live("priority * 10.0"),
                    },
                    ..Default::default()
                },
            ],
            ..TypeDefinition::new("task", vec![])
        };
        registry.register(td).unwrap();

        let compiled = registry.compiled_fields_for("task");
        assert_eq!(compiled.len(), 2);
        // priority_score must come before weight (weight depends on priority_score)
        assert_eq!(compiled[0].0, "priority_score");
        assert_eq!(compiled[1].0, "weight");
        // Verify they're actually compiled (source is preserved)
        assert_eq!(compiled[0].1.source, "priority * 10.0");
        assert_eq!(compiled[1].1.source, "priority_score * 2.0");
    }

    #[test]
    fn add_computed_fields_rejects_invalid_expression() {
        let registry = TypeRegistry::new();
        registry
            .register(TypeDefinition::new(
                "bad",
                vec![FieldSchema::new("id", "TEXT").primary_key()],
            ))
            .unwrap();

        let result = registry
            .add_computed_fields("bad", vec![("broken".to_string(), decl("if {{{ invalid"))]);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("broken"), "Error should name the field: {err}");
        assert!(err.contains("bad"), "Error should name the entity: {err}");
    }

    #[test]
    fn add_computed_fields_to_existing() {
        let registry = TypeRegistry::new();
        registry
            .register(TypeDefinition::new(
                "block",
                vec![FieldSchema::new("id", "TEXT").primary_key()],
            ))
            .unwrap();

        registry
            .add_computed_fields(
                "block",
                vec![("is_task".to_string(), decl("task_state != ()"))],
            )
            .unwrap();

        let compiled = registry.compiled_fields_for("block");
        assert_eq!(compiled.len(), 1);
        assert_eq!(compiled[0].0, "is_task");
    }

    #[test]
    fn default_registry_has_builtins_and_bundled_types() {
        let registry = create_default_registry().unwrap();
        assert!(registry.contains("block"));
        assert!(registry.contains("person"));
        assert!(registry.contains("organization"));
    }

    #[test]
    fn default_registry_loads_block_and_collection_profiles() {
        let registry = create_default_registry().unwrap();

        let block = registry.get("block").unwrap();
        assert!(
            !block.profile_variants.is_empty(),
            "block should have profile variants from block_profile.yaml"
        );
        assert!(
            !block.computed_fields().is_empty(),
            "block should have computed fields from block_profile.yaml"
        );

        let collection = registry.get("collection").unwrap();
        assert!(
            !collection.profile_variants.is_empty(),
            "collection should have variants from collection_profile.yaml"
        );
    }

    /// Double-chevron fix (ruling 2026-07-21): both embedded-page variants must
    /// opt their expand_toggle into the trailing hover-reveal chevron so it no
    /// longer sits adjacent to the tree collapse chevron in the left gutter.
    /// Guards the YAML wiring the GPUI builder relies on.
    #[test]
    fn embedded_page_variants_use_hover_reveal_toggle() {
        let registry = create_default_registry().unwrap();
        let block = registry.get("block").unwrap();
        for name in ["embedded_page", "embedded_page_expanded"] {
            let variant = block
                .profile_variants
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("block profile missing `{name}` variant"));
            assert!(
                variant.render.contains("hover_reveal_toggle: true"),
                "`{name}` must set hover_reveal_toggle on its expand_toggle \
                 (double-chevron fix); render was:\n{}",
                variant.render
            );
        }
    }

    #[test]
    fn parse_person_yaml() {
        let yaml = std::fs::read_to_string("../../assets/default/types/person.yaml")
            .expect("person.yaml not found");
        let td: TypeDefinition = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(td.name, "person");
        // Free-standing (BG-1): person owns its identity, so it declares no
        // id_references and its raw table carries no FK to block.
        assert_eq!(td.id_references, None);
        assert_eq!(td.graph_label.as_deref(), Some("Person"));
        assert!(td.fields.iter().any(|f| f.name == "email"));
        // Schema-only: no profile_variants (those live in person_profile.yaml)
        assert!(td.profile_variants.is_empty());

        let registry = TypeRegistry::new();
        registry.register(td).unwrap();
    }

    #[test]
    fn parse_organization_yaml() {
        let yaml = std::fs::read_to_string("../../assets/default/types/organization.yaml")
            .expect("organization.yaml not found");
        let td: TypeDefinition = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(td.name, "organization");
        assert!(td.fields.iter().any(|f| f.name == "domain"));
    }

    #[test]
    fn enrich_evaluates_computed_fields() {
        let registry = create_default_registry().unwrap();
        let td = registry.get("person").unwrap();

        let mut row = holon_api::StorageEntity::new();
        row.insert("id".into(), holon_api::Value::String("p1".to_string()));
        row.insert(
            "email".into(),
            holon_api::Value::String("alice@example.com".to_string()),
        );
        row.insert(
            "role".into(),
            holon_api::Value::String("Engineer".to_string()),
        );

        let enriched = td.enrich(row);
        let display = enriched
            .get("display_name")
            .expect("display_name should be computed");
        assert_eq!(
            display,
            &holon_api::Value::String("Engineer — alice@example.com".to_string()),
            "display_name should concatenate role and email"
        );
    }

    #[test]
    fn enrich_handles_missing_optional_fields() {
        let registry = create_default_registry().unwrap();
        let td = registry.get("person").unwrap();

        let mut row = holon_api::StorageEntity::new();
        row.insert("id".into(), holon_api::Value::String("p2".to_string()));
        row.insert(
            "email".into(),
            holon_api::Value::String("bob@example.com".to_string()),
        );
        // role is NOT set — the expression should take the else branch

        let enriched = td.enrich(row);
        let display = enriched
            .get("display_name")
            .expect("display_name should be computed");
        assert_eq!(
            display,
            &holon_api::Value::String("bob@example.com".to_string()),
            "display_name should fall back to email when role is absent"
        );
    }

    /// The FK is a property of `id_references`, not of being a bundled type.
    /// A type that names a referent gets the FK; a FREE-STANDING one owns its
    /// identity and must get none — `person` is free-standing, so a DDL
    /// anchoring it to `block` would recreate the coupling the datatype
    /// generalization removed.
    #[test]
    fn extension_table_ddl_has_a_foreign_key_only_when_the_type_references_one() {
        let yaml = std::fs::read_to_string("../../assets/default/types/person.yaml")
            .expect("person.yaml not found");
        let free_standing: TypeDefinition = serde_yaml::from_str(&yaml).unwrap();
        assert!(
            free_standing.id_references.is_none(),
            "person is the free-standing seed type; this test's premise changed"
        );
        let sql = free_standing.to_create_table_sql();
        assert!(
            !sql.contains("REFERENCES"),
            "a free-standing type's table must anchor to nothing: {sql}"
        );

        let mut referencing = free_standing;
        referencing.id_references = Some("block".to_string());
        let sql = referencing.to_create_table_sql();
        assert!(
            sql.contains("REFERENCES \"block\"(id)"),
            "a type declaring `id_references: block` must FK to it: {sql}"
        );
    }
}
