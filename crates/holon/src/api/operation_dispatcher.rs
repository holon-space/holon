//! OperationDispatcher - Composite pattern implementation for operation routing
//!
//! The OperationDispatcher aggregates multiple OperationProvider instances and
//! routes operation execution to the correct provider based on entity_name.
//!
//! This implements the Composite Pattern - both individual caches
//! (QueryableCache<T>) and the dispatcher implement OperationProvider, allowing
//! recursive composition.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::schema::BuiltinSchemas;
use holon_api::schema::SchemaSource;
use holon_core::BoundaryEnforcer;
use holon_core::OperationObserver;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::SyncTokenStore;
use holon_core::UndoAction;
use holon_core::storage::types::StorageEntity;
use tracing::error;
use tracing::info;

use crate::api::guard_world::GuardQuery;

/// Composite dispatcher that aggregates multiple OperationProvider instances
///
/// Routes operations to the correct provider based on entity_name.
/// Implements OperationProvider itself, enabling recursive composition.
/// Supports wildcard entity_name "*" to execute operations on all matching
/// providers.
///
/// Also supports OperationObservers that get notified after operations execute.
/// Observers can filter by entity_name or use "*" to observe all operations.
#[derive(Default)]
pub struct OperationDispatcher {
    providers: Vec<Arc<dyn OperationProvider>>,
    /// Write authorities registered AFTER composition, when a type is declared
    /// at runtime (`crate::core::type_declaration::declare_type`). A declared
    /// type's writes route here exactly as a wired entity's route to
    /// `providers`; the two lists differ only in when they were filled.
    declared_providers: std::sync::RwLock<Vec<Arc<dyn OperationProvider>>>,
    observers: Vec<Arc<dyn OperationObserver>>,
    sync_token_store: Option<Arc<dyn SyncTokenStore>>,
    matview_manager: Option<Arc<crate::sync::MatviewManager>>,
    boundary_enforcer: Option<Arc<dyn BoundaryEnforcer>>,
    /// ADR 0031 Increment 3 — the world declared `#[require]` guards are
    /// evaluated against. Absent only in composition sites with no projection.
    guard_world: Option<Arc<dyn crate::api::guard_world::GuardWorld>>,
    /// ADR 0032 §3 — the marking legality of an operation's whole delta.
    net_guard: Option<Arc<dyn crate::api::net_guard::NetGuard>>,
    /// Classifies `[[…]]` targets in live-edit content. Built from the
    /// `TypeRegistry` at wiring time so a UI-authored `[[<entity>:<id>]]`
    /// resolves for exactly the entities that exist; the `Default` value knows
    /// only the built-in schemes.
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
    /// Entities the composition root deliberately left unwired, each with the
    /// configuration that removed it. An unregistered entity is two different
    /// states — "no such entity" and "this build turned it off" — and only the
    /// composition root can tell them apart, so it says which one this is.
    unavailable_entities: UnavailableEntities,
}

/// Why an entity has no provider in THIS container, keyed by entity name. Built
/// by the composition root and resolved by [`OperationModule`]; a container
/// that registers none behaves exactly as before.
#[derive(Debug, Clone, Default)]
pub struct UnavailableEntities(pub HashMap<EntityName, String>);

impl UnavailableEntities {
    pub fn new(entries: impl IntoIterator<Item = (&'static str, String)>) -> Self {
        Self(
            entries
                .into_iter()
                .map(|(name, reason)| (EntityName::new(name), reason))
                .collect(),
        )
    }

    /// Canonicalized on the way in, like every other entity lookup: the
    /// dispatcher's resolved name is a raw `&str` whose `_`/`-` spelling need
    /// not match the one the composition root registered.
    fn reason_for(&self, entity: &str) -> Option<&str> {
        self.0.get(&EntityName::new(entity)).map(String::as_str)
    }
}

/// Whether the params reaching the dispatcher carry text a human or an agent
/// JUST AUTHORED. Only that case adopts raw org markup in `content` into a
/// stripped label plus a mark set.
///
/// This is PROVENANCE, and it cannot be recovered from the params' shape.
/// Inverses now state every column explicitly — `capture_row` carries NULL
/// columns as `Value::Null`, and a content inverse carries its prior marks as
/// a rich Object — but a mark-free block still resurrects with `marks` NULL,
/// the same shape freshly typed text has. Reading that shape as "unparsed
/// input" makes UNDO rewrite the very bytes it exists to restore (ADR 0024's
/// identity-preserving inverse). The engine holds the origin and is the only
/// place that can say.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthoredInput {
    /// A live authoring intent (`OpOrigin::User` / `OpOrigin::Agent`):
    /// `content` may hold raw `[[Page]]` / `*bold*` the author typed.
    Live,
    /// Everything else — undo/redo replay, inverse ops, rule- and sync-origin
    /// writes, and every already-parsed write. Bytes travel untouched.
    Verbatim,
}
impl OperationDispatcher {
    pub fn new(providers: Vec<Arc<dyn OperationProvider>>) -> Self {
        Self {
            providers,
            ..Default::default()
        }
    }

    pub fn with_observers(
        providers: Vec<Arc<dyn OperationProvider>>,
        observers: Vec<Arc<dyn OperationObserver>>,
    ) -> Self {
        Self {
            providers,
            observers,
            ..Default::default()
        }
    }

    pub fn set_sync_token_store(&mut self, store: Arc<dyn SyncTokenStore>) {
        self.sync_token_store = Some(store);
    }

    pub fn set_matview_manager(&mut self, mgr: Arc<crate::sync::MatviewManager>) {
        self.matview_manager = Some(mgr);
    }

    /// Install the registry-backed link classifier used to parse inline markup
    /// at the UI intent boundary.
    pub fn set_link_classifier(
        &mut self,
        classifier: holon_api::link_parser::LinkTargetClassifier,
    ) {
        self.link_classifier = classifier;
    }

    /// Install the ADR 0028 boundary/authz seam (C3). Consulted before every
    /// dispatched operation that names a subject block; a rejection is returned
    /// as an `Err` and the provider never runs (D2 "reject loud").
    pub fn set_boundary_enforcer(&mut self, enforcer: Arc<dyn BoundaryEnforcer>) {
        self.boundary_enforcer = Some(enforcer);
    }

    /// Install the ADR 0031 guard seam. Consulted before every dispatched
    /// operation whose descriptor declares a `#[require]` guard; a guard that
    /// does not hold for the subject is returned as an `Err` and the provider
    /// never runs.
    pub fn set_guard_world(&mut self, world: Arc<dyn crate::api::guard_world::GuardWorld>) {
        self.guard_world = Some(world);
    }

    /// Install the ADR 0032 §3 net guard. Consulted before every dispatched
    /// operation, after the two gates above; a refused operation is returned as
    /// an `Err` and the provider never runs.
    pub fn set_net_guard(&mut self, guard: Arc<dyn crate::api::net_guard::NetGuard>) {
        self.net_guard = Some(guard);
    }

    /// Record which entities this container left unwired and why, so a dispatch
    /// against one fails naming the configuration instead of the missing
    /// registration.
    pub fn set_unavailable_entities(&mut self, unavailable: UnavailableEntities) {
        self.unavailable_entities = unavailable;
    }

    /// Add an observer to this dispatcher
    pub fn add_observer(&mut self, observer: Arc<dyn OperationObserver>) {
        self.observers.push(observer);
    }

    /// Notify all matching observers of an executed operation
    async fn notify_observers(
        &self,
        entity_name: &str,
        operation: &Operation,
        undo_action: &UndoAction,
    ) {
        for observer in &self.observers {
            let filter = observer.entity_filter();
            if filter == "*" || filter == entity_name {
                observer.on_operation_executed(operation, undo_action).await;
            }
        }
    }

    /// Check if a provider is registered for an entity type
    pub fn has_provider(&self, entity_name: &str) -> bool {
        // Canonicalized first: descriptors carry `EntityName`, whose `_`→`-`
        // fold means a raw `gen_1` compares unequal to the `gen-1` a provider
        // for that very type advertises.
        let entity_name = EntityName::new(entity_name);
        self.all_providers().iter().any(|provider| {
            provider
                .operations()
                .iter()
                .any(|op| op.entity_name == entity_name)
        })
    }

    /// Get list of registered entity names
    pub fn registered_entities(&self) -> Vec<EntityName> {
        let mut entity_names = HashSet::new();
        for provider in &self.all_providers() {
            for op in provider.operations() {
                entity_names.insert(op.entity_name);
            }
        }
        entity_names.into_iter().collect()
    }

    /// Get the number of registered providers
    pub fn provider_count(&self) -> usize {
        self.all_providers().len()
    }

    /// Get a copy of all providers (for reconstructing dispatcher with
    /// additional providers)
    pub fn providers(&self) -> Vec<Arc<dyn OperationProvider>> {
        self.all_providers()
    }

    /// Every provider routing decisions consult: the composed ones plus the
    /// ones runtime type declarations added. Cloned out so no lock is held
    /// across an `await`.
    fn all_providers(&self) -> Vec<Arc<dyn OperationProvider>> {
        let declared = self
            .declared_providers
            .read()
            .expect("declared-provider registry poisoned");
        self.providers
            .iter()
            .chain(declared.iter())
            .cloned()
            .collect()
    }

    /// Give a type declared at runtime its write authority.
    ///
    /// Refuses a provider that would make an already-routable operation
    /// ambiguous. Dispatch selects by the (entity, op) PAIR, so that pair is
    /// the unit of ambiguity: a second provider offering one the registry
    /// already answers means the dispatch lands in whichever the routing scan
    /// reaches first. Providers that share an entity but no op — a connector's
    /// own vocabulary alongside the CRUD derived from the mirror's columns —
    /// route unambiguously and are allowed.
    ///
    /// The refusal is TERMINAL for that pair in this increment. This registry
    /// is append-only — nothing removes a declared authority — so re-declaring
    /// a type is not a recoverable path, and the error says so rather than
    /// naming a teardown that would not help.
    pub fn register_provider(&self, provider: Arc<dyn OperationProvider>) -> Result<()> {
        let registered: HashSet<(EntityName, String)> = self
            .operations()
            .into_iter()
            .map(|op| (op.entity_name, op.name))
            .collect();
        for op in provider.operations() {
            if registered.contains(&(op.entity_name.clone(), op.name.clone())) {
                let entity = &op.entity_name;
                let name = &op.name;
                return Err(format!(
                    "[OperationDispatcher] operation '{name}' on entity '{entity}' already has a \
                     write authority; registering a second one would make the routing scan decide \
                     which of the two a dispatch lands in. Re-declaring a live type is NOT \
                     SUPPORTED in this increment: this registry is append-only, and \
                     `TursoAdapter::teardown` drops only the SQL artifacts, so no sequence of \
                     calls frees the name. Declaring over a live type arrives with the migrate \
                     primitive (OQ-5), which retires this error. Until then, use a name that is \
                     not yet declared."
                )
                .into());
            }
        }
        self.declared_providers
            .write()
            .expect("declared-provider registry poisoned")
            .push(provider);
        Ok(())
    }

    /// Fail-loud guard against the "block pipeline wired but no CRUD" trap.
    ///
    /// `EventInfraModule` registers `SqlBlockOperations`, which advertises only
    /// **structural** block ops (`indent` / `outdent` / `move_*` /
    /// `split_block` / `join_block`). The content-write ops (`create` /
    /// `set_field` / `delete`) come from a *separate* provider -
    /// `LoroBlockOperations` under Loro authority, or a bare
    /// `SqlOperationProvider` in SqlOnly embedders. An embedder that wires
    /// `EventInfraModule` alone therefore gets a block pipeline that
    /// answers structural dispatches but silently drops every content write
    /// as "No provider registered for entity: block" (this bit the
    /// dioxus-web worker; see `frontends/holon-worker/src/lib.rs`).
    ///
    /// This check runs at startup (from [`OperationModule`], during
    /// `BackendEngine` construction) so the misconfiguration crashes loudly
    /// with a clear message instead of degrading to silent data loss. It is
    /// a no-op when no `block` provider is registered at all (a read-only /
    /// nav-only backend never dispatches block writes).
    pub fn assert_content_write_capability(&self) -> Result<()> {
        // No block pipeline => nothing dispatches block writes => nothing to
        // guard. A read-only / nav-only backend never reaches the check.
        if !self.has_provider("block") {
            return Ok(());
        }
        self.assert_write_capability_for("block")
    }

    /// The same check for ONE entity, required rather than optional: the
    /// entity must be routable AND must advertise the full CRUD triple.
    ///
    /// Runtime type declaration calls this after registering a type's write
    /// authority, so a type whose serialization exists but whose writes would
    /// be dropped fails at DECLARATION rather than at the first write.
    pub fn assert_write_capability_for(&self, entity: &str) -> Result<()> {
        // The content-write ops any writable frontend dispatches. Kept in
        // sync with `CrudOperations` (holon-core `traits.rs`): the ops a
        // structural-only provider does NOT advertise.
        const REQUIRED_WRITE_OPS: [&str; 3] = ["create", "set_field", "delete"];

        // Canonicalized: see `has_provider`.
        let entity = EntityName::new(entity);
        let entity_ops: HashSet<String> = self
            .operations()
            .into_iter()
            .filter(|op| op.entity_name == entity)
            .map(|op| op.name)
            .collect();

        let missing: Vec<&str> = REQUIRED_WRITE_OPS
            .into_iter()
            .filter(|op| !entity_ops.contains(*op))
            .collect();

        if missing.is_empty() {
            return Ok(());
        }

        let mut present: Vec<&str> = entity_ops.iter().map(String::as_str).collect();
        present.sort_unstable();

        Err(format!(
            "[OperationDispatcher] the `{entity}` pipeline is wired but the operation registry is \
             missing content-write op(s) {missing:?}. Every dispatch of those ops would be \
             silently dropped as \"No provider registered for entity: {entity}\", losing user \
             content. For `block`: `EventInfraModule` alone advertises only STRUCTURAL ops \
             (indent / outdent / move_* / split_block / join_block); the CRUD ops come from a \
             SEPARATE provider — LoroModule + OrgModeModule (native, via holon-app \
             `add_frontend`) under Loro authority, or a bare `SqlOperationProvider` in SqlOnly \
             embedders (see frontends/holon-worker/src/lib.rs). For a type declared at runtime: \
             its write authority is registered by `core::type_declaration::declare_type`. Present \
             `{entity}` ops: {present:?}"
        )
        .into())
    }

    /// The ADR 0028 C3 boundary/authz decision for one dispatched operation.
    ///
    /// The op's declared [`holon_api::BoundaryBehavior`] plus the containers of
    /// the subject (`id`) and — when the intent carries one — the reparent
    /// destination (`parent_id`) decide allow vs. reject-loud. A rejection is
    /// an `Err`, so the operation never reaches its provider and is never
    /// silently dropped (D2).
    ///
    /// Scoped to ops that NAME a subject: an op naming no block sits in no
    /// container, so there is no boundary to judge.
    fn enforce_boundary(
        &self,
        available_ops: &[OperationDescriptor],
        resolved_entity_name: &str,
        op_name: &str,
        params: &StorageEntity,
    ) -> Result<()> {
        let Some(enforcer) = &self.boundary_enforcer else {
            return Ok(());
        };
        let Some(subject) = params.get("id").and_then(|v| v.as_string()) else {
            return Ok(());
        };
        let descriptor = available_ops
            .iter()
            .find(|op| op.entity_name == resolved_entity_name && op.name == op_name)
            .ok_or_else(|| {
                format!(
                    "boundary seam: no descriptor for {resolved_entity_name}.{op_name} after \
                     provider resolution"
                )
            })?;
        enforcer
            .check(
                op_name,
                &descriptor.boundary_behavior,
                subject,
                params.get("parent_id").and_then(|v| v.as_string()),
            )
            .map_err(|e| format!("ADR 0028 boundary enforcement: {e}"))?;
        Ok(())
    }

    /// The ADR 0031 declared-guard decision for one dispatched operation.
    ///
    /// Ruling G1=A: the guard is evaluated against the **current** world and
    /// refuses before the op fires. An `OpGuard::None` op pays one descriptor
    /// lookup and touches no world.
    ///
    /// The predicate is subject-BOUND, not "is this guard enabled anywhere":
    /// [`holon_api::pattern::GuardResult::enabled`] answers "some row satisfies
    /// this", which would wave an op through on an unrelated block's binding
    /// (R8). [`GuardQuery::bind`] pairs the guard with the op's `id` — the same
    /// subject [`Self::enforce_boundary`] reads.
    async fn enforce_guard(
        &self,
        available_ops: &[OperationDescriptor],
        resolved_entity_name: &str,
        op_name: &str,
        params: &StorageEntity,
    ) -> Result<()> {
        let descriptor = available_ops
            .iter()
            .find(|op| op.entity_name == resolved_entity_name && op.name == op_name)
            .ok_or_else(|| {
                format!(
                    "guard seam: no descriptor for {resolved_entity_name}.{op_name} after \
                     provider resolution"
                )
            })?;
        let (Some(guard), Some(source)) = (descriptor.guard.guard(), descriptor.guard.source())
        else {
            return Ok(());
        };
        let world = self.guard_world.as_ref().ok_or_else(|| {
            format!(
                "ADR 0031 guard gate: {resolved_entity_name}.{op_name} declares the guard \
                 `{source}` but no GuardWorld is installed at this composition site, so the \
                 declaration could only be ignored. Call `set_guard_world`."
            )
        })?;
        let query = GuardQuery::bind(guard, params.get("id").and_then(|v| v.as_string()))?;
        if world.guard_holds(&query).await? {
            return Ok(());
        }
        Err(format!(
            "ADR 0031 guard refusal: {resolved_entity_name}.{op_name} requires `{source}`, \
             which does not hold for {:?} in the current state",
            query.subject()
        )
        .into())
    }

    /// The ADR 0032 §3 net-guard decision for one dispatched operation.
    ///
    /// # Unification with [`Self::enforce_guard`]
    /// The gate above answers the same question — enabledness — for a
    /// subject-bound predicate against the current world; this one answers it
    /// for the whole delta an operation would write. They unify once the
    /// derived net projection exists AND the declared-guard predicates prove
    /// expressible as net arcs: `GuardWorld` generalizes to marking-aware
    /// whole-delta evaluation and [`crate::api::net_guard::NetGuard`] folds
    /// into it.
    async fn enforce_net_guard(
        &self,
        resolved_entity_name: &str,
        op_name: &str,
        params: &StorageEntity,
    ) -> Result<()> {
        let Some(guard) = &self.net_guard else {
            return Ok(());
        };
        let op = crate::api::net_guard::NetGuardOp {
            entity_name: resolved_entity_name,
            op_name,
            params,
            confirmation: crate::api::net_guard::Confirmation::parse(params)?,
        };
        match guard.check(&op).await? {
            crate::api::net_guard::NetVerdict::Confirm => Ok(()),
            crate::api::net_guard::NetVerdict::Refuse(refusal) => Err(format!(
                "ADR 0032 net-guard refusal: {resolved_entity_name}.{op_name} — {}",
                refusal.reason
            )
            .into()),
        }
    }

    /// Fail-loud guard that a composed backend actually installed the ADR 0032
    /// net gate.
    ///
    /// Every composition site installs one, `InertNetGuard` where no placement
    /// policy exists, so `None` means a site that forgot rather than a site
    /// that declined.
    pub fn assert_net_guard_installed(&self) -> Result<()> {
        if self.net_guard.is_some() {
            return Ok(());
        }
        Err(
            "[OperationDispatcher] no NetGuard installed: every operation would execute without \
             the ADR 0032 placement check. Call `set_net_guard` at this composition site \
             (`holon::api::net_guard::InertNetGuard` where no placement policy exists)."
                .into(),
        )
    }

    /// Fail-loud guard that a composed backend actually installed the ADR 0028
    /// boundary seam.
    ///
    /// A dispatcher with no [`BoundaryEnforcer`] executes every op unchecked.
    /// That is invisible from the outside — the vault behaves normally right up
    /// to the point a share policy exists and is not enforced — so a second
    /// composition site that forgets [`Self::set_boundary_enforcer`] must crash
    /// at startup, exactly like the content-write guard above.
    pub fn assert_boundary_seam_installed(&self) -> Result<()> {
        if self.boundary_enforcer.is_some() {
            return Ok(());
        }
        Err(
            "[OperationDispatcher] no BoundaryEnforcer installed: every operation would execute \
             without the ADR 0028 boundary check, so a committed share policy would not be \
             enforced. Call `set_boundary_enforcer` at this composition site (prod installs \
             `holon_sharing::PolicyOverlayEnforcer::inert()`)."
                .into(),
        )
    }
    /// The registration-time half of the two-phase arc check (ADR 0031): every
    /// place a registered descriptor declares must name a relation `schemas`
    /// knows and a field that relation has.
    ///
    /// A descriptor written in-tree already passed the macro's compile-time
    /// parse. One that arrives from outside — a created entity type, an MCP
    /// sidecar — never did, and an arc naming a field its entity does not have
    /// is a declaration that can never be violated and never red. Refuse the
    /// registration instead of carrying the string.
    pub fn assert_declared_arcs_match_schema(&self, schemas: &dyn SchemaSource) -> Result<()> {
        for provider in &self.all_providers() {
            for op in provider.operations() {
                op.arcs.validate_against(schemas).map_err(|e| {
                    format!(
                        "[OperationDispatcher] operation '{}' on entity '{}' declares an arc that \
                         its entity's schema does not have: {e}",
                        op.name, op.entity_name
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Execute an operation by routing to the correct provider
    ///
    /// # Arguments
    /// * `entity_name` - Entity identifier (e.g., "todoist-task" or "*" for
    ///   wildcard)
    /// * `op_name` - Operation name (e.g., "set_state" or "sync")
    /// * `params` - Operation parameters as StorageEntity
    ///
    /// # Returns
    /// Result indicating success or failure
    ///
    /// # Errors
    /// Returns an error if:
    /// - No provider is registered for the entity_name (or wildcard matches no
    ///   providers)
    /// - The provider's execute_operation returns an error
    pub async fn execute_operation_with_input(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        input: AuthoredInput,
    ) -> Result<OperationResult> {
        self.execute_operation_with_provenance(entity_name, op_name, params, input, OpOrigin::User)
            .await
    }

    /// [`Self::execute_operation_with_input`] for a caller that knows the
    /// operation's provenance.
    ///
    /// `origin` decides whether the write earns an undo/redo entry:
    /// [`OpOrigin::User`] is the only origin that does, which is the rule
    /// `OpOrigin` itself states. A derived write — a rule firing, a peer
    /// merging, a vault file re-deriving its rows — must not enter the log:
    /// undoing one is meaningless (the deriving source writes it straight back)
    /// and a vault of files would bury the user's own edits under machine
    /// entries on every boot.
    pub async fn execute_operation_with_provenance(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
        input: AuthoredInput,
        origin: OpOrigin,
    ) -> Result<OperationResult> {
        use tracing::Instrument;
        use tracing::debug;
        use tracing::info;

        // Create tracing span that will be bridged to OpenTelemetry
        // Use .instrument() to maintain context across async boundaries
        let span = tracing::span!(
            tracing::Level::INFO,
            "dispatcher.execute_operation",
            "operation.entity" = entity_name.as_str(),
            "operation.name" = op_name,
            // Filled in below once routing has settled which entity actually
            // answers. The two differ whenever a caller names a view
            // (`focus_roots`) and the `id` param's scheme decides the real
            // provider, so a consumer that must name what RAN — the ADR 0032
            // net's totality check — reads this one.
            "operation.resolved_entity" = tracing::field::Empty
        );

        async {
            info!(
                "[OperationDispatcher] execute_operation: entity={}, op={}, params={:?}",
                entity_name, op_name, params
            );

            // ADR 0031 Increment 3 — guard evaluation does NOT cover this arm,
            // and the gap is vacuous rather than tolerated. The complete set of
            // ops advertised under entity `*` is synthesized in
            // `OperationProvider::operations` below: `sync` and `full_sync`.
            // Both carry `required_params: []`, an empty `id_column`,
            // `TargetScope::Global` and `OpGuard::None` — they name no subject
            // block, so a relational guard has nothing to bind.
            //
            // A wildcard op that DOES take a subject param would break that
            // reasoning; adding one means giving this arm its own gate first.
            if entity_name == "*" {
                info!(
                    "[OperationDispatcher] Wildcard operation detected: op={}",
                    op_name
                );

                // Special handling for full_sync: clear sync tokens, clear caches, then sync
                // IMPORTANT: Tokens must be cleared FIRST because clearing caches can trigger
                // sync_changes callbacks that would load and re-save the old token.
                if op_name == "full_sync" {
                    info!(
                        "[OperationDispatcher] Executing full_sync: clearing sync tokens and \
                         caches first"
                    );

                    // Step 1: Clear all sync tokens FIRST (so any triggered syncs start from
                    // Beginning)
                    if let Some(ref token_store) = self.sync_token_store {
                        match token_store.clear_all_tokens().await {
                            Ok(_) => {
                                info!("[OperationDispatcher] Cleared all sync tokens");
                            }
                            Err(e) => {
                                error!("[OperationDispatcher] Failed to clear sync tokens: {}", e);
                            }
                        }
                    } else {
                        info!(
                            "[OperationDispatcher] No sync token store configured, skipping token \
                             clearing"
                        );
                    }

                    // Step 2: Clear all caches (execute clear_cache on all providers that have it)
                    for provider in &self.all_providers() {
                        if let Some(op) = provider
                            .operations()
                            .iter()
                            .find(|op| op.name == "clear_cache")
                        {
                            let entity_name = op.entity_name.as_str();
                            match provider
                                .execute_operation(
                                    &op.entity_name,
                                    "clear_cache",
                                    StorageEntity::new(),
                                )
                                .await
                            {
                                Ok(_) => {
                                    info!(
                                        "[OperationDispatcher] Cleared cache for entity '{}'",
                                        entity_name
                                    );
                                }
                                Err(e) => {
                                    error!(
                                        "[OperationDispatcher] Failed to clear cache for entity \
                                         '{}': {}",
                                        entity_name, e
                                    );
                                }
                            }
                        }
                    }

                    // Step 3: Drop stale matviews so they get recreated fresh
                    if let Some(ref mgr) = self.matview_manager {
                        match mgr.drop_stale_views().await {
                            Ok(()) => info!("[OperationDispatcher] Dropped stale matviews"),
                            Err(e) => {
                                error!("[OperationDispatcher] Failed to drop stale matviews: {e}")
                            }
                        }
                    }

                    // Step 4: Execute sync on all providers that have it
                    info!("[OperationDispatcher] Executing sync on all providers");
                    let mut sync_success_count = 0;
                    let mut sync_error_count = 0;
                    for provider in &self.all_providers() {
                        if let Some(op) = provider.operations().iter().find(|op| op.name == "sync")
                        {
                            let entity_name = op.entity_name.as_str();
                            match provider
                                .execute_operation(&op.entity_name, "sync", StorageEntity::new())
                                .await
                            {
                                Ok(_) => {
                                    sync_success_count += 1;
                                    info!(
                                        "[OperationDispatcher] Sync succeeded for entity '{}'",
                                        entity_name
                                    );
                                }
                                Err(e) => {
                                    sync_error_count += 1;
                                    error!(
                                        "[OperationDispatcher] Sync failed for entity '{}': {}",
                                        entity_name, e
                                    );
                                }
                            }
                        }
                    }

                    info!(
                        "[OperationDispatcher] full_sync completed: {} sync succeeded, {} failed",
                        sync_success_count, sync_error_count
                    );
                    return Ok(OperationResult::irreversible(Vec::new()));
                }

                // Find all providers that have an operation with matching op_name
                let mut matching_providers = Vec::new();
                for provider in &self.all_providers() {
                    let ops = provider.operations();
                    if ops.iter().any(|op| op.name == op_name) {
                        matching_providers.push(provider.clone());
                    }
                }

                if matching_providers.is_empty() {
                    error!(
                        "[OperationDispatcher] No providers found with operation '{}' for \
                         wildcard dispatch",
                        op_name
                    );
                    return Err(format!(
                        "No providers found with operation '{}' for wildcard dispatch",
                        op_name
                    )
                    .into());
                }

                info!(
                    "[OperationDispatcher] Found {} providers with operation '{}'",
                    matching_providers.len(),
                    op_name
                );

                // Execute operation on each matching provider
                let mut success_count = 0;
                let mut error_count = 0;
                for provider in matching_providers {
                    // For wildcard operations, we need to find the actual entity_name from the
                    // provider Find the first operation with matching op_name
                    let ops = provider.operations();
                    if let Some(op) = ops.iter().find(|op| op.name == op_name) {
                        let actual_entity_name = op.entity_name.as_str();
                        match provider
                            .execute_operation(&op.entity_name, op_name, params.clone())
                            .await
                        {
                            Ok(_) => {
                                success_count += 1;
                                info!(
                                    "[OperationDispatcher] Wildcard operation succeeded on entity \
                                     '{}'",
                                    actual_entity_name
                                );
                            }
                            Err(e) => {
                                error_count += 1;
                                error!(
                                    "[OperationDispatcher] Wildcard operation failed on entity \
                                     '{}': {}",
                                    actual_entity_name, e
                                );
                            }
                        }
                    }
                }

                // Return success if at least one provider succeeded
                // For wildcard operations, we can't return a single inverse operation
                // since multiple providers might have executed
                if success_count > 0 {
                    info!(
                        "[OperationDispatcher] Wildcard operation completed: {} succeeded, {} \
                         failed",
                        success_count, error_count
                    );
                    Ok(OperationResult::irreversible(Vec::new())) // Wildcard operations can't be undone as a single operation
                } else {
                    error!(
                        "[OperationDispatcher] Wildcard operation failed on all {} providers",
                        error_count
                    );
                    Err(format!(
                        "Wildcard operation '{}' failed on all {} providers",
                        op_name, error_count
                    )
                    .into())
                }
            } else {
                // Regular operation - route to specific provider
                let available_ops: Vec<_> = self
                    .all_providers()
                    .iter()
                    .flat_map(|p| p.operations())
                    .collect();
                let entity_name_str = entity_name.as_str();
                let matching_ops: Vec<_> = available_ops
                    .iter()
                    .filter(|op| op.entity_name == entity_name_str && op.name == op_name)
                    .collect();

                debug!(
                    "[OperationDispatcher] Found {} matching operations for entity={}, op={}",
                    matching_ops.len(),
                    entity_name,
                    op_name
                );

                // If no direct match, try inferring entity type from the `id` param's
                // URI scheme. Rows from matviews/views carry the view name as entity_name
                // (e.g. "focus_roots") but the actual entity provider is registered under
                // the scheme (e.g. "block" from "block:xxx").
                let resolved_entity: String;
                let resolved_entity_name: &str = if matching_ops.is_empty() {
                    let scheme = params.get("id").and_then(|v| match v {
                        holon_api::Value::String(s) => {
                            s.split_once(':').map(|(scheme, _)| scheme.to_string())
                        }
                        _ => None,
                    });

                    if let Some(scheme) = scheme {
                        let has_match = available_ops
                            .iter()
                            .any(|op| op.entity_name == scheme.as_str() && op.name == op_name);
                        if has_match {
                            info!(
                                "[OperationDispatcher] Entity '{}' not found, resolved to '{}' \
                                 via id scheme",
                                entity_name, scheme
                            );
                            resolved_entity = scheme;
                            resolved_entity.as_str()
                        } else {
                            entity_name_str
                        }
                    } else {
                        entity_name_str
                    }
                } else {
                    entity_name_str
                };
                tracing::Span::current().record("operation.resolved_entity", resolved_entity_name);

                // Intent boundary (Model.md invariant 3): parse the field of a
                // block `set_field` intent into the closed `BlockWriteField`
                // vocabulary. Order keys (`sort_key`) and storage-internal
                // fields are a loud Err here, in EVERY mode — they are minted /
                // written by the storage layer, never carried by intent. The
                // ordering authority's own writes don't pass through the
                // dispatcher (they call the SQL provider / CRUD seam directly),
                // so this rejects exactly the smuggling path.
                let mut params = params;
                if resolved_entity_name == "block" && op_name == "set_field" {
                    let field = params
                        .get("field")
                        .and_then(|v| v.as_string())
                        .ok_or("block set_field: missing 'field' parameter")?;
                    holon_api::BlockWriteField::parse(field)
                        .map_err(|e| format!("intent boundary: {e}"))?;
                }

                // Adopt inline org markup a human or agent JUST AUTHORED — the one
                // boundary where `[[Page]]` / `*bold*` in `content` becomes a stripped
                // label plus a mark set, so UI-authored text reaches storage in the
                // same shape ingest produces (marks populated, `block_links` junction
                // derived, backlinks live).
                //
                // Both write shapes are covered because a user reaches storage through
                // both: `set_field("content")` when editing an existing block, and
                // `create` when the creation slot commits a freshly typed line.
                //
                // Not reached by ingest at all: org ingest writes through the provider
                // seam directly (`SqlBlockOperations::create_in_tree` →
                // `execute_operation_with_origin`), and `split_block` goes through
                // `BlockOperations` — neither passes this dispatcher.
                //
                // The `marks` write is DERIVED and is decided further down, once the
                // CRUD-authority provider is resolved, by comparing the extracted marks
                // against the block's currently-stored marks (see
                // `content_marks_followup`). That comparison — not this extraction — is
                // what keeps the follow-up from firing spuriously.
                //
                // This EDIT arm is NOT gated on `input`, unlike the `create` arm
                // below, and undo replay is safe from it by SHAPE rather than by
                // origin: a content inverse carries the prior text and marks as one
                // `{text, marks}` Object (#22), and the `as_string()` match below
                // takes String values only, so a replayed inverse never enters this
                // arm and its restored bytes are never re-parsed. The rich write
                // restores both columns itself; nothing here has to clear marks for
                // it (`undo_link_add_restores_prior_pair`,
                // `undo_of_a_content_edit_restores_raw_previous_bytes`).
                let content_edit: Option<(String, String, Vec<holon_api::MarkSpan>)> =
                    if resolved_entity_name == "block"
                        && op_name == "set_field"
                        && params.get("field").and_then(|v| v.as_string()) == Some("content")
                    {
                        match params
                            .get("value")
                            .and_then(|v| v.as_string())
                            .map(str::to_string)
                        {
                            Some(raw) => {
                                let (label, marks) = holon_org_format::extract_inline_marks_with(
                                    &raw,
                                    &self.link_classifier,
                                );
                                let id = params
                                    .get("id")
                                    .and_then(|v| v.as_string())
                                    .ok_or("block set_field(content): missing 'id' parameter")?
                                    .to_string();
                                params.insert(
                                    "value".into(),
                                    holon_api::Value::String(label.clone()),
                                );
                                Some((id, label, marks))
                            }
                            None => None,
                        }
                    } else {
                        None
                    };

                // The create half of the same boundary: a block born from the creation
                // slot carries the typed line raw, so without this a typed `[[Page]]`
                // was stored verbatim with NULL marks and no junction row, and only the
                // NEXT boot's file re-ingest adopted it — rewriting the user's stored
                // text with no action of theirs.
                //
                // A caller that supplies its own `marks` already parsed and is left
                // alone (`instantiate_template` re-enters the engine carrying the
                // definition's spans). Extraction yielding NO marks likewise changes
                // nothing — which keeps a link with no representable label (`[[   ]]`)
                // as the author's bytes instead of erasing them, the same
                // empty-adoption rule `canonicalize_adopted_links` holds on the render
                // side. Marks ride along in the create params rather than a follow-up
                // write: the junction is derived from them in the provider's create arm.
                if input == AuthoredInput::Live
                    && resolved_entity_name == "block"
                    && op_name == "create"
                    && !params.contains_key("marks")
                    && let Some(raw) = params
                        .get("content")
                        .and_then(|v| v.as_string())
                        .map(str::to_string)
                {
                    let (label, marks) =
                        holon_org_format::extract_inline_marks_with(&raw, &self.link_classifier);
                    if !marks.is_empty() {
                        params.insert("content".into(), holon_api::Value::String(label));
                        params.insert(
                            "marks".into(),
                            holon_api::Value::String(holon_api::marks_to_json(&marks)),
                        );
                    }
                }

                if !available_ops
                    .iter()
                    .any(|op| op.entity_name == resolved_entity_name && op.name == op_name)
                {
                    let entity_names: std::collections::HashSet<_> =
                        available_ops.iter().map(|op| &op.entity_name).collect();
                    error!(
                        "[OperationDispatcher] No provider registered for entity: '{}' \
                         (operation: '{}'). Available entities: {:?}",
                        entity_name, op_name, entity_names
                    );
                    return Err(
                        match self.unavailable_entities.reason_for(resolved_entity_name) {
                            Some(reason) => format!(
                                "Entity '{entity_name}' is unavailable in this session: {reason}"
                            )
                            .into(),
                            None => format!(
                                "No provider registered for entity: {entity_name} (operation: \
                             '{op_name}')"
                            )
                            .into(),
                        },
                    );
                }

                let provider = self
                    .all_providers()
                    .into_iter()
                    .find(|provider| {
                        provider
                            .operations()
                            .iter()
                            .any(|op| op.entity_name == resolved_entity_name && op.name == op_name)
                    })
                    .ok_or_else(|| format!("No provider registered for entity: {}", entity_name))?;

                // ADR 0028 C3 — THE boundary/authz seam, before the provider runs
                // and before any I/O.
                self.enforce_boundary(&available_ops, resolved_entity_name, op_name, &params)?;

                // ADR 0031 Increment 3 — THE declared-guard seam, current-state
                // (ruling G1=A), likewise before the provider runs.
                self.enforce_guard(&available_ops, resolved_entity_name, op_name, &params)
                    .await?;

                // ADR 0032 §3 — THE net gate: is the marking this operation
                // would produce legal.
                self.enforce_net_guard(resolved_entity_name, op_name, &params)
                    .await?;

                info!(
                    "[OperationDispatcher] Routing operation to provider: entity={}, op={}",
                    resolved_entity_name, op_name
                );

                // Clone params before execution for observer notification
                // (Operation is the String-keyed serde surface; re-key here).
                let params_for_observer: std::collections::HashMap<String, holon_api::Value> =
                    params
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect();
                let resolved_entity_name_typed = EntityName::new(resolved_entity_name);

                // Links increment 3 — decide the DERIVED `marks` write for a
                // content edit, BEFORE the content write lands (so we read the
                // block's PRIOR stored state, not the value we are about to write).
                //
                // Contract (marks = truth, per links-ruling): fire the follow-up
                // EXACTLY when the marks extracted from the new content differ from
                // the block's currently-stored marks. It must NOT fire when the
                // content commit carried no mark-relevant change, and must NEVER
                // null a block's marks merely because the editor re-committed the
                // already-stripped label without re-supplying markup.
                //
                // The over-dispatch bug (BugFunnel #66) fired a `marks = Null`
                // follow-up on EVERY block `set_field("content")`. On the editor's
                // blur/refocus re-commit — which sends back the stripped label with
                // NO `[[…]]` syntax (SqlOnly hydrates `content` from the matview) —
                // that nulled the real marks, replacing a live `[[link]]` with plain
                // text (the LIVE bug). It also spuriously doubled the dispatch count
                // on plain-text edits, inflating the undo replay tally.
                //
                // - Readable provider (SQL CRUD authority): compare against ground truth. Skip
                //   when the mark set is unchanged, and skip a null-producing re-commit whose
                //   stripped label already equals the stored content (the blur path). Otherwise
                //   dispatch — including a legitimate `marks = Null` when an edit genuinely
                //   REMOVED the link (new label differs from the stored content).
                // - Unreadable provider (Loro CRUD authority, test stubs): fail safe — dispatch
                //   only when the new content actually yields marks (a link was typed); never
                //   null on an unknown prior state.
                let content_marks_followup: Option<(String, holon_api::Value)> =
                    if let Some((id, label, extracted)) = content_edit {
                        let marks_value = |marks: &[holon_api::MarkSpan]| {
                            if marks.is_empty() {
                                holon_api::Value::Null
                            } else {
                                holon_api::Value::String(holon_api::marks_to_json(marks))
                            }
                        };
                        match provider.read_block_content_marks(&id).await? {
                            Some((stored_content, stored_marks_value)) => {
                                let stored_marks: Vec<holon_api::MarkSpan> =
                                    match &stored_marks_value {
                                        holon_api::Value::String(s) if !s.is_empty() => {
                                            holon_api::marks_from_json(s).map_err(|e| {
                                                format!(
                                                    "links increment 3: stored marks JSON for \
                                                     {id} is corrupt: {e}"
                                                )
                                            })?
                                        }
                                        _ => Vec::new(),
                                    };
                                // Two independent skip-reasons (mark set unchanged; blur
                                // re-commit with no marks and unchanged label) that both
                                // resolve to `None` — kept separate, not merged, so each
                                // guard stays legible against the comment above.
                                #[allow(clippy::if_same_then_else)]
                                if extracted == stored_marks {
                                    None
                                } else if extracted.is_empty() && label == stored_content {
                                    None
                                } else {
                                    Some((id, marks_value(&extracted)))
                                }
                            }
                            None => {
                                if extracted.is_empty() {
                                    None
                                } else {
                                    Some((id, marks_value(&extracted)))
                                }
                            }
                        }
                    } else {
                        None
                    };

                // Execute operation and get result with changes and undo action
                let mut operation_result = provider
                    .execute_operation(&resolved_entity_name_typed, op_name, params)
                    .await?;

                // Links increment 3 — the marks write derived from a content edit.
                // Routed straight through the same provider (not re-entering this
                // dispatcher): marks are a DERIVED consequence of the content edit,
                // so they must not spawn a second observer notification or a
                // separate undo entry (one user edit = one undoable content step).
                // In Loro mode this lands via `update_block_marked` (Peritext) and
                // the outbound projector carries `marks` to SQL, deriving the
                // junction in the `update` arm; in SqlOnly mode it hits
                // `set_field("marks")` directly, deriving the junction there.
                if let Some((id, marks_value)) = content_marks_followup {
                    let mut marks_params = StorageEntity::new();
                    marks_params.insert("id".into(), holon_api::Value::String(id));
                    marks_params.insert("field".into(), holon_api::Value::String("marks".into()));
                    marks_params.insert("value".into(), marks_value);
                    provider
                        .execute_operation(&resolved_entity_name_typed, "set_field", marks_params)
                        .await
                        .map_err(|e| {
                            format!("links increment 3: marks write after content edit failed: {e}")
                        })?;
                }
                // Set entity_name on the inverse operation if present
                operation_result.undo = match operation_result.undo {
                    UndoAction::Undo(mut op) => {
                        op.entity_name = resolved_entity_name_typed.clone();
                        UndoAction::Undo(op)
                    }
                    other => other,
                };

                match &operation_result.undo {
                    UndoAction::Undo(_) => {
                        info!(
                            "[OperationDispatcher] Provider execution succeeded: entity={}, op={} \
                             (inverse operation available)",
                            entity_name, op_name
                        );
                    }
                    UndoAction::DeclaredIrreversible(reason) => {
                        info!(
                            "[OperationDispatcher] Provider execution succeeded: entity={}, op={} \
                             (no inverse: {reason})",
                            entity_name, op_name
                        );
                    }
                    UndoAction::Undeclared => {
                        info!(
                            "[OperationDispatcher] Provider execution succeeded: entity={}, op={} \
                             (undo UNDECLARED — engine will reject)",
                            entity_name, op_name
                        );
                    }
                }

                // Notify observers of successful execution
                let executed_operation = Operation::new(
                    resolved_entity_name,
                    op_name,
                    "",
                    params_for_observer
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                );
                if origin.is_user() {
                    self.notify_observers(
                        resolved_entity_name,
                        &executed_operation,
                        &operation_result.undo,
                    )
                    .await;
                }

                // Execute follow-up operations (e.g., editor_focus after split_block).
                for follow_up in std::mem::take(&mut operation_result.follow_ups) {
                    let fu_entity = follow_up.entity_name.clone();
                    let fu_op = follow_up.op_name.clone();
                    info!(
                        "[OperationDispatcher] Executing follow-up: entity={}, op={}",
                        fu_entity, fu_op
                    );
                    self.execute_operation(
                        &fu_entity,
                        &fu_op,
                        follow_up
                            .params
                            .into_iter()
                            .map(|(k, v)| (Arc::from(k.as_str()), v))
                            .collect(),
                    )
                    .await
                    .map_err(|e| format!("Follow-up {fu_entity}.{fu_op} failed: {e}"))?;
                }

                Ok(operation_result)
            }
        }
        .instrument(span)
        .await
    }
}

/// Structural block ops that are knowingly double-advertised under Loro
/// authority (SqlBlockOperations + LoroBlockOperations). A SEPARATE
/// pre-existing duplicate from BugFunnel N1's CRUD dup; tolerated by the
/// registry-uniqueness assertion until the structural-op authority/routing
/// question is resolved.
#[cfg(debug_assertions)]
const STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST: &[&str] = &[
    "indent",
    "outdent",
    "move_block",
    "move_to_position",
    "move_up",
    "move_down",
    "split_block",
    "join_block",
    "restore_split",
    "restore_join",
    "embed_entity",
    "delete_subtree",
    "delete_keep_children",
];

/// Return the `entity::op` keys advertised more than once across `ops`.
///
/// Pure helper for the fail-loud registry-uniqueness invariant in
/// [`OperationDispatcher::operations`]. Empty result == the invariant holds.
/// Keyed on `(entity_name, name)` so per-provider `sync` ops (each carries a
/// distinct `"<provider>.sync"` entity_name) never false-positive.
fn duplicate_operations(ops: &[OperationDescriptor]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut dups = Vec::new();
    for op in ops {
        if !seen.insert((op.entity_name.as_str(), op.name.as_str())) {
            dups.push(format!("{}::{}", op.entity_name, op.name));
        }
    }
    dups
}

#[async_trait]
impl OperationProvider for OperationDispatcher {
    /// Get all operations from all registered providers
    ///
    /// Aggregates operations from all providers and includes wildcard
    /// operations.
    fn operations(&self) -> Vec<OperationDescriptor> {
        let mut ops: Vec<OperationDescriptor> = self
            .all_providers()
            .iter()
            .flat_map(|provider| provider.operations())
            .collect();

        // Add wildcard sync operation if any provider has a "sync" operation
        let has_sync_ops = ops.iter().any(|op| op.name == "sync");
        if has_sync_ops {
            ops.push(OperationDescriptor {
                entity_name: "*".into(),
                entity_short_name: "all".to_string(),
                id_column: String::new(),
                name: "sync".to_string(),
                display_name: "Sync".to_string(),
                description: "Sync registered syncable providers".to_string(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Global,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::External,
                },
                trigger: None,
                bound_params: Default::default(),
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            });

            // Add wildcard full_sync operation (clear caches + sync)
            // This is triggered by Ctrl+clicking the sync button in the UI
            ops.push(OperationDescriptor {
                entity_name: "*".into(),
                entity_short_name: "all".to_string(),
                id_column: String::new(),
                name: "full_sync".to_string(),
                display_name: "Full Sync".to_string(),
                description: "Clear all caches, reset sync tokens, and re-sync from external \
                              systems"
                    .to_string(),
                required_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Global,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::External,
                },
                trigger: None,
                bound_params: Default::default(),
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            });
        }

        // Registry-uniqueness invariant (fail-loud, debug/test builds): no two
        // providers may advertise the same (entity, op) EXCEPT the known,
        // pre-existing structural-block-op overlap. The registry unions provider
        // `operations()` WITHOUT dedup and dispatch is first-registered-wins, so
        // a stray duplicate leaks a second identical slash-menu entry (BugFunnel
        // N1 — 12 block CRUD ops listed twice in SqlOnly). This fix removes the
        // N1 CRUD duplicate at its source (holon_core::OperationSubset in
        // holon-app turso_seams); the assertion guards against it regressing.
        //
        // The STRUCTURAL block ops (indent/outdent/split/join/move…) are ALSO
        // double-advertised under Loro authority (SqlBlockOperations +
        // LoroBlockOperations), a SEPARATE pre-existing dup surfaced by the
        // keystone. Removing it is a structural-op authority/routing decision
        // out of this fix's scope; it is explicitly tolerated here (named
        // allowlist) so the guard stays loud for every OTHER duplicate.
        #[cfg(debug_assertions)]
        {
            let unexpected: Vec<String> = duplicate_operations(&ops)
                .into_iter()
                .filter(|d| {
                    !STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST
                        .contains(&d.strip_prefix("block::").unwrap_or(d))
                })
                .collect();
            assert!(
                unexpected.is_empty(),
                "duplicate operation registrations (two providers advertise the same op — narrow \
                 the redundant provider, see holon_core::OperationSubset): {unexpected:?}"
            );
        }

        ops
    }

    /// Find operations that can be executed with given arguments
    ///
    /// Filters operations based on entity_name and available_args.
    ///
    /// Special handling for generic operations:
    /// - `set_field`: Only requires "id" to be available (field and value are
    ///   runtime parameters)
    /// - Other operations: Require all parameters to be in available_args
    fn find_operations(
        &self,
        entity_name: &EntityName,
        available_args: &[String],
    ) -> Vec<OperationDescriptor> {
        // Filter operations from all providers
        self.operations()
            .into_iter()
            .filter(|op| {
                if op.entity_name != *entity_name {
                    return false;
                }

                // Special case: set_field is a generic operation that can update any field
                // It only needs "id" from the query columns; "field" and "value" are runtime
                // parameters
                if op.name == "set_field" {
                    // Only require "id" to be available
                    return op
                        .required_params
                        .iter()
                        .any(|p| p.name == "id" && available_args.contains(&p.name));
                }

                // For other operations, a param is considered available if:
                // 1. It's directly in available_args, OR
                // 2. It has a param_mapping that can provide it at runtime
                op.required_params.iter().all(|p| {
                    // Direct availability
                    if available_args.contains(&p.name) {
                        return true;
                    }
                    // Can be provided via param_mapping at runtime
                    op.param_mappings
                        .iter()
                        .any(|m| m.provides.contains(&p.name))
                })
            })
            .collect()
    }

    /// Route an operation to the correct provider, treating the params as
    /// VERBATIM: whatever bytes arrive are the bytes written.
    ///
    /// This is the identity-preserving entry point, and it is the one undo/redo
    /// replay uses (`OperationEngine::replay`) — so a replayed inverse can
    /// never be re-parsed into something other than what it restores. A
    /// caller that knows its params carry freshly authored text asks for
    /// adoption explicitly via
    /// [`OperationDispatcher::execute_operation_with_input`].
    ///
    /// # Errors
    /// Returns an error if no provider is registered for `entity_name` (or a
    /// wildcard matches no providers), or if the provider's own
    /// `execute_operation` fails.
    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        self.execute_operation_with_input(entity_name, op_name, params, AuthoredInput::Verbatim)
            .await
    }
}

pub struct OperationModule;

impl Module for OperationModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        injector.provide::<OperationDispatcher>(Provider::root_async(|r| async move {
            let providers = r
                .try_resolve_all_async::<dyn OperationProvider>()
                .await
                .expect("Failed to get all operation providers");
            info!(
                "[OperationModule] Found {} operation providers",
                providers.len()
            );
            let observers = r
                .try_resolve_all_async::<dyn OperationObserver>()
                .await
                .unwrap_or_else(|_| vec![]);
            info!(
                "[OperationModule] Found {} operation observers",
                observers.len()
            );

            let sync_token_store = r.optional_resolve_async::<dyn SyncTokenStore>().await;
            if sync_token_store.is_some() {
                info!("[OperationModule] SyncTokenStore configured for full_sync support");
            }

            let db_handle_provider = r.resolve::<dyn crate::di::DbHandleProvider>();
            let ddl_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
            let matview_mgr = Arc::new(crate::sync::MatviewManager::new(
                db_handle_provider.handle(),
                ddl_mutex,
            ));
            let mut dispatcher = OperationDispatcher::with_observers(providers, observers);
            if let Some(store) = sync_token_store {
                dispatcher.set_sync_token_store(store);
            }
            dispatcher.set_matview_manager(matview_mgr);
            dispatcher.set_guard_world(Arc::new(crate::api::guard_world::SqlGuardWorld::new(
                db_handle_provider.handle(),
            )));
            dispatcher.set_link_classifier(
                r.resolve_async::<holon_profiles::TypeRegistry>()
                    .await
                    .link_target_classifier(),
            );

            // ADR 0028 C3 — install the boundary/authz seam. The concrete
            // policy overlay lives in `holon-sharing`, so a composition root
            // that has policies registers it and this crate never learns the
            // sharing domain. With none registered we fall back to the inert
            // enforcer, which is exactly what prod installed before this seam
            // existed (`PolicyOverlayEnforcer::inert()`: an empty PolicySet
            // whose `check` returns `Ok(())` on the first line).
            let enforcer = r
                .optional_resolve_async::<dyn BoundaryEnforcer>()
                .await
                .unwrap_or_else(|| {
                    Arc::new(holon_core::InertBoundaryEnforcer) as Arc<dyn BoundaryEnforcer>
                });
            dispatcher.set_boundary_enforcer(enforcer);

            // ADR 0032 §3 — install the net gate. The placement policy needs
            // capability profiles and a document-home authority, neither of
            // which this crate links, so a composition root that has them
            // registers one and a container without them gets the inert guard.
            let net_guard = r
                .optional_resolve_async::<dyn crate::api::net_guard::NetGuard>()
                .await
                .unwrap_or_else(|| {
                    Arc::new(crate::api::net_guard::InertNetGuard)
                        as Arc<dyn crate::api::net_guard::NetGuard>
                });
            dispatcher.set_net_guard(net_guard);

            // A container that switched an entity off says which setting did
            // it; one that registers nothing keeps the plain not-found answer.
            if let Some(unavailable) = r.optional_resolve_async::<UnavailableEntities>().await {
                dispatcher.set_unavailable_entities((*unavailable).clone());
            }

            // Every free-standing type the registry carries gets a write
            // authority derived from ITS definition. `FreeStandingTypeViews`
            // creates the type's Turso serialization; without this the type
            // would be queryable but every write to it would find "No provider
            // registered for entity: <type>" — the exact gap the block-shaped
            // write path used to hide by being the only authority there is.
            let type_registry = r.resolve_async::<holon_profiles::TypeRegistry>().await;
            for type_def in type_registry.all() {
                if !crate::di::schema_providers::is_free_standing(&type_def) {
                    continue;
                }
                // A type that mirrors a connector's data is written by that
                // connector, not by a provider derived from its columns. Its
                // authority is already registered whenever the connector
                // declares tools for the entity; deriving a second one over the
                // mirror table would both be refused here and, if it won a
                // routing scan, write where the system of record cannot see it.
                //
                // TODO(bugfunnel:2026-08-23-todoist-projects-second-write-authority-boot-panic,
                // "Adjacent hazards"): a sidecar with an `entity_prefix` names
                // its type `{prefix}{entity}` while its tool descriptors name
                // the bare entity, so this check does not recognise the pair
                // and the prefixed mirror keeps a derived SQL authority no
                // connector serves.
                let entity = EntityName::new(type_def.name.clone());
                if let Some(provider) = type_def.owning_integration()
                    && dispatcher.has_provider(entity.as_str())
                {
                    info!(
                        "[OperationModule] '{}' mirrors integration '{provider}', which already \
                         holds its write authority — deriving none",
                        type_def.name
                    );
                    continue;
                }
                crate::core::type_declaration::register_write_authority(
                    &type_def,
                    &db_handle_provider.handle(),
                    &dispatcher,
                )
                .unwrap_or_else(|e| {
                    panic!(
                        "[OperationModule] write authority for free-standing type '{}': {e}",
                        type_def.name
                    )
                });
            }

            // The pantry's `consume` sits beside the derived authority rather
            // than replacing it: the generic create/set_field/delete stay the
            // write path, and consume adds only the read-modify-write the
            // pantry needs to refuse going negative.
            if dispatcher.has_provider("pantry_item") {
                let pantry: Arc<dyn OperationProvider> =
                    Arc::new(crate::core::pantry_operations::PantryOperations::new(
                        db_handle_provider.handle(),
                    ));
                dispatcher.register_provider(pantry).unwrap_or_else(|e| {
                    panic!("[OperationModule] registering the pantry consume authority: {e}")
                });
            }

            // Fail loud if a block pipeline is wired without its content-write
            // ops (the EventInfraModule-only trap). A silent "No provider" drop
            // of every create/set_field/delete is worse than a startup crash.
            dispatcher
                .assert_content_write_capability()
                .expect("[OperationModule] operation-registry startup check failed");
            dispatcher
                .assert_boundary_seam_installed()
                .expect("[OperationModule] boundary-seam startup check failed");
            dispatcher
                .assert_net_guard_installed()
                .expect("[OperationModule] net-gate startup check failed");
            // Every in-tree descriptor's arcs already passed the macro's
            // compile-time parse; this is the gate for the ones that did not —
            // a descriptor deserialized from a sidecar or a created entity
            // type. `BuiltinSchemas` is the source today because no runtime
            // entity type registers operations yet; a composition site that
            // adds one passes it here alongside the built-ins.
            dispatcher
                .assert_declared_arcs_match_schema(&BuiltinSchemas)
                .expect("[OperationModule] arc-schema startup check failed");

            Shared::new(dispatcher)
        }));

        // The door vault ingest uses for the declared-type rows a file format
        // derives beside its blocks. It routes through the dispatcher above, so
        // those tables keep exactly one writer.
        injector.provide::<dyn holon_core::file_format::TypedRowSink>(Provider::root_async(
            |r| async move {
                Arc::new(crate::core::typed_row_sink::DispatchingTypedRowSink::new(r))
                    as Arc<dyn holon_core::file_format::TypedRowSink>
            },
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use self::super::*;

    // Mock OperationProvider for testing
    struct MockProvider {
        entity_name: String,
        operations_list: Vec<OperationDescriptor>,
    }

    #[async_trait]
    impl OperationProvider for MockProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            self.operations_list.clone()
        }

        async fn execute_operation(
            &self,
            entity_name: &EntityName,
            op_name: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            if entity_name != self.entity_name.as_str() {
                return Err(format!(
                    "Entity mismatch: expected {}, got {}",
                    self.entity_name, entity_name
                )
                .into());
            }
            if matches!(op_name, "test_op" | "set_field") {
                Ok(OperationResult::irreversible(Vec::new()))
            } else {
                Err(format!("Unknown operation: {}", op_name).into())
            }
        }
    }

    fn create_test_operation(entity_name: &str, op_name: &str) -> OperationDescriptor {
        OperationDescriptor {
            entity_name: entity_name.into(),
            entity_short_name: entity_name.to_string(),
            id_column: "id".to_string(),
            name: op_name.to_string(),
            display_name: format!("Test {}", op_name),
            description: format!("Test operation {}", op_name),
            required_params: vec![],
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        }
    }

    #[tokio::test]
    async fn test_provider_registration() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![create_test_operation("entity1", "op1")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1]);
        assert!(dispatcher.has_provider("entity1"));
        assert_eq!(dispatcher.provider_count(), 1);
    }

    #[test]
    fn duplicate_operations_detects_cross_provider_overlap() {
        // Unique across providers → no duplicates.
        let unique = vec![
            create_test_operation("block", "create"),
            create_test_operation("block", "delete"),
            create_test_operation("doc", "create"),
        ];
        assert!(duplicate_operations(&unique).is_empty());

        // Same (entity, op) advertised twice (the N1 shape) → flagged loud.
        let dup = vec![
            create_test_operation("block", "create"),
            create_test_operation("block", "delete"),
            create_test_operation("block", "create"),
        ];
        assert_eq!(
            duplicate_operations(&dup),
            vec!["block::create".to_string()]
        );
    }

    #[tokio::test]
    #[should_panic(expected = "duplicate operation registrations")]
    async fn operations_invariant_fires_loud_on_duplicate_registration() {
        // Two providers advertising the SAME (entity, op) — the exact N1
        // double-registration shape. `operations()` must fail LOUD (debug
        // build), not silently union the duplicate into the menu.
        let p1 = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "create")],
        });
        let p2 = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "create")],
        });
        let dispatcher = OperationDispatcher::new(vec![p1, p2]);
        let _ = dispatcher.operations();
    }

    #[tokio::test]
    async fn a_runtime_registered_provider_becomes_routable_and_writable() {
        let dispatcher = OperationDispatcher::new(vec![]);
        assert!(!dispatcher.has_provider("gen_1"));

        dispatcher
            .register_provider(Arc::new(MockProvider {
                entity_name: "gen_1".to_string(),
                operations_list: vec![
                    create_test_operation("gen_1", "create"),
                    create_test_operation("gen_1", "set_field"),
                    create_test_operation("gen_1", "delete"),
                ],
            }))
            .expect("first authority for gen_1");

        // Looked up by the RAW type name, as a declaration site spells it: the
        // dispatcher canonicalizes, so the `_`→`-` fold cannot make a type look
        // unregistered.
        assert!(dispatcher.has_provider("gen_1"));
        dispatcher
            .assert_write_capability_for("gen_1")
            .expect("a runtime-registered CRUD provider makes its entity writable");

        // A second authority for the same entity would make routing pick
        // whichever the scan reaches first.
        let second = dispatcher.register_provider(Arc::new(MockProvider {
            entity_name: "gen_1".to_string(),
            operations_list: vec![create_test_operation("gen_1", "create")],
        }));
        assert!(second.is_err(), "a duplicate authority must be refused");
    }

    /// The duplicate-authority refusal must not promise a recovery path that
    /// does not exist. Nothing removes a declared authority, so an error
    /// telling the reader to tear the type down and retry would send them
    /// round a loop that cannot terminate.
    ///
    /// This test pins the wording only. The BEHAVIOUR it describes — that not
    /// even a teardown frees the name — is pinned by
    /// `core::type_declaration::tests::a_declared_type_cannot_be_redeclared_even_after_teardown`,
    /// which is where the migrate primitive rewrites the contract.
    #[tokio::test]
    async fn the_duplicate_authority_error_does_not_promise_a_recovery_path() {
        let dispatcher = OperationDispatcher::new(vec![]);
        let authority = || {
            Arc::new(MockProvider {
                entity_name: "gen_1".to_string(),
                operations_list: vec![create_test_operation("gen_1", "create")],
            })
        };
        dispatcher
            .register_provider(authority())
            .expect("first authority for gen_1");

        let msg = dispatcher
            .register_provider(authority())
            .expect_err("a duplicate authority must be refused")
            .to_string();

        assert!(
            msg.contains("NOT SUPPORTED in this increment") && msg.contains("append-only"),
            "the error must say re-declaration is unsupported and why; got: {msg}"
        );
        assert!(
            msg.contains("OQ-5"),
            "the error must name what retires the restriction; got: {msg}"
        );
        assert!(
            !msg.contains("Tear the type down"),
            "teardown drops SQL artifacts only — it never frees the name, so the error must \
             not send the reader round a loop that cannot terminate; got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_operations_aggregation() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![
                create_test_operation("entity1", "op1"),
                create_test_operation("entity1", "op2"),
            ],
        });

        let provider2 = Arc::new(MockProvider {
            entity_name: "entity2".to_string(),
            operations_list: vec![create_test_operation("entity2", "op3")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1, provider2]);

        let all_ops = dispatcher.operations();
        assert_eq!(all_ops.len(), 3);
        assert!(all_ops.iter().any(|op| op.name == "op1"));
        assert!(all_ops.iter().any(|op| op.name == "op2"));
        assert!(all_ops.iter().any(|op| op.name == "op3"));
    }

    #[tokio::test]
    async fn test_execute_operation_routing() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![create_test_operation("entity1", "test_op")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1]);

        // Execute operation on registered entity
        let params = StorageEntity::new();
        let result = dispatcher
            .execute_operation(&EntityName::new("entity1"), "test_op", params)
            .await;
        assert!(result.is_ok());

        // Try to execute on unregistered entity
        let params = StorageEntity::new();
        let result = dispatcher
            .execute_operation(&EntityName::new("entity2"), "test_op", params)
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No provider registered")
        );
    }

    /// Reproduces the `EventInfraModule`-only wiring at the dispatcher level:
    /// a `block` provider advertising ONLY structural ops (what
    /// `SqlBlockOperations` registers) and no CRUD provider. The startup guard
    /// must reject it loudly, naming the missing content-write ops.
    #[tokio::test]
    async fn content_write_guard_rejects_structural_only_block_pipeline() {
        let structural_only = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![
                create_test_operation("block", "indent"),
                create_test_operation("block", "outdent"),
                create_test_operation("block", "split_block"),
                create_test_operation("block", "move_up"),
            ],
        });
        let dispatcher = OperationDispatcher::new(vec![structural_only]);

        let err = dispatcher
            .assert_content_write_capability()
            .expect_err("structural-only block pipeline must fail the content-write guard");
        let msg = err.to_string();
        assert!(
            msg.contains("create"),
            "message names missing create: {msg}"
        );
        assert!(
            msg.contains("set_field"),
            "message names missing set_field: {msg}"
        );
        assert!(
            msg.contains("delete"),
            "message names missing delete: {msg}"
        );
        assert!(
            msg.contains("EventInfraModule"),
            "message points at the culprit module: {msg}"
        );
    }

    /// A block pipeline that DOES advertise the CRUD triple (the fixed wiring:
    /// EventInfraModule + a `SqlOperationProvider`, or Loro authority) passes.
    #[tokio::test]
    async fn content_write_guard_accepts_full_block_pipeline() {
        let structural = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "split_block")],
        });
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![
                create_test_operation("block", "create"),
                create_test_operation("block", "set_field"),
                create_test_operation("block", "delete"),
            ],
        });
        let dispatcher = OperationDispatcher::new(vec![structural, crud]);
        dispatcher
            .assert_content_write_capability()
            .expect("full block pipeline must pass the content-write guard");
    }

    /// A backend with no `block` provider at all (nav-only / read-only) never
    /// dispatches block writes, so the guard is a no-op.
    #[tokio::test]
    async fn content_write_guard_ignores_backend_without_block_provider() {
        let nav = Arc::new(MockProvider {
            entity_name: "navigation".to_string(),
            operations_list: vec![create_test_operation("navigation", "navigate")],
        });
        let dispatcher = OperationDispatcher::new(vec![nav]);
        dispatcher
            .assert_content_write_capability()
            .expect("no block pipeline => guard is a no-op");
    }

    /// Model.md invariant 3 at the intent boundary: a block `set_field`
    /// carrying an order key is rejected by the dispatcher itself, before
    /// any provider runs — mode-independent (SqlOnly's raw-SQL provider
    /// never sees it either).
    #[tokio::test]
    async fn block_set_field_rejects_order_keys_at_intent_boundary() {
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "set_field")],
        });
        let dispatcher = OperationDispatcher::new(vec![crud]);

        for order_key_field in ["sort_key", "after_block_id"] {
            let mut params = StorageEntity::new();
            params.insert("id".into(), holon_api::Value::String("block:a".into()));
            params.insert(
                "field".into(),
                holon_api::Value::String(order_key_field.into()),
            );
            params.insert("value".into(), holon_api::Value::String("A5".into()));
            let err = dispatcher
                .execute_operation(&EntityName::new("block"), "set_field", params)
                .await
                .expect_err("set_field over an order key must be rejected at the boundary");
            let msg = err.to_string();
            assert!(
                msg.contains("order key"),
                "rejection must name the invariant, got: {msg}"
            );
            assert!(
                msg.contains(order_key_field),
                "rejection must name the offending field, got: {msg}"
            );
        }
    }

    /// Storage-internal fields (`depth`, `_expected_*` watermarks, …) are
    /// equally not intent vocabulary — writable only by the storage layer's
    /// own direct calls, which bypass the dispatcher.
    #[tokio::test]
    async fn block_set_field_rejects_storage_internal_fields() {
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "set_field")],
        });
        let dispatcher = OperationDispatcher::new(vec![crud]);

        let mut params = StorageEntity::new();
        params.insert("id".into(), holon_api::Value::String("block:a".into()));
        params.insert("field".into(), holon_api::Value::String("depth".into()));
        params.insert("value".into(), holon_api::Value::Integer(3));
        let err = dispatcher
            .execute_operation(&EntityName::new("block"), "set_field", params)
            .await
            .expect_err("set_field(depth) must be rejected at the boundary");
        assert!(err.to_string().contains("storage bookkeeping"), "{err}");
    }

    /// A normal field write passes the boundary and reaches the provider.
    #[tokio::test]
    async fn block_set_field_allows_intent_vocabulary_fields() {
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "set_field")],
        });
        let dispatcher = OperationDispatcher::new(vec![crud]);

        for field in ["content", "task_state", "DEADLINE"] {
            let mut params = StorageEntity::new();
            params.insert("id".into(), holon_api::Value::String("block:a".into()));
            params.insert("field".into(), holon_api::Value::String(field.into()));
            params.insert("value".into(), holon_api::Value::String("v".into()));
            dispatcher
                .execute_operation(&EntityName::new("block"), "set_field", params)
                .await
                .unwrap_or_else(|e| panic!("set_field({field}) must pass the boundary: {e}"));
        }
    }

    #[tokio::test]
    async fn test_registered_entities() {
        let provider1 = Arc::new(MockProvider {
            entity_name: "entity1".to_string(),
            operations_list: vec![create_test_operation("entity1", "op1")],
        });
        let provider2 = Arc::new(MockProvider {
            entity_name: "entity2".to_string(),
            operations_list: vec![create_test_operation("entity2", "op2")],
        });

        let dispatcher = OperationDispatcher::new(vec![provider1, provider2]);

        let entities = dispatcher.registered_entities();
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&EntityName::new("entity1")));
        assert!(entities.contains(&EntityName::new("entity2")));
    }
}
