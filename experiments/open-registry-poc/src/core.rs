//! The *central* code — and the point of the PoC is how little of it there is.
//!
//! This file knows about NO concrete transition, generator, or invariant. It
//! defines the contracts and two link-time registries plus the `CapMap` SUT.
//! New modules add variants by `inventory::submit!`-ing (via `cap_transition!` /
//! `inventory::submit!`); nothing here changes. Contrast with the production
//! `declare_e2e_transitions! { ApplyMutation, ArrowNavigate, ... }` enum.
//!
//! ## The SUT is a `CapMap` — the same type the γ design composes
//!
//! Rather than a bespoke `dyn Sut` bundle, the SUT is a capability typemap keyed
//! by each cap trait's `TypeId`, storing the provider as `Arc<dyn Cap>` (mirrors
//! `holon-pbt-core/src/composition.rs::CapMap`). A transition drives it through
//! `sut.expect::<dyn SutBlockTreeWrite>()` — but bodies never write that by hand;
//! the `cap_transition!` macro injects the extraction from the declared cap set
//! (the analog of γ's `cap_invariant!`).
//!
//! ## Caps are trait types (`TypeId`), and writes use interior mutability
//!
//! A cap is a fine-grained SUT trait; its identity is `cap::<dyn Trait>()` =
//! `TypeId::of::<dyn Trait>()` (= production `CapId`). Providers live behind an
//! `Arc`, so write caps take `&self` and mutate through interior mutability
//! (`Mutex`) — exactly §4.4 / Recipe 2. The SUT is therefore driven through
//! `&CapMap`; `&mut CapMap` would only be needed to *restructure* the map
//! (add/remove a provider — AddPeer/lifecycle), which this PoC doesn't exercise.

use proptest::strategy::BoxedStrategy;
use std::any::{type_name, Any, TypeId};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────
// Fine-grained SUT capability traits, grouped by the component that provides
// them. Write caps are `&self` (providers sit behind `Arc`; mutation is
// interior, §4.4). The grouping gives §8.7 TWO independent optional axes
// (Toggle, Editor) over an always-on BlockTree substrate.
// ─────────────────────────────────────────────────────────────────────────

// Always-on: the block-tree store.
pub trait SutBlockTreeWrite {
    fn split(&self, target: u64, new_id: u64);
}
pub trait SutBlockRead {
    fn blocks(&self) -> Vec<u64>;
}

// Optional axis 1: Toggle.
pub trait SutToggleWrite {
    fn toggle(&self, target: u64);
}
pub trait SutToggleRead {
    fn is_toggled(&self, id: u64) -> bool;
}

// Optional axis 2: Editor.
pub trait SutEditorWrite {
    fn type_char(&self, ch: char);
}
pub trait SutEditorRead {
    fn text(&self) -> String;
}

/// The optional subsystems §8.7 shrinks over (BlockTree is the always-on
/// substrate, so it isn't in the universe). This is a STABLE, serializable enum
/// — the shrink axis is keyed on it, not on the non-stable `TypeId`/`type_name`
/// that identifies caps. (The point raised earlier: serialize the config on
/// stable names, gate the alphabet on `TypeId`.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Subsystem {
    Toggle,
    Editor,
}

// ─────────────────────────────────────────────────────────────────────────
// CapRef: the `TypeId` of a cap trait + that trait's name (display only —
// identity is the `TypeId`). `cap::<T>()` derives both from the type, so there
// is no `Cap` enum and adding a cap trait needs no central edit.
// ─────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug)]
pub struct CapRef {
    pub id: TypeId,
    pub name: &'static str,
}

pub fn cap<T: ?Sized + 'static>() -> CapRef {
    let full = type_name::<T>();
    CapRef {
        id: TypeId::of::<T>(),
        name: full.rsplit("::").next().unwrap_or(full),
    }
}

impl PartialEq for CapRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for CapRef {}
impl std::hash::Hash for CapRef {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.id.hash(h);
    }
}

#[derive(Clone, Default)]
pub struct CapSet(HashSet<TypeId>);

impl CapSet {
    pub fn satisfies(&self, required: &[CapRef]) -> bool {
        required.iter().all(|c| self.0.contains(&c.id))
    }
    pub fn names(&self, all: &[CapRef]) -> Vec<&'static str> {
        all.iter().filter(|c| self.0.contains(&c.id)).map(|c| c.name).collect()
    }
}

impl std::fmt::Debug for CapSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CapSet({} caps)", self.0.len())
    }
}

// ─────────────────────────────────────────────────────────────────────────
// CapMap — the composed SUT. Keyed by cap `TypeId`, stores `Arc<dyn Cap>`
// round-tripped through `Any`. One `Arc` (a component) backs every cap it
// provides, so writes via one cap are visible through every other.
// ─────────────────────────────────────────────────────────────────────────
#[derive(Default)]
pub struct CapMap {
    caps: HashMap<TypeId, Box<dyn Any>>,
}

impl CapMap {
    pub fn new() -> Self {
        CapMap { caps: HashMap::new() }
    }

    /// Host `provider` under cap `C`. The component unsize-coerces at the call
    /// site (`map.insert::<dyn SutRead>(backend.clone())`).
    pub fn insert<C: ?Sized + 'static>(&mut self, provider: Arc<C>) {
        self.caps.insert(TypeId::of::<C>(), Box::new(provider));
    }

    /// Register a component for all the caps it provides.
    pub fn with_arc<P: CapProvider + 'static>(mut self, c: Arc<P>) -> Self {
        c.register(&mut self);
        self
    }

    /// Look up cap `C`. Panics loud (with the trait name) on a *selected-but-
    /// absent* cap — an assertion of an already-proven fact, never a faked
    /// `None`/`unimplemented!` (selection runs before dispatch).
    pub fn expect<C: ?Sized + 'static>(&self) -> Arc<C> {
        let boxed = self.caps.get(&TypeId::of::<C>()).unwrap_or_else(|| {
            panic!(
                "cap {} selected but absent — selection runs first, so this lookup is proven present",
                type_name::<C>()
            )
        });
        boxed
            .downcast_ref::<Arc<C>>()
            .expect("cap stored under the wrong TypeId")
            .clone()
    }

    pub fn cap_set(&self) -> CapSet {
        CapSet(self.caps.keys().copied().collect())
    }
}

/// A component contributes one or more caps. The single `Arc<Self>` it registers
/// backs each one (interior mutability lets it be both write-driven and read).
pub trait CapProvider {
    fn register(self: Arc<Self>, map: &mut CapMap);
}

// ─────────────────────────────────────────────────────────────────────────
// Reference model.
// ─────────────────────────────────────────────────────────────────────────
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefState {
    pub blocks: Vec<u64>,
    pub toggled: BTreeSet<u64>,
    pub editor: String,
    pub next_id: u64,
}

impl RefState {
    pub fn seeded(ids: &[u64]) -> Self {
        RefState {
            blocks: ids.to_vec(),
            toggled: BTreeSet::new(),
            editor: String::new(),
            next_id: ids.iter().copied().max().map_or(1, |m| m + 1),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The open behaviour trait. NOTE the signature: `apply_to_sut` takes NO ref —
// transitions are self-contained (all data baked in at generation time), the
// direction the production transition refactor went. Ref is read only by the
// *generator* and mutated only by `apply_to_ref`.
//
// `#[typetag::serde]` = open-world replay; `DynClone` = shrink Clone. Production
// `apply_to_sut` is `async` (read caps are `#[async_trait(?Send)]`); kept sync
// here so the PoC needs no async-trait/executor plumbing — orthogonal to the
// three points this revision demonstrates.
// ─────────────────────────────────────────────────────────────────────────
#[typetag::serde(tag = "kind")]
pub trait Transition: dyn_clone::DynClone + std::fmt::Debug + Send + Sync {
    fn variant_name(&self) -> &'static str;
    fn required_caps(&self) -> Vec<CapRef>;
    fn preconditions(&self, state: &RefState) -> Result<(), String>;
    fn apply_to_ref(&self, state: &mut RefState);
    fn apply_to_sut(&self, sut: &CapMap);
}

dyn_clone::clone_trait_object!(Transition);

// ─────────────────────────────────────────────────────────────────────────
// Registry #1 — generators (one per transition, emitted by `cap_transition!`).
// ─────────────────────────────────────────────────────────────────────────
pub struct TransitionGen {
    pub name: &'static str,
    pub required_caps: fn() -> Vec<CapRef>,
    pub gen: fn(&RefState) -> Option<(u32, BoxedStrategy<Box<dyn Transition>>)>,
}
inventory::collect!(TransitionGen);

// ─────────────────────────────────────────────────────────────────────────
// Registry #2 — invariants. Now cap-gated like transitions: an invariant
// declares the caps its body reads, and the runner selects it only when the
// SUT provides them (so `expect` is always a proven-present lookup, never a
// panic). The same gating predicate drives both registries.
// ─────────────────────────────────────────────────────────────────────────
pub struct Invariant {
    pub name: &'static str,
    pub required_caps: fn() -> Vec<CapRef>,
    pub check: fn(&RefState, &CapMap) -> Result<(), String>,
}
inventory::collect!(Invariant);

// ─────────────────────────────────────────────────────────────────────────
// The only consumers of the registries.
// ─────────────────────────────────────────────────────────────────────────

pub fn discovered_transitions() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut v: Vec<_> = inventory::iter::<TransitionGen>
        .into_iter()
        .map(|g| (g.name, (g.required_caps)().iter().map(|c| c.name).collect()))
        .collect();
    v.sort_by_key(|(n, _)| *n);
    v
}

/// Build the weighted alphabet for `state` under `caps`. The variant set comes
/// from the registry; the cap gate compares `TypeId`s.
pub fn build_alphabet(state: &RefState, caps: &CapSet) -> BoxedStrategy<Box<dyn Transition>> {
    use proptest::strategy::{Strategy, Union};

    let mut arms: Vec<(u32, BoxedStrategy<Box<dyn Transition>>)> = Vec::new();
    for g in inventory::iter::<TransitionGen> {
        if !caps.satisfies(&(g.required_caps)()) {
            continue; // cap gate (necessary)
        }
        if let Some(arm) = (g.gen)(state) {
            arms.push(arm); // dynamic gate (sufficient)
        }
    }
    assert!(
        !arms.is_empty(),
        "no transition applicable in {state:?} under {caps:?}"
    );
    Union::new_weighted(arms).boxed()
}

/// Run every registered invariant whose caps the SUT provides; collect failures.
pub fn check_invariants(state: &RefState, sut: &CapMap) -> Result<(), Vec<String>> {
    let caps = sut.cap_set();
    let failures: Vec<String> = inventory::iter::<Invariant>
        .into_iter()
        .filter(|inv| caps.satisfies(&(inv.required_caps)()))
        .filter_map(|inv| (inv.check)(state, sut).err().map(|e| format!("{}: {e}", inv.name)))
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}
