//! Interpreter-level value type — `Value` plus reactive kinds.
//!
//! `Value` (in `lib.rs`) is the JSON-mirror datatype: it must stay
//! serializable and free of `Arc`s / signals / trait objects.
//! Reactive values are an interpreter-level concern, so they live in
//! this separate enum. Consumers at FFI / MCP / serde boundaries
//! always work in `Value`; consumers inside the render interpreter
//! work in `InterpValue`.
//!
//! See `~/.claude/plans/peaceful-strolling-muffin.md` for the design
//! rationale (why we do NOT add reactive variants to `Value` itself).

use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal_vec::SignalVec;

use crate::EntityUri;
use crate::Value;
use crate::widget_spec::DataRow;

/// A value produced during render-expression interpretation.
///
/// The reactive variants are reserved for future kinds (scalar signals,
/// typed signal-vecs). For this PR, only `Value` and `Rows` are
/// materialized.
pub enum InterpValue {
    /// JSON-shaped scalar — literal, column lookup, binary op, or the
    /// return value of a plain-value function.
    Value(Value),
    /// A reactive row set. Produced by value functions like
    /// `focus_chain()`, `chain_ops(level)`, `ops_of(uri)`.
    Rows(Arc<dyn ReactiveRowProvider>),
    // Reserved — not in this PR:
    //   Reactive(Arc<dyn ReactiveScalar>)      // Mutable<Value> / Signal<Value>
    //   ReactiveVec(Arc<dyn ReactiveVec>)      // MutableVec<Value> / SignalVec<Value>
    //
    // Closed-enum form is pragmatic given Rust's lack of HKTs. Adding a
    // new kind is additive — no existing call site needs updating.
}

/// Opaque identifier for one display-placement occurrence of a block.
///
/// "Which display placement produced this row." Deliberately opaque — no
/// arithmetic exposed — so no caller can assume it is a positional/render-path
/// index (ADR 0016 rejects that identity). It is `Ord`/`Hash` so it can key the
/// driver-local `(EntityUri, Occurrence)` keyspace. The inner payload is a
/// deterministic function of the placement (canonical id + anchor), so a fresh
/// provider construction reproduces the same key — the invariant the provider
/// cache's `Weak` lifecycle relies on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OccurrenceId(u64);

impl OccurrenceId {
    /// Mint the id for a display placement of `canonical` under `anchor`.
    /// Deterministic (a hash), not a counter — the provider must be a pure
    /// function of its inputs (see [`ReactiveRowProvider`] doc).
    pub fn for_placement(canonical: &EntityUri, anchor: &EntityUri) -> Self {
        use std::hash::Hash;
        use std::hash::Hasher;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        canonical.as_str().hash(&mut h);
        anchor.as_str().hash(&mut h);
        Self(h.finish())
    }

    /// The stringly suffix appended to a GPUI identity key for a placed
    /// occurrence. This is the ONE stringly-typed boundary for the occurrence
    /// coordinate (parse-don't-validate: the type is authoritative everywhere
    /// else).
    pub fn key_suffix(&self) -> String {
        format!("-occ{:016x}", self.0)
    }
}

/// The occurrence coordinate of a focus/caret/row-identity key (ADR 0016 §1
/// widened tuple). `Canonical` is the default that keeps the widening
/// behavior-preserving: every existing key carries `Occurrence::Canonical` and
/// so renders byte-identical GPUI identity keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Occurrence {
    #[default]
    Canonical,
    Placed(OccurrenceId),
}

impl Occurrence {
    /// Suffix for a GPUI identity key. Empty for `Canonical` (byte-identical
    /// keys), the occurrence suffix for `Placed`.
    pub fn key_suffix(&self) -> String {
        match self {
            Occurrence::Canonical => String::new(),
            Occurrence::Placed(occ) => occ.key_suffix(),
        }
    }
}

/// The per-row identity key carried by
/// [`ReactiveRowProvider::keyed_rows_signal_vec`]: the entity plus which
/// display occurrence of it this row is. The `ReactiveRowSet` store stays
/// `EntityUri`-keyed — this widened key lives only in the driver-local diffing
/// layer (ADR 0016 §3).
pub type RowKey = (EntityUri, Occurrence);

/// Reactive row-set surface consumed by streaming collections.
///
/// **Implementation constraint**: providers MUST be derivable functions
/// of upstream signals (UiState, ProfileResolver, navigation, …), not
/// stateful accumulators. A fresh construction must reproduce the same
/// observable state as an existing one. This is what makes the cache's
/// `Weak`-ref lifecycle safe (see `ProviderCache` in `holon-frontend`).
///
/// If a future provider needs to accumulate transient state (e.g.
/// "first-seen-at"), the cache shape escalates to `Strong` + explicit
/// eviction — out of scope for this PR.
pub trait ReactiveRowProvider: Send + Sync {
    /// Synchronous snapshot of the current rows. Each row is keyed by
    /// the provider's per-row stable identity.
    fn rows_snapshot(&self) -> Vec<Arc<DataRow>>;

    /// Per-row `SignalVec` — emits `VecDiff` on row insert / remove /
    /// update.
    fn rows_signal_vec(&self) -> Pin<Box<dyn SignalVec<Item = Arc<DataRow>> + Send>>;

    /// Per-row keyed `SignalVec` — used by `MutableTree` and driver
    /// code that needs to track per-row identity for `VecDiff::RemoveAt`.
    fn keyed_rows_signal_vec(
        &self,
    ) -> Pin<Box<dyn SignalVec<Item = (RowKey, Arc<DataRow>)> + Send>>;

    /// Stable identity for caching and widget-identity keys. Must be
    /// stable for the lifetime of the provider; equal providers should
    /// return equal identities (e.g. for cache de-dup).
    ///
    /// Trivial impl: `ptr_identity(self)`.
    fn cache_identity(&self) -> u64;

    /// Optional shared `ReadOnlyMutable<Arc<DataRow>>` for the row keyed by
    /// `id`. Returned when the provider is single-writer (e.g.
    /// `ReactiveRowSet` driven by Turso CDC) so downstream nodes can clone
    /// the handle, share the underlying `Arc<MutableState>`, and see updates
    /// without any tree walk.
    ///
    /// **`ReadOnlyMutable` is the load-bearing type here.** The provider
    /// keeps the writable `Mutable` private — calling `.set()` on the row
    /// cell is only possible through the provider's own internal handle.
    /// Every consumer outside the provider receives a `ReadOnlyMutable`
    /// clone, so any leaf builder that tries to mutate row data is a
    /// compile error. This is what keeps the "one writer =
    /// `ReactiveRowSet::apply_change`" invariant from drifting back to
    /// multi-writer land in some future change.
    ///
    /// Default: `None` for synthetic providers (focus_chain, ops_of, …)
    /// that don't have per-row signal cells. Callers fall back to a
    /// one-shot `Mutable::new(snapshot).read_only()` — also `ReadOnlyMutable`,
    /// also no `.set()`, just no upstream to fire.
    fn row_mutable(
        &self,
        _: &EntityUri,
    ) -> Option<futures_signals::signal::ReadOnlyMutable<Arc<DataRow>>> {
        None
    }
}

/// Convenience helper for the common `cache_identity` body —
/// hashes the provider's address. Stable for the lifetime of `&self`.
pub fn ptr_identity<T: ?Sized>(this: &T) -> u64 {
    this as *const T as *const () as usize as u64
}
