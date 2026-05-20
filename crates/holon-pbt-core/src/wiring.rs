//! Typed wiring manifest (ADR 0007).
//!
//! A [`Wiring`] declares which storage adapters, sync adapters, and actors
//! are present in a given composition of Holon. It is the single input to
//! both:
//!
//! - **PBT framework** — which transition alphabet to generate from and
//!   which invariants to run (each transition / invariant declares a
//!   [`RequiredWiring`] that the manifest must satisfy), and
//! - **production DI** (future) — which fragments to construct.
//!
//! This module is *pure data + rules*: no test or DI machinery lives here,
//! so production code can depend on it without pulling in proptest.
//!
//! ## Storage vs sync
//!
//! The categorical line (ADR 0007) is **event-loss tolerance**. A storage
//! adapter's authoritative state is local (file, embedded DB, in-process
//! CRDT) and its event stream MUST NOT be lossy. A sync adapter's
//! authoritative state is remote, the protocol tolerates event loss
//! (webhook misses, rate limits), and recovery is via re-fetch / reconcile.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A storage adapter: authoritative state is local; events are reliable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StorageAdapter {
    Loro,
    Org,
    Markdown,
    Turso,
}

impl StorageAdapter {
    /// Whether this adapter can answer block queries (the substrate an
    /// [`Actor::ActionEngine`] watcher needs). Today only Turso provides
    /// the IVM / PRQL query surface; the other storage adapters are
    /// write/serialize targets read back through Turso.
    pub fn is_query_capable(self) -> bool {
        matches!(self, StorageAdapter::Turso)
    }
}

/// A sync adapter: authoritative state is remote; events may be lost and
/// are reconciled by re-fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SyncAdapter {
    Todoist,
    GCal,
    GMail,
}

/// An actor: an in-process component that mutates the domain in response
/// to user / external stimulus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Actor {
    UI,
    MCPServer,
    ActionEngine,
}

/// Fixed ordering-authority priority (ADR 0007 §"ordering authority",
/// decision #4). The canonical sibling order is owned by the
/// highest-priority *present* storage adapter. Turso is lowest because it
/// is a projection/cache — it stores whatever order the owner assigns and
/// never reorders independently.
///
/// Reconciled against `BlockOrdering` + `file_sync_controller` (H7): in
/// every blessed manifest this reproduces today's behavior —
/// Full→`Loro`, sql_only(`{Turso,Org}`)→`Org`, loro_backend(`{Loro}`)→`Loro`,
/// org_create_ordering(`{Org}`)→`Org`. The org re-ingest place-loop has the
/// SQL owner *adopt* the org line order, so `Org > Turso` matches "Turso
/// never decides sibling order on its own."
const ORDERING_PRIORITY: [StorageAdapter; 4] = [
    StorageAdapter::Loro,
    StorageAdapter::Org,
    StorageAdapter::Markdown,
    StorageAdapter::Turso,
];

/// A typed manifest: which adapters and actors are wired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wiring {
    pub storage_adapters: BTreeSet<StorageAdapter>,
    pub sync_adapters: BTreeSet<SyncAdapter>,
    pub actors: BTreeSet<Actor>,
}

/// Why a [`Wiring`] is invalid. Each variant is one violated rule from the
/// validity table (ADR 0007 §"Validity of a manifest").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiringError {
    /// No storage adapter — there is nothing to display or persist.
    NoStorageAdapter,
    /// `MCPServer` is wired but no storage adapter backs it.
    McpServerWithoutStorage,
    /// `ActionEngine` is wired but no query-capable storage adapter
    /// (see [`StorageAdapter::is_query_capable`]) backs it.
    ActionEngineWithoutQueryAdapter,
}

impl std::fmt::Display for WiringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WiringError::NoStorageAdapter => {
                write!(
                    f,
                    "wiring has no storage adapter (nothing to display/persist)"
                )
            }
            WiringError::McpServerWithoutStorage => {
                write!(f, "Actor::MCPServer requires at least one storage adapter")
            }
            WiringError::ActionEngineWithoutQueryAdapter => write!(
                f,
                "Actor::ActionEngine requires a query-capable storage adapter (Turso)"
            ),
        }
    }
}

impl std::error::Error for WiringError {}

impl Wiring {
    /// Construct a custom (non-preset) manifest. The name is deliberately
    /// explicit: blessed manifests come from the preset constructors
    /// ([`Wiring::full`] etc.); reach for `custom` only when intentionally
    /// building an unblessed wiring (ADR 0007 weakness #4, the
    /// `#[must_bless]` guard — `is_blessed` reports whether the result is
    /// CI-blessed).
    pub fn custom(
        storage_adapters: impl IntoIterator<Item = StorageAdapter>,
        sync_adapters: impl IntoIterator<Item = SyncAdapter>,
        actors: impl IntoIterator<Item = Actor>,
    ) -> Self {
        Wiring {
            storage_adapters: storage_adapters.into_iter().collect(),
            sync_adapters: sync_adapters.into_iter().collect(),
            actors: actors.into_iter().collect(),
        }
    }

    // ── Blessed presets (ADR 0007 §"Blessed vs valid manifests") ────────

    /// `{Loro, Org, Markdown, Turso} + {Todoist} + {UI, MCPServer, ActionEngine}`
    /// — replaces `general_e2e_pbt_full`.
    pub fn full() -> Self {
        Self::custom(
            [
                StorageAdapter::Loro,
                StorageAdapter::Org,
                StorageAdapter::Markdown,
                StorageAdapter::Turso,
            ],
            [SyncAdapter::Todoist],
            [Actor::UI, Actor::MCPServer, Actor::ActionEngine],
        )
    }

    /// `{Turso, Org} + {} + {UI}` — replaces `general_e2e_pbt_sql_only`.
    pub fn sql_only() -> Self {
        Self::custom(
            [StorageAdapter::Turso, StorageAdapter::Org],
            [],
            [Actor::UI],
        )
    }

    /// `{Loro} + {} + {}` — replaces `loro_backend_pbt`.
    pub fn loro_backend() -> Self {
        Self::custom([StorageAdapter::Loro], [], [])
    }

    /// `{Org} + {} + {}` — replaces `org_create_ordering_pbt_full`.
    pub fn org_create_ordering() -> Self {
        Self::custom([StorageAdapter::Org], [], [])
    }

    /// The four manifests CI runs PBT against on every change. Adding to
    /// this list is a deliberate decision, not an automatic consequence of
    /// validity.
    pub fn blessed_manifests() -> Vec<Wiring> {
        vec![
            Wiring::full(),
            Wiring::sql_only(),
            Wiring::loro_backend(),
            Wiring::org_create_ordering(),
        ]
    }

    /// Whether this manifest is one CI is committed to (a blessed manifest).
    pub fn is_blessed(&self) -> bool {
        Self::blessed_manifests().iter().any(|m| m == self)
    }

    // ── Queries ─────────────────────────────────────────────────────────

    pub fn has_storage(&self, a: StorageAdapter) -> bool {
        self.storage_adapters.contains(&a)
    }

    pub fn has_sync(&self, a: SyncAdapter) -> bool {
        self.sync_adapters.contains(&a)
    }

    pub fn has_actor(&self, a: Actor) -> bool {
        self.actors.contains(&a)
    }

    /// The storage adapter that owns canonical sibling order, or `None`
    /// when no storage adapter is wired (an invalid manifest). Fixed
    /// priority `Loro > Org > Markdown > Turso` (see [`ORDERING_PRIORITY`]).
    pub fn ordering_authority(&self) -> Option<StorageAdapter> {
        ORDERING_PRIORITY
            .iter()
            .copied()
            .find(|a| self.has_storage(*a))
    }

    // ── Validity rules table (ADR 0007 §"Validity of a manifest") ───────

    /// Enforce the dependency graph between components. The set of rules is
    /// the architectural commitment for what combinations Holon can run as.
    pub fn validate(&self) -> Result<(), WiringError> {
        // Rule 1: at least one storage adapter (nothing to display/persist).
        if self.storage_adapters.is_empty() {
            return Err(WiringError::NoStorageAdapter);
        }
        // Rule 2: MCPServer requires at least one storage adapter. (Rule 1
        // already guarantees this, but it is kept explicit so removing
        // Rule 1 would not silently weaken the MCP contract.)
        if self.has_actor(Actor::MCPServer) && self.storage_adapters.is_empty() {
            return Err(WiringError::McpServerWithoutStorage);
        }
        // Rule 3: ActionEngine requires a query-capable storage adapter.
        if self.has_actor(Actor::ActionEngine)
            && !self.storage_adapters.iter().any(|a| a.is_query_capable())
        {
            return Err(WiringError::ActionEngineWithoutQueryAdapter);
        }
        Ok(())
    }
}

/// A boolean expression over tier-presence atoms (ADR 0007
/// §"`RequiredWiring` expressiveness"). Disjunction is required: "edit
/// content" needs *some* mutable storage adapter, not a specific one, and
/// flat subsets would force per-adapter transition copies.
///
/// A `RequiredWiring` is **necessary, not sufficient**: a transition whose
/// `RequiredWiring` is satisfied may still be rejected by its dynamic
/// precondition (e.g. "a block exists to edit"). The manifest gates
/// *structurally*; the generator gates *dynamically*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredWiring {
    /// Always satisfied.
    Any,
    HasStorage(StorageAdapter),
    HasSync(SyncAdapter),
    HasActor(Actor),
    /// Satisfied if *any* of the listed storage adapters is present.
    AnyStorageOf(BTreeSet<StorageAdapter>),
    /// Satisfied iff *every* sub-requirement is satisfied.
    All(Vec<RequiredWiring>),
    /// Satisfied iff *at least one* sub-requirement is satisfied.
    AnyOf(Vec<RequiredWiring>),
}

impl RequiredWiring {
    /// Convenience: `AnyStorageOf` from an iterator of adapters.
    pub fn any_storage_of(adapters: impl IntoIterator<Item = StorageAdapter>) -> Self {
        RequiredWiring::AnyStorageOf(adapters.into_iter().collect())
    }

    /// Whether `wiring` structurally satisfies this requirement.
    pub fn satisfied_by(&self, wiring: &Wiring) -> bool {
        match self {
            RequiredWiring::Any => true,
            RequiredWiring::HasStorage(a) => wiring.has_storage(*a),
            RequiredWiring::HasSync(a) => wiring.has_sync(*a),
            RequiredWiring::HasActor(a) => wiring.has_actor(*a),
            RequiredWiring::AnyStorageOf(set) => set.iter().any(|a| wiring.has_storage(*a)),
            RequiredWiring::All(reqs) => reqs.iter().all(|r| r.satisfied_by(wiring)),
            RequiredWiring::AnyOf(reqs) => reqs.iter().any(|r| r.satisfied_by(wiring)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn blessed_manifests_are_all_valid() {
        for m in Wiring::blessed_manifests() {
            assert!(m.validate().is_ok(), "blessed manifest invalid: {m:?}");
            assert!(m.is_blessed());
        }
    }

    #[test]
    fn empty_storage_is_invalid() {
        let w = Wiring::custom([], [], [Actor::UI]);
        assert_eq!(w.validate(), Err(WiringError::NoStorageAdapter));
    }

    #[test]
    fn action_engine_requires_query_adapter() {
        // Loro alone is not query-capable.
        let w = Wiring::custom([StorageAdapter::Loro], [], [Actor::ActionEngine]);
        assert_eq!(
            w.validate(),
            Err(WiringError::ActionEngineWithoutQueryAdapter)
        );
        // Adding Turso satisfies it.
        let w = Wiring::custom(
            [StorageAdapter::Loro, StorageAdapter::Turso],
            [],
            [Actor::ActionEngine],
        );
        assert!(w.validate().is_ok());
    }

    #[test]
    fn ordering_authority_matches_blessed_behavior() {
        assert_eq!(
            Wiring::full().ordering_authority(),
            Some(StorageAdapter::Loro)
        );
        assert_eq!(
            Wiring::sql_only().ordering_authority(),
            Some(StorageAdapter::Org)
        );
        assert_eq!(
            Wiring::loro_backend().ordering_authority(),
            Some(StorageAdapter::Loro)
        );
        assert_eq!(
            Wiring::org_create_ordering().ordering_authority(),
            Some(StorageAdapter::Org)
        );
    }

    #[test]
    fn required_wiring_disjunction() {
        let req = RequiredWiring::any_storage_of([StorageAdapter::Loro, StorageAdapter::Turso]);
        assert!(req.satisfied_by(&Wiring::full()));
        assert!(req.satisfied_by(&Wiring::sql_only())); // has Turso
        assert!(!req.satisfied_by(&Wiring::org_create_ordering())); // Org only
    }

    // ── Wiring validity PBT (ADR 0007 item 5) ───────────────────────────

    fn any_storage() -> impl Strategy<Value = StorageAdapter> {
        prop_oneof![
            Just(StorageAdapter::Loro),
            Just(StorageAdapter::Org),
            Just(StorageAdapter::Markdown),
            Just(StorageAdapter::Turso),
        ]
    }

    fn any_sync() -> impl Strategy<Value = SyncAdapter> {
        prop_oneof![
            Just(SyncAdapter::Todoist),
            Just(SyncAdapter::GCal),
            Just(SyncAdapter::GMail),
        ]
    }

    fn any_actor() -> impl Strategy<Value = Actor> {
        prop_oneof![
            Just(Actor::UI),
            Just(Actor::MCPServer),
            Just(Actor::ActionEngine),
        ]
    }

    prop_compose! {
        fn any_wiring()(
            storage in prop::collection::btree_set(any_storage(), 0..=4),
            sync in prop::collection::btree_set(any_sync(), 0..=3),
            actors in prop::collection::btree_set(any_actor(), 0..=3),
        ) -> Wiring {
            Wiring { storage_adapters: storage, sync_adapters: sync, actors }
        }
    }

    proptest! {
        /// `validate()`'s verdict agrees with the explicit rule table for
        /// every randomly drawn manifest (positive + negative coverage).
        #[test]
        fn validate_agrees_with_rule_table(w in any_wiring()) {
            let expected = if w.storage_adapters.is_empty() {
                Err(WiringError::NoStorageAdapter)
            } else if w.has_actor(Actor::ActionEngine)
                && !w.storage_adapters.iter().any(|a| a.is_query_capable())
            {
                Err(WiringError::ActionEngineWithoutQueryAdapter)
            } else {
                Ok(())
            };
            prop_assert_eq!(w.validate(), expected);
        }

        /// A valid manifest always has an ordering authority; an invalid
        /// one with no storage never does.
        #[test]
        fn ordering_authority_present_iff_has_storage(w in any_wiring()) {
            prop_assert_eq!(
                w.ordering_authority().is_some(),
                !w.storage_adapters.is_empty()
            );
        }
    }
}
