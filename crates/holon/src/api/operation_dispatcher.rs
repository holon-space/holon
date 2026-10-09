//! OperationDispatcher - Composite pattern implementation for operation routing
//!
//! The OperationDispatcher aggregates multiple OperationProvider instances and
//! routes operation execution to the correct provider based on entity_name.
//!
//! This implements the Composite Pattern - both individual caches
//! (QueryableCache<T>) and the dispatcher implement OperationProvider, allowing
//! recursive composition.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
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
use tracing::warn;

use crate::api::guard_world::GuardQuery;

/// The param keys an operation names its subject in, and so the keys every
/// subject-bound gate binds to: the ADR 0028 boundary seam and the write tier
/// judge both, the ADR 0031 guard binds to `id`.
///
/// A gate that learns a new key must add it here, because the wildcard arm
/// refuses exactly these keys — `*` names no relation, so a subject arriving
/// under it would be judged against the descriptor of whichever provider
/// advertises the broadcast.
pub const SUBJECT_PARAM_KEYS: [&str; 2] = ["id", "parent_id"];

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
    /// Every provider's descriptors, collected when a provider is added:
    /// admission reads them for each op it orders, and the profile resolver
    /// follows it so rendered rows offer what dispatch accepts. A provider's
    /// `operations()` is fixed once it is registered.
    op_catalog: futures_signals::signal::Mutable<Arc<Vec<OperationDescriptor>>>,
    observers: Vec<Arc<dyn OperationObserver>>,
    sync_token_store: Option<Arc<dyn SyncTokenStore>>,
    view_rebuild: Option<ViewRebuild>,
    boundary_enforcer: Option<Arc<dyn BoundaryEnforcer>>,
    /// ADR 0031 Increment 3 — the world declared `#[require]` guards are
    /// evaluated against. Absent only in composition sites with no projection.
    guard_world: Option<Arc<dyn crate::api::guard_world::GuardWorld>>,
    /// ADR 0032 §3 — the marking legality of an operation's whole delta.
    net_guard: Option<Arc<dyn crate::api::net_guard::NetGuard>>,
    /// Whether the file behind a block's document accepts writes at all.
    write_tier: Option<Arc<dyn holon_core::WriteTierAuthority>>,
    /// Whether a block write leaves every tagged block in its shape.
    shape_gate: Option<ShapeGate>,
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
    /// Which integration owns an entity's table, and how far it got. An
    /// integration entity has no provider until its integration connects.
    integration_attribution: holon_core::integration_attribution::IntegrationAttribution,
}

/// What the shape gate (Model.md invariant 17) judges with: the registered
/// validators, the store they read the pre-write state from, and the bus a
/// user's refused edit is disclosed on.
struct ShapeGate {
    validators: Arc<holon_core::ShapeValidators>,
    authority: Arc<dyn holon_core::WriteAuthorityReads>,
    bus: Arc<holon_api::ConditionBus>,
}

/// One op of an admitted plan that has not been dispatched yet: its name and
/// every param but the engine's own stamps, sorted by key. A dispatch rides
/// the admission only when it IS that op.
type JudgedOp = (String, Vec<(String, holon_api::Value)>);

tokio::task_local! {
    /// The ops of the plan this task is executing that the shape gate already
    /// judged as part of the whole plan. Each is judged once: a dispatch that
    /// matches one consumes it.
    static JUDGED_PLAN: std::cell::RefCell<Vec<JudgedOp>>;
    /// Set while the task dispatches the editor's keystroke writes.
    static KEYSTROKE: ();
}

/// How often a judged write may find its shape claim grown at re-judgement.
const MAX_SHAPE_CLAIMS: usize = 3;

/// The test hold after a judged op's judgement, inside its claims.
async fn judged_checkpoint(op: &str) -> Result<()> {
    #[cfg(feature = "dispatch-hold")]
    if let Some(write) = crate::api::running_write::RunningWrite::current() {
        write.judged_checkpoint("block", op).await?;
    }
    #[cfg(not(feature = "dispatch-hold"))]
    let _ = op;
    Ok(())
}

/// A user's gesture that leaves or would leave a broken shape is disclosed;
/// an agent or a rule gets the error alone.
fn disclose_shape(
    gate: &ShapeGate,
    broken: &holon_core::shape_gate::ShapeRefused,
    origin: &OpOrigin,
) {
    if origin.is_user() {
        gate.bus.emit(holon_api::Condition {
            subject: broken.root.to_string(),
            reason: holon_api::ConditionKind::EditRefusedByShape {
                tag: broken.tag.clone(),
                rule: broken.rule.clone(),
            },
        });
    }
}

fn judged_key(op_name: &str, params: &StorageEntity) -> JudgedOp {
    let mut authored: Vec<(String, holon_api::Value)> = params
        .iter()
        .filter(|(k, _)| !holon_api::ENGINE_OWNED_PARAM_KEYS.contains(&k.as_ref()))
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    authored.sort_by(|a, b| a.0.cmp(&b.0));
    (op_name.to_string(), authored)
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
        Self::with_observers(providers, Vec::new())
    }

    pub fn with_observers(
        providers: Vec<Arc<dyn OperationProvider>>,
        observers: Vec<Arc<dyn OperationObserver>>,
    ) -> Self {
        let dispatcher = Self {
            providers,
            observers,
            ..Default::default()
        };
        dispatcher.collect_op_catalog();
        dispatcher
    }

    fn collect_op_catalog(&self) {
        let catalog = self
            .all_providers()
            .iter()
            .flat_map(|p| p.operations())
            .collect();
        self.op_catalog.set(Arc::new(catalog));
    }

    /// Every registered provider's descriptors.
    pub fn catalog(&self) -> Arc<Vec<OperationDescriptor>> {
        self.op_catalog.get_cloned()
    }

    /// The operation catalog now and after every registration.
    pub fn catalog_signal(
        &self,
    ) -> impl futures_signals::signal::Signal<Item = Arc<Vec<OperationDescriptor>>> + use<> {
        self.op_catalog.signal_cloned()
    }

    pub fn set_sync_token_store(&mut self, store: Arc<dyn SyncTokenStore>) {
        self.sync_token_store = Some(store);
    }

    /// Wire `*::rebuild_views`: the views it rebuilds, and the bus it
    /// discloses the rebuild on while it runs.
    pub fn set_view_rebuild(
        &mut self,
        manager: Arc<crate::sync::MatviewManager>,
        bus: Arc<holon_api::ConditionBus>,
    ) {
        self.view_rebuild = Some(ViewRebuild {
            manager,
            bus,
            running: std::sync::atomic::AtomicBool::new(false),
        });
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
    /// Install the write-tier seam. Without it a vault holding a read-only
    /// format accepts edits its files can never take.
    pub fn set_write_tier_authority(&mut self, authority: Arc<dyn holon_core::WriteTierAuthority>) {
        self.write_tier = Some(authority);
    }

    /// Install the shape gate. Without it a write may leave a tagged block in a
    /// shape no reader accepts.
    pub fn set_shape_gate(
        &mut self,
        validators: Arc<holon_core::ShapeValidators>,
        authority: Arc<dyn holon_core::WriteAuthorityReads>,
        bus: Arc<holon_api::ConditionBus>,
    ) {
        self.shape_gate = Some(ShapeGate {
            validators,
            authority,
            bus,
        });
    }

    pub fn set_net_guard(&mut self, guard: Arc<dyn crate::api::net_guard::NetGuard>) {
        self.net_guard = Some(guard);
    }

    /// Record which entities this container left unwired and why, so a dispatch
    /// against one fails naming the configuration instead of the missing
    /// registration.
    pub fn set_unavailable_entities(&mut self, unavailable: UnavailableEntities) {
        self.unavailable_entities = unavailable;
    }

    pub fn set_integration_attribution(
        &mut self,
        attribution: holon_core::integration_attribution::IntegrationAttribution,
    ) {
        self.integration_attribution = attribution;
    }

    /// Why no provider serves `entity`, when an integration owns it and has
    /// not put its operations in the dispatcher.
    fn integration_refusal(&self, entity: &str, op_name: &str) -> Option<String> {
        let owner = self
            .integration_attribution
            .owner_of(&EntityName::new(entity).table_name())
            .filter(|owner| !owner.status.serves_operations())?;
        let mut refusal = format!(
            "No provider serves {entity}.{op_name}: '{entity}' belongs to integration '{}' ({}), \
             status: {}",
            owner.integration,
            owner.display_name,
            owner.status.label()
        );
        if !owner.cause.is_empty() {
            refusal.push_str(&format!(" ({})", owner.cause));
        }
        Some(refusal)
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

    /// The entities a dispatch of `op_name` on `entity_name` names: the values
    /// of the params its descriptors declare as entity references, never
    /// free text that happens to parse as a URI. A reference that does not
    /// parse names nothing here; the dispatch refuses it by name
    /// ([`Self::parse_entity_references`]).
    pub(crate) fn named_entities<'p>(
        &self,
        entity_name: &str,
        op_name: &str,
        param: impl Fn(&str) -> Option<&'p holon_api::Value>,
    ) -> Result<BTreeSet<holon_api::EntityUri>> {
        let available_ops = self.op_catalog.get_cloned();
        let advertised = available_ops
            .iter()
            .any(|op| op.entity_name == entity_name && op.name == op_name);
        let resolved = match advertised {
            true => None,
            false => entity_by_id_scheme(&available_ops, op_name, param("id")),
        };
        let references = holon_api::entity_reference_params(
            &available_ops,
            resolved.as_deref().unwrap_or(entity_name),
            op_name,
            &param,
        )?;
        Ok(references
            .iter()
            .filter_map(|reference| param(reference.name)?.as_string())
            .filter_map(holon_api::EntityUri::schemed)
            .collect())
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
        self.check_provider(provider.as_ref())?;
        self.declared_providers
            .write()
            .expect("declared-provider registry poisoned")
            .push(provider);
        self.collect_op_catalog();
        Ok(())
    }

    /// The refusals of [`Self::register_provider`], without registering.
    pub fn check_provider(&self, provider: &dyn OperationProvider) -> Result<()> {
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
        // The declarations this provider adds are ones the operation boundary
        // will parse. Check them where they arrive rather than at the first
        // dispatch: a `String`-typed entity reference, or two descriptors
        // disagreeing about one parameter, is a wiring error the caller can
        // still fix.
        let mut declared = self.operations();
        declared.extend(provider.operations());
        holon_api::validate_entity_references(&declared)?;
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

    /// [`Self::parse_entity_references`] for a caller that reads an
    /// operation's params before it dispatches them. `extra_ops` are
    /// descriptors of operations that run without a provider.
    pub fn parse_entity_references_of(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: &StorageEntity,
        extra_ops: &[OperationDescriptor],
    ) -> Result<()> {
        let available_ops: Vec<_> = self
            .all_providers()
            .iter()
            .flat_map(|p| p.operations())
            .chain(extra_ops.iter().cloned())
            .collect();
        Self::parse_entity_references(&available_ops, entity_name.as_str(), op_name, params)
    }

    /// Parse every entity-reference parameter of one dispatched operation into
    /// an [`holon_api::EntityUri`], and refuse an unschemed value.
    ///
    /// The descriptors name them: an entity reference is a param declared
    /// [`holon_api::TypeHint::EntityId`], which includes the operation's own
    /// subject. Org files on disk store bare ids and the org parser adds the
    /// scheme (`docs/Reference/ORG_SYNTAX.md`); a caller that reached the
    /// dispatcher is past that parse, so a bare id here is a caller that never
    /// did one.
    ///
    /// The pair's declarations are read as a UNION
    /// ([`holon_api::entity_reference_params`]), so which provider answers is
    /// not decided by registration order, and two descriptors that disagree
    /// about one parameter are refused by name rather than resolved silently.
    ///
    /// An unroutable operation is left to the routing error below, which names
    /// the missing provider.
    fn parse_entity_references(
        available_ops: &[OperationDescriptor],
        resolved_entity_name: &str,
        op_name: &str,
        params: &StorageEntity,
    ) -> Result<()> {
        let references = holon_api::entity_reference_params(
            available_ops,
            resolved_entity_name,
            op_name,
            |k| params.get(k),
        )?;

        for reference in references {
            let param = reference.name;
            let expected_scheme = reference.entity_name;
            let Some(holon_api::Value::String(raw)) = params.get(param) else {
                continue;
            };
            // An empty reference is an absent one — `Value::Null` and `""` both
            // reach providers that read the param as optional.
            if raw.is_empty() {
                continue;
            }
            let Some(uri) = holon_api::EntityUri::schemed(raw) else {
                return Err(Box::new(holon_api::UnschemedEntityReference {
                    param: param.to_string(),
                    operation: format!("{resolved_entity_name}/{op_name}"),
                    value: raw.clone(),
                    expected_scheme: expected_scheme.as_str().to_string(),
                }));
            };
            // The root is the stored sentinel, never NULL — legal in a
            // position that DECLARES it (`parent_id` of a top-level block) and
            // nowhere else. As the subject of a write it names no row, so
            // letting it through there is the same silent no-op as a foreign
            // reference.
            if uri == holon_api::EntityUri::no_parent() {
                if reference.admits_root {
                    continue;
                }
                return Err(Box::new(holon_api::ForeignEntityReference {
                    param: param.to_string(),
                    operation: format!("{resolved_entity_name}/{op_name}"),
                    value: raw.clone(),
                    found_scheme: uri.scheme().to_string(),
                    expected_scheme: expected_scheme.as_str().to_string(),
                }));
            }
            // A scheme that merely EXISTS is not a reference to the right
            // thing. `https://example.com/x` and a `block:` id handed to a
            // `test-item` operation both parse, then match no row — and a write
            // matching no row reports success having changed nothing.
            if uri.scheme() != expected_scheme.as_str() {
                return Err(Box::new(holon_api::ForeignEntityReference {
                    param: param.to_string(),
                    operation: format!("{resolved_entity_name}/{op_name}"),
                    value: raw.clone(),
                    found_scheme: uri.scheme().to_string(),
                    expected_scheme: expected_scheme.as_str().to_string(),
                }));
            }
        }
        Ok(())
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

    /// The write-tier decision for one dispatched operation.
    ///
    /// A block homed in a `WriteTier::ReadOnly` file is a projection of input
    /// Holon cannot write back, so accepting a write against it would leave the
    /// store saying one thing and the disk another. The refusal is typed
    /// ([`EditRefused`]) so the UI discloses it as a state of the document
    /// rather than as a generic failure.
    ///
    /// `parent_id` is judged as well as `id`: a `create` names its destination
    /// there and no subject at all.
    ///
    /// Only writes that ORIGINATE in the store are judged. `Ingest` is the file
    /// telling the store what it says — refusing it would break the ingest
    /// rather than protect the file.
    ///
    /// A peer's already-merged history reaches this store through the share
    /// backend's projection legs and the pairing re-import, neither of which
    /// dispatches; each asks this same authority at its own seam
    /// (`adopt_sync_import`), so there is no sync arm here to keep in step with
    /// them.
    ///
    /// `origin` is the COMPOUND's origin for a compound's constituents too:
    /// they carry it down rather than being re-judged as a user's edit.
    async fn enforce_write_tier(
        &self,
        resolved_entity_name: &str,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<()> {
        let Some(refusal) = self.write_tier_refusal(resolved_entity_name, params, origin)? else {
            return Ok(());
        };
        self.write_tier
            .as_ref()
            .expect("a write-tier refusal comes from an installed authority")
            .disclose(&refusal);
        Err(Box::new(refusal))
    }

    /// The write-tier gate's verdict on one operation, without disclosing it.
    pub fn write_tier_refusal(
        &self,
        resolved_entity_name: &str,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<Option<holon_core::EditRefused>> {
        let Some(authority) = &self.write_tier else {
            return Ok(None);
        };
        if resolved_entity_name != holon_core::WRITE_TIER_ENTITY
            || matches!(origin, OpOrigin::Ingest)
        {
            return Ok(None);
        }
        for key in SUBJECT_PARAM_KEYS {
            let Some(subject) = params.get(key).and_then(|v| v.as_string()) else {
                continue;
            };
            if let Some(refusal) = authority.refusal_for(subject)? {
                return Ok(Some(refusal));
            }
        }
        Ok(None)
    }

    /// The shape-gate decision for a plan of block operations: every tagged
    /// block the WHOLE plan touches must pass its validators afterwards.
    ///
    /// Only writes that ORIGINATE in Holon are judged. `Ingest` is a file
    /// telling the store what it says and `Sync` a peer's merged history: both
    /// are foreign input, never refused. Nothing discloses a broken shape they
    /// store yet (Model.md invariant 17).
    async fn judge_shape(
        &self,
        ops: &[(&str, &StorageEntity)],
        origin: &OpOrigin,
    ) -> Result<holon_core::shape_gate::Judgement> {
        let unjudged = || holon_core::shape_gate::Judgement {
            verdict: Ok(()),
            simulated: Vec::new(),
            touched: Vec::new(),
            created: Default::default(),
        };
        let Some(gate) = &self.shape_gate else {
            return Ok(unjudged());
        };
        if matches!(origin, OpOrigin::Ingest | OpOrigin::Sync) {
            return Ok(unjudged());
        }
        let plan: Vec<holon_core::shape_gate::PlanOp<'_>> = ops
            .iter()
            .map(|(op_name, params)| holon_core::shape_gate::PlanOp { op_name, params })
            .collect();
        let judgement =
            holon_core::shape_gate::judge_plan(&gate.validators, gate.authority.clone(), &plan)
                .await?;
        match &judgement.verdict {
            Ok(()) => Ok(judgement),
            Err(refusal) => {
                disclose_shape(gate, refusal, origin);
                Err(Box::new(refusal.clone()))
            }
        }
    }

    /// The error of an admitted plan that stopped part way. Its applied ops
    /// stay applied unless the plan rolled them back, so the blocks it touched
    /// are judged as stored: a broken one is named and disclosed.
    async fn stopped_part_way(
        &self,
        failure: Box<dyn std::error::Error + Send + Sync>,
        touched: &[holon_api::EntityUri],
        origin: &OpOrigin,
    ) -> Box<dyn std::error::Error + Send + Sync> {
        let Some(gate) = &self.shape_gate else {
            return failure;
        };
        match holon_core::shape_gate::judge_stored(
            &gate.validators,
            gate.authority.as_ref(),
            touched,
        )
        .await
        {
            Ok(Ok(())) => failure,
            Ok(Err(broken)) => {
                disclose_shape(gate, &broken, origin);
                format!(
                    "{failure}; the ops that ran before it left the `{}` block {} breaking rule \
                     {}: {}",
                    broken.tag, broken.root, broken.rule, broken.message
                )
                .into()
            }
            Err(read) => {
                format!("{failure}; the shape of what its ops left could not be read: {read}")
                    .into()
            }
        }
    }

    /// The shape gate for ONE dispatched block operation: a plan of one, unless
    /// the task is executing a plan whose admission already judged this op.
    async fn enforce_shape(
        &self,
        resolved_entity_name: &str,
        op_name: &str,
        params: &StorageEntity,
        key: JudgedOp,
        origin: &OpOrigin,
    ) -> Result<Vec<holon_core::shape_gate::Simulated>> {
        if resolved_entity_name != "block" {
            return Ok(Vec::new());
        }
        if KEYSTROKE.try_with(|_| ()).is_ok() {
            return Ok(Vec::new());
        }
        let admitted = JUDGED_PLAN
            .try_with(|plan| {
                let mut plan = plan.borrow_mut();
                plan.iter()
                    .position(|op| *op == key)
                    .map(|at| plan.remove(at))
                    .is_some()
            })
            .unwrap_or(false);
        if admitted {
            return Ok(Vec::new());
        }
        let ops = [(op_name, params)];
        let judgement = self.judge_shape(&ops, origin).await?;
        let judgement = self.hold_shape(&ops, origin, judgement).await?;
        judged_checkpoint(op_name).await?;
        Ok(judgement.simulated)
    }

    /// Claim, for the running write, every tagged block `judgement` shapes,
    /// then judge `ops` again under that claim: only that verdict holds
    /// until the ops run. A claim that grows past
    /// [`MAX_SHAPE_CLAIMS`] judgements, or that closes a wait cycle, is a
    /// named error.
    async fn hold_shape(
        &self,
        ops: &[(&str, &StorageEntity)],
        origin: &OpOrigin,
        mut judgement: holon_core::shape_gate::Judgement,
    ) -> Result<holon_core::shape_gate::Judgement> {
        let Some(write) = crate::api::running_write::RunningWrite::current() else {
            return Ok(judgement);
        };
        let op = ops.first().expect("a judged plan has an op").0;
        for _ in 0..MAX_SHAPE_CLAIMS {
            let wanted = write.unheld(&judgement);
            if wanted.is_empty() {
                return Ok(judgement);
            }
            match write.claim_shape(&wanted, op).await {
                Ok(crate::api::running_write::ShapeClaim::Covered) => return Ok(judgement),
                Ok(crate::api::running_write::ShapeClaim::Waited) => {
                    judgement = self.judge_shape(ops, origin).await?;
                }
                Err(refused) => {
                    tracing::warn!(
                        admission = write.seq(),
                        through = refused.through,
                        ran = write.ran(),
                        "[admission] the shape claim of block.{op} was refused: {refused}"
                    );
                    let landed = if write.ran() {
                        "; an earlier write of the same gesture already landed"
                    } else {
                        ""
                    };
                    return Err(format!(
                        "block.{op} could not claim the tagged blocks it shapes: {refused}{landed}"
                    )
                    .into());
                }
            }
        }
        let wanted = write.unheld(&judgement);
        if wanted.is_empty() {
            return Ok(judgement);
        }
        Err(format!(
            "block.{op} shaped more tagged blocks at each of {MAX_SHAPE_CLAIMS} judgements; \
             last wanted {wanted:?}"
        )
        .into())
    }

    /// Judge `ops`, which the running write dispatches one by one, as a whole
    /// and claim what they shape before the first of them runs, so a later
    /// one finds its blocks held instead of claiming them after an earlier
    /// one landed.
    pub(crate) async fn claim_shape_of(
        &self,
        ops: &[(&str, &StorageEntity)],
        origin: &OpOrigin,
    ) -> Result<()> {
        let judgement = self.judge_shape(ops, origin).await?;
        self.hold_shape(ops, origin, judgement).await.map(drop)
    }

    /// Claim, for the running write, the [`neighbourhood`] of `targets` and
    /// `subtrees` before its first op runs: for a write whose follow-up writes
    /// are known only after it ran, so none of them needs a claim that could
    /// be refused once part of the write landed. A claim waited for is read
    /// again, at most [`MAX_SHAPE_CLAIMS`] times.
    ///
    /// [`neighbourhood`]: holon_core::shape_gate::neighbourhood
    pub(crate) async fn claim_neighbourhood(
        &self,
        op: &str,
        targets: &[holon_api::EntityUri],
        subtrees: &[holon_api::EntityUri],
        origin: &OpOrigin,
    ) -> Result<()> {
        let Some(gate) = &self.shape_gate else {
            return Ok(());
        };
        if matches!(origin, OpOrigin::Ingest | OpOrigin::Sync) {
            return Ok(());
        }
        let Some(write) = crate::api::running_write::RunningWrite::current() else {
            return Ok(());
        };
        for _ in 0..MAX_SHAPE_CLAIMS {
            let wanted = holon_core::shape_gate::neighbourhood(
                &gate.validators,
                gate.authority.as_ref(),
                targets,
                subtrees,
            )
            .await?;
            if wanted.is_empty() || write.holds(&wanted) {
                return Ok(());
            }
            match write.claim_shape(&wanted, op).await {
                Ok(crate::api::running_write::ShapeClaim::Covered) => return Ok(()),
                Ok(crate::api::running_write::ShapeClaim::Waited) => {}
                Err(refused) => {
                    return Err(format!(
                        "block.{op} could not claim the tagged blocks its follow-up writes \
                         shape: {refused}"
                    )
                    .into());
                }
            }
        }
        Err(format!(
            "block.{op} found more tagged blocks around its target at each of \
             {MAX_SHAPE_CLAIMS} claims"
        )
        .into())
    }

    /// Hand the simulator's prediction to the shape audit, when one is
    /// enabled.
    async fn audit_shape(
        &self,
        what: &str,
        simulated: &[holon_core::shape_gate::Simulated],
    ) -> Result<()> {
        let audit = holon_core::shape_gate::ShapeAudit::global();
        match &self.shape_gate {
            Some(gate) if audit.is_enabled() => {
                audit
                    .compare(gate.authority.as_ref(), what, simulated)
                    .await
            }
            _ => Ok(()),
        }
    }

    /// Run `keystroke`, whose writes are the editor's typing, without the shape
    /// gate: refusing a keystroke mid-word is hostile, so a keystroke that
    /// breaks a tagged shape lands, and nothing discloses it yet (Model.md
    /// invariant 17).
    pub(crate) async fn as_keystroke<F: std::future::Future>(&self, keystroke: F) -> F::Output {
        KEYSTROKE.scope((), keystroke).await
    }

    /// Judge `ops` as ONE plan, then run `execute` with them admitted, so each
    /// op it dispatches in this task is not judged again on its own. A plan
    /// that is legal only as a whole — a decision created before its options,
    /// a new `choose` together with a ruling of that size — lands; a plan whose
    /// result breaks a shape writes nothing.
    pub async fn execute_judged<F, T>(
        &self,
        ops: &[Operation],
        origin: &OpOrigin,
        execute: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let params: Vec<StorageEntity> = ops
            .iter()
            .map(|op| {
                op.params
                    .iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
                    .collect()
            })
            .collect();
        let blocks: Vec<(&str, &StorageEntity)> = ops
            .iter()
            .zip(&params)
            .filter(|(op, _)| op.entity_name.as_str() == "block")
            .map(|(op, p)| (op.op_name.as_str(), p))
            .collect();
        let judgement = self.judge_shape(&blocks, origin).await?;
        let judgement = if blocks.is_empty() {
            judgement
        } else {
            let judgement = self.hold_shape(&blocks, origin, judgement).await?;
            judged_checkpoint(blocks[0].0).await?;
            judgement
        };
        let admitted: Vec<JudgedOp> = blocks
            .iter()
            .map(|(op_name, p)| judged_key(op_name, p))
            .collect();
        let done = match JUDGED_PLAN
            .scope(std::cell::RefCell::new(admitted), execute)
            .await
        {
            Ok(done) => done,
            Err(failure) => {
                return Err(self
                    .stopped_part_way(failure, &judgement.touched, origin)
                    .await);
            }
        };
        let names: Vec<&str> = blocks.iter().map(|(op_name, _)| *op_name).collect();
        self.audit_shape(&format!("plan {names:?}"), &judgement.simulated)
            .await?;
        Ok(done)
    }

    /// Fail-loud guard that a composed backend actually installed the shape
    /// gate (Model.md invariant 17). A dispatcher without one lets any write
    /// leave a tagged block in a shape no reader accepts.
    pub fn assert_shape_gate_installed(&self) -> Result<()> {
        let Some(gate) = &self.shape_gate else {
            return Err(
                "OperationDispatcher has no shape gate installed: every tagged block \
                        (decision, ...) is writable into shapes its readers refuse"
                    .into(),
            );
        };
        let missing = gate.validators.missing_registered();
        if !missing.is_empty() {
            return Err(format!(
                "OperationDispatcher's shape gate has no validator for {missing:?}: blocks with \
                 those tags are writable into shapes their readers refuse"
            )
            .into());
        }
        Ok(())
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

    /// The broadcasts this container can run — the ONE place that decides,
    /// read by [`OperationProvider::operations`], which advertises them, and by
    /// the wildcard arm, which accepts them.
    ///
    /// The op name is caller-supplied, so the two must be the same set: an op
    /// accepted on a container that advertises it nowhere is an effect no
    /// composition offered (`*::full_sync` cleared every sync token on a
    /// container with no syncable provider at all).
    fn offered_broadcasts(&self, snapshot: &OperationSnapshot) -> Vec<OfferedBroadcast<'_>> {
        let mut offered = Vec::new();
        if snapshot.advertises(BroadcastOp::Sync.as_str()) {
            offered.push(OfferedBroadcast::Sync);
            offered.push(OfferedBroadcast::FullSync);
        }
        if let Some(rebuild) = self.view_rebuild.as_ref() {
            offered.push(OfferedBroadcast::RebuildViews(rebuild));
        }
        offered
    }

    /// One read of every provider's `operations()`, for a dispatch that must
    /// decide, judge and run from the same answer.
    fn snapshot_operations(&self) -> OperationSnapshot {
        OperationSnapshot {
            providers: self
                .all_providers()
                .into_iter()
                .map(|provider| {
                    let ops = provider.operations();
                    (provider, ops)
                })
                .collect(),
        }
    }

    /// The gate chain [`Self::execute_operation_with_provenance`] runs for a
    /// named dispatch, for one fan-out member under that provider's own entity
    /// name.
    async fn judge_fan_out_member(
        &self,
        available_ops: &[OperationDescriptor],
        entity: &str,
        op_name: &str,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<()> {
        self.enforce_boundary(available_ops, entity, op_name, params)?;
        self.enforce_guard(available_ops, entity, op_name, params)
            .await?;
        self.enforce_net_guard(entity, op_name, params).await?;
        self.enforce_write_tier(entity, params, origin).await
    }

    /// Judge every provider that advertises `op_name`, and run NONE of them.
    ///
    /// The gates bind to the subject an operation's params name and to the
    /// declarations on its descriptor. A fan-out member names no subject, so
    /// they normally decide nothing; what matters is that a policy which DOES
    /// speak about one of these descriptors is consulted rather than skipped,
    /// so no provider write leaves this dispatcher unjudged.
    ///
    /// Judging comes first for the whole fan-out, and one refusal refuses the
    /// broadcast naming every provider that refused. A broadcast has no
    /// inverse, so a refusal met half way through would leave effects nothing
    /// describes — cleared sync tokens, one cache cleared, nothing re-synced —
    /// and a caller shown one refusal at a time cannot see what the next
    /// attempt meets.
    async fn judge_fan_out<'a>(
        &self,
        snapshot: &OperationSnapshot,
        op_name: &'a str,
        params: &'a StorageEntity,
        origin: &OpOrigin,
    ) -> Result<JudgedFanOut<'a>> {
        let available_ops = snapshot.all_ops();
        let mut members = Vec::new();
        let mut refused = Vec::new();
        for (provider, ops) in &snapshot.providers {
            let Some(op) = ops.iter().find(|op| op.name == op_name).cloned() else {
                continue;
            };
            match self
                .judge_fan_out_member(
                    &available_ops,
                    op.entity_name.as_str(),
                    op_name,
                    params,
                    origin,
                )
                .await
            {
                Ok(()) => members.push((Arc::clone(provider), op)),
                Err(e) => refused.push(format!("{}: {e}", op.entity_name)),
            }
        }
        if !refused.is_empty() {
            return Err(format!(
                "*::{op_name} was refused for {} of the {} providers that advertise it, so none of \
                 them ran: {}",
                refused.len(),
                refused.len() + members.len(),
                refused.join("; ")
            )
            .into());
        }
        Ok(JudgedFanOut {
            op_name,
            params,
            members,
        })
    }

    /// [`BroadcastOp::RebuildViews`]: drop every watch view and recreate the
    /// ones a live watch listens to.
    async fn rebuild_views(&self, rebuild: &ViewRebuild) -> Result<OperationResult> {
        let _running = rebuild.start()?;
        let rebuilt = rebuild
            .manager
            .rebuild_watch_views()
            .await
            .map_err(|e| format!("rebuild_views: {e:#}"))?;
        let summary = format!(
            "dropped {} watch views; recreated {} for their live subscribers: {}",
            rebuilt.dropped.len(),
            rebuilt.recreated.len(),
            rebuilt.recreated.join(", ")
        );
        info!("[OperationDispatcher] rebuild_views {summary}");
        Ok(OperationResult::irreversible(Vec::new())
            .with_response(holon_api::Value::String(summary)))
    }

    /// [`BroadcastOp::FullSync`]: clear every sync token, then every provider
    /// cache, then re-sync.
    ///
    /// Tokens go FIRST: clearing a cache can trigger a `sync_changes` callback
    /// that would load and re-save the token just cleared. BOTH legs are judged
    /// before that, so a gate refusal cannot leave the tokens cleared with
    /// nothing re-synced.
    async fn full_sync(
        &self,
        snapshot: &OperationSnapshot,
        origin: &OpOrigin,
    ) -> Result<OperationResult> {
        let no_params = StorageEntity::new();
        let clearing = self
            .judge_fan_out(snapshot, "clear_cache", &no_params, origin)
            .await?;
        let syncing = self
            .judge_fan_out(snapshot, "sync", &no_params, origin)
            .await?;
        assert!(
            !syncing.is_empty(),
            "full_sync is offered only where a provider advertises `sync` in the snapshot it is \
             judged from, so the sync leg cannot be empty"
        );

        let mut ran = Vec::new();
        match &self.sync_token_store {
            Some(store) => {
                store
                    .clear_all_tokens()
                    .await
                    .map_err(|e| format!("full_sync: clearing the sync tokens: {e}"))?;
                info!("[OperationDispatcher] Cleared all sync tokens");
                ran.push("the sync tokens were cleared".to_string());
            }
            None => info!(
                "[OperationDispatcher] No sync token store configured, skipping token clearing"
            ),
        }
        let cleared = clearing.run().await;
        ran.push(cleared.summary("clear_cache"));
        let synced = syncing.run().await;
        ran.push(synced.summary("sync"));
        let summary = ran.join("; ");

        // A leg that lost EVERY provider did not happen, and the caller asked
        // for the leg. Losing SOME is a single unreachable external system,
        // which must not fail the providers that did re-sync — it is disclosed
        // in the summary instead.
        let lost_legs: Vec<String> = [("clear_cache", &cleared), ("sync", &synced)]
            .into_iter()
            .filter(|(_, fan)| fan.lost_every_provider())
            .map(|(leg, _)| format!("nothing of the {leg} leg happened"))
            .collect();
        if !lost_legs.is_empty() {
            return Err(format!(
                "*::full_sync failed: {}, because every provider that advertises it failed. \
                 What ran: {summary}",
                lost_legs.join("; ")
            )
            .into());
        }
        info!("[OperationDispatcher] full_sync completed: {summary}");
        Ok(OperationResult::irreversible(Vec::new())
            .with_response(holon_api::Value::String(summary)))
    }

    /// [`BroadcastOp::Sync`]: run `sync` on every provider that advertises one.
    ///
    /// The broadcast is irreversible as a whole: several providers may have run
    /// and no single inverse describes what they did.
    async fn broadcast_sync(
        &self,
        snapshot: &OperationSnapshot,
        params: &StorageEntity,
        origin: &OpOrigin,
    ) -> Result<OperationResult> {
        let judged = self.judge_fan_out(snapshot, "sync", params, origin).await?;
        assert!(
            !judged.is_empty(),
            "*::sync is offered only where a provider advertises `sync` in the snapshot it is \
             judged from, so the fan-out cannot be empty"
        );
        let fan = judged.run().await;
        if fan.lost_every_provider() {
            return Err(format!(
                "*::sync failed on every provider that has one: {}",
                fan.failed.join(", ")
            )
            .into());
        }
        let summary = fan.summary("*::sync");
        info!("[OperationDispatcher] {summary}");
        Ok(OperationResult::irreversible(Vec::new())
            .with_response(holon_api::Value::String(summary)))
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
                "[OperationDispatcher] execute_operation: entity={}, op={}, params=[{}]",
                entity_name,
                op_name,
                crate::api::param_keys_for_logs(&params)
            );

            // Entity `*` names no relation. The gate chain the named arm below
            // runs binds to the subject an operation's params name and to the
            // declarations on the descriptor of its `entity.op` pair; a
            // broadcast has neither. `BroadcastOp::parse` is what keeps that
            // sound rather than merely true today: the three ops this
            // dispatcher synthesizes under `*` take no subject and write no
            // field, and any other op name is refused here instead of being
            // routed to whichever provider happens to advertise it.
            if entity_name.is_wildcard() {
                let broadcast = BroadcastOp::parse(op_name)?;
                broadcast.reject_subject_params(&params)?;
                let snapshot = self.snapshot_operations();
                let offered = self
                    .offered_broadcasts(&snapshot)
                    .into_iter()
                    .find(|offered| offered.op() == broadcast)
                    .ok_or(BroadcastNotOffered {
                        op: broadcast,
                        requires: broadcast.requires(),
                    })?;
                info!("[OperationDispatcher] broadcast operation: {broadcast}");
                match offered {
                    OfferedBroadcast::RebuildViews(rebuild) => self.rebuild_views(rebuild).await,
                    OfferedBroadcast::FullSync => self.full_sync(&snapshot, &origin).await,
                    OfferedBroadcast::Sync => {
                        self.broadcast_sync(&snapshot, &params, &origin).await
                    }
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
                let resolved_entity_name: &str = if matching_ops.is_empty()
                    && let Some(scheme) =
                        entity_by_id_scheme(&available_ops, op_name, params.get("id"))
                {
                    info!(
                        "[OperationDispatcher] Entity '{}' not found, resolved to '{}' via id \
                         scheme",
                        entity_name, scheme
                    );
                    resolved_entity = scheme;
                    resolved_entity.as_str()
                } else {
                    entity_name_str
                };
                tracing::Span::current().record("operation.resolved_entity", resolved_entity_name);

                // The shape gate's admission key is the op AS DISPATCHED, before
                // this dispatcher rewrites any param below.
                let judged_key = judged_key(op_name, &params);

                // THE entity-reference seam: every id this operation names is
                // parsed here, once, and an unschemed one is refused.
                Self::parse_entity_references(
                    &available_ops,
                    resolved_entity_name,
                    op_name,
                    &params,
                )?;

                // Intent boundary (Model.md invariants 3 and 16): parse the
                // field of a block `set_field` intent into the closed
                // `BlockWriteField` vocabulary, and refuse a block `update`
                // that names a private field. Private fields, order keys and
                // storage-internal fields are a loud Err here, in EVERY mode —
                // their owners write them, never a generic intent. The
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
                if resolved_entity_name == "block" && op_name == "update" {
                    if let Some((field, private)) = holon_api::schema::BLOCK
                        .private_fields()
                        .into_iter()
                        .find(|(field, _)| params.contains_key(*field))
                    {
                        let refusal = holon_api::BlockWriteFieldError::Private {
                            field: field.to_string(),
                            route: private.route,
                        };
                        return Err(format!("intent boundary: {refusal}").into());
                    }
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
                                let (label, marks) = holon_org_format::extract_block_marks_with(
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
                        holon_org_format::extract_block_marks_with(&raw, &self.link_classifier);
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
                    // An integration that has not connected yet is a disclosed
                    // state, not a wiring error.
                    if self
                        .unavailable_entities
                        .reason_for(resolved_entity_name)
                        .is_none()
                        && let Some(refusal) =
                            self.integration_refusal(resolved_entity_name, op_name)
                    {
                        warn!("[OperationDispatcher] {refusal}");
                        return Err(refusal.into());
                    }
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

                // THE write-tier gate: a block whose file Holon cannot write
                // back may not be edited into the store.
                self.enforce_write_tier(resolved_entity_name, &params, &origin)
                    .await?;

                // THE shape gate: a block write may not leave a tagged block in
                // a shape its validator refuses.
                let shape_simulated = self
                    .enforce_shape(resolved_entity_name, op_name, &params, judged_key, &origin)
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
                // - Readable provider (SQL or Loro CRUD authority): compare against ground
                //   truth. Skip when the mark set is unchanged, and skip a null-producing
                //   re-commit whose stripped label already equals the stored content (the blur
                //   path). Otherwise dispatch — including a legitimate `marks = Null` when an
                //   edit genuinely REMOVED the link (new label differs from the stored
                //   content).
                // - Unreadable provider (test stubs): fail safe — dispatch only when the new
                //   content actually yields marks (a link was typed); never null on an unknown
                //   prior state.
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

                if let Some(write) = crate::api::running_write::RunningWrite::current() {
                    write.mark_ran();
                }
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

                self.audit_shape(
                    &format!("{resolved_entity_name}.{op_name}"),
                    &shape_simulated,
                )
                .await?;
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

/// The entity an op routes to when no provider advertises it under the name
/// it was dispatched with: rows of a view carry the view's name, and their
/// `id`'s scheme names the entity whose provider runs it.
fn entity_by_id_scheme(
    available_ops: &[OperationDescriptor],
    op_name: &str,
    id: Option<&holon_api::Value>,
) -> Option<String> {
    let Some(holon_api::Value::String(id)) = id else {
        return None;
    };
    let (scheme, _) = id.split_once(':')?;
    available_ops
        .iter()
        .any(|op| op.entity_name == scheme && op.name == op_name)
        .then(|| scheme.to_string())
}

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

/// How long an operation may take to become visible. An interaction is held
/// to the interaction→visible SLO; a maintenance op is a deliberate job an
/// agent or the user starts, which reports what it did when it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpClass {
    Interaction,
    Maintenance,
}

/// Which provider entities a fan-out ran, and which ones it could not.
#[derive(Default)]
struct FanOut {
    succeeded: Vec<String>,
    failed: Vec<String>,
}

impl FanOut {
    /// Every provider that advertises the op failed, so nothing of this leg
    /// happened. The caller asked for the leg, not for an attempt at it.
    fn lost_every_provider(&self) -> bool {
        self.succeeded.is_empty() && !self.failed.is_empty()
    }

    /// What ran and what was lost, by provider entity. A broadcast that lost
    /// one provider still succeeded, so this is how the loss reaches the
    /// caller instead of only the log.
    fn summary(&self, op_name: &str) -> String {
        format!(
            "{op_name} ran on [{}] and failed on [{}]",
            self.succeeded.join(", "),
            self.failed.join(", ")
        )
    }
}

/// Every provider paired with the operations it advertised in ONE read.
///
/// A broadcast is offered, judged and run from the same snapshot, so a
/// provider whose `operations()` changes between reads cannot make an offered
/// broadcast find nobody to run.
struct OperationSnapshot {
    providers: Vec<(Arc<dyn OperationProvider>, Vec<OperationDescriptor>)>,
}

impl OperationSnapshot {
    fn all_ops(&self) -> Vec<OperationDescriptor> {
        self.providers
            .iter()
            .flat_map(|(_, ops)| ops.iter().cloned())
            .collect()
    }

    fn advertises(&self, op_name: &str) -> bool {
        self.providers
            .iter()
            .any(|(_, ops)| ops.iter().any(|op| op.name == op_name))
    }
}

/// Every provider that advertises one op of a broadcast, each already judged
/// by the gate chain a named dispatch runs. Holding one is the proof that no
/// gate refuses any member, so running them is all that is left — which is why
/// [`OperationDispatcher::judge_fan_out`] is the only way to obtain one.
struct JudgedFanOut<'a> {
    op_name: &'a str,
    params: &'a StorageEntity,
    members: Vec<(Arc<dyn OperationProvider>, OperationDescriptor)>,
}

impl JudgedFanOut<'_> {
    /// No provider advertises the op. Legitimate for a leg prod wires nobody
    /// for; an assertion elsewhere where the broadcast is offered only because
    /// a provider exists.
    fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Run every judged member.
    ///
    /// A provider's OWN failure is counted, not propagated: one unreachable
    /// external system must not stop the others from syncing. The [`FanOut`]
    /// carries what ran to the caller, which decides what losing a provider
    /// means for the broadcast as a whole.
    async fn run(self) -> FanOut {
        let mut fan = FanOut::default();
        let op_name = self.op_name;
        for (provider, op) in self.members {
            let entity = op.entity_name.as_str().to_string();
            match provider
                .execute_operation(&op.entity_name, op_name, self.params.clone())
                .await
            {
                Ok(_) => {
                    info!("[OperationDispatcher] {op_name} succeeded on entity '{entity}'");
                    fan.succeeded.push(entity);
                }
                Err(e) => {
                    error!("[OperationDispatcher] {op_name} failed on entity '{entity}': {e}");
                    fan.failed.push(entity);
                }
            }
        }
        fan
    }
}

/// A [`BroadcastOp`] THIS container can run, holding the wiring that answers
/// it. The wildcard arm reaches a broadcast only through one of these, so an
/// op the container cannot run has no arm to reach.
enum OfferedBroadcast<'a> {
    Sync,
    FullSync,
    RebuildViews(&'a ViewRebuild),
}

impl OfferedBroadcast<'_> {
    fn op(&self) -> BroadcastOp {
        match self {
            Self::Sync => BroadcastOp::Sync,
            Self::FullSync => BroadcastOp::FullSync,
            Self::RebuildViews(_) => BroadcastOp::RebuildViews,
        }
    }
}

/// The closed set of operations entity `*` dispatches — exactly the ones
/// [`OperationDispatcher::operations`] synthesizes under that name.
///
/// `*` names no relation, so the gate chain a named dispatch runs — the ADR
/// 0028 boundary seam, the ADR 0031 declared guard, the ADR 0032 net gate, the
/// write tier — has no subject to bind and no descriptor to read a declaration
/// off. These three earn that: each takes no subject param and writes no
/// field, so there is nothing for those gates to decide.
/// [`Self::reject_subject_params`] holds the caller to the first half, which is
/// about params rather than about the op name and so cannot be settled by this
/// set alone.
///
/// The set is a type rather than a string match because `entity_name` is
/// caller-supplied and unconstrained (MCP `execute_operation` passes it
/// verbatim). Matching the op name against whatever the registered providers
/// advertise let `*` carry an ordinary subject-taking write — `set_field`,
/// `delete` — straight to its provider past all four gates
/// (`2026-10-05-wildcard-entity-dispatches-any-op-past-every-write-gate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastOp {
    Sync,
    FullSync,
    RebuildViews,
}

impl BroadcastOp {
    /// The closed set, and the only route to a value of this type: [`parse`]
    /// resolves names against it, so a variant missing here is one the
    /// dispatcher can never broadcast.
    ///
    /// [`parse`]: Self::parse
    pub const ALL: [Self; 3] = [Self::Sync, Self::FullSync, Self::RebuildViews];

    /// # Errors
    /// Any other op name: a caller naming entity `*` for an operation this
    /// dispatcher does not broadcast.
    pub fn parse(op_name: &str) -> std::result::Result<Self, NotABroadcastOp> {
        Self::ALL
            .into_iter()
            .find(|op| op.as_str() == op_name)
            .ok_or_else(|| NotABroadcastOp {
                op_name: op_name.to_string(),
            })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::FullSync => "full_sync",
            Self::RebuildViews => "rebuild_views",
        }
    }

    /// The descriptor a container advertises for this broadcast, written beside
    /// the arm that runs it: `operations()` builds the `*` menu from exactly
    /// the broadcasts `offered_broadcasts` offers, so neither list can gain
    /// an entry the other lacks.
    pub fn descriptor(self) -> OperationDescriptor {
        let (display_name, description) = match self {
            Self::Sync => ("Sync", "Sync registered syncable providers"),
            Self::FullSync => (
                "Full Sync",
                "Clear all caches, reset sync tokens, and re-sync from external systems",
            ),
            Self::RebuildViews => (
                "Rebuild Views",
                "Drop every watch view and recreate the ones a live watch listens to, correcting \
                 what each watch holds; reports each view rebuilt and each one that failed",
            ),
        };
        OperationDescriptor {
            entity_name: "*".into(),
            entity_short_name: "all".to_string(),
            id_column: String::new(),
            name: self.as_str().to_string(),
            display_name: display_name.to_string(),
            description: description.to_string(),
            required_params: vec![],
            optional_params: vec![],
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
        }
    }

    /// What a container must be wired with to run this broadcast — the
    /// condition `OperationDispatcher::offered_broadcasts` reads, said in the
    /// words of whoever composed the container.
    pub fn requires(self) -> &'static str {
        match self {
            Self::Sync | Self::FullSync => "a registered provider that advertises a `sync` op",
            Self::RebuildViews => "a view rebuild, installed with `set_view_rebuild`",
        }
    }

    /// # Errors
    /// The params name a subject. Params reach a dispatch verbatim from
    /// whoever asked for it, and no broadcast takes a subject: one arriving
    /// here would be fanned out to every provider advertising the op, each
    /// judging it against a descriptor written for an operation that touches
    /// no block.
    pub fn reject_subject_params(
        self,
        params: &StorageEntity,
    ) -> std::result::Result<(), BroadcastCarriesASubject> {
        match SUBJECT_PARAM_KEYS
            .into_iter()
            .find(|key| params.get(*key).is_some())
        {
            Some(key) => Err(BroadcastCarriesASubject { op: self, key }),
            None => Ok(()),
        }
    }
}

impl fmt::Display for BroadcastOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A caller named entity `*` for an operation that is not a [`BroadcastOp`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotABroadcastOp {
    pub op_name: String,
}

impl fmt::Display for NotABroadcastOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "entity '*' broadcasts only sync, full_sync and rebuild_views; '{}' is not one of \
             them. A broadcast runs under no relation, so the boundary seam, the declared guard, \
             the net gate and the write tier have no subject to judge — dispatching '{}' this way \
             would run it past all four. Name the entity the operation belongs to.",
            self.op_name, self.op_name
        )
    }
}

impl std::error::Error for NotABroadcastOp {}

/// A caller named a broadcast this composition cannot run: nothing it is
/// wired with answers the op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroadcastNotOffered {
    pub op: BroadcastOp,
    /// What a composition must have wired to offer `op`
    /// ([`BroadcastOp::requires`]).
    pub requires: &'static str,
}

impl fmt::Display for BroadcastNotOffered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "entity '*' broadcasts '{}' only where this container has {}, and this one does not, \
             so there is nothing to broadcast it to. The operation is advertised exactly where it \
             can run.",
            self.op, self.requires
        )
    }
}

impl std::error::Error for BroadcastNotOffered {}

/// A caller named entity `*` and put a subject in the params.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroadcastCarriesASubject {
    pub op: BroadcastOp,
    pub key: &'static str,
}

impl fmt::Display for BroadcastCarriesASubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "entity '*' broadcasts '{}', which names no subject, but the params carry '{}'. A \
             broadcast runs under no relation, so the boundary seam, the declared guard and the \
             write tier would bind that subject to the descriptor of whichever provider \
             advertises '{}' — written for an operation that touches no block. Name the entity \
             the subject belongs to.",
            self.op, self.key, self.op
        )
    }
}

impl std::error::Error for BroadcastCarriesASubject {}

/// The class of `entity::op`, declared beside the wildcard ops this
/// dispatcher synthesizes.
pub fn op_class(entity: &str, op: &str) -> OpClass {
    match (entity, op) {
        ("*", "rebuild_views") => OpClass::Maintenance,
        _ => OpClass::Interaction,
    }
}

#[async_trait]
impl OperationProvider for OperationDispatcher {
    /// The one rollback-capable provider in the wired set.
    ///
    /// Runtime-declared providers are not searched: a declared type brings its
    /// own table, never the block store whose version a batch is measured
    /// against.
    fn batch_rollback(&self) -> Option<&dyn holon_core::batch_rollback::BatchRollback> {
        let mut found = self.providers.iter().filter_map(|p| p.batch_rollback());
        let first = found.next();
        assert!(
            found.next().is_none(),
            "two providers offer batch rollback; a batch point taken from one would carry the \
             other's store back to a version it never had"
        );
        first
    }

    /// Get all operations from all registered providers
    ///
    /// Aggregates operations from all providers and includes wildcard
    /// operations.
    fn operations(&self) -> Vec<OperationDescriptor> {
        let snapshot = self.snapshot_operations();
        let mut ops = snapshot.all_ops();

        // The wildcard ops this container can run — the same decision the
        // wildcard arm reads, so the menu offers no broadcast that arm refuses
        // and the arm accepts none this container never offered.
        ops.extend(
            self.offered_broadcasts(&snapshot)
                .iter()
                .map(|offered| offered.op().descriptor()),
        );

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

struct ViewRebuild {
    manager: Arc<crate::sync::MatviewManager>,
    bus: Arc<holon_api::ConditionBus>,
    running: std::sync::atomic::AtomicBool,
}

impl ViewRebuild {
    /// The bus holds one condition per key and no count, so a second
    /// concurrent rebuild would have its disclosure cleared by the first.
    fn start(&self) -> Result<RebuildRunning<'_>> {
        if self.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err("rebuild_views: a view rebuild is already running".into());
        }
        self.bus.emit(Self::condition());
        Ok(RebuildRunning(self))
    }

    fn condition() -> holon_api::Condition {
        holon_api::Condition {
            subject: holon_api::condition_bus::WATCH_VIEWS_SUBJECT.to_string(),
            reason: holon_api::ConditionKind::WatchViewsRebuilding,
        }
    }
}

/// The one running rebuild. Its condition stands exactly as long as this
/// lives, however the rebuild ends.
struct RebuildRunning<'a>(&'a ViewRebuild);

impl Drop for RebuildRunning<'_> {
    fn drop(&mut self) {
        self.0.bus.clear(&ViewRebuild::condition().condition_key());
        self.0
            .running
            .store(false, std::sync::atomic::Ordering::SeqCst);
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
            let bus = r.resolve_async::<Arc<holon_api::ConditionBus>>().await;
            dispatcher.set_view_rebuild(matview_mgr, (*bus).clone());
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

            // The write-tier gate. Answering needs a block reader and the
            // vault's format registry, so a composition root with a vault
            // registers one; a container without files registers none and
            // every block it holds is writable.
            if let Some(authority) = r
                .optional_resolve_async::<dyn holon_core::WriteTierAuthority>()
                .await
            {
                dispatcher.set_write_tier_authority(authority);
            }

            // The shape gate. A composition root that registers tagged shapes
            // gets it; it reads the pre-write state from the block write
            // authority, which is the SQL tables when no separate one exists.
            {
                let validators = r.resolve_async::<holon_core::ShapeValidators>().await;
                let authority = match r
                    .optional_resolve_async::<dyn holon_core::WriteAuthorityReads>()
                    .await
                {
                    Some(authority) => authority,
                    None => Arc::new(crate::core::sql_write_authority::SqlWriteAuthority::new(
                        db_handle_provider.handle(),
                    )) as Arc<dyn holon_core::WriteAuthorityReads>,
                };
                dispatcher.set_shape_gate(validators, authority, (*bus).clone());
            }

            // A container that switched an entity off says which setting did
            // it; one that registers nothing keeps the plain not-found answer.
            let unserved = match r
                .optional_resolve_async::<crate::di::DbReady<crate::di::schema_providers::FreeStandingTypeViews>>()
                .await
            {
                Some(_) => r.resolve_async::<crate::di::schema_providers::UnservedTypes>().await.entries(),
                None => Default::default(),
            };
            let mut unavailable = r
                .optional_resolve_async::<UnavailableEntities>()
                .await
                .map(|u| (*u).clone())
                .unwrap_or_default();
            unavailable.0.extend(
                unserved
                    .iter()
                    .map(|(name, reason)| (EntityName::new(name.as_str()), reason.clone())),
            );
            dispatcher.set_unavailable_entities(unavailable);
            if let Some(attribution) = r
                .optional_resolve_async::<holon_core::integration_attribution::IntegrationAttribution>()
                .await
            {
                dispatcher.set_integration_attribution((*attribution).clone());
            }

            // Every served free-standing type gets a write authority derived
            // from ITS definition, over the Turso serialization
            // `FreeStandingTypeViews` created. An unserved type gets none, so
            // its writes fail with the reason recorded above.
            let type_registry = r.resolve_async::<holon_profiles::TypeRegistry>().await;
            for type_def in type_registry.all() {
                if unserved.contains_key(type_def.name.as_str()) {
                    continue;
                }
                crate::core::type_declaration::derive_write_authority(
                    &type_def,
                    &db_handle_provider.handle(),
                    &dispatcher,
                )
                .and_then(|()| {
                    crate::core::type_declaration::register_companion_operations(
                        &type_def.name,
                        &db_handle_provider.handle(),
                        &dispatcher,
                    )
                })
                .unwrap_or_else(|e| {
                    panic!(
                        "[OperationModule] write authority for free-standing type '{}': {e}",
                        type_def.name
                    )
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
            dispatcher
                .assert_shape_gate_installed()
                .expect("[OperationModule] shape-gate startup check failed");
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
            if matches!(op_name, "test_op" | "set_field" | "create") {
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
            optional_params: vec![],
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

    /// The guard never reads the authority: it checks what is installed.
    struct Unread;

    use holon_api::EntityUri;

    #[async_trait]
    impl holon_core::WriteAuthorityReads for Unread {
        async fn block_exists(&self, _: &EntityUri) -> Result<bool> {
            unreachable!("the install guard reads no block")
        }
        async fn block_is_page(&self, _: &EntityUri) -> Result<bool> {
            unreachable!("the install guard reads no block")
        }
        async fn block(&self, _: &EntityUri) -> Result<Option<holon_api::Block>> {
            unreachable!("the install guard reads no block")
        }
        async fn subtree(&self, _: &EntityUri) -> Result<Option<Vec<holon_api::Block>>> {
            unreachable!("the install guard reads no block")
        }
        async fn children(&self, _: &EntityUri) -> Result<Vec<EntityUri>> {
            unreachable!("the install guard reads no block")
        }
    }

    fn gated(validators: holon_core::ShapeValidators) -> OperationDispatcher {
        let mut dispatcher = OperationDispatcher::new(vec![]);
        dispatcher.set_shape_gate(
            Arc::new(validators),
            Arc::new(Unread),
            Arc::new(holon_api::ConditionBus::new()),
        );
        dispatcher
    }

    #[test]
    fn a_shape_gate_without_the_registered_validators_fails_the_install_guard() {
        let err = gated(holon_core::ShapeValidators::new(vec![]))
            .assert_shape_gate_installed()
            .expect_err("an empty registry judges nothing and must not pass as installed");
        assert!(err.to_string().contains("decision"), "{err}");
        gated(holon_core::ShapeValidators::registered())
            .assert_shape_gate_installed()
            .expect("the registered validators pass");
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

        let mut params = StorageEntity::new();
        params.insert("id".into(), holon_api::Value::String("block:a".into()));
        params.insert(
            "field".into(),
            holon_api::Value::String("after_block_id".into()),
        );
        params.insert("value".into(), holon_api::Value::String("block:b".into()));
        let err = dispatcher
            .execute_operation(&EntityName::new("block"), "set_field", params)
            .await
            .expect_err("set_field over an order key must be rejected at the boundary");
        let msg = err.to_string();
        assert!(
            msg.contains("order key") && msg.contains("after_block_id"),
            "rejection must name the invariant and the offending field, got: {msg}"
        );
    }

    /// Model.md invariant 16 at the intent boundary: neither generic write
    /// reaches a private field, whatever the mode or the provider.
    #[tokio::test]
    async fn block_generic_writes_reject_private_fields_at_intent_boundary() {
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![
                create_test_operation("block", "set_field"),
                create_test_operation("block", "update"),
            ],
        });
        let dispatcher = OperationDispatcher::new(vec![crud]);

        for (field, private) in holon_api::schema::BLOCK.private_fields() {
            let expected = holon_api::BlockWriteFieldError::Private {
                field: field.to_string(),
                route: private.route,
            }
            .to_string();

            let mut set_field = StorageEntity::new();
            set_field.insert("id".into(), holon_api::Value::String("block:a".into()));
            set_field.insert("field".into(), holon_api::Value::String(field.into()));
            set_field.insert("value".into(), holon_api::Value::String("block:b".into()));
            let mut update = StorageEntity::new();
            update.insert("id".into(), holon_api::Value::String("block:a".into()));
            update.insert(field.into(), holon_api::Value::String("block:b".into()));

            for (op, params) in [("set_field", set_field), ("update", update)] {
                let err = dispatcher
                    .execute_operation(&EntityName::new("block"), op, params)
                    .await
                    .expect_err("a generic write of a private field must be refused");
                assert!(
                    err.to_string().contains(&expected),
                    "{op} of {field}: expected {expected:?}, got: {err}"
                );
            }
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

    /// A `set_field` naming the engine-owned bag is refused at the boundary
    /// (ruling D126.a).
    #[tokio::test]
    async fn block_set_field_rejects_the_whole_properties_bag() {
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "set_field")],
        });
        let dispatcher = OperationDispatcher::new(vec![crud]);

        let mut params = StorageEntity::new();
        params.insert("id".into(), holon_api::Value::String("block:a".into()));
        params.insert(
            "field".into(),
            holon_api::Value::String("properties".into()),
        );
        params.insert(
            "value".into(),
            holon_api::Value::String(r#"{"Probe":"2026-08-22T10:00:00Z"}"#.into()),
        );
        let err = dispatcher
            .execute_operation(&EntityName::new("block"), "set_field", params)
            .await
            .expect_err("a whole-bag set_field must be rejected at the boundary");
        let msg = err.to_string();
        assert!(
            msg.contains("set_field(\"properties\")") && msg.contains("property key"),
            "the refusal must name the offending write and the per-property route, got: {msg}"
        );
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

    #[tokio::test]
    async fn rebuild_views_rebuilds_past_a_view_it_cannot_and_names_both() {
        use futures::StreamExt;
        use holon_turso::matview_manager::MatviewManager;

        let (backend, db) = holon_turso::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory db");
        std::mem::forget(backend);
        for ddl in [
            "CREATE TABLE items (id TEXT PRIMARY KEY, content TEXT DEFAULT '')",
            "CREATE TABLE gone (id TEXT PRIMARY KEY)",
        ] {
            db.execute_ddl(ddl).await.expect("create table");
        }
        let manager = Arc::new(MatviewManager::new(
            db.clone(),
            Arc::new(tokio::sync::Mutex::new(())),
        ));
        let (healthy, mut healthy_stream) = manager
            .ensure_and_subscribe("SELECT id, content FROM items", None)
            .await
            .expect("subscribe healthy");
        let (broken, _broken_stream) = manager
            .ensure_and_subscribe("SELECT id FROM gone", None)
            .await
            .expect("subscribe broken");
        db.execute_ddl("DROP TABLE gone").await.expect("drop gone");
        let mut dispatcher = OperationDispatcher::new(vec![]);
        let bus = Arc::new(holon_api::ConditionBus::new());
        dispatcher.set_view_rebuild(manager.clone(), bus.clone());
        assert!(
            dispatcher
                .operations()
                .iter()
                .any(|op| op.entity_name == "*" && op.name == "rebuild_views"),
            "rebuild_views must be advertised where a matview manager is wired"
        );

        let err = dispatcher
            .execute_operation(&EntityName::new("*"), "rebuild_views", StorageEntity::new())
            .await
            .expect_err("a listened view over a dropped table cannot be rebuilt")
            .to_string();
        assert!(
            err.contains(&broken) && err.contains(&healthy),
            "the error must name the view that failed ({broken}) and the one rebuilt \
             ({healthy}): {err}"
        );
        assert!(
            bus.current().is_empty(),
            "the failed rebuild left its condition raised: {:?}",
            bus.current()
        );

        db.execute(
            "INSERT INTO items (id, content) VALUES ('after', 'x')",
            Vec::new(),
        )
        .await
        .expect("insert");
        tokio::time::timeout(std::time::Duration::from_secs(5), healthy_stream.next())
            .await
            .unwrap_or_else(|_| panic!("{healthy} was not recreated past the failure"))
            .expect("the healthy stream ended");
    }

    #[tokio::test]
    async fn a_rebuild_overlapping_a_running_one_is_refused_and_the_disclosure_stands() {
        use holon_turso::matview_manager::MatviewManager;

        let (backend, db) = holon_turso::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory db");
        std::mem::forget(backend);
        db.execute_ddl("CREATE TABLE items (id TEXT PRIMARY KEY, content TEXT DEFAULT '')")
            .await
            .expect("create items");
        let manager = Arc::new(MatviewManager::new(
            db.clone(),
            Arc::new(tokio::sync::Mutex::new(())),
        ));
        let (_view, _stream) = manager
            .ensure_and_subscribe("SELECT id, content FROM items", None)
            .await
            .expect("subscribe");
        let mut dispatcher = OperationDispatcher::new(vec![]);
        let bus = Arc::new(holon_api::ConditionBus::new());
        dispatcher.set_view_rebuild(manager, bus.clone());
        let rebuilding = || {
            bus.current()
                .iter()
                .any(|c| c.condition_key().kind == holon_api::ConditionKind::WATCH_VIEWS_REBUILDING)
        };
        let wildcard = EntityName::new("*");
        let rebuild =
            || dispatcher.execute_operation(&wildcard, "rebuild_views", StorageEntity::new());

        let mut first = Box::pin(rebuild());
        assert!(
            futures::poll!(first.as_mut()).is_pending(),
            "the first rebuild never yielded, so nothing can overlap it"
        );
        let mut second = Box::pin(rebuild());
        let second_on_first_poll = futures::poll!(second.as_mut());
        first.await.expect("the first rebuild");

        if second_on_first_poll.is_pending() {
            assert!(
                rebuilding(),
                "the second rebuild is still running and the bus does not say so: {:?}",
                bus.current()
            );
        }
        let err = match second_on_first_poll {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => second.await,
        }
        .expect_err("a rebuild overlapping a running one must be refused")
        .to_string();
        assert!(
            err.contains("a view rebuild is already running"),
            "the refusal must say why: {err}"
        );
        assert!(!rebuilding(), "no rebuild runs and the condition stands");
    }

    /// A provider whose `sync` writes one row, as a real sync writes what it
    /// fetched.
    struct SyncingProvider {
        db: holon_turso::turso::DbHandle,
        synced: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl OperationProvider for SyncingProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![create_test_operation("items", "sync")]
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            op_name: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            assert_eq!(op_name, "sync", "only sync is advertised");
            self.db
                .execute(
                    "INSERT INTO items (id, content) VALUES ('synced', 'x')",
                    Vec::new(),
                )
                .await?;
            self.synced.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(OperationResult::irreversible(Vec::new()))
        }
    }

    #[tokio::test]
    async fn full_sync_syncs_without_touching_the_watch_views() {
        use futures::StreamExt;
        use holon_turso::matview_manager::MatviewManager;

        let (backend, db) = holon_turso::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory db");
        std::mem::forget(backend);
        db.execute_ddl("CREATE TABLE items (id TEXT PRIMARY KEY, content TEXT DEFAULT '')")
            .await
            .expect("create items");
        let manager = Arc::new(MatviewManager::new(
            db.clone(),
            Arc::new(tokio::sync::Mutex::new(())),
        ));
        let (view, mut stream) = manager
            .ensure_and_subscribe("SELECT id, content FROM items", None)
            .await
            .expect("subscribe");
        let provider = Arc::new(SyncingProvider {
            db: db.clone(),
            synced: std::sync::atomic::AtomicBool::new(false),
        });
        let mut dispatcher = OperationDispatcher::new(vec![provider.clone()]);
        dispatcher.set_view_rebuild(manager.clone(), Arc::new(holon_api::ConditionBus::new()));
        let unlistened = manager
            .ensure_view("SELECT id FROM items")
            .await
            .expect("ensure unlistened view");
        let watch_views = || async {
            db.query(
                "SELECT name FROM sqlite_master WHERE name LIKE 'watch_view_%' ORDER BY name",
                std::collections::HashMap::new(),
            )
            .await
            .expect("read sqlite_master")
        };
        let before = watch_views().await;
        assert_eq!(
            before.len(),
            2,
            "{view} and {unlistened} must exist before full_sync"
        );

        dispatcher
            .execute_operation(&EntityName::new("*"), "full_sync", StorageEntity::new())
            .await
            .expect("full_sync");

        assert!(
            provider.synced.load(std::sync::atomic::Ordering::SeqCst),
            "full_sync must run the providers' sync"
        );
        assert_eq!(
            watch_views().await,
            before,
            "full_sync must drop no watch view; a view rebuild would drop the unlistened \
             {unlistened}"
        );
        let batch = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .unwrap_or_else(|_| panic!("the subscriber of {view} never saw the synced row"))
            .expect("the stream ended");
        assert!(
            batch.inner.items.iter().any(|item| matches!(
                &item.change,
                holon_api::Change::Created { data, .. }
                    if data.get("id") == Some(&holon_api::Value::String("synced".into()))
            )),
            "the subscriber of {view} must be sent the row the sync wrote: {batch:?}"
        );
    }

    #[test]
    fn only_rebuild_views_is_a_maintenance_op() {
        assert_eq!(op_class("*", "rebuild_views"), OpClass::Maintenance);
        for (entity, op) in [("*", "full_sync"), ("*", "sync"), ("block", "set_field")] {
            assert_eq!(op_class(entity, op), OpClass::Interaction, "{entity}::{op}");
        }
    }

    /// Refuses one named op and confirms every other, so a test can tell a
    /// gated write apart from a gate that never ran.
    struct RefuseOp(&'static str);

    #[async_trait]
    impl crate::api::net_guard::NetGuard for RefuseOp {
        async fn check(
            &self,
            op: &crate::api::net_guard::NetGuardOp<'_>,
        ) -> Result<crate::api::net_guard::NetVerdict> {
            if op.op_name != self.0 {
                return Ok(crate::api::net_guard::NetVerdict::Confirm);
            }
            Ok(crate::api::net_guard::NetVerdict::Refuse(
                crate::api::net_guard::NetRefusal {
                    class: crate::api::net_guard::RefusalClass::Authorization,
                    reason: format!("the test policy refuses every {}", self.0),
                },
            ))
        }
    }

    fn set_field_params() -> StorageEntity {
        let mut params = StorageEntity::new();
        params.insert("id".into(), holon_api::Value::String("block:a".into()));
        params.insert("field".into(), holon_api::Value::String("content".into()));
        params.insert("value".into(), holon_api::Value::String("v".into()));
        params
    }

    #[tokio::test]
    async fn the_wildcard_entity_carries_no_write_past_the_net_gate() {
        let crud = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "set_field")],
        });
        let mut dispatcher = OperationDispatcher::new(vec![crud]);
        dispatcher.set_net_guard(Arc::new(RefuseOp("set_field")));

        let named = dispatcher
            .execute_operation(&EntityName::new("block"), "set_field", set_field_params())
            .await
            .expect_err("entity `block` must be refused by the net gate")
            .to_string();
        assert!(
            named.contains("net-guard refusal"),
            "the named dispatch must be refused BY THE NET GATE, else this test proves nothing \
             about the wildcard arm: {named}"
        );

        let wildcard = dispatcher
            .execute_operation(&EntityName::new("*"), "set_field", set_field_params())
            .await
            .expect_err(
                "entity `*` must not carry `set_field` to a provider: `*` names no relation, so \
                 the net gate, the boundary seam, the declared guard and the write tier have no \
                 subject to judge and the write would run unjudged",
            )
            .to_string();
        assert!(
            wildcard.contains("set_field") && wildcard.contains("sync"),
            "the refusal must name the op refused and the broadcast ops that exist: {wildcard}"
        );
    }

    #[tokio::test]
    async fn a_broadcast_fan_out_asks_the_gates_a_named_dispatch_asks() {
        let syncable = Arc::new(MockProvider {
            entity_name: "block".to_string(),
            operations_list: vec![create_test_operation("block", "sync")],
        });
        let mut dispatcher = OperationDispatcher::new(vec![syncable]);
        dispatcher.set_net_guard(Arc::new(RefuseOp("sync")));

        let err = dispatcher
            .execute_operation(&EntityName::new("*"), "sync", StorageEntity::new())
            .await
            .expect_err(
                "a broadcast reaches each provider under that provider's own entity name, so the \
                 gates that judge a named dispatch of it must judge the fan-out too",
            )
            .to_string();
        assert!(
            err.contains("net-guard refusal") && err.contains("block.sync"),
            "the fan-out must be refused by the net gate, naming the provider entity it ran \
             under: {err}"
        );
    }

    #[test]
    fn the_broadcast_set_is_exactly_the_wildcard_ops_the_dispatcher_advertises() {
        for (op, parsed) in [
            ("sync", BroadcastOp::Sync),
            ("full_sync", BroadcastOp::FullSync),
            ("rebuild_views", BroadcastOp::RebuildViews),
        ] {
            assert_eq!(BroadcastOp::parse(op).expect("a broadcast op"), parsed);
            assert_eq!(parsed.as_str(), op);
        }
        for op in ["set_field", "delete", "create", "clear_cache", ""] {
            let err = BroadcastOp::parse(op)
                .expect_err("only the three synthesized wildcard ops broadcast")
                .to_string();
            assert!(
                err.contains(op) && err.contains("rebuild_views"),
                "the refusal must name the op and the closed set: {err}"
            );
        }
    }

    /// Succeeds on every op it advertises and records each subject param it is
    /// handed, so a test can tell a refused broadcast from one that reached a
    /// provider carrying a subject.
    struct RecordingProvider {
        subjects_seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl OperationProvider for RecordingProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![
                create_test_operation("block", "sync"),
                create_test_operation("block", "clear_cache"),
            ]
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            op_name: &str,
            params: StorageEntity,
        ) -> Result<OperationResult> {
            let mut seen = self
                .subjects_seen
                .lock()
                .expect("no test panics holding it");
            for key in SUBJECT_PARAM_KEYS {
                if let Some(subject) = params.get(key).and_then(|v| v.as_string()) {
                    seen.push(format!("{op_name} got {key}={subject}"));
                }
            }
            Ok(OperationResult::irreversible(Vec::new()))
        }
    }

    /// Every broadcast names no subject, which is the premise that lets the
    /// wildcard arm run without the subject-bound gates: under `*` there is no
    /// relation for the boundary seam, the declared guard or the write tier to
    /// bind a subject to, so a subject arriving there would be judged against
    /// the descriptor of whichever provider advertises the broadcast.
    ///
    /// The `match` is exhaustive on purpose. A new [`BroadcastOp`] must say
    /// here which param keys it names a subject in; any answer but none means
    /// the op belongs under that subject's entity and cannot broadcast.
    #[tokio::test]
    async fn every_broadcast_op_is_subjectless() {
        let provider = Arc::new(RecordingProvider {
            subjects_seen: std::sync::Mutex::new(Vec::new()),
        });
        let dispatcher = OperationDispatcher::new(vec![provider.clone()]);

        for broadcast in BroadcastOp::ALL {
            let subject_keys: &[&str] = match broadcast {
                BroadcastOp::Sync | BroadcastOp::FullSync | BroadcastOp::RebuildViews => &[],
            };
            assert!(
                subject_keys.is_empty(),
                "`*::{broadcast}` declares the subject params {subject_keys:?}, so it names a \
                 relation and cannot be broadcast"
            );

            for key in SUBJECT_PARAM_KEYS {
                let mut params = StorageEntity::new();
                params.insert(key.into(), holon_api::Value::String("block:a".into()));
                let refusal = dispatcher
                    .execute_operation(&EntityName::new("*"), broadcast.as_str(), params)
                    .await
                    .expect_err(&format!(
                        "`*::{broadcast}` must refuse the subject param `{key}`: it reaches every \
                         provider advertising {broadcast} judged only by that provider's \
                         subjectless descriptor"
                    ))
                    .to_string();
                assert!(
                    refusal.contains(&format!("params carry '{key}'"))
                        && refusal.contains(broadcast.as_str()),
                    "the refusal must name the subject param and the broadcast: {refusal}"
                );
            }
        }

        dispatcher
            .execute_operation(
                &EntityName::new("*"),
                BroadcastOp::Sync.as_str(),
                StorageEntity::new(),
            )
            .await
            .expect(
                "the same dispatcher runs a subjectless broadcast, so the refusals above are \
                 attributable to the subject param and not to this wiring",
            );

        let reached = provider
            .subjects_seen
            .lock()
            .expect("no test panics holding it");
        assert!(
            reached.is_empty(),
            "a broadcast carried a subject into a provider: {reached:?}"
        );
    }

    /// The wildcard ops a composition ADVERTISES and the ones its wildcard arm
    /// ACCEPTS must be the same set, in both directions. An op advertised but
    /// refused is a menu entry that fails. An op accepted but not advertised is
    /// a caller-reachable effect no composition offered — which `*::full_sync`
    /// was: on a dispatcher with no syncable provider it cleared every sync
    /// token and reported success.
    #[tokio::test]
    async fn the_wildcard_ops_a_dispatcher_advertises_are_exactly_the_ones_it_accepts() {
        for (syncable, rebuild_wired) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let providers: Vec<Arc<dyn OperationProvider>> = if syncable {
                vec![FanOutProbe::new("alpha", &[])]
            } else {
                Vec::new()
            };
            let mut dispatcher = OperationDispatcher::new(providers);
            if rebuild_wired {
                let (backend, db) = holon_turso::turso::TursoBackend::new_in_memory()
                    .await
                    .expect("in-memory db");
                std::mem::forget(backend);
                dispatcher.set_view_rebuild(
                    Arc::new(crate::sync::MatviewManager::new(
                        db,
                        Arc::new(tokio::sync::Mutex::new(())),
                    )),
                    Arc::new(holon_api::ConditionBus::new()),
                );
            }
            let composition = format!("syncable={syncable}, rebuild wired={rebuild_wired}");

            let advertised: std::collections::BTreeSet<String> = dispatcher
                .operations()
                .into_iter()
                .filter(|op| op.entity_name.is_wildcard())
                .map(|op| op.name)
                .collect();
            // Measured, so the comparison below cannot pass by both sides
            // being empty: a syncable provider earns `sync` + `full_sync`, a
            // wired view rebuild earns `rebuild_views`.
            assert_eq!(
                advertised.len(),
                usize::from(syncable) * 2 + usize::from(rebuild_wired),
                "{composition}: advertised {advertised:?}"
            );

            let mut accepted = std::collections::BTreeSet::new();
            for broadcast in BroadcastOp::ALL {
                let outcome = dispatcher
                    .execute_operation(
                        &EntityName::new("*"),
                        broadcast.as_str(),
                        StorageEntity::new(),
                    )
                    .await;
                let not_offered = match &outcome {
                    Err(e) => e.downcast_ref::<BroadcastNotOffered>().is_some(),
                    Ok(_) => false,
                };
                if !not_offered {
                    accepted.insert(broadcast.as_str().to_string());
                }
            }
            assert_eq!(accepted, advertised, "{composition}");
        }
    }

    /// Advertises `sync` and `clear_cache` under its own entity name, records
    /// every call that RAN, and fails the ops named in `fails`.
    struct FanOutProbe {
        entity: &'static str,
        fails: &'static [&'static str],
        ran: std::sync::Mutex<Vec<String>>,
    }

    impl FanOutProbe {
        fn new(entity: &'static str, fails: &'static [&'static str]) -> Arc<Self> {
            Arc::new(Self {
                entity,
                fails,
                ran: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn ran(&self) -> Vec<String> {
            self.ran.lock().expect("no test panics holding it").clone()
        }
    }

    #[async_trait]
    impl OperationProvider for FanOutProbe {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![
                create_test_operation(self.entity, "sync"),
                create_test_operation(self.entity, "clear_cache"),
            ]
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            op_name: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            self.ran
                .lock()
                .expect("no test panics holding it")
                .push(format!("{}.{op_name}", self.entity));
            if self.fails.iter().any(|failing| *failing == op_name) {
                return Err(format!("{}.{op_name}: unreachable in this test", self.entity).into());
            }
            Ok(OperationResult::irreversible(Vec::new()))
        }
    }

    /// Advertises `sync` on the first `operations()` call after
    /// [`Self::arm`] and on no other, so a dispatch that reads its operations
    /// more than once sees the provider vanish.
    struct FlappingProvider {
        armed: std::sync::atomic::AtomicBool,
        ran: std::sync::Mutex<Vec<String>>,
    }

    impl FlappingProvider {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                armed: std::sync::atomic::AtomicBool::new(false),
                ran: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn arm(&self) {
            self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn ran(&self) -> Vec<String> {
            self.ran.lock().expect("no test panics holding it").clone()
        }
    }

    #[async_trait]
    impl OperationProvider for FlappingProvider {
        fn operations(&self) -> Vec<OperationDescriptor> {
            if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                vec![create_test_operation("flap", "sync")]
            } else {
                Vec::new()
            }
        }

        async fn execute_operation(
            &self,
            _: &EntityName,
            op_name: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            self.ran
                .lock()
                .expect("no test panics holding it")
                .push(op_name.to_string());
            Ok(OperationResult::irreversible(Vec::new()))
        }
    }

    /// The broadcast the dispatcher offered is the broadcast it judges and
    /// runs: a provider whose `operations()` changes between two reads inside
    /// one dispatch must not turn an offered `sync` into an empty fan-out.
    #[tokio::test]
    async fn a_provider_that_stops_advertising_sync_mid_dispatch_is_still_run() {
        for broadcast in ["sync", "full_sync"] {
            let flap = FlappingProvider::new();
            let dispatcher = OperationDispatcher::new(vec![flap.clone()]);
            flap.arm();

            dispatcher
                .execute_operation(&EntityName::new("*"), broadcast, StorageEntity::new())
                .await
                .unwrap_or_else(|e| panic!("`*::{broadcast}` was offered, so it must run: {e}"));

            assert_eq!(
                flap.ran(),
                vec!["sync".to_string()],
                "`*::{broadcast}` is offered on the strength of one read of the provider's \
                 operations, so that read decides who runs"
            );
        }
    }

    /// Records whether the sync tokens were cleared — `full_sync`'s first
    /// effect, and so the proof that some of it ran.
    #[derive(Default)]
    struct TokenProbe {
        cleared: std::sync::atomic::AtomicBool,
    }

    impl TokenProbe {
        fn cleared(&self) -> bool {
            self.cleared.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SyncTokenStore for TokenProbe {
        async fn load_token(&self, _: &str) -> Result<Option<holon_api::StreamPosition>> {
            Ok(None)
        }

        async fn save_token(&self, _: &str, _: holon_api::StreamPosition) -> Result<()> {
            Ok(())
        }

        async fn clear_all_tokens(&self) -> Result<()> {
            self.cleared
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Refuses every operation on the entities named here, so a test can tell a
    /// fan-out that judged all its members BEFORE running any apart from one
    /// that judged each member as it reached it.
    struct RefuseEntities(&'static [&'static str]);

    #[async_trait]
    impl crate::api::net_guard::NetGuard for RefuseEntities {
        async fn check(
            &self,
            op: &crate::api::net_guard::NetGuardOp<'_>,
        ) -> Result<crate::api::net_guard::NetVerdict> {
            if !self.0.iter().any(|entity| *entity == op.entity_name) {
                return Ok(crate::api::net_guard::NetVerdict::Confirm);
            }
            Ok(crate::api::net_guard::NetVerdict::Refuse(
                crate::api::net_guard::NetRefusal {
                    class: crate::api::net_guard::RefusalClass::Authorization,
                    reason: format!("the test policy refuses every op on {}", op.entity_name),
                },
            ))
        }
    }

    /// A broadcast that lost every provider it fanned out to did not happen.
    /// `*::sync` says so; `*::full_sync` discarded the same count and reported
    /// success, so the two disagreed about one outcome.
    #[tokio::test]
    async fn a_full_sync_that_lost_every_provider_fails_loudly() {
        let alpha = FanOutProbe::new("alpha", &["sync", "clear_cache"]);
        let bravo = FanOutProbe::new("bravo", &["sync", "clear_cache"]);
        let tokens = Arc::new(TokenProbe::default());
        let mut dispatcher = OperationDispatcher::new(vec![alpha.clone(), bravo.clone()]);
        dispatcher.set_sync_token_store(tokens.clone());

        let err = dispatcher
            .execute_operation(&EntityName::new("*"), "full_sync", StorageEntity::new())
            .await
            .expect_err(
                "every provider failed both legs, so nothing of the full sync happened; \
                 reporting success leaves the caller believing in a re-sync it never got",
            )
            .to_string();

        assert!(
            err.contains("alpha") && err.contains("bravo"),
            "the error must name every provider it lost: {err}"
        );
        assert!(
            err.contains("sync token"),
            "the tokens are cleared before any provider runs, so the error must name that as \
             what already ran: {err}"
        );
        assert!(
            tokens.cleared(),
            "the token clear is what the error has to disclose, so it must have run"
        );
    }

    /// The lost leg is named, and the leg that did run is not reported as lost:
    /// the caches were not cleared, but the sync leg ran on every provider.
    #[tokio::test]
    async fn a_full_sync_that_lost_the_clear_cache_leg_names_that_leg_only() {
        let alpha = FanOutProbe::new("alpha", &["clear_cache"]);
        let dispatcher = OperationDispatcher::new(vec![alpha.clone()]);

        let err = dispatcher
            .execute_operation(&EntityName::new("*"), "full_sync", StorageEntity::new())
            .await
            .expect_err("the cache clear lost every provider, so the full sync did not happen")
            .to_string();

        assert!(
            err.contains("nothing of the clear_cache leg happened"),
            "the error must name the leg that was lost: {err}"
        );
        assert!(
            !err.contains("NOT re-synced"),
            "the sync leg ran on every provider, so the error must not claim it did not: {err}"
        );
        assert!(
            err.contains("sync ran on [alpha] and failed on []"),
            "the error must state what did run: {err}"
        );
        assert_eq!(
            alpha.ran(),
            vec!["alpha.clear_cache".to_string(), "alpha.sync".to_string()],
            "both legs ran, in this order"
        );
    }

    /// The mirror case: the sync leg is the lost one, the cache clear ran.
    #[tokio::test]
    async fn a_full_sync_that_lost_the_sync_leg_names_that_leg_only() {
        let alpha = FanOutProbe::new("alpha", &["sync"]);
        let dispatcher = OperationDispatcher::new(vec![alpha]);

        let err = dispatcher
            .execute_operation(&EntityName::new("*"), "full_sync", StorageEntity::new())
            .await
            .expect_err("the sync lost every provider, so the vault was not re-synced")
            .to_string();

        assert!(
            err.contains("nothing of the sync leg happened"),
            "the error must name the leg that was lost: {err}"
        );
        assert!(
            !err.contains("nothing of the clear_cache leg happened"),
            "the cache clear ran, so the error must not claim it did not: {err}"
        );
        assert!(
            err.contains("clear_cache ran on [alpha] and failed on []"),
            "the error must state what did run: {err}"
        );
    }

    /// The other half of the same rule: one unreachable provider must not stop
    /// the others from syncing — but losing it is disclosed, not discarded.
    #[tokio::test]
    async fn a_broadcast_that_lost_one_provider_discloses_it_and_succeeds() {
        let alpha = FanOutProbe::new("alpha", &["sync"]);
        let bravo = FanOutProbe::new("bravo", &[]);
        let dispatcher = OperationDispatcher::new(vec![alpha, bravo]);

        let result = dispatcher
            .execute_operation(&EntityName::new("*"), "sync", StorageEntity::new())
            .await
            .expect("one unreachable provider must not stop the others from syncing");

        let disclosed = result
            .response
            .expect("a broadcast that lost a provider must say so in its result")
            .as_string()
            .expect("the disclosure is text")
            .to_string();
        assert!(
            disclosed.contains("alpha") && disclosed.contains("bravo"),
            "the disclosure must name the provider lost and the one that synced: {disclosed}"
        );
    }

    /// A gate refusal is a refusal of the WHOLE broadcast, so every member is
    /// judged before any of them runs. A refusal found half way through leaves
    /// effects no inverse describes — cleared sync tokens, one cache cleared,
    /// nothing re-synced — and names only the provider it stopped at, so the
    /// caller cannot see what else would refuse.
    #[tokio::test]
    async fn a_broadcast_whose_gates_refuse_runs_nothing() {
        let alpha = FanOutProbe::new("alpha", &[]);
        let bravo = FanOutProbe::new("bravo", &[]);
        let charlie = FanOutProbe::new("charlie", &[]);
        let tokens = Arc::new(TokenProbe::default());
        let mut dispatcher =
            OperationDispatcher::new(vec![alpha.clone(), bravo.clone(), charlie.clone()]);
        dispatcher.set_net_guard(Arc::new(RefuseEntities(&["bravo", "charlie"])));
        dispatcher.set_sync_token_store(tokens.clone());

        let err = dispatcher
            .execute_operation(&EntityName::new("*"), "full_sync", StorageEntity::new())
            .await
            .expect_err("the net gate refuses two of the three providers")
            .to_string();

        assert!(
            err.contains("bravo") && err.contains("charlie"),
            "the error must name EVERY refusing provider, or the caller clears one refusal and \
             meets the next: {err}"
        );
        assert_eq!(
            alpha.ran(),
            Vec::<String>::new(),
            "a refused broadcast runs nothing: the provider the fan-out reaches first ran {:?}",
            alpha.ran()
        );
        assert!(
            !tokens.cleared(),
            "a refused full_sync must clear no sync token, or the caller is left with a \
             half-cleared vault and no re-sync"
        );
    }

    /// A broadcast no provider can run has no effect and says which wiring it
    /// wanted. `*::full_sync` was reachable on a composition that advertises
    /// nothing under `*`, where it cleared every sync token past all four gates
    /// and reported success.
    #[tokio::test]
    async fn a_broadcast_no_provider_can_run_is_refused_with_no_effect() {
        let tokens = Arc::new(TokenProbe::default());
        let mut dispatcher = OperationDispatcher::new(vec![]);
        dispatcher.set_sync_token_store(tokens.clone());

        let mut outcomes = Vec::new();
        for broadcast in BroadcastOp::ALL {
            outcomes.push((
                broadcast,
                dispatcher
                    .execute_operation(
                        &EntityName::new("*"),
                        broadcast.as_str(),
                        StorageEntity::new(),
                    )
                    .await,
            ));
        }

        assert!(
            !tokens.cleared(),
            "a full_sync no provider can run must clear no sync token"
        );
        for (broadcast, outcome) in outcomes {
            let err = outcome.expect_err(&format!(
                "nothing here advertises `*::{broadcast}`, so there is nothing to broadcast it to"
            ));
            assert!(
                err.downcast_ref::<BroadcastNotOffered>().is_some(),
                "`*::{broadcast}` must be refused as a broadcast this composition cannot run: \
                 {err}"
            );
            let err = err.to_string();
            assert!(
                err.contains(broadcast.as_str()) && err.contains(broadcast.requires()),
                "the refusal must name the broadcast and the wiring it needs: {err}"
            );
        }
    }
}
