//! **`TwoInstanceE2E` — the composed slice with a SECOND real instance.**
//!
//! Two `compose_sut(full_headless)` sessions in ONE process (owner + receiver),
//! an [`InMemoryRelay`] between them, and one [`TestClock`] driving every lease
//! decision. Everything else is the keystone's: the same `ComposedSlice`
//! kernel, the same `E2ETransition` enum, the same shared invariant catalog.
//! The slice contributes only the axes that genuinely differ — a second boot,
//! the sharing caps, and a narrowed alphabet.
//!
//! @pbt kind slice
//! @pbt covers two-instance-share, two-instance-sync — one-way share +
//!   convergence over a real transport between two real Holon instances.
//!
//! ## Why two instances can coexist in one process
//! Vault isolation was already per-component (`TempDir` per
//! `HeadlessFrontendComponent`). The one genuine collision was the Loro peer
//! id, read from the process-global `HOLON_LORO_PEER_ID`: both sessions would
//! author under the SAME peer id and their CRDT histories would silently fail
//! to converge. The peer-id injection seam (`SessionConfig::loro_peer_id`) is
//! what makes this slice possible at all, which is why Inc0's test asserts the
//! ids are distinct.
//!
//! ## Why the alphabet is narrowed
//! Two full sessions per case cost roughly twice a keystone case, so sequences
//! are short and the alphabet is the smallest one that can exercise the sharing
//! question: create content, type into it, share, sync. It deliberately
//! EXCLUDES navigation and relocating (indent / outdent / move) transitions —
//! once boundary enforcement is wired into op dispatch, those are fail-closed
//! against a shared container, and modeling those refusals is a later
//! increment's job. Widening the alphabet before then would produce
//! refusal-driven REDs that say nothing about sharing.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::TestClock;
use holon_loro::sync_transport::InMemoryRelay;
use holon_loro::sync_transport::StablePeerId;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutOrgRead;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::capabilities::SyncRoundWitness;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::InvariantId;
use holon_sharing::acceptor::AcceptorContext;
use holon_sharing::lease::Issuer;
use holon_sharing::lease::Lease;
use holon_sharing::lease::MembershipCert;
use holon_sharing::lease::MembershipChain;
use holon_sharing::policy::Capabilities;
use holon_sharing::policy::Principal;
use holon_sharing::policy::UnverifiedVerifier;
use holon_sharing::sync::OutboundAuth;
use holon_sharing::sync::SyncSession;
use holon_sharing::sync::pull_once;
use holon_sharing::sync::push_once;
use holon_sharing::types::BlockId;
use holon_sharing::types::UnverifiedAuthority;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest_state_machine::ReferenceStateMachine;

use crate::pbt::composed::builder::compose_sut_seeded_with_peer_id;
use crate::pbt::composed::harness::ComposedSlice;
use crate::pbt::composed::wide_e2e::CONVERGE_BUDGET;
use crate::pbt::composed::wide_e2e::SETTLE;
use crate::pbt::composed::wide_e2e::WideHandle;
use crate::pbt::composed::wide_e2e::boot_and_seed_wide_with_peer_id;
use crate::pbt::composed::wide_e2e::converge_handle;
use crate::pbt::composed::wide_e2e::set_for_wiring;
use crate::pbt::composed::wide_e2e::wide_e2e_ref;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::sharing_state::RECEIVER_PRINCIPAL;
use crate::pbt::transitions::CreateBlockUnderFocus;
use crate::pbt::transitions::E2ETransition;
use crate::pbt::transitions::Nothing;
use crate::pbt::transitions::ShareContainer;
use crate::pbt::transitions::SyncNow;
use crate::pbt::transitions::TypeChars;

/// Loro peer ids for the two instances. Fixed and distinct — the reference
/// model's merge prediction assumes low, stable peer ids, and RGA tiebreaks on
/// peer id, so a random pair would make concurrent-insert ordering
/// non-reproducible.
pub const OWNER_PEER_ID: u64 = 1;
pub const RECEIVER_PEER_ID: u64 = 2;

/// The receiver boots its OWN, disjoint vault. Sharing a seed with the owner
/// would make every owner block trivially "present" on the receiver and the
/// convergence oracle vacuous.
pub const RECEIVER_SEED_ORG: &str = "#+ID: receiver-root\n* Receiver local page\n";

/// Lease window for the receiver's membership cert. Long relative to a case, so
/// Inc1 never trips expiry by accident; a later `RevokeLease` drives the clock
/// past it deliberately.
const LEASE_TTL_MILLIS: i64 = 60 * 60 * 1000;

/// The mutable sharing state the transitions drive.
#[derive(Default)]
struct SharingRuntime {
    /// The owner-issued chain granting the receiver membership. `None` until a
    /// `ShareContainer` runs — which is exactly why an unshared `SyncNow`
    /// transports nothing: there is no proof to attach.
    grant: Option<MembershipChain>,
    owner_session: SyncSession,
    receiver_session: SyncSession,
    witness: SyncRoundWitness,
    /// The owner's block ids as of the last owner→receiver round — the exact
    /// set that round could have carried. See
    /// [`SutReceiverBackend::owner_block_ids_at_last_round`].
    ///
    /// This is a push-TIME snapshot of `block_raw`, which coincides with the
    /// Envelope's contents only while the owner is quiescent at push. Once a
    /// concurrent edit can race a push (the concurrent-edit increment), the
    /// stronger basis is the exported Loro tree ids — what the Envelope
    /// literally carried — at the cost of comparing across two id surfaces.
    owner_ids_at_last_round: BTreeSet<EntityUri>,
}

/// The two booted instances plus everything between them.
pub struct TwoInstanceHandle {
    owner: WideHandle,
    receiver: WideHandle,
    /// Read caps captured from each side's CapMap. Captured (rather than
    /// holding the whole owner `CapMap`) because the handle is itself
    /// inserted INTO the owner's map — holding it would be circular.
    owner_backend: Arc<dyn SutBackend>,
    owner_org: Option<Arc<dyn SutOrgRead>>,
    receiver_backend: Arc<dyn SutBackend>,
    receiver_org: Option<Arc<dyn SutOrgRead>>,
    relay: InMemoryRelay,
    clock: Arc<TestClock>,
    /// Ids the receiver held immediately after boot — its own seed plus the
    /// programmatic default layout both instances mint under the same fixed
    /// ids.
    receiver_boot_ids: BTreeSet<EntityUri>,
    state: Mutex<SharingRuntime>,
}

impl TwoInstanceHandle {
    pub fn owner(&self) -> &WideHandle {
        &self.owner
    }
    pub fn receiver(&self) -> &WideHandle {
        &self.receiver
    }
    pub fn relay(&self) -> &InMemoryRelay {
        &self.relay
    }
    pub fn clock(&self) -> &Arc<TestClock> {
        &self.clock
    }

    /// Block ids present in one side's LORO TREE — the layer BETWEEN the
    /// transport and the SQL store. Reading it is how a convergence failure is
    /// localized to the export side (absent here) or the projection side
    /// (present here, absent in `block_raw`).
    pub async fn loro_tree_ids(&self, owner: bool) -> BTreeSet<String> {
        let side = if owner { "owner" } else { "receiver" };
        let handle = if owner { &self.owner } else { &self.receiver };
        let doc = Self::registry(handle, side)
            .store()
            .get_global_doc()
            .await
            .unwrap_or_else(|e| panic!("the {side} instance has no global Loro doc: {e:#}"));
        holon_loro::loro_backend::snapshot_blocks_from_doc(&doc.doc())
            .into_keys()
            .collect()
    }

    /// The full block tree one side's LIVE Loro doc holds — id → block, not
    /// just the id set. The two-writer convergence oracle compares these
    /// directly: two peers that agree on ids but disagree on a block's parent,
    /// order or text have NOT converged, and an id-set oracle would call that
    /// green.
    ///
    /// Reads the live document (no fork), so it answers "have they converged",
    /// not `crdt_converged`'s "would they converge if synced once more".
    /// `exclude` drops the fixed-id boot roots both instances mint
    /// independently (`block:root-layout`, `block:__default__`, the journals
    /// roots). They collide by construction, which is Inc 1's defect, not this
    /// oracle's.
    pub async fn loro_tree_state(
        &self,
        owner: bool,
        exclude: &BTreeSet<EntityUri>,
    ) -> BTreeMap<String, holon_loro::loro_backend::SnapshotBlock> {
        let side = if owner { "owner" } else { "receiver" };
        let handle = if owner { &self.owner } else { &self.receiver };
        let doc = Self::registry(handle, side)
            .store()
            .get_global_doc()
            .await
            .unwrap_or_else(|e| panic!("the {side} instance has no global Loro doc: {e:#}"));
        let skip: BTreeSet<&str> = exclude.iter().map(EntityUri::as_str).collect();
        holon_loro::loro_backend::snapshot_blocks_from_doc(&doc.doc())
            .into_iter()
            .filter(|(id, _)| !skip.contains(id.as_str()))
            .collect()
    }

    fn with_transport_counters(&self, mut witness: SyncRoundWitness) -> SyncRoundWitness {
        witness.transport_consultations = self.relay.witness().total();
        witness.transport_envelopes = self.relay.stored_envelopes();
        witness
    }

    fn registry(handle: &WideHandle, side: &str) -> holon_loro::ContainerRegistry {
        let store = handle
            .frontend()
            .unwrap_or_else(|| {
                panic!(
                    "the {side} instance has no frontend session; the two-instance slice must \
                     boot both sides at the full_headless wiring"
                )
            })
            .loro_doc_store()
            .unwrap_or_else(|| {
                panic!(
                    "the {side} instance has no Loro document store; two-instance sync moves Loro \
                     blobs, so the CRDT layer must be enabled on both sides"
                )
            });
        holon_loro::ContainerRegistry::new(store)
    }
}

#[async_trait::async_trait(?Send)]
impl SutTwoInstance for TwoInstanceHandle {
    async fn instance_peer_ids(&self) -> (u64, u64) {
        (OWNER_PEER_ID, RECEIVER_PEER_ID)
    }

    async fn live_doc_peer_ids(&self) -> (u64, u64) {
        async fn live(handle: &WideHandle, side: &str) -> u64 {
            TwoInstanceHandle::registry(handle, side)
                .store()
                .get_global_doc()
                .await
                .unwrap_or_else(|e| panic!("the {side} instance has no global Loro doc: {e:#}"))
                .peer_id()
        }
        (
            live(&self.owner, "owner").await,
            live(&self.receiver, "receiver").await,
        )
    }

    async fn share_container(&self, selector: &str, principal: &str) {
        let cert = MembershipCert::issue(
            BlockId(selector.to_string()),
            Principal(principal.to_string()),
            Issuer::Owner,
            // Own-device pairing (D68.b) makes the peer a full WRITER, so the
            // cert the slice issues is the one production will issue. `admit`
            // gates on `Read` alone today; `Write` is what the reverse leg's
            // authorization will read once it stops reusing the receiver's
            // audience for both directions (see `sync_now`).
            Capabilities::read_write(),
            false,
            Lease::starting_at(holon_api::Clock::now_millis(&*self.clock), LEASE_TTL_MILLIS),
            &UnverifiedAuthority,
        );
        self.state.lock().expect("sharing runtime lock").grant =
            Some(MembershipChain::direct(cert));
    }

    async fn sync_now(&self, owner_to_receiver: bool) -> SyncRoundWitness {
        let (publisher, consumer, sender) = if owner_to_receiver {
            (&self.owner, &self.receiver, OWNER_PEER_ID)
        } else {
            (&self.receiver, &self.owner, RECEIVER_PEER_ID)
        };
        let pub_registry = Self::registry(publisher, "publishing");
        let con_registry = Self::registry(consumer, "consuming");

        let grant = self
            .state
            .lock()
            .expect("sharing runtime lock")
            .grant
            .clone();
        let auth = OutboundAuth {
            sender: StablePeerId(sender),
            audience: Principal(RECEIVER_PRINCIPAL.to_string()),
            epoch: 0,
            // Unshared: an EMPTY chain. `push_once` then publishes NOTHING and
            // reports every container as `unauthorized` — state must not reach an
            // untrusted relay under an unproven claim. The acceptor's own refusal
            // paths are covered by its unit tests, not by leaking here.
            chain: grant.unwrap_or_else(|| MembershipChain::new(Vec::new())),
        };
        let receiver_principal = Principal(RECEIVER_PRINCIPAL.to_string());
        let ctx = AcceptorContext {
            receiver: &receiver_principal,
            clock: self.clock.as_ref(),
            verifier: &UnverifiedVerifier,
        };

        // Snapshot what the owner holds BEFORE publishing: that is precisely the
        // state this round can carry, and the convergence oracle judges against
        // it rather than against a live set that may grow afterwards. The owner
        // is settled here (the previous tick's settle ran to a fixed point), so
        // this matches what the export sees.
        let owner_now = if owner_to_receiver {
            Some(backend_ids(&self.owner_backend).await)
        } else {
            None
        };

        // The round RUNS even with nothing shared: it walks the replication set
        // and consults the transport. That is what makes the negative assertion
        // ("nothing crossed") non-vacuous.
        let (push, pull) = {
            let mut guard = self.state.lock().expect("sharing runtime lock");
            let SharingRuntime {
                owner_session,
                receiver_session,
                ..
            } = &mut *guard;
            let (pub_session, con_session) = if owner_to_receiver {
                (owner_session, receiver_session)
            } else {
                (receiver_session, owner_session)
            };
            let push = push_once(&pub_registry, &self.relay, pub_session, &auth)
                .await
                .expect("push_once surfaces transport failure as Err; the slice must not hide it");
            let pull = pull_once(&con_registry, &self.relay, con_session, &ctx)
                .await
                .expect("pull_once surfaces transport failure as Err; the slice must not hide it");
            (push, pull)
        };

        let mut guard = self.state.lock().expect("sharing runtime lock");
        let witness = &mut guard.witness;
        witness.rounds_run += 1;
        witness.containers_visited = push.containers_visited.max(pull.containers_visited);
        witness.pushed = push.pushed.len();
        witness.imported = pull.imported.len();
        witness.refusals = pull
            .refusals
            .iter()
            .map(|(c, d)| format!("{c}: {d:?}"))
            .collect();
        witness.unmounted = pull.unmounted.iter().map(ToString::to_string).collect();
        witness.unauthorized = push.unauthorized.iter().map(ToString::to_string).collect();
        let out = witness.clone();
        if let Some(owner_now) = owner_now {
            guard.owner_ids_at_last_round = owner_now;
        }
        drop(guard);
        self.with_transport_counters(out)
    }

    async fn sync_witness(&self) -> SyncRoundWitness {
        let witness = self
            .state
            .lock()
            .expect("sharing runtime lock")
            .witness
            .clone();
        self.with_transport_counters(witness)
    }
}

async fn backend_ids(backend: &Arc<dyn SutBackend>) -> BTreeSet<EntityUri> {
    backend
        .block_raw_snapshot()
        .await
        .into_iter()
        .map(|b| b.id)
        .collect()
}

async fn org_ids(org: Option<&Arc<dyn SutOrgRead>>) -> BTreeSet<EntityUri> {
    match org {
        Some(org) => org
            .org_block_snapshot()
            .await
            .into_iter()
            .map(|b| b.id)
            .collect(),
        None => BTreeSet::new(),
    }
}

#[async_trait::async_trait(?Send)]
impl SutReceiverBackend for TwoInstanceHandle {
    async fn receiver_block_ids(&self) -> BTreeSet<EntityUri> {
        backend_ids(&self.receiver_backend).await
    }

    async fn receiver_org_block_ids(&self) -> BTreeSet<EntityUri> {
        org_ids(self.receiver_org.as_ref()).await
    }

    async fn owner_block_ids(&self) -> BTreeSet<EntityUri> {
        backend_ids(&self.owner_backend).await
    }

    async fn owner_block_ids_at_last_round(&self) -> BTreeSet<EntityUri> {
        self.state
            .lock()
            .expect("sharing runtime lock")
            .owner_ids_at_last_round
            .clone()
    }

    async fn receiver_boot_block_ids(&self) -> BTreeSet<EntityUri> {
        self.receiver_boot_ids.clone()
    }

    async fn owner_org_block_ids(&self) -> BTreeSet<EntityUri> {
        org_ids(self.owner_org.as_ref()).await
    }

    async fn crdt_converged(&self) -> Option<bool> {
        let owner = self.owner.frontend()?.loro_doc_store()?;
        let receiver = self.receiver.frontend()?.loro_doc_store()?;
        let a = owner.get_global_doc().await.ok()?.doc();
        let b = receiver.get_global_doc().await.ok()?.doc();
        // Pairwise fixed point over THROWAWAY forks: import each side's delta
        // into a fork of the other and compare. Forks, so the live documents the
        // rest of the case reads stay untouched.
        let fork_a = a.fork();
        let fork_b = b.fork();
        fork_a
            .import(
                &b.export(loro::ExportMode::updates(&fork_a.oplog_vv()))
                    .ok()?,
            )
            .ok()?;
        fork_b
            .import(
                &a.export(loro::ExportMode::updates(&fork_b.oplog_vv()))
                    .ok()?,
            )
            .ok()?;
        Some(fork_a.get_deep_value() == fork_b.get_deep_value())
    }
}

/// Boot both instances and assemble the two-instance cap map.
///
/// The receiver's own `CapMap` is dropped here: the composed slice's oracle is
/// the OWNER's `ReferenceState`, which models a single writer. Tests that need
/// the receiver to author (own-device pairing, D68.b) call
/// [`boot_two_instances_with_receiver_caps`] and judge by convergence instead.
pub async fn boot_two_instances(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (CapMap, Arc<TwoInstanceHandle>, BTreeSet<EntityUri>) {
    let (owner_caps, _receiver_caps, handle, scaffold) =
        boot_two_instances_with_receiver_caps(resolver, ref_state).await;
    (owner_caps, handle, scaffold)
}

/// Boot both instances and hand back BOTH cap maps, so a caller can drive
/// production writes on either peer.
///
/// This is the two-writer seam. The receiver's map is the SAME
/// `compose_sut(full_headless)` surface as the owner's — same transitions, same
/// drivers — so a peer-side write goes through production code, not a test
/// shortcut. It is returned separately rather than merged into the owner's map
/// because `CapMap` is keyed by cap TYPE: one map cannot hold two realizations
/// of `SutBlockCreate`.
pub async fn boot_two_instances_with_receiver_caps(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
) -> (CapMap, CapMap, Arc<TwoInstanceHandle>, BTreeSet<EntityUri>) {
    let (mut owner_caps, owner, scaffold) =
        boot_and_seed_wide_with_peer_id(resolver, ref_state, Some(OWNER_PEER_ID)).await;

    // The receiver boots the SAME wiring over its OWN vault and its OWN peer id,
    // from a disjoint seed. A second `IdResolver` keeps its mints out of the
    // owner-side reconcile map — the oracle models the OWNER.
    let receiver_resolver = IdResolver::default();
    let set = set_for_wiring(&ref_state.harness.wiring);
    let receiver_bundle = compose_sut_seeded_with_peer_id(
        &set,
        &receiver_resolver,
        &[("receiver-root.org", RECEIVER_SEED_ORG)],
        &[],
        RECEIVER_PEER_ID,
    )
    .await;
    let receiver = WideHandle::from_bundle(&receiver_bundle);
    let receiver_caps = receiver_bundle.caps;

    let receiver_backend = receiver_caps.expect::<dyn SutBackend>();
    let receiver_boot_ids = backend_ids(&receiver_backend).await;

    let handle = Arc::new(TwoInstanceHandle {
        owner,
        receiver,
        owner_backend: owner_caps.expect::<dyn SutBackend>(),
        owner_org: owner_caps.get::<dyn SutOrgRead>(),
        receiver_backend,
        receiver_org: receiver_caps.get::<dyn SutOrgRead>(),
        relay: InMemoryRelay::new(),
        clock: crate::pbt::frontend_slice::components::keystone_boot_clock(),
        receiver_boot_ids,
        state: Mutex::new(SharingRuntime::default()),
    });

    owner_caps.insert(handle.clone() as Arc<dyn SutTwoInstance>);
    owner_caps.insert(handle.clone() as Arc<dyn SutReceiverBackend>);
    // The non-vacuity guard reads `two_instance_cap_ids` as this slice's cap
    // evidence; a claim this composition does not back would let a genuinely
    // dead sharing transition read as drawable.
    let provided = owner_caps.cap_set();
    for cap in two_instance_cap_ids() {
        assert!(
            provided.contains(&cap),
            "two_instance_cap_ids claims {} but the composed owner map does not provide it",
            cap.name(),
        );
    }
    (owner_caps, receiver_caps, handle, scaffold)
}

/// The caps this slice adds ON TOP of the wide owner map it boots through the
/// same production builder. Read by the non-vacuity guard as cap evidence that
/// the sharing transitions have a shipped home — the two-instance analog of
/// [`live_mcp_cap_ids`](super::live_mcp::live_mcp_cap_ids). Checked against the
/// real inserts by [`compose_two_instance`] itself, so it cannot over-claim.
pub fn two_instance_cap_ids() -> Vec<holon_pbt_core::composition::CapId> {
    use holon_pbt_core::composition::CapId;
    vec![
        CapId::of::<dyn SutTwoInstance>(),
        CapId::of::<dyn SutReceiverBackend>(),
    ]
}

/// Reference machine over the SAME `E2ETransition` enum, restricted to the
/// alphabet this slice can drive (see the module docs for why it is narrow).
pub struct TwoInstanceMachine;

impl ReferenceStateMachine for TwoInstanceMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        // Always the widest headless wiring: a two-instance share over a
        // Loro-only draw would have no org/SQL projections to converge, i.e. no
        // receiver-side writeback to check.
        Just(wide_e2e_ref()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        let mut arms: Vec<(u32, BoxedStrategy<E2ETransition>)> = Vec::new();
        macro_rules! offer {
            ($ty:ty) => {
                if let ::validated::Validated::Good(Some(arm)) =
                    ::holon_pbt_core::weighted_arm::<_, $ty, E2ETransition>(state, 1, |v| {
                        E2ETransition::from(v)
                    })
                {
                    arms.push(arm);
                }
            };
        }
        offer!(ShareContainer);
        offer!(SyncNow);
        offer!(CreateBlockUnderFocus);
        offer!(TypeChars);
        // `Nothing` has no preconditions, so `arms` is never empty and the
        // Union below cannot panic on a state where everything else is gated.
        offer!(Nothing);
        proptest::strategy::Union::new_weighted(arms).boxed()
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        use holon_pbt_core::TransitionRef;
        transition.preconditions(state).is_good()
    }

    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        use holon_pbt_core::TransitionRef;
        transition.apply_to_ref(&mut state);
        state.action.last_transition_kind = Some(transition.variant_name());
        state
    }
}

/// The two-instance slice.
pub struct TwoInstanceE2E;

impl ComposedSlice for TwoInstanceE2E {
    type Transition = E2ETransition;
    type Machine = TwoInstanceMachine;
    type Handle = Arc<TwoInstanceHandle>;

    /// The two invariants this slice EXISTS to run. Unlike the keystone's
    /// derive-from-cap_set floor, this list is explicit: if either deselects,
    /// the slice proves nothing and must RED rather than pass.
    const REQUIRED_INVARIANTS: &'static [&'static str] =
        &["inv-two-instance-convergence", "inv-boundary-respected"];
    const SETTLE: Duration = SETTLE;
    const MULTI_THREAD: bool = true;

    /// Only the two sharing invariants are required. The rest of the catalog
    /// still RUNS (the owner arm is a full `compose_sut`), but a keystone
    /// invariant that legitimately deselects here must not fail this floor.
    fn required_invariants(_: &ReferenceState) -> Vec<InvariantId> {
        Self::REQUIRED_INVARIANTS
            .iter()
            .copied()
            .map(InvariantId)
            .collect()
    }

    async fn build(
        resolver: &IdResolver,
        ref_state: &ReferenceState,
    ) -> (CapMap, Arc<TwoInstanceHandle>, BTreeSet<EntityUri>) {
        boot_two_instances(resolver, ref_state).await
    }

    /// Settle BOTH instances: a receiver whose CDC / org has not drained looks
    /// like a convergence failure that is really a read race.
    async fn settle_after_apply(handle: &Arc<TwoInstanceHandle>, _: &CapMap) {
        // `CONVERGE_BUDGET`, not the keystone's 150ms flat-sleep replacement: a
        // received blob has to clear the receiver's Loro import AND its whole
        // Loro→SQL→org projection chain, which is far slower than any
        // single-instance write. The loop still returns at the first quiet
        // floor, so a settled pair costs one poll.
        converge_handle(handle.owner(), CONVERGE_BUDGET).await;
        converge_handle(handle.receiver(), CONVERGE_BUDGET).await;
    }

    async fn apply_transition(
        transition: &E2ETransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        TransitionImpl::apply_to_sut(transition, ref_state, caps).await;
    }
}
