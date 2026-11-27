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
//!   origin, source, ms, in_flight, backlog, contended, superseded_ms,
//!   quiet_batches_us, delivery_batch, feed)`
//!
//!   `delivery_batch` names the `rows_delivered` call that closed the entry,
//!   so a consumer can tell which samples one applied batch retired together.
//!   `feed` names the [`Feed`] that applied it, and `quiet_batches_us` the
//!   batches that feed applied since without closing a clock of this origin,
//!   so a consumer sees every pass of every feed.
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
//! # Cell edits: correlated by commit, carried by the trace
//!
//! A cell keystroke commits straight to the Loro text and dispatches no op, so
//! no row ever carries its token. Its clock ([`Observable::CellCommit`]) names
//! the commit's [`CellOp`]; the projection pass that read past that op claims
//! it ([`projection_covers`]) and runs inside the interaction's trace, so its
//! writes reach the block mirror stamped with that trace id. The clock closes
//! on a delivery of its block whose batch carries that trace, never on a row
//! that merely has the same id or content. Without an OpenTelemetry layer no
//! trace id exists and no cell clock is enrolled.
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

/// The clocks one closure retired besides its winner: the older clocks on the
/// winner's target it superseded, as their ages in whole milliseconds at the
/// close. A consumer places each at its own dispatch, `delivered_at - age`.
///
/// Emitted as the `superseded_ms` field: the ages separated by single spaces,
/// the empty string for none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Superseded(Vec<u64>);

impl Superseded {
    pub fn new(ages_ms: Vec<u64>) -> Self {
        Self(ages_ms)
    }

    pub fn ages_ms(&self) -> &[u64] {
        &self.0
    }
}

/// The batches a [`Feed`] applied since its last one that closed a clock of
/// this sample's origin, none of which closed one: their ages in whole
/// microseconds at this close, oldest first. Each is a pass of the feed that
/// no sample reports.
///
/// Emitted as the `quiet_batches_us` field, in the form of `superseded_ms`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuietBatches(Vec<u64>);

impl QuietBatches {
    pub fn new(ages_us: Vec<u64>) -> Self {
        Self(ages_us)
    }

    pub fn ages_us(&self) -> &[u64] {
        &self.0
    }
}

fn write_ages(ages: &[u64], f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let mut sep = "";
    for age in ages {
        write!(f, "{sep}{age}")?;
        sep = " ";
    }
    Ok(())
}

/// Refuses anything but whole numbers separated by single spaces, so a mangled
/// field is never read as fewer ages.
fn parse_ages(field: &'static str, s: &str) -> Result<Vec<u64>, MalformedAges> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(' ')
        .map(|age| age.parse::<u64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| MalformedAges {
            field,
            value: s.to_string(),
        })
}

impl std::fmt::Display for Superseded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_ages(&self.0, f)
    }
}

impl std::fmt::Display for QuietBatches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_ages(&self.0, f)
    }
}

impl std::str::FromStr for Superseded {
    type Err = MalformedAges;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_ages("superseded_ms", s).map(Self)
    }
}

impl std::str::FromStr for QuietBatches {
    type Err = MalformedAges;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_ages("quiet_batches_us", s).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedAges {
    pub field: &'static str,
    pub value: String,
}

impl std::fmt::Display for MalformedAges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "malformed `{}` {:?} (expected whole numbers separated by single spaces)",
            self.field, self.value
        )
    }
}

impl std::error::Error for MalformedAges {}

/// One serial consumer of deliveries: a `LiveData` subscriber or a projector.
/// It applies its batches one after another, so they are one pass sequence.
/// Two subscribers of one source are two feeds running side by side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Feed(u64);

impl Feed {
    /// A feed no other has named.
    pub fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The emitted `feed` field.
    pub fn id(self) -> u64 {
        self.0
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
    /// A cell edit committed straight to the CRDT. Never delivered by a mirror
    /// itself: [`rows_delivered`] derives it from a block row whose batch
    /// carries the trace of the projection pass that covered the commit.
    CellCommit(CellOp),
    /// A row the interaction created lands in a block mirror, in a batch that
    /// carries the interaction's trace: a split's new row, which no id known at
    /// dispatch names. Its target is the block the op addresses. The op's
    /// `write_seq` is the clock's token only when no trace exists.
    NewRow(Option<WriteSeq>),
}

/// The Loro op that committed one cell edit, and the doc it lives in. Two
/// docs can share a peer id, so the op id alone does not name a commit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CellOp {
    pub doc: u64,
    pub peer: u64,
    pub counter: i32,
}

/// What a delivery names about an op instance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Token {
    Seq(WriteSeq),
    Op(CellOp),
}

/// The [`Observable`] discriminant, without the token. Two observables
/// correlate only when their kinds are equal.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    BlockRow,
    FocusRoot,
    CellCommit,
    NewRow,
}

impl Observable {
    fn kind(self) -> Kind {
        match self {
            Observable::BlockRow(_) => Kind::BlockRow,
            Observable::FocusRoot => Kind::FocusRoot,
            Observable::CellCommit(_) => Kind::CellCommit,
            Observable::NewRow(_) => Kind::NewRow,
        }
    }

    fn token(self) -> Option<Token> {
        match self {
            Observable::BlockRow(seq) => seq.map(Token::Seq),
            Observable::FocusRoot => None,
            Observable::CellCommit(op) => Some(Token::Op(op)),
            Observable::NewRow(_) => None,
        }
    }

    /// What a dispatched block op becomes visible as: a split as its new row,
    /// every other op as its target's row (with the op's `write_seq` token).
    pub fn of_block_op(op_name: &str, params: &HashMap<String, crate::Value>) -> Self {
        match op_name {
            "split_block" => Observable::NewRow(write_seq_from_params(params)),
            _ => Observable::BlockRow(write_seq_from_params(params)),
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
    /// The interaction span of a UI clock, held open until its close.
    interaction: Option<Interaction>,
}

struct Interaction {
    span: tracing::Span,
    trace_id: String,
    /// The trace of the projection pass that covered or linked the commit.
    pass_trace: Option<String>,
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
    /// The older clocks on its target this closure retired with the winner.
    /// The rows it delivered made all of them visible, so a drain rate must
    /// count them all, each at its own dispatch.
    superseded: Superseded,
    quiet: QuietBatches,
    span: Option<tracing::Span>,
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
    quiet: Vec<QuietLog>,
}

/// The batches one feed applied since its last one that closed a clock of one
/// origin.
struct QuietLog {
    feed: Feed,
    origin: ClockOrigin,
    since: Vec<Instant>,
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

    fn quiet(&mut self, feed: Feed, origin: ClockOrigin) -> &mut Vec<Instant> {
        let at = match self
            .quiet
            .iter()
            .position(|q| q.feed == feed && q.origin == origin)
        {
            Some(at) => at,
            None => {
                self.quiet.push(QuietLog {
                    feed,
                    origin,
                    since: Vec::new(),
                });
                self.quiet.len() - 1
            }
        };
        &mut self.quiet[at].since
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
    quiet: Vec::new(),
});
/// Per-ORIGIN capacity: one origin's burst can never evict another's entries.
pub const MAX_PENDING: usize = 64;
/// Sequence of `rows_delivered` calls; every sample one call closes shares it.
static DELIVERY_BATCH: AtomicU64 = AtomicU64::new(0);
pub(crate) const EXPIRY: Duration = Duration::from_secs(30);

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
    let interaction = match origin {
        ClockOrigin::Ui => interaction_of(tracing::Span::current()),
        ClockOrigin::Facade => None,
    };
    // Without a trace no batch can name the new row as this interaction's: the
    // clock falls back to its target's row with the op's `write_seq`.
    let observable = match (observable, &interaction) {
        (Observable::NewRow(seq), None) => Observable::BlockRow(seq),
        (o, _) => o,
    };
    start(action, target, observable, origin, interaction);
}

/// `span` as the interaction a clock closes under, when it has an
/// OpenTelemetry trace: without a trace id a cell close could not be told from
/// another write of the block.
fn interaction_of(span: tracing::Span) -> Option<Interaction> {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let trace_id = span.context().span().span_context().trace_id();
    (trace_id != opentelemetry::trace::TraceId::INVALID).then(|| Interaction {
        span,
        trace_id: format!("{trace_id:032x}"),
        pass_trace: None,
    })
}

/// Start the clock of a cell edit whose commit starts with `op`, under the
/// keystroke's interaction `span`, held open until the edit is visible. Call
/// it before the commit is visible to a projection pass. Enrols nothing when
/// `span` has no OpenTelemetry context, because without a trace id the close
/// could not be told from another write of the block.
pub fn cell_edit_committed(target: &str, op: CellOp, span: &tracing::Span) {
    let Some(interaction) = interaction_of(span.clone()) else {
        return;
    };
    start(
        CELL_EDIT,
        target,
        Observable::CellCommit(op),
        ClockOrigin::Ui,
        Some(interaction),
    );
}

/// The action name of a cell keystroke's e2e sample.
pub const CELL_EDIT: &str = "cell_edit";

/// Claim the pending cell commits a projection pass read past. The pass runs
/// as a child of the newest claimed interaction and links the rest, so its
/// writes carry the newest one's trace, and every claimed clock waits for a
/// delivery carrying that trace. A pass that fails before
/// [`PassClaim::wrote`] releases its claims when the claim drops, so the pass
/// that retries can claim them again.
pub fn projection_covers(covered: impl Fn(CellOp) -> bool) -> PassClaim {
    if PENDING_LEN.load(Ordering::Acquire) == 0 {
        return PassClaim {
            claim: Claim::default(),
            registry: &PENDING,
        };
    }
    claim_in(&PENDING, covered)
}

fn claim_in(registry: &'static Mutex<Registry>, covered: impl Fn(CellOp) -> bool) -> PassClaim {
    let claim = cover(
        registry
            .lock()
            .expect("latency_e2e mutex poisoned")
            .slot(ClockOrigin::Ui),
        covered,
    );
    PassClaim { claim, registry }
}

/// The cell commits one projection pass claimed.
pub struct PassClaim {
    claim: Claim,
    registry: &'static Mutex<Registry>,
}

impl PassClaim {
    /// The claimed interaction spans, newest first.
    pub fn spans(&self) -> &[tracing::Span] {
        &self.claim.spans
    }

    /// The pass runs as `pass` and links `committed`, the commit spans it
    /// read besides its claims. A row one of those commits changes reaches
    /// the sink under the pass's trace, so its clock waits for that trace
    /// too. Call it before the pass writes the sink.
    pub fn link(&mut self, pass: &tracing::Span, committed: &[tracing::Span]) {
        self.claim.linked = committed
            .iter()
            .filter_map(|span| interaction_of(span.clone()))
            .map(|i| i.trace_id)
            .collect();
        let Some(pass) = interaction_of(pass.clone()) else {
            return;
        };
        let mut registry = self.registry.lock().expect("latency_e2e mutex poisoned");
        link_rows(
            registry.slot(ClockOrigin::Ui),
            &self.claim.linked,
            &pass.trace_id,
        );
    }

    /// The pass succeeded. `retire` names, per block, why the pass made
    /// nothing visible for it, or `None` when it did. A claimed cell edit on
    /// such a block is retired, and so is a linked commit's block-row clock.
    /// Nothing will ever become visible for them.
    pub fn wrote(mut self, retire: impl Fn(&str) -> Option<Retire>) {
        let claim = std::mem::take(&mut self.claim);
        let mut registry = self.registry.lock().expect("latency_e2e mutex poisoned");
        let retired = retire_invisible(
            registry.slot(ClockOrigin::Ui),
            &claim,
            retire,
            // ALLOW(physical_time): a latency sample's age, it decides nothing.
            Instant::now(),
        );
        if std::ptr::eq(self.registry, &PENDING) {
            PENDING_LEN.store(registry.len(), Ordering::Release);
        }
        drop(registry);
        for r in retired {
            tracing::info!(
                target: "holon_latency",
                stage = "e2e_retired",
                action = %r.clock.action,
                block = %r.clock.target,
                origin = r.clock.origin.as_str(),
                waited_ms = r.clock.waited_ms,
                reason = r.reason.as_str(),
                "an interaction's projection changed nothing visible on its block",
            );
            if let Some(span) = &r.span {
                let _retired = tracing::info_span!(
                    parent: span,
                    "interaction.retired",
                    action = %r.clock.action,
                    block = %r.clock.target,
                    reason = r.reason.as_str(),
                )
                .entered();
            }
        }
    }
}

impl Drop for PassClaim {
    fn drop(&mut self) {
        if self.claim.claimed.is_empty() {
            return;
        }
        let mut registry = self.registry.lock().expect("latency_e2e mutex poisoned");
        release(registry.slot(ClockOrigin::Ui), &self.claim);
    }
}

/// Pure core of [`PassClaim`]: what one pass claimed, newest first.
#[derive(Default)]
struct Claim {
    claimed: Vec<(CellOp, String)>,
    spans: Vec<tracing::Span>,
    trace: String,
    /// The traces of the commits the pass links.
    linked: Vec<String>,
}

/// Why a pass made nothing visible for a block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Retire {
    /// The pass emitted no op for the block.
    Unwritten,
    /// The pass's op leaves the block's stored row as it was: the sink stores
    /// content through `content_canonical::canonicalize_stored_content`, so
    /// an edit of edge whitespace only changes nothing another view shows.
    NoVisibleChange,
}

impl Retire {
    pub const fn as_str(self) -> &'static str {
        match self {
            Retire::Unwritten => "unwritten",
            Retire::NoVisibleChange => "no_visible_change",
        }
    }
}

/// A clock a pass retired, with its reason and its interaction span.
struct Retired {
    clock: Expired,
    reason: Retire,
    span: Option<tracing::Span>,
}

fn cover(pending: &mut [Pending], covered: impl Fn(CellOp) -> bool) -> Claim {
    let mut by_age: Vec<&mut Pending> = pending.iter_mut().collect();
    by_age.sort_by_key(|p| std::cmp::Reverse(p.t0));
    let mut claim = Claim::default();
    for p in by_age {
        let Observable::CellCommit(op) = p.observable else {
            continue;
        };
        let interaction = p
            .interaction
            .as_mut()
            .expect("a cell clock is enrolled with its interaction");
        if interaction.pass_trace.is_some() || !covered(op) {
            continue;
        }
        if claim.spans.is_empty() {
            claim.trace = interaction.trace_id.clone();
        }
        interaction.pass_trace = Some(claim.trace.clone());
        claim.claimed.push((op, p.target.clone()));
        claim.spans.push(interaction.span.clone());
    }
    claim
}

fn retire_invisible(
    pending: &mut Vec<Pending>,
    claim: &Claim,
    retire: impl Fn(&str) -> Option<Retire>,
    now: Instant,
) -> Vec<Retired> {
    let mut retired = Vec::new();
    pending.retain(|p| {
        let claimed = matches!(p.observable, Observable::CellCommit(op)
            if claim.claimed.iter().any(|(c, _)| *c == op));
        let linked = p.observable.kind() == Kind::BlockRow
            && p.interaction
                .as_ref()
                .is_some_and(|i| claim.linked.contains(&i.trace_id));
        let reason = match retire(&p.target) {
            Some(reason) if claimed || linked => reason,
            _ => return true,
        };
        retired.push(Retired {
            clock: Expired {
                action: p.action.clone(),
                target: p.target.clone(),
                origin: p.origin,
                waited_ms: now.duration_since(p.t0).as_millis() as u64,
            },
            reason,
            span: p.interaction.as_ref().map(|i| i.span.clone()),
        });
        false
    });
    retired
}

/// Point the row clocks of the `linked` commits at the pass trace their rows
/// reach the sink under.
fn link_rows(pending: &mut [Pending], linked: &[String], pass_trace: &str) {
    for p in pending {
        if p.observable.kind() == Kind::CellCommit {
            continue;
        }
        if let Some(i) = p
            .interaction
            .as_mut()
            .filter(|i| linked.contains(&i.trace_id))
        {
            i.pass_trace = Some(pass_trace.to_string());
        }
    }
}

fn release(pending: &mut [Pending], claim: &Claim) {
    for p in pending {
        let Observable::CellCommit(op) = p.observable else {
            continue;
        };
        let interaction = p
            .interaction
            .as_mut()
            .expect("a cell clock is enrolled with its interaction");
        if interaction.pass_trace.as_deref() == Some(claim.trace.as_str())
            && claim.claimed.iter().any(|(c, _)| *c == op)
        {
            interaction.pass_trace = None;
        }
    }
}

/// The cell commits a batch makes visible: a block row of a clock's target in
/// a batch that carries the trace of the pass that covered its commit.
fn cell_deliveries(
    pending: &[Pending],
    deliveries: &[(String, Observable)],
    traces: &[&str],
) -> Vec<(String, Observable)> {
    pending
        .iter()
        .filter_map(|p| {
            let Observable::CellCommit(op) = p.observable else {
                return None;
            };
            let pass = p.interaction.as_ref()?.pass_trace.as_deref()?;
            let delivered = traces.contains(&pass)
                && deliveries
                    .iter()
                    .any(|(id, o)| *id == p.target && o.kind() == Kind::BlockRow);
            delivered.then(|| (p.target.clone(), Observable::CellCommit(op)))
        })
        .collect()
}

/// The new rows a batch makes visible: a created block row in a batch that
/// carries the trace of a pending [`Observable::NewRow`] clock's interaction,
/// or of the pass that linked its commit.
fn new_row_deliveries(
    pending: &[Pending],
    deliveries: &[(String, Observable)],
    traces: &[&str],
) -> Vec<(String, Observable)> {
    let created = deliveries.iter().any(|(_, o)| o.kind() == Kind::NewRow);
    pending
        .iter()
        .filter(|p| p.observable.kind() == Kind::NewRow)
        .filter(|p| {
            created
                && p.interaction.as_ref().is_some_and(|i| {
                    traces.contains(&i.trace_id.as_str())
                        || i.pass_trace.as_deref().is_some_and(|t| traces.contains(&t))
                })
        })
        .map(|p| (p.target.clone(), p.observable))
        .collect()
}

/// What a batch offers one origin's `pending` clocks: its rows as delivered,
/// and the cell commits and new rows it makes visible by its `traces`.
fn offered(
    pending: &[Pending],
    deliveries: &[(String, Observable)],
    traces: &[&str],
) -> Vec<(String, Observable)> {
    let mut offered = cell_deliveries(pending, deliveries, traces);
    offered.extend(new_row_deliveries(pending, deliveries, traces));
    offered.extend(
        deliveries
            .iter()
            .filter(|(_, o)| o.kind() != Kind::NewRow)
            .cloned(),
    );
    offered
}

fn start(
    action: &str,
    target: &str,
    observable: Observable,
    origin: ClockOrigin,
    interaction: Option<Interaction>,
) {
    let mut registry = PENDING.lock().expect("latency_e2e mutex poisoned");
    let (expired, evicted) = enrol(
        &mut registry,
        action,
        target,
        observable,
        origin,
        interaction,
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
    interaction: Option<Interaction>,
    now: Instant,
) -> (Vec<Expired>, Option<Evicted>) {
    let mut expired = Vec::new();
    for o in ORIGINS {
        expired.extend(prune_expired(registry.slot(o), now));
    }
    // Cross-origin contention is recorded as an EVENT here, on both sides, so
    // that it covers the whole overlap rather than two sampled instants:
    //   * this clock is contended if the other slot already has anything in it;
    //   * every entry already pending in the other slot becomes contended,
    //     because this clock now shares the pipeline with it.
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
        interaction,
    });
    (expired, evicted)
}

/// Retire the pending entry of an interaction that produced no write, because
/// the op was **refused or failed**. Without this the entry squats until
/// `EXPIRY` and any unrelated later delivery for that row — a background
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
        .chain(changes.iter().filter_map(created_row))
        .collect()
}

/// A created block row, offered to the [`Observable::NewRow`] clocks whose
/// trace its batch carries (see [`new_row_deliveries`]).
fn created_row(c: &crate::Change<crate::StorageEntity>) -> Option<(String, Observable)> {
    let crate::Change::Created { data, .. } = c else {
        return None;
    };
    // ALLOW(raw_row_id_column): key — the latency ledger keys on the id TEXT
    let id = data.get("id").and_then(|v| v.as_string())?;
    Some((id.to_string(), Observable::NewRow(None)))
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
        // ALLOW(raw_row_id_column): key — the latency ledger keys on the id
        // TEXT; it resolves no entity
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

/// Report the entities made visible by a batch `feed` applied, as
/// `(id, Observable)` pairs (see [`touched_entities`]). Emits one `stage="e2e"`
/// event per closed entry.
///
/// `received_at` is when the feed took the batch off its stream. A clock
/// dispatched after that cannot be what the batch carries, so it is not closed
/// here: see `close_received`.
///
/// A batch that closes no clock of an origin is reported on the feed's next
/// closure of that origin, in `quiet_batches_us`: see `log_quiet`. A batch
/// applied while no clock of ANY origin is pending is not logged: every clock
/// a later batch closes was dispatched after it. A batch applied while only
/// the other origin has clocks pending is logged for this origin too.
pub fn rows_delivered<'a>(
    source: &'static str,
    feed: Feed,
    received_at: Instant,
    deliveries: impl IntoIterator<Item = (&'a str, Observable)>,
    origins: &[crate::streaming::BatchTraceContext],
) {
    if PENDING_LEN.load(Ordering::Acquire) == 0 {
        return;
    }
    let deliveries: Vec<(String, Observable)> = deliveries
        .into_iter()
        .map(|(id, observable)| (id.to_string(), observable))
        .collect();
    let traces: Vec<&str> = origins.iter().map(|o| o.trace_id.as_str()).collect();
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
        let mut of_origin = close_batch(slot, &deliveries, &traces, received_at, now);
        log_quiet(registry.quiet(feed, o), &mut of_origin, now);
        closed.extend(of_origin);
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
        // wait, and an agent typing faster than a human turns a healthy
        // pipeline into a wall of SLO banners (BugFunnel 2026-08-31).
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
            superseded_ms = %c.superseded,
            quiet_batches_us = %c.quiet,
            delivery_batch,
            feed = feed.id(),
            "holon_latency",
        );
        if let Some(interaction) = &c.span {
            let _visible = tracing::info_span!(
                parent: interaction,
                "interaction.visible",
                action = %c.action,
                source,
                block = %c.target,
                ms = c.ms,
                superseded = c.superseded.ages_ms().len(),
            )
            .entered();
        }
    }
}

/// Log one batch of a feed against `since`, the feed's batches since its last
/// one that closed a clock of `closed`'s origin. A batch that closed none joins
/// the log; one that closed some reports the log on each closure and empties
/// it.
///
/// A batch older than [`EXPIRY`] is dropped from the log: every clock still
/// pending was dispatched after it, so no later pass waits on work queued at
/// its landing.
fn log_quiet(since: &mut Vec<Instant>, closed: &mut [Closed], now: Instant) {
    since.retain(|&at| now.duration_since(at) < EXPIRY);
    if closed.is_empty() {
        since.push(now);
        return;
    }
    let quiet = QuietBatches(
        since
            .drain(..)
            .map(|at| now.duration_since(at).as_micros() as u64)
            .collect(),
    );
    for c in closed {
        c.quiet = quiet.clone();
    }
}

/// The clocks of one origin's `pending` that a batch makes visible: its
/// `deliveries`, written under `traces`, received at `received_at`.
fn close_batch(
    pending: &mut Vec<Pending>,
    deliveries: &[(String, Observable)],
    traces: &[&str],
    received_at: Instant,
    now: Instant,
) -> Vec<Closed> {
    let offered = offered(pending, deliveries, traces);
    close_received(pending, &offered, traces, received_at, now)
}

/// Whether a batch written under `traces` carries `p`'s interaction: its own
/// trace or the trace of the pass that read its commit. A clock without an
/// interaction, or a batch without a trace, cannot tell, so it carries.
fn carried(p: &Pending, traces: &[&str]) -> bool {
    traces.is_empty()
        || p.interaction.as_ref().is_none_or(|i| {
            traces.contains(&i.trace_id.as_str())
                || i.pass_trace.as_deref().is_some_and(|t| traces.contains(&t))
        })
}

/// [`close_delivered`] over the clocks dispatched BEFORE the batch was
/// received.
///
/// A delivered row that carries no `WriteSeq` names no op instance, so
/// `close_delivered` gives it to the newest pending clock on its target that
/// the batch carries. A clock dispatched after the batch was received is newer
/// than anything the batch can carry: the batch would close it and report a
/// latency too short to be true. The second `LiveData` subscriber of one
/// source, which delivers the same rows again, is the same case.
fn close_received<S: AsRef<str>>(
    pending: &mut Vec<Pending>,
    deliveries: &[(S, Observable)],
    traces: &[&str],
    received_at: Instant,
    now: Instant,
) -> Vec<Closed> {
    let (mut eligible, dispatched_after): (Vec<Pending>, Vec<Pending>) = std::mem::take(pending)
        .into_iter()
        .partition(|p| p.t0 <= received_at);
    let mut closed = close_delivered(&mut eligible, deliveries, traces, now);
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
/// A clock wins without its own token only when the batch carries it
/// ([`carried`]): a row another interaction's pass wrote says nothing about an
/// op still in flight.
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
    traces: &[&str],
    now: Instant,
) -> Vec<Closed> {
    let mut by_target: HashMap<(&str, Kind), (Vec<Token>, bool)> = HashMap::new();
    for (id, observable) in deliveries {
        let entry = by_target
            .entry((id.as_ref(), observable.kind()))
            .or_insert((Vec::new(), false));
        match observable.token() {
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
            let matches = p.observable.token().is_some_and(|s| seqs.contains(&s));
            if matches && winner.is_none_or(|w| p.t0 > pending[w].t0) {
                winner = Some(i);
            }
        }
        // 2/3. anonymous delivery — the batch named no pending op instance.
        if winner.is_none() {
            let mut newest: Option<usize> = None;
            for (i, p) in pending.iter().enumerate() {
                if p.target != target || p.observable.kind() != kind || !carried(p, traces) {
                    continue;
                }
                if newest.is_none_or(|w| p.t0 > pending[w].t0) {
                    newest = Some(i);
                }
            }
            // A delivered token nobody is waiting for is the STICKY row column,
            // not evidence about an op in flight: `write_seq` records the row's
            // last editor write and every later projection of that row repeats
            // it. So it may not veto a tokenless entry's closure (rule 3) —
            // that veto is what made split/indent/join unmeasurable on any
            // block the user had ever typed into. A tokenful entry still waits
            // for its own token unless the batch carried a genuinely tokenless
            // row (rule 2), which is what keeps a stale no-op re-commit from
            // stealing.
            if let Some(i) = newest {
                if has_tokenless || pending[i].observable.token().is_none() {
                    winner = Some(i);
                }
            }
        }
        let Some(winner) = winner else {
            continue;
        };
        let winner_t0 = pending[winner].t0;
        let superseded = Superseded(
            pending
                .iter()
                .enumerate()
                .filter(|&(i, p)| {
                    i != winner
                        && p.target == target
                        && p.observable.kind() == kind
                        && p.t0 <= winner_t0
                })
                .map(|(_, p)| now.duration_since(p.t0).as_millis() as u64)
                .collect(),
        );
        closed.push(Closed {
            action: pending[winner].action.clone(),
            target: pending[winner].target.clone(),
            origin: pending[winner].origin,
            ms: now.duration_since(winner_t0).as_millis() as u64,
            in_flight: pending[winner].in_flight,
            backlog: 0,
            contended: pending[winner].contended,
            superseded,
            quiet: QuietBatches::default(),
            span: pending[winner].interaction.as_ref().map(|i| i.span.clone()),
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
mod tests;
