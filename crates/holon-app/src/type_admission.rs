//! Admission control for a type declaration: may this type be declared against
//! the home it names? (CV-E / ruling D54.a.)
//!
//! THE SEAT LIVES HERE, not in `holon`, and that placement is the point.
//! Capability profiles DESCRIBE formats; the layer that COMPOSES formats and
//! profiles is the layer that may decide admission. `holon` is certified
//! against `holon-native.yaml` by
//! `crates/holon/tests/capability_certification.rs`, and the architecture test
//! `a_format_crate_never_links_holon_capability_outside_tests` keeps the two
//! apart — so enforcement (here) and certification (format vs profile) stay
//! independent measurements, and admission against the default `holon-native`
//! home is not self-confirming.
//!
//! TWO ways a type becomes real, and BOTH are guarded here. The
//! `declare_type` op goes through [`declare_type_admitted`]. Registry seeding
//! at boot — `create_default_registry`,
//! `holon_kitchen::register_kitchen_types`, an MCP sidecar — does NOT: it
//! builds the same end state (registry entry, SQL artifacts, write authority)
//! by a different route. That route is covered by [`sweep_registry`], which
//! `holon_app::new_from_config_with_di` runs over the whole registry and which
//! refuses startup on any offender.
//!
//! `holon::core::type_declaration::declare_type` is still public, so the op
//! seat is bypassable the way `move_block` bypasses [`crate::move_guard`]; its
//! only remaining direct callers are that function's own unit tests.

use std::sync::Arc;

use fluxdi::Injector;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeDefinition;
use holon_api::TypeHint;
use holon_capability::HomeSeat;
use holon_capability::ProfileRegistry;
use holon_capability::check_computed_persisted;
use holon_capability::check_declared_homes_exist;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_profiles::TypeRegistry;
use holon_turso::turso_adapter::TursoArtifacts;

/// Why a declaration is refused, or `Ok(())` when the homes it names can carry
/// what it declares.
///
/// Split from [`declare_type_admitted`] so the verdict can be read without
/// declaring anything — the PN guard leg asks exactly this.
pub fn admits(
    profiles: &ProfileRegistry,
    type_def: &TypeDefinition,
) -> std::result::Result<(), String> {
    check_declared_homes_exist(profiles, type_def).map_err(|e| e.to_string())?;
    check_computed_persisted(profiles, type_def, &HomeSeat::Declaration).map_err(|e| e.to_string())
}

/// Run [`admits`] over every type a registry holds, refusing boot if any fails.
///
/// THE REGISTRY, not a list of known doors. Types become real two ways — the
/// `declare_type` op, and registry seeding at boot
/// (`create_default_registry`, `holon_kitchen::register_kitchen_types`, an MCP
/// sidecar) — and only the first goes through [`declare_type_admitted`].
/// Sweeping the registry covers every seeder that shares it, including ones
/// added later, because they all end up in the same map. A list of call sites
/// would have to be maintained, and the door it missed is exactly the door that
/// ships an unchecked type.
///
/// Reports EVERY offender, not the first: at boot the reader wants the whole
/// list to fix, not one round trip per bad type.
pub fn sweep_registry(
    profiles: &ProfileRegistry,
    registry: &TypeRegistry,
) -> std::result::Result<(), String> {
    let refusals: Vec<String> = registry
        .all()
        .iter()
        .filter_map(|type_def| admits(profiles, type_def).err())
        .collect();
    if refusals.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} declared type(s) name a home that cannot carry what they declare:\n  - {}",
        refusals.len(),
        refusals.join("\n  - ")
    ))
}

/// Declare `type_def` only if [`admits`] says its homes can carry it.
///
/// Refusing BEFORE `declare_type` is load-bearing: declaration is one-way (a
/// name, once registered, stays registered), so a refusal that ran afterwards
/// would leave the type half-declared and unrecoverable.
pub async fn declare_type_admitted(
    profiles: &ProfileRegistry,
    type_def: &TypeDefinition,
    db_handle: &holon::storage::turso::DbHandle,
    registry: &TypeRegistry,
    dispatcher: &holon::api::operation_dispatcher::OperationDispatcher,
) -> Result<TursoArtifacts> {
    admits(profiles, type_def)
        .map_err(|e| format!("declaring '{}' is refused: {e}", type_def.name))?;
    holon::core::type_declaration::declare_type(type_def, db_handle, registry, dispatcher).await
}

/// The entity a type declaration acts on. Not a stored row anywhere — the
/// subject IS the type system, so the op is scoped
/// [`holon_api::TargetScope::Global`] and names no `id_column`.
///
/// NOT the wildcard `*`: that arm broadcasts one call to every provider and
/// carries its own ADR 0031 argument, which holds only for ops taking no
/// parameters (`sync`, `full_sync`). This op takes one, and it must run once.
pub const TYPE_ENTITY: &str = "type";
pub const DECLARE_TYPE_OP: &str = "declare_type";

/// The type declaration as a PN action (ADR 0024).
///
/// THIN, deliberately: registering the descriptor makes type onboarding
/// reachable through the generic operation surface — `list_operations` /
/// `execute_operation`, MCP included — so the direction is discoverable by
/// reading the code. Bundled types do NOT come through this op: they are seeded
/// into the registry and admitted by [`sweep_registry`] at boot. Routing them
/// through the PN is deferred (see docs/Plans/BlockGeneralization.md §I3-2
/// follow-ups).
pub fn declare_type_descriptor() -> OperationDescriptor {
    OperationDescriptor {
        entity_name: TYPE_ENTITY.into(),
        entity_short_name: TYPE_ENTITY.to_string(),
        id_column: String::new(),
        name: DECLARE_TYPE_OP.to_string(),
        display_name: "Declare a datatype".to_string(),
        description: "Declare a datatype from a TypeDefinition, refusing it when the home it \
                      names cannot carry what it declares (CV-E). NOT UNDOABLE: declaration is \
                      one-way — a declared name stays declared for the life of the dispatcher."
            .to_string(),
        required_params: vec![OperationParam {
            name: "definition".to_string(),
            type_hint: TypeHint::String,
            description: "The TypeDefinition, as JSON".to_string(),
        }],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Global,
        // A schema op reached by an agent or a harness, never by a gesture on a
        // block — `External` is the variant that says so.
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::External,
        },
        // Fail-closed, and correct rather than merely defaulted: this op names
        // no container, so ANY boundary interaction should be rejected loudly.
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        trigger: None,
        bound_params: Default::default(),
        // The subject is the type system, not any entity kind, so no kind's
        // marking moves.
        marking_delta: holon_api::marking::MarkingDelta::Static { kinds: vec![] },
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Declared {
            reads: vec![],
            emits: vec![],
        },
    }
}

/// Serves [`declare_type_descriptor`] through the dispatcher.
///
/// Resolves its collaborators lazily for the same reason [`crate::move_guard`]
/// does: this provider is a member of the set the dispatcher is built from, so
/// resolving at construction would be a cycle.
pub struct TypeAdmissionProvider {
    injector: Injector,
    profiles: Arc<ProfileRegistry>,
}

impl TypeAdmissionProvider {
    pub fn new(injector: Injector, profiles: Arc<ProfileRegistry>) -> Self {
        Self { injector, profiles }
    }
}

#[async_trait::async_trait]
impl OperationProvider for TypeAdmissionProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![declare_type_descriptor()]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name.as_str() != TYPE_ENTITY || op_name != DECLARE_TYPE_OP {
            return Err(format!(
                "TypeAdmissionProvider: advertises only '{TYPE_ENTITY}::{DECLARE_TYPE_OP}', got \
                 '{entity_name}::{op_name}'"
            )
            .into());
        }

        let definition = params
            .get("definition")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{DECLARE_TYPE_OP}: missing required parameter 'definition'"))?;
        let type_def: TypeDefinition = serde_json::from_str(definition)
            .map_err(|e| format!("{DECLARE_TYPE_OP}: 'definition' is not a TypeDefinition: {e}"))?;

        let db = self
            .injector
            .resolve_async::<dyn holon::di::DbHandleProvider>()
            .await
            .handle();
        let registry = self.injector.resolve_async::<TypeRegistry>().await;
        let dispatcher = self
            .injector
            .resolve_async::<holon::api::operation_dispatcher::OperationDispatcher>()
            .await;

        declare_type_admitted(&self.profiles, &type_def, &db, &registry, &dispatcher).await?;

        Ok(OperationResult::declared_irreversible(
            Vec::new(),
            "type declaration is one-way: it mints SQL artifacts and write authorities that have \
             no inverse; see type_admission.rs",
        ))
    }
}
