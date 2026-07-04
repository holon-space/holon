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

use proptest::strategy::{BoxedStrategy, Just, Strategy, Union};
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

// ── PBT generator (ADR 0007 item 5) ─────────────────────────────────
//
// Promoted out of `#[cfg(test)]` so the composed keystone
// (`general_e2e_composed_pbt`) can generate a shrinkable, validity-filtered
// manifest as the `init_state` of a `prop_state_machine!`. The keystone drives
// the composed `CapMap` from whatever wiring this yields, and on failure the
// manifest shrinks toward the minimal causal subsystem set.

/// Probability that a query-capable (expensive) storage adapter — today only
/// [`StorageAdapter::Turso`] — is included in a generated manifest. Turso boots
/// the full `BackendEngine`; every other storage adapter runs on the cheap
/// in-memory Loro backend. Biasing this low keeps most generated cases fast
/// while still exercising Turso (and shrinking it *out* first, since a weighted
/// bool shrinks `true → false`). The bias is explicit, not incidental: a
/// uniform subset would include Turso in ~half of all draws.
const QUERY_ADAPTER_INCLUSION_PROB: f64 = 0.15;

/// The toggleable adapter/actor universe a generated [`Wiring`] may draw from.
///
/// **Default = headless-faithful scope:** storage `{Loro, Org, Turso}`, sync
/// `{}`, actors `{MCPServer, ActionEngine}`. Deliberately **no [`Actor::UI`]**
/// (whether a headless `E2ESut` honestly executes editor transitions is
/// unverified, so editor transitions must not generate here yet) and no
/// `Markdown`/`GCal`/`GMail` (the SUT only faithfully backs Turso-vs-LoroMemory).
///
/// Overridable via `HOLON_PBT_WIRING_AXES`, three `;`-separated comma lists
/// `"storage;sync;actors"` — e.g. `"Loro,Turso,Org;Todoist;MCPServer,ActionEngine"`
/// or `"Loro,Turso;;"` to scope a run to two storage axes and no sync/actors.
/// **Fail-loud on a typo** (unknown token or wrong section count panics).
pub fn wiring_axes() -> (Vec<StorageAdapter>, Vec<SyncAdapter>, Vec<Actor>) {
    match std::env::var("HOLON_PBT_WIRING_AXES") {
        Err(_) => (
            vec![
                StorageAdapter::Loro,
                StorageAdapter::Org,
                StorageAdapter::Turso,
            ],
            vec![],
            vec![Actor::MCPServer, Actor::ActionEngine],
        ),
        Ok(spec) => parse_wiring_axes(&spec),
    }
}

/// Parse the `HOLON_PBT_WIRING_AXES` override. Fail-loud on a malformed spec.
fn parse_wiring_axes(spec: &str) -> (Vec<StorageAdapter>, Vec<SyncAdapter>, Vec<Actor>) {
    let sections: Vec<&str> = spec.split(';').collect();
    assert!(
        sections.len() == 3,
        "HOLON_PBT_WIRING_AXES must have exactly 3 ';'-separated sections \
         (storage;sync;actors), got {} in {spec:?}",
        sections.len()
    );
    (
        parse_axis(sections[0], parse_storage_adapter, "storage adapter"),
        parse_axis(sections[1], parse_sync_adapter, "sync adapter"),
        parse_axis(sections[2], parse_actor, "actor"),
    )
}

/// Parse an EXACT pinned manifest (the `HOLON_PBT_PIN_WIRING` format): the same
/// `"storage;sync;actors"` spec as `HOLON_PBT_WIRING_AXES`, but interpreted as
/// the exact component sets of ONE wiring rather than a drawable universe.
/// Fail-loud on a malformed spec or an invalid manifest -- a typo'd pin must
/// never silently test a different grid point.
pub fn wiring_from_exact_spec(spec: &str) -> Wiring {
    let (storage, sync, actors) = parse_wiring_axes(spec);
    let wiring = Wiring::custom(storage, sync, actors);
    if let Err(e) = wiring.validate() {
        panic!("pinned wiring {spec:?} is invalid: {e}");
    }
    wiring
}

/// Parse one comma-separated axis section into a deduplicated, order-preserving
/// `Vec`. An empty section yields an empty axis.
fn parse_axis<T: Copy + PartialEq>(
    section: &str,
    parse_one: fn(&str) -> Option<T>,
    kind: &str,
) -> Vec<T> {
    let mut out = Vec::new();
    for tok in section.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let parsed = parse_one(tok).unwrap_or_else(|| {
            panic!("HOLON_PBT_WIRING_AXES: unknown {kind} {tok:?}");
        });
        if !out.contains(&parsed) {
            out.push(parsed);
        }
    }
    out
}

fn parse_storage_adapter(tok: &str) -> Option<StorageAdapter> {
    match tok {
        "Loro" => Some(StorageAdapter::Loro),
        "Org" => Some(StorageAdapter::Org),
        "Markdown" => Some(StorageAdapter::Markdown),
        "Turso" => Some(StorageAdapter::Turso),
        _ => None,
    }
}

fn parse_sync_adapter(tok: &str) -> Option<SyncAdapter> {
    match tok {
        "Todoist" => Some(SyncAdapter::Todoist),
        "GCal" => Some(SyncAdapter::GCal),
        "GMail" => Some(SyncAdapter::GMail),
        _ => None,
    }
}

fn parse_actor(tok: &str) -> Option<Actor> {
    match tok {
        "UI" => Some(Actor::UI),
        "MCPServer" => Some(Actor::MCPServer),
        "ActionEngine" => Some(Actor::ActionEngine),
        _ => None,
    }
}

/// A uniform shrinkable subset (`btree_set`, size `0..=len`, shrinks by removing
/// elements) over a fixed axis. An empty axis yields the empty set.
fn uniform_subset<T>(items: &[T]) -> BoxedStrategy<BTreeSet<T>>
where
    T: Copy + Ord + std::fmt::Debug + 'static,
{
    if items.is_empty() {
        return Just(BTreeSet::new()).boxed();
    }
    let n = items.len();
    let arms: Vec<_> = items.iter().map(|&i| Just(i)).collect();
    proptest::collection::btree_set(Union::new(arms), 0..=n).boxed()
}

/// A subset over `items` where each element is included independently with
/// probability `prob` (a weighted bool shrinks `true → false`, so an included
/// element shrinks *out* first). Used for the expensive query-capable adapters.
fn weighted_subset<T>(items: &[T], prob: f64) -> BoxedStrategy<BTreeSet<T>>
where
    T: Copy + Ord + std::fmt::Debug + 'static,
{
    items
        .iter()
        .fold(Just(BTreeSet::new()).boxed(), move |acc, &item| {
            (acc, proptest::bool::weighted(prob))
                .prop_map(move |(mut set, include)| {
                    if include {
                        set.insert(item);
                    }
                    set
                })
                .boxed()
        })
}

/// A shrinkable, validity-filtered [`Wiring`] strategy over [`wiring_axes`].
///
/// Storage is split into cheap (uniform subset) and query-capable/expensive
/// (low-probability inclusion, see [`QUERY_ADAPTER_INCLUSION_PROB`]) so most
/// draws stay on the fast LoroMemory backend. Removing elements (cheap-set
/// element removal; expensive bool `true → false`) shrinks the manifest toward
/// the minimal combination — the architectural delta-debug axis.
///
/// `.prop_filter` drops manifests that fail [`Wiring::validate`] (≥1 storage;
/// `ActionEngine` ⇒ a query-capable storage adapter). Because
/// `run_invariant_registry` always supplies `ViewModel` + `EditorState` at
/// runtime, any `validate()`-valid wiring is observation-valid — `validate()`
/// is the only filter needed.
pub fn any_valid_wiring() -> BoxedStrategy<Wiring> {
    let (storage_axis, sync_axis, actor_axis) = wiring_axes();

    let cheap_storage: Vec<StorageAdapter> = storage_axis
        .iter()
        .copied()
        .filter(|a| !a.is_query_capable())
        .collect();
    let query_storage: Vec<StorageAdapter> = storage_axis
        .iter()
        .copied()
        .filter(|a| a.is_query_capable())
        .collect();

    let cheap = uniform_subset(&cheap_storage);
    let expensive = weighted_subset(&query_storage, QUERY_ADAPTER_INCLUSION_PROB);
    let sync = uniform_subset(&sync_axis);
    let actors = uniform_subset(&actor_axis);

    (cheap, expensive, sync, actors)
        .prop_map(|(mut storage, query, sync_adapters, actors)| {
            storage.extend(query);
            Wiring {
                storage_adapters: storage,
                sync_adapters,
                actors,
            }
        })
        .prop_filter("invalid wiring (Wiring::validate)", |w| {
            w.validate().is_ok()
        })
        .boxed()
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
