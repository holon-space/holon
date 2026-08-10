//! ADR 0031 / task #89 — the registration-time half of the two-phase arc check.
//!
//! An in-tree `#[emits("block.typo")]` is a compile error: the macro parses the
//! literal against the declared schema at expansion. A descriptor that arrives
//! from OUTSIDE the tree — a created entity type, an MCP sidecar — never met
//! that parser, and its relation is not one the built-ins know. It is
//! representable by design, so the only place it can be caught is registration.
//!
//! These tests prove the gate refuses there, names the offending field and the
//! schema it was checked against, and does not refuse a declaration that is
//! merely unfamiliar to the built-ins but correct against its own entity.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon::api::OperationDispatcher;
use holon_api::ArcEmit;
use holon_api::ArcPlace;
use holon_api::BoundaryBehavior;
use holon_api::EntityName;
use holon_api::FieldSchema;
use holon_api::MenuExposure;
use holon_api::NonMenuSurface;
use holon_api::OperationDescriptor;
use holon_api::TargetScope;
use holon_api::TransitionArcs;
use holon_api::TypeDefinition;
use holon_api::pattern::OpGuard;
use holon_api::schema::BuiltinSchemas;
use holon_api::schema::SchemaSource;
use holon_api::schema::SchemaSources;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

/// The entity an MCP sidecar contributes: known at runtime, invisible to the
/// macro.
const DYNAMIC_ENTITY: &str = "claude_session";

struct FixtureProvider {
    ops: Vec<OperationDescriptor>,
}

#[async_trait]
impl OperationProvider for FixtureProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        self.ops.clone()
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        _: &str,
        _: StorageEntity,
    ) -> Result<OperationResult> {
        Ok(OperationResult::irreversible(Vec::new()))
    }
}

fn descriptor(entity: &str, op: &str, arcs: TransitionArcs) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: entity.into(),
        entity_short_name: entity.to_string(),
        id_column: "id".to_string(),
        name: op.to_string(),
        display_name: op.to_string(),
        description: String::new(),
        required_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        menu_exposure: MenuExposure::NotListed {
            surface: NonMenuSurface::Internal,
        },
        boundary_behavior: BoundaryBehavior::PrivateOnly,
        target_scope: TargetScope::Block,
        trigger: None,
        bound_params: HashMap::new(),
        guard: OpGuard::None,
        arcs,
    }
}

fn session_type() -> TypeDefinition {
    TypeDefinition::new(
        DYNAMIC_ENTITY,
        vec![
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("title", "TEXT"),
            FieldSchema::new("started_at", "INTEGER"),
        ],
    )
}

fn dispatcher_with(arcs: TransitionArcs) -> OperationDispatcher {
    OperationDispatcher::new(vec![Arc::new(FixtureProvider {
        ops: vec![descriptor(DYNAMIC_ENTITY, "close_session", arcs)],
    })])
}

fn writes(field: &str) -> TransitionArcs {
    TransitionArcs::Declared {
        reads: vec![],
        emits: vec![ArcEmit::Writes(ArcPlace::new(DYNAMIC_ENTITY, field))],
    }
}

#[test]
fn an_arc_naming_a_field_the_dynamic_entity_lacks_refuses_the_registration() {
    let session = session_type();
    let schemas = SchemaSources(vec![&BuiltinSchemas as &dyn SchemaSource, &session]);

    let err = dispatcher_with(writes("titel"))
        .assert_declared_arcs_match_schema(&schemas)
        .expect_err("an arc on a field the entity does not have must refuse");
    let msg = err.to_string();

    assert!(
        msg.contains("titel"),
        "the refusal must name the offending field: {msg}"
    );
    assert!(
        msg.contains("close_session") && msg.contains(DYNAMIC_ENTITY),
        "the refusal must name the operation and its entity: {msg}"
    );
    assert!(
        msg.contains("\"title\"") && msg.contains("\"started_at\""),
        "the refusal must name the schema it was checked against: {msg}"
    );
}

/// The gate must not turn every runtime entity into a refusal: an arc that is
/// correct against its own `TypeDefinition` passes even though the built-in
/// declarations never heard of the relation.
#[test]
fn an_arc_correct_against_the_runtime_type_passes() {
    let session = session_type();
    let schemas = SchemaSources(vec![&BuiltinSchemas as &dyn SchemaSource, &session]);

    dispatcher_with(writes("title"))
        .assert_declared_arcs_match_schema(&schemas)
        .expect("a place the entity really has must register");
}

/// Without the entity's own schema in the source, the relation is unknown to
/// everything — a wiring bug, and refused as one rather than waved through as
/// an unrecognised string.
#[test]
fn a_relation_no_source_knows_refuses_and_names_the_known_relations() {
    let err = dispatcher_with(writes("title"))
        .assert_declared_arcs_match_schema(&BuiltinSchemas)
        .expect_err("an unregistered relation must refuse");
    let msg = err.to_string();

    assert!(
        msg.contains("unknown arc relation") && msg.contains(DYNAMIC_ENTITY),
        "the refusal must name the unknown relation: {msg}"
    );
    assert!(
        msg.contains("\"block\""),
        "and the relations it was checked against: {msg}"
    );
}

/// The fail-closed variant names no places, so the gate has nothing to refuse —
/// "cannot say" must not be read as "declares something wrong".
#[test]
fn a_declaration_that_names_nothing_cannot_name_a_bad_place() {
    dispatcher_with(TransitionArcs::Undeclared)
        .assert_declared_arcs_match_schema(&BuiltinSchemas)
        .expect("Undeclared names no places");
}
