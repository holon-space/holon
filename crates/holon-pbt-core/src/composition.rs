//! Capability-composition spine for trivially-sliceable PBT (γ design).
//!
//! This is the PoC backbone for the refactor discussed in
//! `docs/Testing/PbtSlicing_Trivialization_Handoff.md` and its design
//! follow-up. It de-risks the three pieces that were genuinely uncertain:
//!
//!   1. **The capability typemap** ([`CapMap`]) — a map from a capability
//!      *trait object type* (`dyn SutBackend`) to an `Arc<dyn Cap>` provider,
//!      so a SUT/Ref is *composed* from independent components instead of
//!      being one god-type that implements the union of every cap. This is the
//!      mechanically-risky part (storing + downcasting `Arc<dyn Trait>` through
//!      `Any`); the tests below prove it compiles and round-trips.
//!
//!   2. **Single-sourced invariants** ([`cap_invariant!`]) — one declaration
//!      drives *both* the selection metadata (`needs()`) *and* the typed cap
//!      lookups in the body, so the declared need-set and the actual reads
//!      cannot drift. Missing-cap access is a selection-guarded assertion
//!      ([`CapMap::expect`]), never an `unimplemented!()` stand-in.
//!
//!   3. **Positive + negative selection** ([`Needs::selected_against`]) — an
//!      invariant runs iff `present ⊆ have ∧ absent ∩ have = ∅`. The negative
//!      half is the one new primitive the Ref-side audit found we need: it lets
//!      a *degraded-mode* invariant ("GPUI shows a query block's source when no
//!      query capability is wired") be a complementary twin selected by
//!      capability *absence*, rather than a config-branching projection.
//!
//! ## Also de-risked here (the real `Sut*` traits forced these)
//! The real cap traits use native `async fn` (NOT object-safe → `dyn
//! SutBackend` is illegal) and `&mut self` (uncallable through a shared `Arc`).
//! The typemap forces `dyn`, so the PoC proves the two resolutions:
//! - **async caps as boxed futures** ([`CapInvariant::check_boxed`] + the toy
//!   async caps) — object-safe, awaitable through `Arc<dyn Cap>`, zero new
//!   deps (same hand-rolled shape as the existing `DynInvariant`). The real
//!   traits get this ergonomically via `#[async_trait]`.
//! - **`&mut self` → `&self` + interior mutability** (toy `SutBlockTreeWrite`
//!   mutates a `Mutex` through `Arc<dyn Cap>`), so write caps compose into the
//!   same map as read caps.
//! - **the sync→async `StateMachineTest` glue**: proptest's `check_invariants`
//!   is sync, so the toy `Slice::check_invariants` `block_on`s the async
//!   `run_selected` — the exact shape today's `E2ESut` uses.
//!
//! ## Still stubbed (known-solved, not de-risked here)
//! - **the caching adapter**: the per-tick memoisation on `CachingProxy<S>`
//!   becomes one generic lookup-and-memoise wrapper over `CapMap`.
//! - **real components/Refs**: the toy caps stand in for the ~23 real
//!   `Sut*`/`Ref*` traits; implementing them over `MemoryBackend` is the
//!   refactor's labor, not a risk.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use crate::invariant::{InvariantId, InvariantResult, RunMode};

// ─────────────────────────────────────────────────────────────────────────
// Capability identity
// ─────────────────────────────────────────────────────────────────────────

/// Attaches a stable human name to a capability trait-object type so it can be
/// reported in diagnostics. Implemented for each `dyn Cap` by [`capability!`].
pub trait CapName: 'static {
    const NAME: &'static str;
}

/// Identity of a capability: the `TypeId` of its `dyn` trait-object type plus a
/// human name. `TypeId` drives set membership; `name` drives fail-loud reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CapId {
    type_id: TypeId,
    name: &'static str,
}

impl CapId {
    pub fn of<C: ?Sized + CapName>() -> CapId {
        CapId {
            type_id: TypeId::of::<C>(),
            name: C::NAME,
        }
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
}

/// Declare a trait as a composable capability: `capability!(SutBackend);`.
/// Emits the [`CapName`] impl on the trait-object type so `CapId::of::<dyn
/// SutBackend>()` and `CapMap::{insert,get,expect}::<dyn SutBackend>()` work.
#[macro_export]
macro_rules! capability {
    ($cap:path) => {
        impl $crate::composition::CapName for dyn $cap {
            const NAME: &'static str = stringify!($cap);
        }
    };
}

// ─────────────────────────────────────────────────────────────────────────
// CapMap — the typemap of capability providers (THE de-risked piece)
// ─────────────────────────────────────────────────────────────────────────

/// A set of provided capabilities, assembled from components. Keyed by each
/// capability's trait-object `TypeId`; the stored value is the provider boxed
/// as `Arc<dyn Cap>` (one component's `Arc` can back several caps).
#[derive(Default)]
pub struct CapMap {
    /// `TypeId::of::<dyn Cap>()` → `Box<dyn Any>` erasing an `Arc<dyn Cap>`.
    providers: HashMap<TypeId, Box<dyn Any>>,
    /// Parallel name table, only for fail-loud "what *is* present" reporting.
    names: HashMap<TypeId, &'static str>,
}

impl CapMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider for capability `C`. Typically called by a component's
    /// [`CapProvider::register`]; one `Arc<Concrete>` is cast to `Arc<dyn C>`
    /// per cap it provides.
    pub fn insert<C: ?Sized + CapName>(&mut self, provider: Arc<C>) {
        let tid = TypeId::of::<C>();
        self.names.insert(tid, C::NAME);
        // `Arc<dyn C>` is a concrete, `'static`, `Sized` type → it is `Any`.
        self.providers.insert(tid, Box::new(provider));
    }

    pub fn get<C: ?Sized + CapName>(&self) -> Option<Arc<C>> {
        self.providers.get(&TypeId::of::<C>()).map(|boxed| {
            boxed
                .downcast_ref::<Arc<C>>()
                .expect("CapMap type key invariant")
                .clone()
        })
    }

    /// Look up a capability that selection has already proven present. Panicking
    /// here means the selection predicate disagreed with the map — an internal
    /// harness bug, not a missing feature (the fail-loud-correct kind).
    pub fn expect<C: ?Sized + CapName>(&self) -> Arc<C> {
        self.get::<C>().unwrap_or_else(|| {
            panic!(
                "capability `{}` was selected but is absent from the CapMap \
                 (selection/composition bug). Present: {:?}",
                C::NAME,
                self.names.values().collect::<Vec<_>>()
            )
        })
    }

    /// Borrow a capability's provider with the map's own lifetime, instead of
    /// cloning the `Arc` like [`expect`]/[`get`]. A forwarded sync cap method
    /// that returns a *borrow* (`Option<&str>`, `&T`) would dangle off the
    /// temporary `Arc` that `expect` hands back; rooting the reference in
    /// `&self` lets the borrow outlive the call. Same fail-loud contract as
    /// [`expect`] — a miss is a selection/composition bug, not a feature gap.
    pub fn expect_ref<C: ?Sized + CapName>(&self) -> &C {
        self.providers
            .get(&TypeId::of::<C>())
            .and_then(|boxed| boxed.downcast_ref::<Arc<C>>())
            .map(|arc| &**arc)
            .unwrap_or_else(|| {
                panic!(
                    "capability `{}` was selected but is absent from the CapMap \
                     (selection/composition bug). Present: {:?}",
                    C::NAME,
                    self.names.values().collect::<Vec<_>>()
                )
            })
    }

    pub fn cap_set(&self) -> CapSet {
        CapSet(self.providers.keys().copied().collect())
    }

    /// Merge `other`'s providers into `self`, keeping the EXISTING provider for
    /// any capability already present — **first-registered wins**.
    ///
    /// This is the composition primitive for assembling a `CapMap` from multiple
    /// components whose cap sets overlap (e.g. several backends all provide
    /// `SutBackend`): register the precedence-winning component first, then
    /// `merge_missing` the rest, and each contributes only the caps not already
    /// supplied. Without it, a later [`insert`](Self::insert) of an
    /// already-present cap would **silently shadow** the earlier provider (the
    /// typemap keeps exactly one provider per cap `TypeId`).
    ///
    /// Returns the names of the caps that were **skipped** because `self` already
    /// had them — so a caller can disclose precedence collisions (log / assert)
    /// rather than hide them. An empty return means the two maps were disjoint.
    pub fn merge_missing(&mut self, other: CapMap) -> Vec<&'static str> {
        let mut shadowed = Vec::new();
        for (tid, provider) in other.providers {
            if self.providers.contains_key(&tid) {
                // `other.names` is a distinct field, untouched by moving
                // `other.providers` into the loop — accessing it here is a legal
                // partial borrow.
                if let Some(name) = other.names.get(&tid) {
                    shadowed.push(*name);
                }
                continue;
            }
            self.providers.insert(tid, provider);
            if let Some(name) = other.names.get(&tid) {
                self.names.insert(tid, *name);
            }
        }
        shadowed
    }
}

/// The set of capabilities a config provides (membership only — providers live
/// in the [`CapMap`]).
#[derive(Clone, Debug, Default)]
pub struct CapSet(HashSet<TypeId>);

impl CapSet {
    pub fn contains(&self, cap: &CapId) -> bool {
        self.0.contains(&cap.type_id)
    }

    /// Return a copy of this cap set with `cap` removed. Used to express a
    /// DELIBERATE, DOCUMENTED narrowing of the generation alphabet for a config that
    /// *provides* a cap but cannot yet faithfully drive its transitions (the cap stays
    /// in the `CapMap` so its read invariants keep selecting; only the generation gate
    /// drops the unconverged transitions). Identity if `cap` is absent.
    pub fn without(mut self, cap: &CapId) -> Self {
        self.0.remove(&cap.type_id);
        self
    }
}

/// A component (sub-SUT or sub-Ref): something that contributes capabilities to
/// a composed map. `register` is `self: Arc<Self>` so the same `Arc` backs every
/// cap the component provides.
pub trait CapProvider {
    fn register(self: Arc<Self>, caps: &mut CapMap);
}

/// Builder: a config is *just a list of components*. This is the entire
/// per-slice surface — "GPUI + memory" is two `.with(...)` calls.
#[derive(Default)]
pub struct Config {
    caps: CapMap,
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with<P: CapProvider + 'static>(mut self, component: P) -> Self {
        Arc::new(component).register(&mut self.caps);
        self
    }
    /// Register an already-`Arc`'d component — for expensive components (e.g. a
    /// headless frontend session) built once and shared across runs.
    pub fn with_arc<P: CapProvider + 'static>(mut self, component: Arc<P>) -> Self {
        component.register(&mut self.caps);
        self
    }
    pub fn build(self) -> CapMap {
        self.caps
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Invariants: needs + dispatch
// ─────────────────────────────────────────────────────────────────────────

/// What an invariant requires to be *selected*. Positive sets must be subsets of
/// the provided caps; the negative set must be disjoint from them.
#[derive(Clone, Debug, Default)]
pub struct Needs {
    pub sut_present: Vec<CapId>,
    pub sut_absent: Vec<CapId>,
    pub ref_present: Vec<CapId>,
}

impl Needs {
    /// `present ⊆ have ∧ absent ∩ have = ∅`, on both the SUT and Ref axes.
    pub fn selected_against(&self, sut: &CapSet, ref_: &CapSet) -> bool {
        self.sut_present.iter().all(|c| sut.contains(c))
            && self.sut_absent.iter().all(|c| !sut.contains(c))
            && self.ref_present.iter().all(|c| ref_.contains(c))
    }
}

/// Object-safe invariant dispatched against composed maps rather than a typed
/// `S`. The body looks up exactly the caps it declared — see [`cap_invariant!`].
///
/// `check_boxed` returns a boxed future (real bodies are `async` and `.await`
/// cap methods); the hand-rolled box keeps the trait object-safe with no async
/// runtime dependency, mirroring the existing `DynInvariant` shim.
pub trait CapInvariant {
    fn id(&self) -> InvariantId;
    fn mode(&self) -> RunMode;
    fn needs(&self) -> Needs;
    fn check_boxed<'a>(
        &'a self,
        sut: &'a CapMap,
        ref_: &'a CapMap,
    ) -> Pin<Box<dyn Future<Output = InvariantResult> + 'a>>;
}

/// Declare an invariant once. The `sut`/`ref` cap lists drive BOTH `needs()`
/// (selection) AND the in-body lookups, so they cannot drift. `sut_absent`
/// expresses a degraded-mode twin (runs only when a cap is *not* wired).
///
/// ```ignore
/// cap_invariant! {
///     name: InvNoOrphanBlocks, id: "inv-no-orphan-blocks", mode: Strict,
///     sut: { backend: dyn SutBackend },
///     sut_absent: [],
///     ref: { tree: dyn RefBlockTree },
///     check: { /* uses `backend`, `tree` */ InvariantResult::Ok }
/// }
/// ```
#[macro_export]
macro_rules! cap_invariant {
    (
        name: $name:ident,
        id: $id:literal,
        mode: $mode:ident,
        sut: { $($sbind:ident : dyn $scap:path),* $(,)? },
        sut_absent: [ $(dyn $acap:path),* $(,)? ],
        ref: { $($rbind:ident : dyn $rcap:path),* $(,)? },
        check: $body:block
    ) => {
        #[derive(Default)]
        pub struct $name;

        impl $crate::composition::CapInvariant for $name {
            fn id(&self) -> $crate::composition::InvariantId {
                $crate::composition::InvariantId($id)
            }
            fn mode(&self) -> $crate::composition::RunMode {
                $crate::composition::RunMode::$mode
            }
            fn needs(&self) -> $crate::composition::Needs {
                $crate::composition::Needs {
                    sut_present: vec![ $( $crate::composition::CapId::of::<dyn $scap>() ),* ],
                    sut_absent:  vec![ $( $crate::composition::CapId::of::<dyn $acap>() ),* ],
                    ref_present: vec![ $( $crate::composition::CapId::of::<dyn $rcap>() ),* ],
                }
            }
            #[allow(unused_variables)] // `sut`/`ref_` may be unused when a side declares no caps
            fn check_boxed<'a>(
                &'a self,
                sut: &'a $crate::composition::CapMap,
                ref_: &'a $crate::composition::CapMap,
            ) -> ::std::pin::Pin<
                Box<dyn ::std::future::Future<Output = $crate::composition::InvariantResult> + 'a>,
            > {
                // Lookups are selection-proven (`expect`); the owned `Arc<dyn Cap>`
                // bindings move into the future and outlive every `.await`.
                $( let $sbind = sut.expect::<dyn $scap>(); )*
                $( let $rbind = ref_.expect::<dyn $rcap>(); )*
                Box::pin(async move { $body })
            }
        }
    };
}

// ─────────────────────────────────────────────────────────────────────────
// Runner
// ─────────────────────────────────────────────────────────────────────────

/// One run's dispositions. `deselected` is disclosed (not silently absent) so a
/// scoped slice can prove *why* a body didn't run.
#[derive(Debug, Default)]
pub struct RunReport {
    pub ran: Vec<(InvariantId, InvariantResult)>,
    pub deselected: Vec<InvariantId>,
}

impl RunReport {
    pub fn ran_ids(&self) -> Vec<&'static str> {
        self.ran.iter().map(|(id, _)| id.0).collect()
    }
    pub fn failures(&self) -> Vec<(&'static str, &str)> {
        self.ran
            .iter()
            .filter_map(|(id, r)| match r {
                InvariantResult::Fail(m) => Some((id.0, m.as_str())),
                _ => None,
            })
            .collect()
    }
}

/// Select from `registry` against the composed maps and dispatch the survivors.
/// This is the extracted, SUT-agnostic runner core (handoff "Move A"), reduced
/// to its essence. Async because real invariant bodies `.await` cap reads.
pub async fn run_selected(
    registry: &[Box<dyn CapInvariant>],
    sut: &CapMap,
    ref_: &CapMap,
) -> RunReport {
    let have_sut = sut.cap_set();
    let have_ref = ref_.cap_set();
    let mut report = RunReport::default();
    for inv in registry {
        if inv.needs().selected_against(&have_sut, &have_ref) {
            let result = inv.check_boxed(sut, ref_).await;
            report.ran.push((inv.id(), result));
        } else {
            report.deselected.push(inv.id());
        }
    }
    report
}

/// Bridge a statically-typed [`Invariant`](crate::invariant::Invariant) body —
/// the real registry bodies in `holon-integration-tests`, written generic over
/// `R`/`S` with a `where S: <caps>` bound — into an object-safe [`CapInvariant`]
/// dispatched against composed [`CapMap`]s.
///
/// The body is monomorphised at `R = S = CapMap`, so it reads its caps through
/// the macro-generated `impl <Cap> for CapMap` adapters (`#[capmap_adapter]`).
/// `needs` is declared alongside: it is the **one place** the body's cap
/// requirements become *data* the selector can read — the runtime analog of the
/// body's compile-time `where S: …` bound. Selection (`run_selected`) proves
/// `needs ⊆ provided` before dispatch, so every adapter `expect::<dyn Cap>()`
/// the body triggers is a proven lookup, never a fake (§4.1).
///
/// Bodies that ignore the reference (`check(&self, _: &R, sut)`) impose **no**
/// bound on `R`, so `R = CapMap` needs no Ref-cap adapter — those bridge with
/// only their SUT caps hosted. A body that reads the ref needs `CapMap` to host
/// the corresponding `Ref*` caps too.
pub struct BridgedInvariant<I> {
    inv: I,
    mode: RunMode,
    needs: Needs,
}

impl<I> BridgedInvariant<I> {
    pub fn new(inv: I, mode: RunMode, needs: Needs) -> Self {
        Self { inv, mode, needs }
    }
}

impl<I> CapInvariant for BridgedInvariant<I>
where
    I: crate::invariant::Invariant<CapMap, CapMap>,
{
    fn id(&self) -> InvariantId {
        self.inv.id()
    }
    fn mode(&self) -> RunMode {
        self.mode
    }
    fn needs(&self) -> Needs {
        self.needs.clone()
    }
    fn check_boxed<'a>(
        &'a self,
        sut: &'a CapMap,
        ref_: &'a CapMap,
    ) -> Pin<Box<dyn Future<Output = InvariantResult> + 'a>> {
        // Real bodies take `(ref_, sut)`; `CapInvariant` is `(sut, ref_)`.
        Box::pin(self.inv.check(ref_, sut))
    }
}

// Read-cap union-adapters (`impl <ReadCap> for CapMap`) are GENERATED by
// `#[holon_macros::capmap_adapter]` on each read trait in `capabilities.rs` —
// one attribute per cap, no hand-written forwarding. See `SutOrgRender`.

// ═════════════════════════════════════════════════════════════════════════
// PoC demonstration — throwaway. Stands in for the real Sut*/Ref* traits,
// components, configs, and invariants. Proves: typemap round-trips, the macro
// single-sources, and positive+negative selection drives three configs from
// one shared registry.
// ═════════════════════════════════════════════════════════════════════════
#[cfg(test)]
#[allow(dead_code)] // toy stand-ins: not every cap method is exercised by a test
mod poc {
    use super::*;
    use std::sync::Mutex;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    /// Boxed-future alias — what `#[async_trait]` desugars an `async fn` cap
    /// method to. Hand-written here to prove `dyn`-cap async works with zero
    /// runtime deps (the real `Sut*` traits would use `#[async_trait]`).
    type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

    /// Minimal sync→async bridge (no tokio dep): the real glue uses
    /// `runtime.block_on`. Toy futures are always-ready, so this returns at once.
    fn block_on<F: Future>(fut: F) -> F::Output {
        fn raw() -> RawWaker {
            fn clone(_: *const ()) -> RawWaker {
                raw()
            }
            fn noop(_: *const ()) {}
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(clone, noop, noop, noop),
            )
        }
        let waker = unsafe { Waker::from_raw(raw()) };
        let mut cx = Context::from_waker(&waker);
        let mut fut = Box::pin(fut);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    // ── Capabilities (toy stand-ins for the real Sut*/Ref* trait surface) ──
    // `SutBackend` is ASYNC + object-safe (boxed futures) — mirrors the real
    // trait, whose native `async fn` is NOT dyn-safe. `SutBlockTreeWrite` is a
    // WRITE cap: the real trait takes `&mut self`; here it takes `&self` and
    // mutates interior state, so it composes into the same shared `Arc` map.
    pub trait SutBackend {
        fn live_block_ids(&self) -> BoxFut<'_, Vec<u64>>;
        fn parent_of(&self, id: u64) -> BoxFut<'_, Option<u64>>;
    }
    pub trait SutBlockTreeWrite {
        fn apply_split(&self, parent: u64, new_child: u64) -> BoxFut<'_, ()>;
    }
    pub trait SutEditorMirror {
        fn caret(&self) -> usize;
    }
    pub trait SutRender {
        /// In full mode this is the decompiled query result; in degraded mode
        /// (no query engine wired) the frontend shows the block's source.
        fn root_kind(&self) -> &'static str;
    }
    /// Present only when a query engine is wired. Its *absence* is what selects
    /// the degraded-mode twin invariant.
    pub trait SutQueryResults {
        fn row_count(&self) -> usize;
    }
    // Ref projections over the omnipotent core.
    pub trait RefBlockTree {
        fn block_exists(&self, id: u64) -> bool;
        fn all_ids(&self) -> Vec<u64>;
    }
    pub trait RefEditor {
        fn caret(&self) -> usize;
    }

    capability!(SutBackend);
    capability!(SutBlockTreeWrite);
    capability!(SutEditorMirror);
    capability!(SutRender);
    capability!(SutQueryResults);
    capability!(RefBlockTree);
    capability!(RefEditor);

    // ── Components (sub-SUTs). One Arc backs all caps a component provides. ──
    struct MemoryBackend {
        // interior mutability: `&self` write cap mutates through the Mutex
        // (resolves the real traits' `&mut self`, uncallable behind `Arc`).
        blocks: Mutex<Vec<(u64, Option<u64>)>>,
        caret: usize,
    }
    impl SutBackend for MemoryBackend {
        fn live_block_ids(&self) -> BoxFut<'_, Vec<u64>> {
            Box::pin(async move {
                self.blocks
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(id, _)| *id)
                    .collect()
            })
        }
        fn parent_of(&self, id: u64) -> BoxFut<'_, Option<u64>> {
            Box::pin(async move {
                self.blocks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(b, _)| *b == id)
                    .and_then(|(_, p)| *p)
            })
        }
    }
    impl SutBlockTreeWrite for MemoryBackend {
        fn apply_split(&self, parent: u64, new_child: u64) -> BoxFut<'_, ()> {
            Box::pin(async move {
                self.blocks.lock().unwrap().push((new_child, Some(parent)));
            })
        }
    }
    impl SutEditorMirror for MemoryBackend {
        fn caret(&self) -> usize {
            self.caret
        }
    }
    impl CapProvider for MemoryBackend {
        fn register(self: Arc<Self>, caps: &mut CapMap) {
            caps.insert(self.clone() as Arc<dyn SutBackend>);
            caps.insert(self.clone() as Arc<dyn SutBlockTreeWrite>);
            caps.insert(self.clone() as Arc<dyn SutEditorMirror>);
        }
    }

    struct GpuiFrontend {
        degraded: bool,
    }
    impl SutRender for GpuiFrontend {
        fn root_kind(&self) -> &'static str {
            if self.degraded {
                "source_code"
            } else {
                "query_result"
            }
        }
    }
    impl CapProvider for GpuiFrontend {
        fn register(self: Arc<Self>, caps: &mut CapMap) {
            caps.insert(self as Arc<dyn SutRender>);
        }
    }

    struct QueryEngine;
    impl SutQueryResults for QueryEngine {
        fn row_count(&self) -> usize {
            3
        }
    }
    impl CapProvider for QueryEngine {
        fn register(self: Arc<Self>, caps: &mut CapMap) {
            caps.insert(self as Arc<dyn SutQueryResults>);
        }
    }

    // ── The omnipotent Ref: one coupled core, exposed via projection caps. ──
    struct RefCore {
        blocks: Vec<(u64, Option<u64>)>,
        caret: usize,
    }
    impl RefBlockTree for RefCore {
        fn block_exists(&self, id: u64) -> bool {
            self.blocks.iter().any(|(b, _)| *b == id)
        }
        fn all_ids(&self) -> Vec<u64> {
            self.blocks.iter().map(|(b, _)| *b).collect()
        }
    }
    impl RefEditor for RefCore {
        fn caret(&self) -> usize {
            self.caret
        }
    }
    impl CapProvider for RefCore {
        fn register(self: Arc<Self>, caps: &mut CapMap) {
            caps.insert(self.clone() as Arc<dyn RefBlockTree>);
            caps.insert(self.clone() as Arc<dyn RefEditor>);
        }
    }

    fn omnipotent_ref() -> CapMap {
        Config::new()
            .with(RefCore {
                blocks: vec![(1, None), (2, Some(1)), (3, Some(1))],
                caret: 4,
            })
            .build()
    }

    // ── Invariants (single-sourced; the registry is shared by every config). ──
    // Note the async body: `backend.live_block_ids().await` — the macro wraps
    // the body in `Box::pin(async move { … })` so cap reads can `.await`.
    cap_invariant! {
        name: InvNoOrphanBlocks, id: "inv-no-orphan-blocks", mode: Strict,
        sut: { backend: dyn SutBackend },
        sut_absent: [],
        ref: { tree: dyn RefBlockTree },
        check: {
            for id in backend.live_block_ids().await {
                if let Some(p) = backend.parent_of(id).await {
                    if !tree.block_exists(p) {
                        return InvariantResult::Fail(format!("block {id} parent {p} missing"));
                    }
                }
            }
            InvariantResult::Ok
        }
    }

    cap_invariant! {
        name: InvEditorCaretMatchesRef, id: "inv-editor-caret-matches-ref", mode: Strict,
        sut: { editor: dyn SutEditorMirror },
        sut_absent: [],
        ref: { r_editor: dyn RefEditor },
        check: {
            if editor.caret() == r_editor.caret() {
                InvariantResult::Ok
            } else {
                InvariantResult::Fail(format!("caret {} != ref {}", editor.caret(), r_editor.caret()))
            }
        }
    }

    // Full-mode render check: needs BOTH a renderer AND a query engine.
    cap_invariant! {
        name: InvDecompiledRowsRendered, id: "inv-decompiled-rows-rendered", mode: Strict,
        sut: { render: dyn SutRender, query: dyn SutQueryResults },
        sut_absent: [],
        ref: { },
        check: {
            let _ = query.row_count();
            if render.root_kind() == "query_result" {
                InvariantResult::Ok
            } else {
                InvariantResult::Fail(format!("expected query_result, got {}", render.root_kind()))
            }
        }
    }

    // Degraded-mode twin: selected ONLY when no query engine is wired. This is
    // the negative-selection primitive the Ref audit said we need.
    cap_invariant! {
        name: InvShowsSourceWhenNoQuery, id: "inv-shows-source-when-no-query", mode: Strict,
        sut: { render: dyn SutRender },
        sut_absent: [dyn SutQueryResults],
        ref: { },
        check: {
            if render.root_kind() == "source_code" {
                InvariantResult::Ok
            } else {
                InvariantResult::Fail(format!("degraded: expected source_code, got {}", render.root_kind()))
            }
        }
    }

    fn registry() -> Vec<Box<dyn CapInvariant>> {
        vec![
            Box::new(InvNoOrphanBlocks),
            Box::new(InvEditorCaretMatchesRef),
            Box::new(InvDecompiledRowsRendered),
            Box::new(InvShowsSourceWhenNoQuery),
        ]
    }

    // ── Configs: each is just a component list. ──
    fn mem_backend() -> MemoryBackend {
        MemoryBackend {
            blocks: Mutex::new(vec![(1, None), (2, Some(1)), (3, Some(1))]),
            caret: 4,
        }
    }
    fn memory_wide() -> CapMap {
        Config::new().with(mem_backend()).build()
    }
    fn gpui_memory_degraded() -> CapMap {
        Config::new()
            .with(mem_backend())
            .with(GpuiFrontend { degraded: true })
            .build()
    }
    fn full_e2e() -> CapMap {
        Config::new()
            .with(mem_backend())
            .with(GpuiFrontend { degraded: false })
            .with(QueryEngine)
            .build()
    }

    /// The per-slice glue, shaped exactly like a `proptest_state_machine::
    /// StateMachineTest` impl: `apply` mutates the SUT through a write cap (via
    /// `&`), `check_invariants` is SYNC and `block_on`s the async runner.
    struct Slice {
        sut: CapMap,
        ref_: CapMap,
        registry: Vec<Box<dyn CapInvariant>>,
    }
    impl Slice {
        fn apply_split(&self, parent: u64, new_child: u64) {
            // write cap is `&self` (interior mutability) → callable behind `Arc`.
            block_on(
                self.sut
                    .expect::<dyn SutBlockTreeWrite>()
                    .apply_split(parent, new_child),
            );
        }
        fn check_invariants(&self) -> RunReport {
            block_on(run_selected(&self.registry, &self.sut, &self.ref_))
        }
    }

    // ── Tests ──

    /// Risk #1: the async trait-object typemap round-trips; one Arc backs many
    /// caps; an async cap read works through `Arc<dyn Cap>`.
    #[test]
    fn capmap_roundtrips_async_trait_objects() {
        let sut = memory_wide();
        let backend = sut.expect::<dyn SutBackend>();
        let editor = sut.expect::<dyn SutEditorMirror>();
        assert_eq!(block_on(backend.live_block_ids()), vec![1, 2, 3]);
        assert_eq!(editor.caret(), 4); // same underlying MemoryBackend, three caps
        assert!(sut.get::<dyn SutRender>().is_none());
    }

    /// Risk #1b: looking up a cap the config never provided is a loud panic,
    /// not a silent None or a faked value.
    #[test]
    #[should_panic(expected = "was selected but is absent")]
    fn expect_absent_cap_panics_loud() {
        let _ = memory_wide().expect::<dyn SutRender>();
    }

    /// Risk #3: one shared registry, three configs, selection drives coverage —
    /// including the degraded twin via capability ABSENCE.
    #[test]
    fn selection_drives_three_configs_from_one_registry() {
        let reg = registry();
        let r = omnipotent_ref();

        let mem = block_on(run_selected(&reg, &memory_wide(), &r));
        assert_eq!(
            mem.ran_ids(),
            vec!["inv-no-orphan-blocks", "inv-editor-caret-matches-ref"],
        );
        // No renderer → neither render invariant selects.
        assert!(mem
            .deselected
            .iter()
            .any(|i| i.0 == "inv-decompiled-rows-rendered"));
        assert!(mem
            .deselected
            .iter()
            .any(|i| i.0 == "inv-shows-source-when-no-query"));

        let degraded = block_on(run_selected(&reg, &gpui_memory_degraded(), &r));
        // Renderer present, query ABSENT → degraded twin selects, full does not.
        assert!(degraded
            .ran_ids()
            .contains(&"inv-shows-source-when-no-query"));
        assert!(!degraded.ran_ids().contains(&"inv-decompiled-rows-rendered"));

        let full = block_on(run_selected(&reg, &full_e2e(), &r));
        // Query present → full render check selects, degraded twin does NOT.
        assert!(full.ran_ids().contains(&"inv-decompiled-rows-rendered"));
        assert!(!full.ran_ids().contains(&"inv-shows-source-when-no-query"));

        // Every selected body holds in every config (no false divergence).
        assert!(mem.failures().is_empty(), "{:?}", mem.failures());
        assert!(degraded.failures().is_empty(), "{:?}", degraded.failures());
        assert!(full.failures().is_empty(), "{:?}", full.failures());
    }

    /// The complementary twins are mutually exclusive — exactly one renders in
    /// any config that has a renderer.
    #[test]
    fn degraded_and_full_twins_are_mutually_exclusive() {
        let reg = registry();
        let r = omnipotent_ref();
        for sut in [gpui_memory_degraded(), full_e2e()] {
            let ran = block_on(run_selected(&reg, &sut, &r)).ran_ids();
            let full = ran.contains(&"inv-decompiled-rows-rendered");
            let degraded = ran.contains(&"inv-shows-source-when-no-query");
            assert!(
                full ^ degraded,
                "exactly one twin must run; got full={full} degraded={degraded}"
            );
        }
    }

    /// Risk #2: the `StateMachineTest` shape — a `&mut self`-free write cap
    /// mutates the SUT through `&`, then the SYNC `check_invariants` drives the
    /// ASYNC runner via `block_on`, and an async cap read observes the write.
    #[test]
    fn statemachinetest_glue_drives_async_through_sync_check() {
        let slice = Slice {
            sut: memory_wide(),
            ref_: omnipotent_ref(),
            registry: registry(),
        };
        // Structural op: split block 1, adding child 4 — through the write cap.
        slice.apply_split(1, 4);

        let report = slice.check_invariants();
        assert!(report.ran_ids().contains(&"inv-no-orphan-blocks"));
        assert!(report.failures().is_empty(), "{:?}", report.failures());

        // The write landed and an async read cap saw it.
        let backend = slice.sut.expect::<dyn SutBackend>();
        assert!(block_on(backend.live_block_ids()).contains(&4));
    }

    /// `merge_missing` folds a second component's DISJOINT caps into the first
    /// map and reports nothing shadowed — the common case in the by-subsystem
    /// builder (e.g. the canonical backend + a render-only component).
    #[test]
    fn merge_missing_merges_disjoint_caps() {
        let mut a = memory_wide(); // SutBackend, SutBlockTreeWrite, SutEditorMirror
        let b = Config::new()
            .with(GpuiFrontend { degraded: false }) // SutRender
            .with(QueryEngine) // SutQueryResults
            .build();
        let shadowed = a.merge_missing(b);
        assert!(
            shadowed.is_empty(),
            "disjoint merge must shadow nothing, got {shadowed:?}"
        );
        // All five caps now resolve from the single merged map.
        assert!(a.get::<dyn SutBackend>().is_some());
        assert!(a.get::<dyn SutBlockTreeWrite>().is_some());
        assert!(a.get::<dyn SutEditorMirror>().is_some());
        assert!(a.get::<dyn SutRender>().is_some());
        assert!(a.get::<dyn SutQueryResults>().is_some());
        assert_eq!(a.expect::<dyn SutRender>().root_kind(), "query_result");
    }

    /// `merge_missing` is FIRST-REGISTERED-WINS: when two components provide the
    /// same caps (both backends provide `SutBackend`/…), the precedence-winning
    /// (already-present) provider is kept and the loser's overlapping caps are
    /// returned for disclosure. This is the silent-shadow hazard the primitive
    /// exists to make explicit.
    #[test]
    fn merge_missing_keeps_first_registered_and_reports_shadow() {
        let mut a = Config::new()
            .with(MemoryBackend {
                blocks: Mutex::new(vec![(1, None)]),
                caret: 4,
            })
            .build();
        let b = Config::new()
            .with(MemoryBackend {
                blocks: Mutex::new(vec![(9, None)]),
                caret: 99,
            })
            .build();
        let mut shadowed = a.merge_missing(b);
        shadowed.sort_unstable();
        assert_eq!(
            shadowed,
            vec!["SutBackend", "SutBlockTreeWrite", "SutEditorMirror"],
            "all three overlapping caps must be reported as shadowed"
        );
        // First-registered wins: A's caret 4 (not B's 99), A's blocks [1] (not [9]).
        assert_eq!(a.expect::<dyn SutEditorMirror>().caret(), 4);
        assert_eq!(
            block_on(a.expect::<dyn SutBackend>().live_block_ids()),
            vec![1]
        );
    }
}
