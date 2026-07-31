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
use std::sync::Arc;
use std::sync::RwLock;

use anyhow::Context;
use anyhow::Result;
use holon_api::CompiledExpr;
use holon_api::FieldLifetime;
use holon_api::TypeDefinition;
/// A compiled computed field: name + pre-compiled Rhai AST.
/// Stored in topological order (dependencies before dependents).
pub use holon_api::entity_profile::CompiledComputedField;
use holon_api::link_parser::LinkSchemeRegistry;
use holon_api::link_parser::LinkTargetClassifier;
use holon_core::util::expr_references;
use holon_core::util::topo_sort_kahn;
use rhai::Engine as RhaiEngine;

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

/// The registry IS the closed set of entity schemes a `[[…]]` target may
/// resolve to. Registered names are already URI-scheme normalized
/// (`EntityName`'s `_`→`-`), so the scheme is looked up verbatim.
impl LinkSchemeRegistry for TypeRegistry {
    fn is_registered_entity_scheme(&self, scheme: &str) -> bool {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .contains_key(scheme)
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
    pub fn register(&self, mut type_def: TypeDefinition) -> Result<()> {
        topo_sort_fields(&mut type_def);
        let name = type_def.name.clone();
        self.types
            .write()
            .expect("TypeRegistry poisoned")
            .insert(name, type_def);
        Ok(())
    }

    /// Add computed fields to an existing type definition.
    /// Compiles expressions and recomputes the topo-sorted order.
    fn add_computed_fields(&self, entity_name: &str, fields: Vec<(String, String)>) -> Result<()> {
        let engine = RhaiEngine::new();
        let mut types = self.types.write().expect("TypeRegistry poisoned");
        let Some(type_def) = types.get_mut(entity_name) else {
            anyhow::bail!(
                "TypeRegistry: cannot add computed fields to unknown entity '{entity_name}'"
            );
        };

        for (name, expr_source) in fields {
            let expr = CompiledExpr::compile(&engine, &expr_source).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to compile computed field '{name}' on entity '{entity_name}': {e}"
                )
            })?;
            if let Some(existing) = type_def.fields.iter_mut().find(|f| f.name == name) {
                existing.lifetime = FieldLifetime::Computed { expr };
            } else {
                type_def.fields.push(holon_api::FieldSchema {
                    name,
                    sql_type: "TEXT".to_string(),
                    lifetime: FieldLifetime::Computed { expr },
                    ..Default::default()
                });
            }
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
        let Some(type_def) = types.get_mut(entity_name) else {
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
        let computed: Vec<(String, String)> = profile.computed.into_iter().collect();
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
    pub fn get(&self, name: &str) -> Option<TypeDefinition> {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .get(name)
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
            .get(entity_name)
            .map(|td| {
                td.fields
                    .iter()
                    .filter_map(|f| match &f.lifetime {
                        FieldLifetime::Computed { expr } => Some((f.name.clone(), expr.clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Check if an entity is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.types
            .read()
            .expect("TypeRegistry poisoned")
            .contains_key(name)
    }
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
            FieldLifetime::Computed { expr } => Some((f.name.clone(), expr.source.clone())),
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
        (BLOCK_PROFILE_YAML, false),     // Block already registered above
        (PERSON_PROFILE_YAML, false),    // Person already registered above
        (COLLECTION_PROFILE_YAML, true), // standalone, needs its own TypeDefinition
    ] {
        let profile = parse_profile_yaml(yaml)
            .with_context(|| "Failed to parse bundled profile YAML".to_string())?;
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

    fn compile(expr: &str) -> CompiledExpr {
        let engine = RhaiEngine::new();
        CompiledExpr::compile(&engine, expr).unwrap()
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
                        expr: compile("priority_score * 2.0"),
                    },
                    ..Default::default()
                },
                FieldSchema {
                    name: "priority_score".to_string(),
                    sql_type: "REAL".to_string(),
                    lifetime: FieldLifetime::Computed {
                        expr: compile("priority * 10.0"),
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

        let result = registry.add_computed_fields(
            "bad",
            vec![("broken".to_string(), "if {{{ invalid".to_string())],
        );
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
                vec![("is_task".to_string(), "task_state != ()".to_string())],
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
        assert_eq!(td.id_references.as_deref(), Some("block"));
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

    #[test]
    fn extension_table_ddl_has_foreign_key() {
        let yaml = std::fs::read_to_string("../../assets/default/types/person.yaml")
            .expect("person.yaml not found");
        let td: TypeDefinition = serde_yaml::from_str(&yaml).unwrap();
        let sql = td.to_create_table_sql();
        assert!(
            sql.contains("REFERENCES \"block\"(id)"),
            "Extension table should FK to block: {sql}"
        );
    }
}
