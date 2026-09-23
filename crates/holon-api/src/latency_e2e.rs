//! End-to-end interaction latency: dispatch → rows-visible (`stage="e2e"`).
//!
//! The per-stage `holon_latency` events (`dispatch`, `projection`, `rows`)
//! measure pipeline components in isolation; none measures what the user
//! feels. This module correlates the two ends of the prod pipeline across the
//! async task boundary and emits the wall time from interaction to visible row.
//!
//! # Correlation identity — why FIFO-by-target is wrong
//!
//! An interaction addresses a **target** entity (the op's `id` param). The
//! naive correlator matched a delivered CDC batch to the *oldest* pending
//! entry for that target (FIFO). That mis-attributes whenever a dispatch
//! produces **no CDC delta** — e.g. a coalesced / identity re-commit (the
//! editor's blur re-commit of already-stored content). The no-op leaves a
//! pending entry that squats until its 30s expiry and then *steals* the match
//! from the next real commit on the same block, reporting that stale entry's
//! huge elapsed as the e2e latency. This flooded the SLO oracle with phantom
//! `>10s` banners on pipelines whose real p95 was ~30ms (BugFunnel 2026-07-13).
//!
//! The fix correlates by **op instance**, not FIFO:
//!
//! - Editor content writes already carry a [`WriteSeq`] token (process-global,
//!   monotonic; stamped into the op params, round-tripped through
//!   `block_raw.write_seq`, and projected into the block matview → it reaches
//!   the delivered CDC row). Where a token is present on both ends we close
//!   **exactly** the entry whose `WriteSeq` the delivered row carries. A no-op
//!   commit's token never appears in any delta, so it can never be closed and
//!   can never steal — it is dropped the moment a newer commit on its target
//!   completes, or it expires harmlessly.
//! - Ops that carry no token (toggle / split / delete / create-by-parent) fall
//!   back to closing the **newest** pending entry for the target and dropping
//!   all older same-target entries as superseded no-ops. This holds even when
//!   the delivered row carries a token: `write_seq` is **sticky on the row**
//!   (it records the last *editor* write, and every later projection repeats
//!   it), so a token nobody is waiting for is not evidence about the op in
//!   flight and never blocks a tokenless entry.
//!
//! In both paths the invariant holds: a measurement is **never** attributed
//! across a newer, already-completed entry on the same target — so the SLO
//! oracle (which merely reads the emitted `e2e` events) can never fire on a
//! mis-attributed measurement.
//!
//! # Correlation identity, part two: an id is not enough
//!
//! A target id alone does not say *what* was delivered. A navigation and an
//! edit can address the same block, and their deliveries arrive in different
//! mirrors: an edit surfaces as a **block row**, a navigation as the
//! **focus-root row** naming the block. [`Observable`] carries that distinction
//! on both ends, and matching requires equality.
//!
//! Without it a navigation had no reachable observable at all — `focus_roots`
//! rows are `(region, root_id)`, no `id`, so the block-row reader saw an empty
//! batch — while any later edit under the page delivered `parent_id = page_id`
//! and closed the navigate, billing it every millisecond since the click: four
//! full-width false `[latency-slo]` banners in one 15-minute session, up to
//! 11982ms for a navigation visible in ~100ms (BugFunnel 2026-08-08).
//!
//! A navigation whose focus root does not change (a **warm switch**) produces
//! no delivery, so it has no measurement — it expires loudly as
//! `stage="e2e_expired"`. Measuring it would need a signal the pipeline does
//! not emit; borrowing an unrelated batch's clock is what this fix removes.
//!
//! - [`interaction_dispatched`] — called at the operation-dispatch entry point
//!   (`holon-frontend` `dispatch_operation` / `dispatch_intent{,_sync}`) with
//!   the op name, the target entity id, and the [`Observable`] the interaction
//!   will become visible as. Starts the clock.
//! - [`interaction_failed`] — called from the `Err` arm of the same dispatch
//!   seams. A refused or failed op writes nothing, so its entry is retired at
//!   once instead of squatting until expiry, where any unrelated later delivery
//!   for that row would close it as a measurement of nothing.
//! - [`rows_delivered`] — called from `LiveData::subscribe` with the `(id,
//!   Observable)` pairs an applied batch made visible (see
//!   [`touched_entities`]). Closes the matching entries and emits, per closure:
//!
//!   `tracing::info!(target="holon_latency", stage="e2e", action, block,
//!   origin, source, ms, in_flight, backlog, delivery_batch)`
//!
//!   `delivery_batch` names the `rows_delivered` call that closed the entry,
//!   so a consumer can tell which samples one applied batch retired together.
//!
//! # Two clocks, never one number
//!
//! Every clock names its [`ClockOrigin`]. A `ui` clock opens where a platform
//! input event crosses the frontend dispatch seam; a `facade` clock opens
//! inside `HolonService::execute_operation`, above that seam, so it excludes
//! the frontend's own dispatch cost. Their samples measure different spans and
//! are scored in separate windows (D119.a) — see
//! [`crate::latency_slo::SloWindow`], which is origin-scoped so no consumer can
//! pool them by accident.
//!
//! The separation reaches deeper than the windows: the pending registry itself
//! is PARTITIONED by origin (`Registry`). Queue depth, supersession and
//! overflow eviction are each computed inside one partition, because each of
//! them otherwise leaks one origin's traffic into the other's measurements —
//! queue depth decides UI service-time eligibility, a supersession silently
//! deletes the other origin's clock, and an eviction silently drops it.
//!
//! `ms` alone is service time plus queue wait. `in_flight` (queue depth at
//! dispatch, **within this origin**) and `backlog` (this origin's depth after
//! the delivery) are what let a consumer separate the two — see
//! [`crate::latency_slo`], which both the runtime oracle and the land gate
//! score through.
//!
//! # SLO endpoint: projection-visible, deliberately NOT frame-present
//!
//! The `stage="e2e"` span closes **here** — inside the `LiveData::subscribe`
//! tokio actor, right after `apply_changes` lands the batch in the reactive
//! mirror (see [`rows_delivered`]'s call site in `holon_api::live_data`). That
//! is *projection-visible*: the data is committed and available for the view
//! model to render. It is the endpoint the project SLO names
//! ("interaction→projection-visible < 200ms", CLAUDE.md), and it is the stage
//! the latency-slo oracle judges.
//!
//! We intentionally do **not** advance this endpoint to GPU frame-present. A
//! backgrounded / occluded window defers presents indefinitely (the OS
//! throttles the render loop), so anchoring the SLO on paint would convert OS
//! scheduling into multi-second *false* SLO violations — exactly the "navigate
//! 28.7s" artifact observed while a window was backgrounded for screenshotting
//! (dogfood-round3 B3). Projection-visible is measured on the tokio CDC actor,
//! independent of the render loop, so it reflects the pipeline the SLO is
//! about.
//!
//! Honest boundary: when the *whole* foreground pipeline is stalled (the app is
//! backgrounded and the write/re-projection work that feeds the CDC stream is
//! itself throttled), even this projection-visible measurement inflates. That
//! inflation is a real stall, so we **surface it, never suppress it** (the
//! oracle still fires); distinguishing "user waited" from "OS throttled us"
//! requires a foreground-state signal the correlator does not have.
//!
//! Boundary disclosures:
//! - Final GPU paint is out of scope (this is a feature, see above — not a
//!   gap).
//! - Ops without an `id` param, and deletes whose CDC `Deleted.id` is a rowid
//!   rather than the entity id, are not correlated (no event).
//! - Entries expire after 30s (op failed / never touched its target row), and
//!   every expiry is disclosed as `stage="e2e_expired"`. With identity
//!   correlation an unexpired no-op entry is inert — it cannot steal.
//!
//! Cost: one small mutex op per dispatch; per-batch cost is a single atomic
//! load while no interaction is pending (the overwhelmingly common case).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use crate::write_seq::WriteSeq;

/// Which seam opened the interaction clock.
///
/// A [`ClockOrigin::Ui`] sample is a whole user interaction: a platform input
/// event crosses the frontend dispatch seam and the clock starts there. A
/// [`ClockOrigin::Facade`] sample starts INSIDE
/// `HolonService::execute_operation` — the session facade agent/MCP-driven
/// operations enter through — so it never carries the frontend's dispatch cost.
/// The two therefore measure different spans of the same pipeline and must
/// never be pooled into one percentile (Martin's ruling D119.a, 2026-09-12);
/// [`crate::latency_slo::SloWindow`] is origin-scoped so a pooled statistic
/// cannot be computed at all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClockOrigin {
    /// A frontend dispatch seam (`holon_frontend::operations` / `reactive`).
    Ui,
    /// `holon::api::HolonService::execute_operation` — the agent/MCP session
    /// facade. Above the dispatch seam, so the frontend's own cost is excluded.
    Facade,
}

impl ClockOrigin {
    /// The other seam. Two variants, so cross-origin contention has exactly one
    /// counterpart and a caller never has to enumerate.
    pub const fn other(self) -> ClockOrigin {
        match self {
            ClockOrigin::Ui => ClockOrigin::Facade,
            ClockOrigin::Facade => ClockOrigin::Ui,
        }
    }

    /// The wire form carried on the `origin` field of every `stage="e2e"`
    /// event.
    pub const fn as_str(self) -> &'static str {
        match self {
            ClockOrigin::Ui => "ui",
            ClockOrigin::Facade => "facade",
        }
    }
}

/// An `origin` field value that names no known clock seam. Parsing fails loudly
/// rather than defaulting: a defaulted origin is how a facade sample would end
/// up scored as a UI one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownClockOrigin(pub String);

impl std::fmt::Display for UnknownClockOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown latency clock origin {:?} (expected \"ui\" or \"facade\")",
            self.0
        )
    }
}

impl std::error::Error for UnknownClockOrigin {}

impl std::str::FromStr for ClockOrigin {
    type Err = UnknownClockOrigin;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ui" => Ok(ClockOrigin::Ui),
            "facade" => Ok(ClockOrigin::Facade),
            other => Err(UnknownClockOrigin(other.to_string())),
        }
    }
}

/// What an interaction is waiting to SEE, and what a batch actually delivered —
/// the same vocabulary on both ends of the correlation.
///
/// A block write becomes visible as its block row; a navigation becomes visible
/// as its region's focus-root row, which lives in a different mirror and
/// carries no `id` at all. Matching requires the two ends to agree, because a
/// block-row batch is no evidence that a navigation rendered: billing one to
/// the other is how a navigate sample accumulated 12s of idle human time and
/// fired the SLO oracle (BugFunnel 2026-08-08).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Observable {
    /// The target's row lands in a block mirror. Carries the op-instance token
    /// when the op stamped one (editor content writes); `None` for tokenless
    /// ops (toggle/split/delete/...) and for `parent_id` correlations.
    BlockRow(Option<WriteSeq>),
    /// The target becomes a region's focus root — the delivery that IS a
    /// navigation. Carries no token by construction: navigation writes
    /// `navigation_history`, never a block row.
    FocusRoot,
}

/// The [`Observable`] discriminant, without the token. Two observables
/// correlate only when their kinds are equal.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    BlockRow,
    FocusRoot,
}

impl Observable {
    fn kind(self) -> Kind {
        match self {
            Observable::BlockRow(_) => Kind::BlockRow,
            Observable::FocusRoot => Kind::FocusRoot,
        }
    }

    fn seq(self) -> Option<WriteSeq> {
        match self {
            Observable::BlockRow(seq) => seq,
            Observable::FocusRoot => None,
        }
    }
}

struct Pending {
    action: String,
    target: String,
    /// The seam that opened this clock. Carried through to the emitted event so
    /// a consumer never has to infer it from the action name.
    origin: ClockOrigin,
    /// What this interaction is waiting to see (and its op-instance token, if
    /// the op carried one).
    observable: Observable,
    t0: Instant,
    /// Interactions of THIS origin in flight the moment this one was
    /// dispatched, itself included. `1` means nothing of its own kind was
    /// queued ahead of it. Partitioned so a population never moves with the
    /// other origin's traffic — see `Registry`.
    in_flight: usize,
    /// Whether ANY interaction of the OTHER origin overlapped this one's life.
    ///
    /// An EVENT over the interval, not a reading at an instant. Set when this
    /// clock is enrolled into a non-empty other slot, and set again on every
    /// pending entry of the other slot whenever a clock is enrolled there —
    /// so an overlap is recorded whichever of the two started first. Never
    /// cleared: a delivery ends this interaction's life, it does not un-share
    /// the pipeline it ran in.
    ///
    /// Two sampled instants (at dispatch, and after the batch settled) could
    /// not carry this. A facade clock that opened AFTER a UI clock's dispatch
    /// and closed BEFORE its delivery left both readings at zero, and the UI
    /// sample scored as uncontended service time — and containment is the
    /// EXPECTED shape here, because facade spans are systematically shorter
    /// than UI spans (23ms vs 29ms p50, measured on this lane's own rung).
    contended: bool,
}

/// One closed correlation, ready to emit. Returned by the pure
/// [`close_delivered`] core so the matching logic is testable without the
/// tracing/global plumbing.
struct Closed {
    action: String,
    target: String,
    origin: ClockOrigin,
    ms: u64,
    in_flight: usize,
    /// Interactions of THIS origin still pending after this one closed. `> 0`
    /// means the pipeline was saturated at this delivery, so the wait until the
    /// next delivery is drain time and not the driver idling.
    backlog: usize,
    /// Whether the other origin overlapped this interaction's life, carried
    /// from [`Pending`].
    contended: bool,
}

/// The pending-interaction registry, **partitioned by [`ClockOrigin`]**.
///
/// One shared `Vec` was the wrong shape once a second seam existed, and in
/// three separate ways — all of them cross-origin contamination of the UI
/// percentile:
///
/// * `in_flight` / `backlog` counted BOTH origins, and
///   [`crate::latency_slo::E2eSample::is_service_time`] gates admission to the
///   service-time rung on those counters. A facade clock in flight made a
///   concurrent UI interaction ineligible, so the UI rung's population moved
///   with the agent traffic mix — the harm D119.a forbids, arriving through the
///   eligibility filter instead of through the window.
/// * `close_delivered` deleted older entries of the same `(target, kind)` as
///   superseded no-ops. A UI clock and a facade clock on one block are not
///   supersessions of each other, and whichever was older vanished silently.
/// * overflow eviction dropped the globally-oldest entry, so a burst of facade
///   clocks could evict pending UI ones.
///
/// Partitioning fixes all three at once and by construction rather than by
/// three remembered filters: every count, every supersession and every eviction
/// is scoped to one slot because it can only ever see one slot.
///
/// A delivery is still offered to BOTH slots (see [`rows_delivered`]): two
/// interactions of different origins waiting on the same row are both made
/// visible by it, and each gets its own sample.
#[derive(Default)]
struct Registry {
    ui: Vec<Pending>,
    facade: Vec<Pending>,
}

impl Registry {
    fn slot(&mut self, origin: ClockOrigin) -> &mut Vec<Pending> {
        match origin {
            ClockOrigin::Ui => &mut self.ui,
            ClockOrigin::Facade => &mut self.facade,
        }
    }

    fn len(&self) -> usize {
        self.ui.len() + self.facade.len()
    }

    /// Every pending entry, both origins. For diagnostics and the fast-path
    /// count ONLY — never for a measurement, which is always slot-scoped.
    fn iter(&self) -> impl Iterator<Item = &Pending> {
        self.ui.iter().chain(self.facade.iter())
    }
}

/// Every origin, so a loop over the whole registry cannot forget one.
const ORIGINS: [ClockOrigin; 2] = [ClockOrigin::Ui, ClockOrigin::Facade];

static PENDING_LEN: AtomicUsize = AtomicUsize::new(0);
static PENDING: Mutex<Registry> = Mutex::new(Registry {
    ui: Vec::new(),
    facade: Vec::new(),
});
/// Per-ORIGIN capacity: one origin's burst can never evict another's entries.
pub const MAX_PENDING: usize = 64;
/// Sequence of `rows_delivered` calls; every sample one call closes shares it.
static DELIVERY_BATCH: AtomicU64 = AtomicU64::new(0);
const EXPIRY: Duration = Duration::from_secs(30);

/// Extract the op-instance token from op params: the editor stamps `write_seq`
/// (`> 0`) into content-write params; every other op omits it. A `0` value is
/// the row default (never editor-written) and is treated as absent.
pub fn write_seq_from_params(params: &HashMap<String, crate::Value>) -> Option<WriteSeq> {
    write_seq_from_value(params.get("write_seq"))
}

/// [`write_seq_from_params`] for a params bag that is not keyed by `String` —
/// the facade dispatches a [`crate::StorageEntity`], whose keys are `Arc<str>`.
pub fn write_seq_from_value(v: Option<&crate::Value>) -> Option<WriteSeq> {
    v.and_then(|v| v.as_i64())
        .filter(|&s| s > 0)
        .map(WriteSeq::from_i64)
}

/// One pending entry removed without a measurement — expired, or retired
/// because its op produced no write. Carries what the disclosure names.
struct Expired {
    action: String,
    target: String,
    origin: ClockOrigin,
    waited_ms: u64,
}

/// Drop the entries older than [`EXPIRY`], returning every one of them for
/// disclosure. An expiry means the correlator never saw a delivery it could
/// tie to this interaction — for writes it is either a genuine no-op re-commit
/// (expected, harmless) or a correlation defect that suppresses the whole
/// measurement for that op class. The two are indistinguishable from inside,
/// so the caller warns for both: an unmeasured interaction dropped in silence
/// is precisely how the uncloseable-`split_block` defect survived a dogfood
/// night, and silent degradation is what this project forbids.
fn prune_expired(pending: &mut Vec<Pending>, now: Instant) -> Vec<Expired> {
    let mut expired = Vec::new();
    pending.retain(|e| {
        let waited = now.duration_since(e.t0);
        if waited < EXPIRY {
            return true;
        }
        expired.push(Expired {
            action: e.action.clone(),
            target: e.target.clone(),
            origin: e.origin,
            waited_ms: waited.as_millis() as u64,
        });
        false
    });
    expired
}

/// One pending entry dropped because its origin's slot was full. Distinct from
/// [`Expired`] because the cause is different and so is the remedy: an expiry
/// means no delivery arrived, an eviction means the correlator ran out of room.
struct Evicted {
    action: String,
    target: String,
    waited_ms: u64,
}

/// Overflow is a LOST measurement, so it is disclosed like every other one. A
/// silent `remove(0)` is the "degrades to look fine" case: the interaction
/// simply never appears in any window and nothing says why.
fn disclose_evicted(e: &Evicted, origin: ClockOrigin) {
    tracing::warn!(
        target: "holon_latency",
        stage = "e2e_evicted",
        action = %e.action,
        block = %e.target,
        origin = origin.as_str(),
        waited_ms = e.waited_ms,
        capacity = MAX_PENDING,
        "holon_latency: oldest pending interaction of this origin evicted at capacity — its latency is unmeasurable",
    );
}

/// The one emission site for an unmeasured interaction. An expiry is the
/// "silently degrades to look fine" case this module exists to prevent, so it
/// is a WARN naming the action, the entity and the wait — never a bare drop.
fn disclose_expired(e: &Expired) {
    tracing::warn!(
        target: "holon_latency",
        stage = "e2e_expired",
        action = %e.action,
        block = %e.target,
        origin = e.origin.as_str(),
        waited_ms = e.waited_ms,
        "holon_latency: interaction expired without a delivered row",
    );
}

/// Record a user interaction entering the op pipeline. `target` is the entity
/// the op addresses (the `id` param, or the navigated-to block). `observable`
/// is the delivery this interaction will become visible as — the caller knows
/// which gesture it dispatched, so it names the observable rather than letting
/// the correlator guess from the op name.
pub fn interaction_dispatched(
    action: &str,
    target: &str,
    observable: Observable,
    origin: ClockOrigin,
) {
    let mut registry = PENDING.lock().expect("latency_e2e mutex poisoned");
    let (expired, evicted) = enrol(
        &mut registry,
        action,
        target,
        observable,
        origin,
        Instant::now(),
    );
    PENDING_LEN.store(registry.len(), Ordering::Release);
    drop(registry);

    for e in expired {
        disclose_expired(&e);
    }
    if let Some(e) = evicted {
        disclose_evicted(&e, origin);
    }
}

/// Pure core of [`interaction_dispatched`]: enrol the clock in its origin's
/// slot, returning what the caller must disclose. Operates on a caller-owned
/// registry so the counting rules are testable without the process-global one.
fn enrol(
    registry: &mut Registry,
    action: &str,
    target: &str,
    observable: Observable,
    origin: ClockOrigin,
    now: Instant,
) -> (Vec<Expired>, Option<Evicted>) {
    let mut expired = Vec::new();
    for o in ORIGINS {
        expired.extend(prune_expired(registry.slot(o), now));
    }
    // Cross-origin contention is recorded as an EVENT here, on both sides, so
    // that it covers the whole overlap rather than two sampled instants:
    //   * this clock is contended if the other slot already has anything in it;
    //   * every entry already pending in the other slot becomes contended, because
    //     this clock now shares the pipeline with it.
    // Between them, any overlap of two lives is caught by whichever clock
    // started second — including a foreign clock that opens and closes entirely
    // INSIDE this one's life, which is the common shape (facade spans are
    // shorter than UI spans) and which no pair of instant readings can see.
    let other = registry.slot(origin.other());
    let contended = !other.is_empty();
    for p in other.iter_mut() {
        p.contended = true;
    }
    let slot = registry.slot(origin);
    // Capacity is per origin, so one origin's burst cannot evict the other's
    // pending clocks.
    let evicted = (slot.len() >= MAX_PENDING).then(|| {
        let p = slot.remove(0);
        Evicted {
            action: p.action,
            target: p.target,
            waited_ms: now.duration_since(p.t0).as_millis() as u64,
        }
    });
    // Counted over THIS origin's slot only: the queue depth that decides a UI
    // sample's service-time eligibility must not move with agent traffic.
    let in_flight = slot.len() + 1;
    slot.push(Pending {
        action: action.to_string(),
        target: target.to_string(),
        origin,
        observable,
        t0: now,
        in_flight,
        contended,
    });
    (expired, evicted)
}

/// Retire the pending entry of an interaction that produced no write, because
/// the op was **refused or failed**. Without this the entry squats until
/// [`EXPIRY`] and any unrelated later delivery for that row — a background
/// re-projection, a peer edit — closes it as a multi-second measurement of
/// nothing, which the latency-slo oracle then reports as a real SLO breach.
///
/// Retires the NEWEST entry matching BOTH action and target (the one this
/// dispatch pushed), so a concurrent interaction on the same row is untouched.
/// No `e2e` sample is emitted: the interaction produced no visible change, so
/// there is nothing to measure. Called from the `Err` arm of every op-dispatch
/// seam (`holon_frontend::operations` / `reactive`).
pub fn interaction_failed(action: &str, target: &str, origin: ClockOrigin) {
    let mut registry = PENDING.lock().expect("latency_e2e mutex poisoned");
    let retired = retire_failed(registry.slot(origin), action, target, Instant::now());
    PENDING_LEN.store(registry.len(), Ordering::Release);
    drop(registry);
    if let Some(r) = retired {
        tracing::info!(
            target: "holon_latency",
            stage = "e2e_retired",
            action = %r.action,
            block = %r.target,
            origin = r.origin.as_str(),
            waited_ms = r.waited_ms,
            reason = "op refused or failed — no write, nothing to measure",
            "holon_latency",
        );
    }
}

/// Pure core of [`interaction_failed`]: remove and return the newest entry of
/// ONE origin's slot matching `action` and `target`. Origin-scoping is
/// structural — the caller passes that origin's slot — so a failed facade op
/// cannot retire a UI interaction still in flight on the same row.
fn retire_failed(
    pending: &mut Vec<Pending>,
    action: &str,
    target: &str,
    now: Instant,
) -> Option<Expired> {
    let mut newest: Option<usize> = None;
    for (i, p) in pending.iter().enumerate() {
        if p.action != action || p.target != target {
            continue;
        }
        if newest.is_none_or(|w| p.t0 > pending[w].t0) {
            newest = Some(i);
        }
    }
    let i = newest?;
    let p = pending.remove(i);
    Some(Expired {
        action: p.action,
        target: p.target,
        origin: p.origin,
        waited_ms: now.duration_since(p.t0).as_millis() as u64,
    })
}

/// Diagnostic inspection of the correlation registry: the targets of all
/// currently-pending interactions. Lets another crate prove the dispatch
/// wiring started the clock (e.g. that `navigation.focus` enrolled its
/// `block_id`) without reaching into this module's internals.
pub fn pending_targets() -> Vec<String> {
    PENDING
        .lock()
        .expect("latency_e2e mutex poisoned")
        .iter()
        .map(|p| p.target.clone())
        .collect()
}

/// The `LiveData` mirror of the `focus_roots` matview — the one whose
/// deliveries ARE navigations. Its rows are `(region, root_id)`: no `id`, no
/// `parent_id`, so [`touched_entities`] must read them by their own column or
/// a navigation's own delivery reaches the correlator as nothing at all.
pub const FOCUS_ROOTS_SOURCE: &str = "focus_roots";

/// The entities an applied CDC batch makes visible, each tagged with the
/// [`Observable`] it is. Called by `LiveData::subscribe` with the mirror's
/// source name, which is what tells a focus-root row from a block row: the two
/// mirrors have disjoint schemas and only the source names them.
pub fn touched_entities(
    source: &str,
    changes: &[crate::Change<crate::StorageEntity>],
) -> Vec<(String, Observable)> {
    if source == FOCUS_ROOTS_SOURCE {
        return changes.iter().filter_map(focus_root_delivery).collect();
    }
    changes
        .iter()
        .flat_map(block_row_pairs)
        .map(|(id, seq)| (id, Observable::BlockRow(seq)))
        .collect()
}

/// The block a `focus_roots` change makes the region's root.
///
/// `Deleted` yields nothing BY DESIGN: the matview is
/// `navigation_history WHERE closed_at IS NULL`, so a delete means a root was
/// CLOSED — the navigation that replaced it arrives as its own `Created` row in
/// the same batch, and that row is the observable. (CDC deletes on this mirror
/// carry only a rowid anyway; it has no `id` column.)
///
/// `FieldsChanged` is a shape this mirror does not produce — its rows are
/// inserted and removed, never partially patched — and it carries the matview's
/// rowid rather than a block id, so the delivered entity could not be named
/// even if we wanted to guess. It is disclosed rather than dropped: a silent
/// `None` here is the defect this module just fixed, one mirror narrower.
fn focus_root_delivery(c: &crate::Change<crate::StorageEntity>) -> Option<(String, Observable)> {
    use crate::Change;
    let data = match c {
        Change::Created { data, .. } | Change::Updated { data, .. } => data,
        Change::Deleted { .. } => return None,
        Change::FieldsChanged {
            entity_id, fields, ..
        } => {
            tracing::warn!(
                target: "holon_latency",
                stage = "e2e_delivery_unreadable",
                source = FOCUS_ROOTS_SOURCE,
                change = "fields_changed",
                entity_id = %entity_id,
                fields = ?fields.iter().map(|(name, _, _)| name.as_str()).collect::<Vec<_>>(),
                "holon_latency: focus_roots emitted a partial field delta — navigation latency is unmeasurable for this batch",
            );
            return None;
        }
    };
    let Some(root_id) = data.get("root_id").and_then(|v| v.as_string()) else {
        // The mirror's schema is `(region, root_id)`. A row without it means
        // the matview changed under us and every navigation measurement is
        // silently gone — the exact failure this module exists to prevent.
        tracing::error!(
            target: "holon_latency",
            stage = "e2e_delivery_unreadable",
            source = FOCUS_ROOTS_SOURCE,
            "holon_latency: focus_roots row carries no `root_id` — navigation latency is unmeasurable",
        );
        return None;
    };
    Some((root_id.to_string(), Observable::FocusRoot))
}

/// The `(id, WriteSeq?)` pairs a block-mirror change makes visible: the row's
/// own `id` carries its op-instance token; the `parent_id` correlation (a
/// create/split dispatched at a parent completes via its new child's row) is
/// always tokenless.
fn block_row_pairs(c: &crate::Change<crate::StorageEntity>) -> Vec<(String, Option<WriteSeq>)> {
    use crate::Change;
    let row_pairs = |data: &crate::StorageEntity| {
        let seq = data
            .get("write_seq")
            .and_then(|v| v.as_i64())
            .filter(|&s| s > 0)
            .map(WriteSeq::from_i64);
        let mut out = Vec::new();
        // ALLOW(raw_row_id_column): key — the latency ledger keys on the id TEXT; it
        // resolves no entity
        if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
            out.push((id.to_string(), seq));
        }
        if let Some(pid) = data.get("parent_id").and_then(|v| v.as_string()) {
            out.push((pid.to_string(), None));
        }
        out
    };
    match c {
        Change::Created { data, .. } => row_pairs(data),
        Change::Updated { id, data, .. } => {
            let mut pairs = row_pairs(data);
            if !pairs.iter().any(|(pid, _)| pid == id) {
                pairs.push((id.clone(), None));
            }
            pairs
        }
        Change::Deleted { id, .. } => vec![(id.clone(), None)],
        Change::FieldsChanged { entity_id, .. } => vec![(entity_id.clone(), None)],
    }
}

/// Report the entities made visible by an applied `LiveData` batch, as
/// `(id, Observable)` pairs (see [`touched_entities`]). Emits one `stage="e2e"`
/// event per closed entry.
///
/// `received_at` is when the subscriber took the batch off its stream. A clock
/// dispatched after that cannot be what the batch carries, so it is not closed
/// here: see [`close_received`].
pub fn rows_delivered<'a>(
    source: &'static str,
    received_at: Instant,
    deliveries: impl IntoIterator<Item = (&'a str, Observable)>,
) {
    if PENDING_LEN.load(Ordering::Acquire) == 0 {
        return;
    }
    let deliveries: Vec<(String, Observable)> = deliveries
        .into_iter()
        .map(|(id, observable)| (id.to_string(), observable))
        .collect();
    if deliveries.is_empty() {
        return;
    }
    let mut registry = PENDING.lock().expect("latency_e2e mutex poisoned");
    let now = Instant::now();
    // Offered to EVERY origin's slot, each closed on its own. Two interactions
    // of different origins can wait on the same row — an agent op and a user
    // edit on one block — and that row makes BOTH visible, so each gets its own
    // sample. Scoping the close to one slot at a time is also what stops a
    // cross-origin supersession: an entry is only ever deleted as a superseded
    // no-op by a newer entry of its OWN origin.
    let mut expired = Vec::new();
    let mut closed: Vec<Closed> = Vec::new();
    for o in ORIGINS {
        let slot = registry.slot(o);
        // Reap here too, not only at dispatch: an interaction whose observable
        // never arrives must be disclosed while the app is still running, and a
        // delivery is the more frequent event. An entry older than EXPIRY could
        // otherwise close on this batch and report a >30s "measurement" the SLO
        // oracle would fire on.
        expired.extend(prune_expired(slot, now));
        closed.extend(close_received(slot, &deliveries, received_at, now));
    }
    PENDING_LEN.store(registry.len(), Ordering::Release);
    drop(registry);
    for e in expired {
        disclose_expired(&e);
    }
    let delivery_batch = DELIVERY_BATCH.fetch_add(1, Ordering::Relaxed) + 1;
    for c in closed {
        // End-to-end (interaction -> PROJECTION-VISIBLE): closes on the tokio
        // CDC actor the moment the batch is applied to the reactive mirror —
        // NOT at GPU frame-present (see the module-level "SLO endpoint"
        // disclosure). This is the stage the latency-slo oracle judges. Prod
        // counterpart of the harness-only `action_total` stage.
        // `in_flight` / `backlog` are what let a consumer tell a queued sample
        // from a quiet one. Without them `ms` conflates service time with queue
        // wait, and an agent typing faster than a human turns a healthy pipeline
        // into a wall of SLO banners (BugFunnel 2026-08-31).
        tracing::info!(
            target: "holon_latency",
            stage = "e2e",
            action = %c.action,
            block = %c.target,
            // Which seam opened the clock. A `facade` sample skips the frontend
            // dispatch cost, so a consumer must score it in its own window
            // (D119.a) — never pooled with `ui` into one percentile.
            origin = c.origin.as_str(),
            source = source,
            ms = c.ms,
            in_flight = c.in_flight,
            backlog = c.backlog,
            // Cross-origin contention over this interaction's WHOLE life.
            // `in_flight`/`backlog` are this origin's own queue (partitioned, so
            // a population cannot move with foreign traffic); this says whether
            // foreign traffic shared the pipeline anyway, which is what tells an
            // uncontended sample from a queued one (D119.a rounds 2-3).
            contended = c.contended,
            delivery_batch,
            "holon_latency",
        );
    }
}

/// [`close_delivered`] over the clocks dispatched BEFORE the batch was
/// received.
///
/// A delivered row that carries no `WriteSeq` names no op instance, so
/// `close_delivered` gives it to the newest pending clock on its target. A
/// clock dispatched after the batch was received is newer than anything the
/// batch can carry: the batch would close it and report a latency too short to
/// be true. The second `LiveData` subscriber of one source, which delivers the
/// same rows again, is the same case.
fn close_received<S: AsRef<str>>(
    pending: &mut Vec<Pending>,
    deliveries: &[(S, Observable)],
    received_at: Instant,
    now: Instant,
) -> Vec<Closed> {
    let (mut eligible, dispatched_after): (Vec<Pending>, Vec<Pending>) = std::mem::take(pending)
        .into_iter()
        .partition(|p| p.t0 <= received_at);
    let mut closed = close_delivered(&mut eligible, deliveries, now);
    eligible.extend(dispatched_after);
    for c in &mut closed {
        c.backlog = eligible.len();
    }
    *pending = eligible;
    closed
}

/// Pure correlation core: close the pending entries a batch's deliveries
/// identify, returning the measurements to emit. Operates on a caller-owned
/// `Vec` so it is hermetic under the parallel test runner.
///
/// **It sees ONE origin's slot.** [`rows_delivered`] calls it once per origin,
/// which is what confines the supersession rule below to entries of the same
/// origin: a facade clock and a UI clock on one block are two interactions, not
/// one superseding the other, and each closes with its own sample off the same
/// delivery. `backlog` is likewise this origin's remaining depth.
///
/// Matching is scoped to one **(target, observable kind)** at a time: an entry
/// closes only on a delivery of the kind it is waiting for. A pending
/// `navigate` on a page therefore ignores that page's block rows entirely — it
/// is neither closed nor superseded by them — and a write on a page ignores the
/// page's focus-root rows.
///
/// Per (target, kind) present in the batch:
/// 1. **Exact op-instance match** — if the batch delivered a `WriteSeq` equal
///    to some pending entry's token, that entry is the winner (the newest such
///    on a tie). This closes exactly the op instance that produced the delta.
/// 2. **Tokenless path** — else, if the batch delivered any tokenless row for
///    the target, the winner is the *newest* pending entry for that target.
/// 3. **Anonymous-token path** — else (only tokens nobody is waiting for) the
///    winner is the newest pending entry *if it is tokenless*. Such a token is
///    the sticky `write_seq` column left by an earlier editor write, so it says
///    nothing about the op in flight; a tokenless op must not be blocked by it.
///    A *tokenful* entry still closes on nothing here — it waits for its own
///    token, which is what stops a stale no-op re-commit from stealing an
///    untracked dispatch's delta.
///
/// The winner and every pending entry of the same origin and (target, kind)
/// **older than** it are removed (superseded no-ops). Entries newer than the
/// winner remain pending for their own delta. This guarantees a measurement is
/// never attributed across a newer completed entry on the same target.
fn close_delivered<S: AsRef<str>>(
    pending: &mut Vec<Pending>,
    deliveries: &[(S, Observable)],
    now: Instant,
) -> Vec<Closed> {
    let mut by_target: HashMap<(&str, Kind), (Vec<WriteSeq>, bool)> = HashMap::new();
    for (id, observable) in deliveries {
        let entry = by_target
            .entry((id.as_ref(), observable.kind()))
            .or_insert((Vec::new(), false));
        match observable.seq() {
            Some(s) => entry.0.push(s),
            None => entry.1 = true,
        }
    }

    let mut closed = Vec::new();
    for ((target, kind), (seqs, has_tokenless)) in by_target {
        // 1. exact op-instance match — newest entry whose token was delivered.
        let mut winner: Option<usize> = None;
        for (i, p) in pending.iter().enumerate() {
            if p.target != target || p.observable.kind() != kind {
                continue;
            }
            let matches = p.observable.seq().is_some_and(|s| seqs.contains(&s));
            if matches && winner.is_none_or(|w| p.t0 > pending[w].t0) {
                winner = Some(i);
            }
        }
        // 2/3. anonymous delivery — the batch named no pending op instance.
        if winner.is_none() {
            let mut newest: Option<usize> = None;
            for (i, p) in pending.iter().enumerate() {
                if p.target != target || p.observable.kind() != kind {
                    continue;
                }
                if newest.is_none_or(|w| p.t0 > pending[w].t0) {
                    newest = Some(i);
                }
            }
            // A delivered token nobody is waiting for is the STICKY row column,
            // not evidence about an op in flight: `write_seq` records the row's
            // last editor write and every later projection of that row repeats
            // it. So it may not veto a tokenless entry's closure (rule 3) — that
            // veto is what made split/indent/join unmeasurable on any block the
            // user had ever typed into. A tokenful entry still waits for its own
            // token unless the batch carried a genuinely tokenless row (rule 2),
            // which is what keeps a stale no-op re-commit from stealing.
            if let Some(i) = newest {
                if has_tokenless || pending[i].observable.seq().is_none() {
                    winner = Some(i);
                }
            }
        }
        let Some(winner) = winner else {
            continue;
        };
        let winner_t0 = pending[winner].t0;
        closed.push(Closed {
            action: pending[winner].action.clone(),
            target: pending[winner].target.clone(),
            origin: pending[winner].origin,
            ms: now.duration_since(winner_t0).as_millis() as u64,
            in_flight: pending[winner].in_flight,
            backlog: 0,
            contended: pending[winner].contended,
        });
        // Remove the winner and all OLDER entries of the same (target, kind)
        // (superseded). A different kind on the same target is a different
        // interaction awaiting its own delivery — never collateral.
        pending.retain(|p| p.target != target || p.observable.kind() != kind || p.t0 > winner_t0);
    }
    // Backlog is what survives the WHOLE batch, so it is stamped once the loop
    // has closed every target this delivery named.
    let backlog = pending.len();
    for c in &mut closed {
        c.backlog = backlog;
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pend(action: &str, target: &str, seq: Option<i64>, t0: Instant) -> Pending {
        pend_from(ClockOrigin::Ui, action, target, seq, t0)
    }

    fn pend_from(
        origin: ClockOrigin,
        action: &str,
        target: &str,
        seq: Option<i64>,
        t0: Instant,
    ) -> Pending {
        Pending {
            action: action.to_string(),
            target: target.to_string(),
            origin,
            observable: Observable::BlockRow(seq.map(WriteSeq::from_i64)),
            t0,
            in_flight: 1,
            contended: false,
        }
    }

    /// A pending navigation: waits for its own focus-root delivery.
    fn pend_nav(target: &str, t0: Instant) -> Pending {
        Pending {
            action: "navigate".to_string(),
            target: target.to_string(),
            origin: ClockOrigin::Ui,
            observable: Observable::FocusRoot,
            t0,
            in_flight: 1,
            contended: false,
        }
    }

    /// A delivered block row, with or without an op-instance token.
    fn row(seq: Option<i64>) -> Observable {
        Observable::BlockRow(seq.map(WriteSeq::from_i64))
    }

    /// One captured `target="holon_latency"` event. Only the fields the
    /// emission pins assert on.
    #[derive(Default)]
    struct Captured {
        stage: Option<String>,
        action: Option<String>,
        block: Option<String>,
        change: Option<String>,
        origin: Option<String>,
        waited_ms: Option<u64>,
    }

    type Sink = std::sync::Arc<std::sync::Mutex<Vec<Captured>>>;

    impl tracing::field::Visit for Captured {
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            if field.name() == "waited_ms" {
                self.waited_ms = Some(value);
            }
        }
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.put(field.name(), value.to_string());
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            let v = format!("{value:?}");
            self.put(field.name(), v.trim_matches('"').to_string());
        }
    }

    impl Captured {
        fn put(&mut self, name: &str, value: String) {
            match name {
                "stage" => self.stage = Some(value),
                "action" => self.action = Some(value),
                "block" => self.block = Some(value),
                "change" => self.change = Some(value),
                "origin" => self.origin = Some(value),
                _ => {}
            }
        }
    }

    struct CaptureLayer(Sink);
    impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for CaptureLayer {
        // Register `holon_latency` callsites as ALWAYS-interested. Sibling
        // latency tests in this binary hit the emit callsites with NO subscriber
        // installed, which caches their process-global `Interest` as `never` —
        // and a `never`-cached callsite is skipped before any thread-local
        // subscriber is consulted, so a plain `enabled` override cannot rescue
        // it. `always` (combined with `rebuild_interest_cache` below) pins the
        // callsite open; the event then always dispatches to whatever
        // thread-local default is active — our sink on this thread, a no-op
        // elsewhere.
        fn register_callsite(
            &self,
            metadata: &tracing::Metadata<'_>,
        ) -> tracing::subscriber::Interest {
            if metadata.target() == "holon_latency" {
                tracing::subscriber::Interest::always()
            } else {
                tracing::subscriber::Interest::never()
            }
        }
        fn enabled(
            &self,
            metadata: &tracing::Metadata<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) -> bool {
            metadata.target() == "holon_latency"
        }
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() != "holon_latency" {
                return;
            }
            let mut c = Captured::default();
            event.record(&mut c);
            self.0.lock().unwrap().push(c);
        }
    }

    /// Run `f` under a scoped subscriber that captures every
    /// `target="holon_latency"` event, and return what it captured. `f` gets
    /// the live sink so a retry loop can watch for its own event.
    ///
    /// This asserts at the EMISSION layer: the `tracing` event a real
    /// subscriber sees, not a return value. It needs no
    /// `SpanCollector::global()` touch — that requirement belongs to the
    /// ERROR-capture harness, which reads a process-global collector; here
    /// the layer IS the subscriber, installed for the closure's duration.
    fn capture_latency_events(f: impl FnOnce(&Sink)) -> Vec<Captured> {
        use tracing_subscriber::prelude::*;
        let sink: Sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CaptureLayer(sink.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            f(&sink);
        });
        std::sync::Arc::try_unwrap(sink)
            .unwrap_or_else(|s| std::sync::Mutex::new(s.lock().unwrap().drain(..).collect()))
            .into_inner()
            .unwrap()
    }

    /// The expiry disclosure must actually REACH a subscriber. Pinning only
    /// `prune_expired`'s return value would leave "expires loudly" asserted at
    /// the wrong layer: a change that dropped the `warn!` would keep every
    /// return-value pin green while the app went silent again — which is
    /// exactly how the uncloseable-`split_block` defect survived a dogfood
    /// night.
    #[test]
    fn an_expiry_emits_a_warn_a_subscriber_can_see() {
        let events = capture_latency_events(|_| {
            disclose_expired(&Expired {
                action: "navigate".to_string(),
                target: "block:warm-emit".to_string(),
                origin: ClockOrigin::Ui,
                waited_ms: 56693,
            });
        });
        let mine = events
            .iter()
            .find(|c| c.block.as_deref() == Some("block:warm-emit"))
            .expect("the expiry must emit a holon_latency event");
        assert_eq!(mine.stage.as_deref(), Some("e2e_expired"));
        assert_eq!(mine.action.as_deref(), Some("navigate"));
        assert_eq!(
            mine.waited_ms,
            Some(56693),
            "the disclosure names how long the interaction went unmeasured"
        );
    }

    /// A CDC shape this mirror does not produce must not be dropped in silence:
    /// a partial field delta on `focus_roots` carries the matview's rowid, not
    /// a block id, so no delivery can be named — and an unnameable delivery
    /// means navigations stop closing, the defect this module just fixed.
    /// It is disclosed instead, naming the change kind.
    #[test]
    fn a_focus_roots_fields_changed_is_disclosed_not_silently_dropped() {
        let changes = vec![crate::Change::FieldsChanged {
            entity_id: "rowid:7".to_string(),
            fields: vec![(
                "root_id".to_string(),
                crate::Value::String("block:old".to_string()),
                crate::Value::String("block:new".to_string()),
            )],
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        }];
        let mut touched = Vec::new();
        let events = capture_latency_events(|_| {
            touched = touched_entities(FOCUS_ROOTS_SOURCE, &changes);
        });
        assert!(
            touched.is_empty(),
            "a rowid cannot be correlated to a navigation target"
        );
        let mine = events
            .iter()
            .find(|c| c.stage.as_deref() == Some("e2e_delivery_unreadable"))
            .expect("the unexpected change kind must be disclosed, not dropped");
        assert_eq!(
            mine.change.as_deref(),
            Some("fields_changed"),
            "the disclosure names WHICH shape arrived"
        );
    }

    /// The phantom-measurement steal (BugFunnel 2026-07-13): a no-op dispatch
    /// leaves a stale pending entry; the next real commit on the same block
    /// must be measured as ITS OWN fast latency, not the stale entry's huge
    /// elapsed. Old FIFO-by-target code returned the oldest (stale) entry.
    #[test]
    fn real_commit_not_stolen_by_stale_noop_entry() {
        let base = Instant::now();
        // A: no-op blur re-commit (its write_seq never lands in any delta).
        // B: the real edit 16s later, its own newer token.
        let mut pending = vec![
            pend("set_field", "block:x", Some(100), base),
            pend(
                "set_field",
                "block:x",
                Some(101),
                base + Duration::from_secs(16),
            ),
        ];
        // The real edit's delta carries token 101.
        let now = base + Duration::from_secs(16) + Duration::from_millis(10);
        let closed = close_delivered(&mut pending, &[("block:x", row(Some(101)))], now);
        assert_eq!(closed.len(), 1, "exactly one measurement");
        assert_eq!(closed[0].action, "set_field");
        assert_eq!(
            closed[0].ms, 10,
            "must report the REAL 10ms commit, not the stale 16s no-op"
        );
        assert!(
            pending.is_empty(),
            "the matched entry and the superseded stale no-op are both cleared"
        );
    }

    /// A delivered token that matches NO pending entry closes nothing (the
    /// delta belongs to an untracked dispatch) — never mis-attribute it to a
    /// stale same-target entry.
    #[test]
    fn nonmatching_token_closes_nothing() {
        let base = Instant::now();
        let mut pending = vec![pend("set_field", "block:x", Some(100), base)];
        let closed = close_delivered(
            &mut pending,
            &[("block:x", row(Some(999)))],
            base + Duration::from_millis(5),
        );
        assert!(closed.is_empty(), "no exact match ⇒ no measurement");
        assert_eq!(pending.len(), 1, "the stale entry is left untouched");
    }

    /// A structural op (`split_block`/`indent`/`join`) on a block that was EVER
    /// typed into must still be measurable. `write_seq` is a **sticky column on
    /// the block row**: once an editor write stamps it, every later projection
    /// of that row carries the same value — including the row the split writes.
    /// The delivered token therefore names an op instance that closed long ago,
    /// and it must NOT veto the tokenless split's closure.
    #[test]
    fn structural_op_closes_over_a_stale_row_token() {
        let base = Instant::now();
        let mut pending = vec![pend("split_block", "block:typed", None, base)];
        let now = base + Duration::from_millis(36);
        // The split's own delta on its target: the row still carries the token
        // of the last EDITOR write (42), whose pending entry already closed.
        let closed = close_delivered(&mut pending, &[("block:typed", row(Some(42)))], now);
        assert_eq!(closed.len(), 1, "the split must yield its OWN measurement");
        assert_eq!(closed[0].action, "split_block");
        assert_eq!(closed[0].ms, 36);
        assert!(pending.is_empty(), "the split entry is consumed");
    }

    /// The dogfood gesture end to end (click → type → Enter), at the pure core:
    /// a content write closes on its own token, then the split of THAT block
    /// must close too. Before the anonymous-delivery rule the split's entry
    /// survived, expired, and was pruned — so `split_block` produced `dispatch`
    /// samples and ZERO `e2e` samples on any block the user had typed into.
    #[test]
    fn typed_then_split_the_same_block_both_measure() {
        let base = Instant::now();
        let mut pending = Vec::new();
        // 1. the keystroke's content write, stamped write_seq 7.
        pending.push(pend("set_field", "block:t", Some(7), base));
        let typed = close_delivered(
            &mut pending,
            &[("block:t", row(Some(7)))],
            base + Duration::from_millis(20),
        );
        assert_eq!(typed.len(), 1, "the content write closes on its own token");
        assert!(pending.is_empty());
        // 2. Enter → split_block, tokenless. Its delta re-projects the row, which still
        //    carries the sticky write_seq 7.
        let t1 = base + Duration::from_millis(100);
        pending.push(pend("split_block", "block:t", None, t1));
        let split = close_delivered(
            &mut pending,
            &[("block:t", row(Some(7))), ("block:new-child", row(None))],
            t1 + Duration::from_millis(36),
        );
        assert_eq!(split.len(), 1, "the split closes too — not only the type");
        assert_eq!(split[0].action, "split_block");
        assert_eq!(
            split[0].ms, 36,
            "measures the split, not time-to-something-else"
        );
    }

    /// Rule 3's own attribution order, pinned. Two tokenless entries on one
    /// target + an anonymous delivery: the NEWEST closes and the older is
    /// dropped as superseded, exactly as the genuinely-tokenless route already
    /// does (`tokenless_closes_newest_and_drops_older`). Without this a future
    /// change could silently flip rule 3 to FIFO and charge the split's delta
    /// to whatever was dispatched before it.
    #[test]
    fn anonymous_delivery_closes_newest_and_drops_older() {
        let base = Instant::now();
        let mut pending = vec![
            pend("split_block", "block:x", None, base),
            pend("indent", "block:x", None, base + Duration::from_millis(50)),
        ];
        let closed = close_delivered(
            &mut pending,
            &[("block:x", row(Some(42)))],
            base + Duration::from_millis(60),
        );
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].action, "indent", "the NEWEST entry is the winner");
        assert_eq!(closed[0].ms, 10, "its own 10ms, not the older entry's 60ms");
        assert!(
            pending.is_empty(),
            "the older entry is dropped as superseded"
        );
    }

    /// A REFUSED op writes nothing, so its entry must be retired at the source
    /// — otherwise rule 3 lets any unrelated later delivery for that row (a
    /// background re-projection, a peer edit) close it as a multi-second sample
    /// of nothing, which the latency-slo oracle reports as a real SLO breach.
    /// Retiring is what the `Err` arm of every dispatch seam now does.
    #[test]
    fn refused_op_is_retired_so_a_later_delivery_cannot_phantom_close_it() {
        let base = Instant::now();
        let mut pending = vec![pend("outdent", "block:refused", None, base)];
        // Without the retirement this delivery closes a 25s phantom.
        let retired = retire_failed(
            &mut pending,
            "outdent",
            "block:refused",
            base + Duration::from_millis(3),
        )
        .expect("the refused op's entry must be retired");
        assert_eq!(retired.action, "outdent");
        assert_eq!(retired.target, "block:refused");
        assert!(pending.is_empty());
        let closed = close_delivered(
            &mut pending,
            &[("block:refused", row(Some(5)))],
            base + Duration::from_secs(25),
        );
        assert!(
            closed.is_empty(),
            "no phantom 25s sample survives the retirement"
        );
    }

    /// Retirement targets the op instance that failed, never a concurrent
    /// interaction on the same row: it matches action AND target, and takes the
    /// newest such entry.
    #[test]
    fn retirement_leaves_a_concurrent_interaction_on_the_same_row_pending() {
        let base = Instant::now();
        let mut pending = vec![
            pend("split_block", "block:r", None, base),
            pend("outdent", "block:r", None, base + Duration::from_millis(5)),
        ];
        let retired = retire_failed(
            &mut pending,
            "outdent",
            "block:r",
            base + Duration::from_millis(6),
        );
        assert_eq!(retired.expect("outdent retired").action, "outdent");
        assert_eq!(
            pending.len(),
            1,
            "the concurrent split still awaits its delta"
        );
        assert_eq!(pending[0].action, "split_block");
        // Nothing to retire is not an error — the entry may already have closed.
        assert!(
            retire_failed(&mut pending, "outdent", "block:r", base).is_none(),
            "a second retirement of the same op instance is a no-op"
        );
    }

    /// An expired entry is disclosed for EVERY action, not only `navigate`.
    /// A silently-pruned write entry is the "silently degrades to look fine"
    /// case: it is exactly how the uncloseable-`split_block` defect stayed
    /// invisible through a whole dogfood night.
    #[test]
    fn expired_write_entry_is_disclosed() {
        let base = Instant::now();
        let mut pending = vec![
            pend("split_block", "block:stuck", None, base),
            pend("navigate", "block:page", None, base),
            pend("set_field", "block:fresh", Some(3), base + EXPIRY),
        ];
        let expired = prune_expired(&mut pending, base + EXPIRY + Duration::from_millis(5));
        let actions: Vec<&str> = expired.iter().map(|e| e.action.as_str()).collect();
        assert!(
            actions.contains(&"split_block"),
            "a write entry that never saw its row must be disclosed, not dropped in silence: {actions:?}"
        );
        assert!(actions.contains(&"navigate"));
        assert_eq!(
            expired
                .iter()
                .find(|e| e.action == "split_block")
                .unwrap()
                .target,
            "block:stuck",
            "the disclosure names the entity"
        );
        assert_eq!(pending.len(), 1, "the unexpired entry survives");
    }

    /// Tokenless ops (toggle/split/delete) correlate by newest-per-target and
    /// drop older same-target entries — a tokenless no-op cannot steal either.
    #[test]
    fn tokenless_closes_newest_and_drops_older() {
        let base = Instant::now();
        let mut pending = vec![
            pend("toggle_state", "block:y", None, base),
            pend(
                "toggle_state",
                "block:y",
                None,
                base + Duration::from_secs(5),
            ),
        ];
        let now = base + Duration::from_secs(5) + Duration::from_millis(7);
        let closed = close_delivered(&mut pending, &[("block:y", row(None))], now);
        assert_eq!(closed.len(), 1);
        assert_eq!(
            closed[0].ms, 7,
            "newest entry's elapsed, not the older one's"
        );
        assert!(
            pending.is_empty(),
            "older same-target entry dropped as superseded"
        );
    }

    /// A keystroke typed while the previous keystroke's batch is still being
    /// applied is not what that batch carries. The delivered row has no token,
    /// so without the receipt bound the batch closes the NEWER clock after 25ms
    /// and drops the older one as superseded: a 283ms write reported as 25ms.
    #[test]
    fn a_batch_never_closes_a_clock_dispatched_after_it_was_received() {
        let base = Instant::now();
        let received = base + Duration::from_millis(28);
        let mut pending = vec![
            pend("set_field", "block:host", Some(3), base),
            pend(
                "set_field",
                "block:host",
                Some(4),
                base + Duration::from_millis(258),
            ),
        ];
        let now = base + Duration::from_millis(283);
        let closed = close_received(&mut pending, &[("block:host", row(None))], received, now);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].ms, 283, "the batch closes the write it carries");
        assert_eq!(closed[0].backlog, 1, "the newer write is still in flight");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].observable, row(Some(4)));
    }

    /// The second subscriber of one source delivers the same rows again. A
    /// clock dispatched between the two deliveries stays pending.
    #[test]
    fn a_repeated_delivery_of_one_batch_closes_no_later_clock() {
        let base = Instant::now();
        let received = base + Duration::from_millis(20);
        let mut pending = vec![pend("set_field", "block:host", Some(11), base)];
        let first = close_received(
            &mut pending,
            &[("block:host", row(None))],
            received,
            base + Duration::from_millis(272),
        );
        assert_eq!(first.len(), 1);
        pending.push(pend(
            "set_field",
            "block:host",
            Some(12),
            base + Duration::from_millis(273),
        ));
        let repeat = close_received(
            &mut pending,
            &[("block:host", row(None))],
            received,
            base + Duration::from_millis(274),
        );
        assert_eq!(repeat.len(), 0, "the repeat closed a clock it cannot carry");
        assert_eq!(pending.len(), 1);
    }

    /// Exact match leaves a NEWER same-target entry pending for its own delta.
    #[test]
    fn exact_match_preserves_newer_entry() {
        let base = Instant::now();
        let mut pending = vec![
            pend("set_field", "block:z", Some(50), base),
            pend(
                "set_field",
                "block:z",
                Some(51),
                base + Duration::from_secs(2),
            ),
        ];
        let closed = close_delivered(
            &mut pending,
            &[("block:z", row(Some(50)))],
            base + Duration::from_millis(3),
        );
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].ms, 3);
        assert_eq!(
            pending.len(),
            1,
            "the newer (seq 51) entry still awaits its delta"
        );
        assert_eq!(
            pending[0].observable,
            Observable::BlockRow(Some(WriteSeq::from_i64(51)))
        );
    }

    /// End-to-end through the process-global registry: parallel-safe because it
    /// asserts only on its own target.
    #[test]
    fn global_dispatch_and_delivery_clears_entry() {
        interaction_dispatched(
            "toggle_state",
            "block:e2e-test-a",
            row(None),
            ClockOrigin::Ui,
        );
        rows_delivered(
            "block",
            Instant::now(),
            [("block:other", row(None)), ("block:e2e-test-a", row(None))],
        );
        let pending = PENDING.lock().unwrap();
        assert!(
            pending.iter().all(|p| p.target != "block:e2e-test-a"),
            "matched entry must be consumed"
        );
    }

    #[test]
    fn global_unmatched_ids_leave_entry_pending() {
        interaction_dispatched("set_field", "block:e2e-test-b", row(None), ClockOrigin::Ui);
        rows_delivered("block", Instant::now(), [("block:unrelated", row(None))]);
        let pending = PENDING.lock().unwrap();
        assert!(pending.iter().any(|p| p.target == "block:e2e-test-b"));
    }

    /// Navigation is a first-class e2e interaction, and it closes on ITS OWN
    /// delivery: the `focus_roots` row naming the navigated-to block. That row
    /// is what makes the new page the region's content — the navigation's
    /// observable.
    #[test]
    fn navigation_closes_on_its_own_focus_root_delivery() {
        let base = Instant::now();
        let mut pending = vec![pend_nav("block:page", base)];
        let now = base + Duration::from_millis(12);
        let closed = close_delivered(&mut pending, &[("block:page", Observable::FocusRoot)], now);
        assert_eq!(closed.len(), 1, "navigation produces one e2e measurement");
        assert_eq!(closed[0].action, "navigate");
        assert_eq!(closed[0].ms, 12);
        assert!(pending.is_empty(), "the navigate entry is consumed");
    }

    /// The navigate clock-bleed (BugFunnel 2026-08-08): a block-row batch is
    /// NOT the navigation's observable, so it may not close a pending
    /// `navigate`. Before the observable split, any later delivery for the
    /// page id — a child edit carries `parent_id = page_id` — closed the
    /// navigate entry and billed every millisecond of idle human time since
    /// the click to it, firing a full-width false `[latency-slo]` banner
    /// (11982ms measured for a navigation that was visible in ~100ms).
    #[test]
    fn a_block_row_delivery_never_closes_a_pending_navigate() {
        let base = Instant::now();
        let mut pending = vec![pend_nav("block:page", base)];
        // 12s later the user types into a child of that page; the child's row
        // delivers `parent_id = block:page` as a tokenless block-row entry.
        let closed = close_delivered(
            &mut pending,
            &[("block:page", row(None)), ("block:child", row(Some(9)))],
            base + Duration::from_secs(12),
        );
        assert!(
            closed.iter().all(|c| c.action != "navigate"),
            "a block row is not evidence the navigation rendered: {:?}",
            closed
                .iter()
                .map(|c| (c.action.as_str(), c.ms))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            pending.len(),
            1,
            "the navigate still awaits its own focus-root delivery"
        );
    }

    /// The two observables share a target id without interfering: navigating to
    /// a page and editing that same page's row are concurrent interactions, and
    /// each closes on its own delivery with its own elapsed.
    #[test]
    fn navigate_and_a_write_on_the_same_id_close_independently() {
        let base = Instant::now();
        let mut pending = vec![
            pend_nav("block:p", base),
            pend(
                "set_field",
                "block:p",
                Some(4),
                base + Duration::from_millis(30),
            ),
        ];
        let write = close_delivered(
            &mut pending,
            &[("block:p", row(Some(4)))],
            base + Duration::from_millis(40),
        );
        assert_eq!(write.len(), 1);
        assert_eq!(write[0].action, "set_field");
        assert_eq!(write[0].ms, 10, "the write measures its own 10ms");
        assert_eq!(
            pending.len(),
            1,
            "the block row must not consume the navigate as a superseded entry"
        );
        let nav = close_delivered(
            &mut pending,
            &[("block:p", Observable::FocusRoot)],
            base + Duration::from_millis(45),
        );
        assert_eq!(nav.len(), 1);
        assert_eq!(nav[0].action, "navigate");
        assert_eq!(nav[0].ms, 45, "the navigate measures its own elapsed");
    }

    /// A `focus_roots` CDC row is a focus-root delivery naming its `root_id` —
    /// the mirror's rows are `(region, root_id)` with NO `id` and no
    /// `parent_id`, so reading them by the block-row columns yields nothing at
    /// all and the navigation's own delivery never reaches the correlator.
    /// That silence is the other half of the clock-bleed.
    #[test]
    fn a_focus_roots_batch_delivers_its_root_id() {
        let mut row_data = crate::StorageEntity::new();
        row_data.insert("region".into(), crate::Value::String("main".to_string()));
        row_data.insert(
            "root_id".into(),
            crate::Value::String("block:page".to_string()),
        );
        let changes = vec![crate::Change::Created {
            data: row_data,
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        }];
        let touched = touched_entities(FOCUS_ROOTS_SOURCE, &changes);
        assert_eq!(
            touched,
            vec![("block:page".to_string(), Observable::FocusRoot)],
            "the navigation's own delivery must reach the correlator"
        );
    }

    /// A **warm switch** — a navigation whose focus-root row does not change,
    /// so no CDC batch is produced — has no observable at all, and expires
    /// LOUDLY rather than borrowing someone else's delivery. That is the whole
    /// ruling: the correlator cannot measure what the pipeline never reports,
    /// and an unmeasured navigation must look unmeasured. Closing it on a
    /// foreign batch is what produced the false SLO banners.
    #[test]
    fn a_navigation_that_delivers_nothing_expires_loudly() {
        let base = Instant::now();
        let mut pending = vec![pend_nav("block:warm", base)];
        let expired = prune_expired(&mut pending, base + EXPIRY + Duration::from_millis(1));
        assert_eq!(expired.len(), 1, "the warm switch must be disclosed");
        assert_eq!(expired[0].action, "navigate");
        assert_eq!(expired[0].target, "block:warm");
        assert!(pending.is_empty());
    }

    /// A block mirror never emits focus-root deliveries, whatever its columns:
    /// `blocks_with_paths` carries a `root_id` (the row's root ancestor) that
    /// has nothing to do with navigation. The source decides, not the column.
    #[test]
    fn a_block_batch_with_a_root_id_column_is_still_block_rows() {
        let mut row_data = crate::StorageEntity::new();
        row_data.insert("id".into(), crate::Value::String("block:b".to_string()));
        row_data.insert(
            "root_id".into(),
            crate::Value::String("block:ancestor".to_string()),
        );
        let changes = vec![crate::Change::Created {
            data: row_data,
            origin: crate::ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        }];
        let touched = touched_entities("block", &changes);
        assert_eq!(
            touched,
            vec![("block:b".to_string(), Observable::BlockRow(None))],
            "a block mirror's root_id column is not a focus root"
        );
    }

    /// End-to-end through the process-global registry: a dispatched navigation
    /// closes when its focus-root row is delivered, so a `stage="e2e"`
    /// `action="navigate"` event is emitted (the emit is unconditional once an
    /// entry closes).
    #[test]
    fn global_navigation_dispatch_and_delivery_clears_entry() {
        interaction_dispatched(
            "navigate",
            "block:e2e-nav-test",
            Observable::FocusRoot,
            ClockOrigin::Ui,
        );
        rows_delivered(
            FOCUS_ROOTS_SOURCE,
            Instant::now(),
            [("block:e2e-nav-test", Observable::FocusRoot)],
        );
        let pending = PENDING.lock().unwrap();
        assert!(
            pending.iter().all(|p| p.target != "block:e2e-nav-test"),
            "navigation entry must be consumed when its focus root lands"
        );
    }

    /// Per-interaction emission lock: N sequential dispatch→delivery cycles on
    /// the SAME target must each close and each produce ITS OWN measurement.
    /// This locks the "e2e must fire per interaction, repeats included"
    /// contract (dogfood-round3 B3) at the pure correlation core — a
    /// regression that made the correlator emit only once (e.g. a once-cell
    /// guard or a never-cleared entry) would drop the later closures and
    /// turn this red.
    #[test]
    fn repeat_interactions_each_emit_their_own_measurement() {
        let base = Instant::now();
        let mut measured = Vec::new();
        // Simulate three back-to-back navigations to the same page, each
        // followed by a delivery of that page's rows.
        for i in 0..3u64 {
            let mut pending = vec![pend_nav(
                "block:repeat-page",
                base + Duration::from_millis(i * 100),
            )];
            let now = base + Duration::from_millis(i * 100 + 5);
            let closed = close_delivered(
                &mut pending,
                &[("block:repeat-page", Observable::FocusRoot)],
                now,
            );
            assert_eq!(
                closed.len(),
                1,
                "cycle {i}: delivery must close the pending navigate"
            );
            assert_eq!(closed[0].ms, 5, "cycle {i}: each measures its own 5ms");
            assert!(pending.is_empty(), "cycle {i}: entry consumed, none stuck");
            measured.push(closed[0].ms);
        }
        assert_eq!(
            measured.len(),
            3,
            "three interactions ⇒ three measurements — never collapsed to one"
        );
    }

    /// Closure-point lock: the emitted event carries `stage="e2e"` and closes
    /// from the DELIVERY (projection-visible) path — not a frame-present/paint
    /// stage. Captures the real `tracing` event a subscriber sees, so a change
    /// that re-anchored the SLO endpoint onto GPU paint (a different stage name
    /// or a different emit site) would turn this red. Parallel-safe: keys on a
    /// unique target and asserts only on its own captured event.
    #[test]
    fn emitted_e2e_event_closes_at_projection_visible_stage() {
        let sink = capture_latency_events(|captured| {
            for _ in 0..100 {
                // Re-register every callsite against THIS scoped dispatcher each
                // iteration. A sibling latency test may register (or have
                // registered) `rows_delivered`'s callsite as `never` while
                // running under no subscriber — and that only becomes visible
                // AFTER the callsite is first hit, which can happen after a
                // one-time rebuild. Rebuilding in the loop re-flips it to our
                // `always`; siblings never call rebuild, so once flipped it
                // stays open and the next emit lands. Also covers the
                // `PENDING_LEN` gate race (a lost dispatch→deliver retries).
                tracing::callsite::rebuild_interest_cache();
                interaction_dispatched(
                    "navigate",
                    "block:proj-visible-lock",
                    Observable::FocusRoot,
                    ClockOrigin::Ui,
                );
                // A CDC batch applied to the reactive mirror (projection-visible).
                rows_delivered(
                    FOCUS_ROOTS_SOURCE,
                    Instant::now(),
                    [("block:proj-visible-lock", Observable::FocusRoot)],
                );
                if captured
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|c: &Captured| c.block.as_deref() == Some("block:proj-visible-lock"))
                {
                    break;
                }
            }
        });

        let mine = sink
            .iter()
            .find(|c| c.block.as_deref() == Some("block:proj-visible-lock"))
            .expect("the delivery must emit a holon_latency event for our target");
        assert_eq!(
            mine.stage.as_deref(),
            Some("e2e"),
            "the SLO endpoint stage is projection-visible 'e2e', not a paint stage"
        );
        assert_eq!(mine.action.as_deref(), Some("navigate"));
        assert_eq!(
            mine.origin.as_deref(),
            Some("ui"),
            "the emitted sample must name the seam that opened its clock, or a \
             consumer cannot keep the two origins in separate windows"
        );
    }

    /// The facade clock reaches the emitted event as `origin="facade"`. Without
    /// the field surviving open→close, every scorer files facade samples into
    /// the UI window and the pooled percentile D119.a forbids is back.
    #[test]
    fn a_facade_clock_emits_a_facade_origin_sample() {
        let sink = capture_latency_events(|captured| {
            for _ in 0..100 {
                tracing::callsite::rebuild_interest_cache();
                interaction_dispatched(
                    "set_field",
                    "block:facade-origin-lock",
                    Observable::BlockRow(None),
                    ClockOrigin::Facade,
                );
                rows_delivered(
                    "blocks",
                    Instant::now(),
                    [("block:facade-origin-lock", Observable::BlockRow(None))],
                );
                if captured
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|c: &Captured| c.block.as_deref() == Some("block:facade-origin-lock"))
                {
                    break;
                }
            }
        });

        let mine = sink
            .iter()
            .find(|c| c.block.as_deref() == Some("block:facade-origin-lock"))
            .expect("the delivery must emit a holon_latency event for our target");
        assert_eq!(mine.stage.as_deref(), Some("e2e"));
        assert_eq!(mine.origin.as_deref(), Some("facade"));
    }

    // ── D119.a: the registry partition ───────────────────────────────────────
    // These three pin the leaks a shared registry produced. Each states the
    // production consequence, because a reader has to be able to tell an
    // arbitrary bookkeeping choice from a load-bearing one.

    /// **D2.** A UI clock and a facade clock on the SAME block are two
    /// interactions, not one superseding the other. `close_delivered` sees one
    /// origin's slot at a time, so the delivery that makes the row visible
    /// closes BOTH, each with its own sample.
    ///
    /// On a shared registry the older of the two was deleted as a "superseded
    /// no-op" with no sample, no expiry and no disclosure — a real user
    /// interaction or a real agent op vanishing without trace.
    #[test]
    fn a_ui_clock_and_a_facade_clock_on_one_target_both_close() {
        let base = Instant::now();
        let mut ui = vec![pend("set_field", "block:shared", None, base)];
        let mut facade = vec![pend_from(
            ClockOrigin::Facade,
            "set_field",
            "block:shared",
            None,
            base + Duration::from_millis(1),
        )];
        let delivery = [("block:shared", row(None))];
        let now = base + Duration::from_millis(10);

        let closed_ui = close_delivered(&mut ui, &delivery, now);
        let closed_facade = close_delivered(&mut facade, &delivery, now);

        assert_eq!(closed_ui.len(), 1, "the UI interaction must be measured");
        assert_eq!(closed_ui[0].origin, ClockOrigin::Ui);
        assert_eq!(
            closed_facade.len(),
            1,
            "the facade interaction must be measured too — the same row made it visible"
        );
        assert_eq!(closed_facade[0].origin, ClockOrigin::Facade);
    }

    /// **D1.** Queue depth is counted within one origin. `in_flight` and
    /// `backlog` are what `crate::latency_slo::E2eSample::is_service_time`
    /// gates the UI service-time rung on, so counting a facade clock there
    /// would drop a concurrent UI sample out of the percentile the 200ms SLO
    /// scores — the UI population moving with agent traffic, which is the harm
    /// D119.a names.
    ///
    /// Hermetic on a caller-owned registry: the process-global one is shared
    /// with every sibling test in this binary, and an absolute depth assertion
    /// against it would flake whenever one of them had a UI clock in flight.
    #[test]
    fn a_facade_clock_in_flight_leaves_a_ui_sample_at_depth_one() {
        let base = Instant::now();
        let mut registry = Registry::default();
        for i in 0..5 {
            enrol(
                &mut registry,
                "set_field",
                &format!("block:agent-{i}"),
                Observable::BlockRow(None),
                ClockOrigin::Facade,
                base,
            );
        }
        enrol(
            &mut registry,
            "set_field",
            "block:human",
            Observable::BlockRow(None),
            ClockOrigin::Ui,
            base + Duration::from_millis(1),
        );

        let ui = &registry.ui[0];
        assert_eq!(
            ui.in_flight, 1,
            "five facade clocks in flight must not count against the UI queue depth"
        );
        // But the overlap IS recorded. Partitioning the COUNTS keeps the
        // population stable; forgetting the contention entirely would let a
        // queued sample be scored as uncontended service time, which is the
        // opposite fake (D119.a round 2).
        assert!(
            ui.contended,
            "the shared pipeline's foreign traffic must be recorded, not discarded"
        );

        let closed = close_delivered(
            &mut registry.ui,
            &[("block:human", row(None))],
            base + Duration::from_millis(10),
        );
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].in_flight, 1);
        assert_eq!(
            closed[0].backlog, 0,
            "the facade queue is not this sample's backlog"
        );
        assert!(closed[0].contended);
    }

    /// **D3.** Overflow evicts within one origin, so a burst of facade clocks
    /// cannot drop pending UI ones.
    #[test]
    fn overflow_evicts_within_one_origin() {
        let base = Instant::now();
        let mut registry = Registry::default();
        enrol(
            &mut registry,
            "set_field",
            "block:human",
            Observable::BlockRow(None),
            ClockOrigin::Ui,
            base,
        );
        let mut evictions = 0;
        for i in 0..=MAX_PENDING {
            let (_, evicted) = enrol(
                &mut registry,
                "set_field",
                &format!("block:agent-{i}"),
                Observable::BlockRow(None),
                ClockOrigin::Facade,
                base,
            );
            evictions += usize::from(evicted.is_some());
        }

        assert_eq!(
            registry.ui.len(),
            1,
            "a facade burst past capacity must not evict a pending UI clock"
        );
        assert_eq!(registry.ui[0].target, "block:human");
        assert_eq!(
            evictions, 1,
            "the one entry over capacity is evicted, and the caller is told"
        );
    }

    /// **D3, the disclosure half.** An eviction is a LOST measurement, so it
    /// must reach a subscriber. A silent `remove(0)` makes the interaction
    /// absent from every window with nothing saying why — the "degrades to look
    /// fine" case this module exists to prevent.
    #[test]
    fn an_eviction_emits_a_warn_a_subscriber_can_see() {
        let evicted = Evicted {
            action: "set_field".to_string(),
            target: "block:evicted-disclosure".to_string(),
            waited_ms: 12,
        };
        let sink = capture_latency_events(|_| {
            tracing::callsite::rebuild_interest_cache();
            disclose_evicted(&evicted, ClockOrigin::Facade);
        });

        let mine = sink
            .iter()
            .find(|c| c.block.as_deref() == Some("block:evicted-disclosure"))
            .expect("the eviction must be disclosed");
        assert_eq!(mine.stage.as_deref(), Some("e2e_evicted"));
        assert_eq!(mine.origin.as_deref(), Some("facade"));
    }

    /// **D6 — contention is an EVENT over the interval, not two readings.**
    ///
    /// The verifier's interleaving, verbatim: a UI clock opens into an empty
    /// pipeline, a facade clock opens 2ms later, the facade clock CLOSES at
    /// 8ms, and the UI clock closes at 20ms. The facade life is contained
    /// entirely inside the UI life.
    ///
    /// Sampling the other slot's depth at dispatch and again after the delivery
    /// batch saw zero both times, so the UI sample scored as uncontended
    /// service time — reporting queue wait as service time on the very shape
    /// that is most common, since facade spans are systematically shorter than
    /// UI spans (23ms vs 29ms p50 on this lane's own rung).
    #[test]
    fn a_facade_clock_contained_inside_a_ui_clocks_life_still_contends() {
        let t0 = Instant::now();
        let mut registry = Registry::default();

        // t0: the UI interaction is dispatched into a completely empty pipeline.
        enrol(
            &mut registry,
            "set_field",
            "block:human",
            Observable::BlockRow(None),
            ClockOrigin::Ui,
            t0,
        );
        assert!(
            !registry.ui[0].contended,
            "nothing has shared the pipeline yet"
        );

        // t0+2ms: a facade op starts. THIS is the moment the UI interaction
        // stops being alone, and nothing about the UI entry is re-read later.
        enrol(
            &mut registry,
            "set_field",
            "block:agent",
            Observable::BlockRow(None),
            ClockOrigin::Facade,
            t0 + Duration::from_millis(2),
        );

        // t0+8ms: the facade op is delivered and leaves the registry.
        let facade_closed = close_delivered(
            &mut registry.facade,
            &[("block:agent", row(None))],
            t0 + Duration::from_millis(8),
        );
        assert_eq!(facade_closed.len(), 1);
        assert!(
            facade_closed[0].contended,
            "the facade op shared the pipeline too — contention is symmetric"
        );
        assert!(registry.facade.is_empty(), "the facade slot is empty again");

        // t0+20ms: the UI interaction is delivered. Its own queue was empty at
        // both ends and the other slot is empty NOW, so every instant reading
        // available at this point says "alone".
        let closed = close_delivered(
            &mut registry.ui,
            &[("block:human", row(None))],
            t0 + Duration::from_millis(20),
        );
        assert_eq!(closed.len(), 1);
        assert_eq!((closed[0].in_flight, closed[0].backlog), (1, 0));
        assert!(
            closed[0].contended,
            "a facade clock that opened and closed INSIDE this interaction's life still shared \
             the pipeline with it — the overlap must be recorded when it happens, because no \
             reading taken at dispatch or at delivery can see it afterwards"
        );
    }

    /// The symmetric case, so the marking is not one-directional: a clock
    /// enrolled into a slot whose counterpart is already busy is contended from
    /// the start.
    #[test]
    fn a_clock_enrolled_into_a_busy_pipeline_is_contended_from_the_start() {
        let t0 = Instant::now();
        let mut registry = Registry::default();
        enrol(
            &mut registry,
            "set_field",
            "block:agent",
            Observable::BlockRow(None),
            ClockOrigin::Facade,
            t0,
        );
        enrol(
            &mut registry,
            "set_field",
            "block:human",
            Observable::BlockRow(None),
            ClockOrigin::Ui,
            t0 + Duration::from_millis(1),
        );
        assert!(registry.ui[0].contended);
        assert!(registry.facade[0].contended);
    }

    /// And a genuinely solitary interaction is NOT marked — otherwise the rule
    /// would exclude everything and the rung would never judge anything.
    #[test]
    fn a_solitary_interaction_is_not_contended() {
        let t0 = Instant::now();
        let mut registry = Registry::default();
        enrol(
            &mut registry,
            "set_field",
            "block:human",
            Observable::BlockRow(None),
            ClockOrigin::Ui,
            t0,
        );
        let closed = close_delivered(
            &mut registry.ui,
            &[("block:human", row(None))],
            t0 + Duration::from_millis(10),
        );
        assert_eq!(closed.len(), 1);
        assert!(!closed[0].contended);
    }

    /// **D7.** An expiry names its origin, like an eviction does. Without it a
    /// reader of a `stage="e2e_expired"` warning cannot tell which seam lost a
    /// measurement, and the two seams fail for different reasons.
    #[test]
    fn an_expiry_names_its_origin() {
        let expired = Expired {
            action: "set_field".to_string(),
            target: "block:expiry-origin".to_string(),
            origin: ClockOrigin::Facade,
            waited_ms: 30_001,
        };
        let sink = capture_latency_events(|_| {
            tracing::callsite::rebuild_interest_cache();
            disclose_expired(&expired);
        });
        let mine = sink
            .iter()
            .find(|c| c.block.as_deref() == Some("block:expiry-origin"))
            .expect("the expiry must be disclosed");
        assert_eq!(mine.stage.as_deref(), Some("e2e_expired"));
        assert_eq!(mine.origin.as_deref(), Some("facade"));
    }
}
