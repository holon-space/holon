//! `inv-no-steady-reseed-leak` — observe-only pin over LoroProjection's
//! full-reseed attribution (Inc 0 of the reseed-latency workstream; BugFunnel
//! row 71).
//!
//! ## What it watches
//! `LoroProjection::project` emits one `holon_latency` tracing event per
//! projection pass with `mode=full|incremental` and, for `mode=full`, a
//! `reason=coldboot|empty_pending_moved_frontier|unsettled|orphan|oversized|
//! sink_fail` label (see `holon_loro::loro_sync_controller::FullReason`).
//! A full reseed is O(document): a `coldboot` pass is a legitimate one-time
//! seed, but a full reseed fired by an *interactive* transition at steady state
//! (post-seed) is the reseed *leak* the workstream is chasing — an SLO-busting
//! O(N) re-projection where the O(changed) fast path should have run.
//!
//! ## Mechanism ([`ReseedObserverLayer`])
//! A `tracing` layer filtered to the `holon_latency` target records every
//! projection event into a process-global [`ReseedObserver`]. The harness marks
//! the observer *steady* on the first interactive transition and hands it that
//! transition's label ([`ReseedObserver::note_transition`]), so each recorded
//! full-reseed event is attributed to the transition it fired under and tagged
//! seed vs steady. This is the ref-less, process-global pattern of
//! [`crate::pbt::composed::observed_errors`].
//!
//! ## Observe-only, one-line flip
//! [`InvNoSteadyReseedLeak::check`] logs the running per-reason summary and
//! returns `Ok` UNLESS `HOLON_PBT_RESEED_ORACLE=enforce`, in which case a
//! steady-state leak fails the case. Inc 0 lands it OFF (no new RED); a later
//! increment sets the env var to enforce it. The leak is also announced once
//! per (reason) on stderr the first time it fires at steady state — the
//! non-vacuity evidence the baseline is read from.

// ── NON-VACUITY BASELINE (keystone N, RE-MEASURED 2026-07-17) ────────────────
// SUPERSEDES the 2026-07-16 table below. The earlier table was measured under
// the tick-0 journals abort (keystone reddened at the FIRST `check_invariants`
// before any interactive transition ran), so it reported every reason as "never
// reached" — an ARTIFACT of the abort, not a real steady-state measurement.
//
// Two changes lifted the abort: (1) the journals ingest-data-loss oracle fix
// landed; (2) F8 paired `inv-display-placement-canonical-inert`'s selection
// with its injection env, so the default keystone no longer reds on it. Full
// interactive sequences now run to steady state. Re-measured by running
// `general_e2e_composed_pbt` with the observer armed under
// `HOLON_PBT_RESEED_ORACLE=enforce HOLON_PBT_FORCE_FULL=1 PROPTEST_CASES=32`
// (confounding non-reseed reds — the `CreateBlockUnderFocus` history gap and
// the advice-injection collision — softened to `warn` so sequences run long; 89
// per-sequence reseed summaries observed):
//
//   reason                          fires at keystone N (steady state)?
//   ------------------------------  -----------------------------------
//   coldboot (legit boot seed)      YES — 4–6× per sequence (NOT a leak)
//   empty_pending_moved_frontier    NO
//   unsettled                       NO
//   orphan                          NO
//   oversized                       NO
//   sink_fail (recovery)            NO
//
// Steady state IS now reached: `incremental` passes scale with sequence length
// (observed 1..61 per sequence), so the O(changed) fast path handles the
// generated interactive edits. Across ALL 89 summaries: `steady_leaks=0`, every
// `mode=full` pass is `coldboot`. Under `enforce` the invariant did NOT red on
// the reseed axis.
//
// Consequence for the enforce-flip: the prerequisite is NOT "reasons now fire"
// — none of the four LEAK reasons fire at keystone N, so flipping `enforce`
// would be GREEN but VACUOUS (no leak occurs for it to catch). The keystone now
// actively CONFIRMS the incremental path holds at N (a strict improvement over
// the old "never reached" vacuity), but still cannot PROVE it catches a leak
// regression — that non-vacuity needs the wall-clock soak / diag guard
// (`HOLON_SOAK_SEED_FILES`) where a real O(N) reseed leak reproduces. Do NOT
// flip `enforce` by default on this basis (reseed workstream's call).
// See docs/Testing/BugFunnel.md row 71.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::OnceLock;

use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// The four full-reseed reasons that are LEAKS when they fire at steady state
/// (post-seed) under an interactive transition. `coldboot` (legitimate seed)
/// and `sink_fail` (disclosed recovery) are excluded.
const LEAK_REASONS: &[&str] = &[
    "empty_pending_moved_frontier",
    "unsettled",
    "orphan",
    "oversized",
];

fn is_leak_reason(reason: &str) -> bool {
    LEAK_REASONS.contains(&reason)
}

/// The four steady-state full-reseed LEAK reasons, as a typed mirror of the
/// (private) `FullReason` leak arms in `holon_loro::loro_sync_controller`.
/// Parse-don't-validate target for `HOLON_SOAK_RESEED_REASON` and the
/// reason-scoped [`ReseedSummary::steady_leak_count_for`] query — so the soak
/// rung asks "how many `EmptyPendingMovedFrontier` leaks fired?" against a
/// type, not a stringly-typed reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReseedLeakReason {
    EmptyPendingMovedFrontier,
    Unsettled,
    Orphan,
    Oversized,
}

impl ReseedLeakReason {
    /// The on-wire reason string emitted by `holon_latency` (must match the
    /// `FullReason::as_str` arms the observer records).
    pub fn as_str(self) -> &'static str {
        match self {
            ReseedLeakReason::EmptyPendingMovedFrontier => "empty_pending_moved_frontier",
            ReseedLeakReason::Unsettled => "unsettled",
            ReseedLeakReason::Orphan => "orphan",
            ReseedLeakReason::Oversized => "oversized",
        }
    }

    /// Parse an on-wire reason string. Fails loud (`Err`) on any non-leak
    /// reason so the boundary env parser can panic with a clear message rather
    /// than silently defaulting.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "empty_pending_moved_frontier" => Ok(ReseedLeakReason::EmptyPendingMovedFrontier),
            "unsettled" => Ok(ReseedLeakReason::Unsettled),
            "orphan" => Ok(ReseedLeakReason::Orphan),
            "oversized" => Ok(ReseedLeakReason::Oversized),
            other => Err(format!(
                "unknown reseed leak reason {other:?}; expected one of {LEAK_REASONS:?}"
            )),
        }
    }
}

/// Per-case accumulation of projection-mode attribution.
#[derive(Default)]
struct ReseedInner {
    /// Label of the interactive transition currently applying (`None` during
    /// boot/seed, before the first `note_transition`).
    current_transition: Option<String>,
    /// `true` once the first interactive transition has been noted — every
    /// projection event before that is boot/seed and NOT a leak candidate.
    steady: bool,
    /// `mode=full` reason → count, ALL passes (boot seed included).
    full_by_reason: BTreeMap<String, usize>,
    /// `mode=incremental` pass count.
    incremental: usize,
    /// Steady-state (transition, reason) for every full-reseed LEAK — the
    /// attribution the enforcement path fails on.
    steady_leaks: Vec<(String, String)>,
}

/// A read snapshot handed to the invariant (and printed at teardown).
#[derive(Clone, Debug, Default)]
pub struct ReseedSummary {
    pub steady_leak_total: usize,
    pub steady_leaks: Vec<(String, String)>,
    pub full_by_reason: BTreeMap<String, usize>,
    pub incremental: usize,
}

impl ReseedSummary {
    /// Count the steady-state leaks attributed to one specific reason (the
    /// reason-scoped query the soak rung asserts on: "did the TARGET reason —
    /// default `EmptyPendingMovedFrontier` — fire at least once?").
    pub fn steady_leak_count_for(&self, reason: ReseedLeakReason) -> usize {
        self.steady_leaks
            .iter()
            .filter(|(_, r)| r == reason.as_str())
            .count()
    }

    pub fn report(&self) -> String {
        let full: usize = self.full_by_reason.values().sum();
        let reasons: Vec<String> = self
            .full_by_reason
            .iter()
            .map(|(r, n)| format!("{r}={n}"))
            .collect();
        format!(
            "incremental={} full={} [{}] steady_leaks={}",
            self.incremental,
            full,
            reasons.join(" "),
            self.steady_leak_total,
        )
    }
}

/// Process-global observer of LoroProjection full-reseed attribution.
pub struct ReseedObserver {
    inner: Mutex<ReseedInner>,
    /// Leak reasons already announced on stderr (process-wide, never reset) so
    /// the first steady-state occurrence of each reason is reported exactly
    /// once — the non-vacuity baseline record.
    announced: Mutex<BTreeSet<String>>,
}

static OBSERVER: OnceLock<ReseedObserver> = OnceLock::new();

impl ReseedObserver {
    pub fn global() -> &'static ReseedObserver {
        OBSERVER.get_or_init(|| ReseedObserver {
            inner: Mutex::new(ReseedInner::default()),
            announced: Mutex::new(BTreeSet::new()),
        })
    }

    /// Start a new case: print the finished case's summary (teardown log) and
    /// clear the per-case accumulation. Called from the slice `build` (once per
    /// `init_test`). The process-wide `announced` set is deliberately NOT
    /// cleared — first-seen leak evidence must survive across cases/shrinks.
    pub fn reset(&self) {
        let mut inner = self.inner.lock().expect("reseed observer lock");
        let had_data = inner.incremental > 0 || !inner.full_by_reason.is_empty();
        if had_data {
            let summary = ReseedSummary {
                steady_leak_total: inner.steady_leaks.len(),
                steady_leaks: inner.steady_leaks.clone(),
                full_by_reason: inner.full_by_reason.clone(),
                incremental: inner.incremental,
            };
            eprintln!("[RESEED-ORACLE] case summary: {}", summary.report());
        }
        *inner = ReseedInner::default();
    }

    /// Mark the start of an interactive transition: the observer becomes steady
    /// (post-seed) and subsequent projection events are attributed to `label`.
    pub fn note_transition(&self, label: &str) {
        let mut inner = self.inner.lock().expect("reseed observer lock");
        inner.steady = true;
        inner.current_transition = Some(label.to_string());
    }

    /// Record one projection pass (the tracing layer calls this).
    pub fn record_projection(&self, mode: &str, reason: &str) {
        let announce = {
            let mut inner = self.inner.lock().expect("reseed observer lock");
            if mode == "incremental" {
                inner.incremental += 1;
                return;
            }
            // mode == "full"
            *inner.full_by_reason.entry(reason.to_string()).or_default() += 1;
            if inner.steady && is_leak_reason(reason) {
                let label = inner
                    .current_transition
                    .clone()
                    .unwrap_or_else(|| "<unknown>".to_string());
                inner.steady_leaks.push((label.clone(), reason.to_string()));
                Some((label, reason.to_string()))
            } else {
                None
            }
        };
        if let Some((label, reason)) = announce {
            let mut announced = self.announced.lock().expect("reseed announced lock");
            if announced.insert(reason.clone()) {
                eprintln!(
                    "[RESEED-ORACLE] FIRST steady-state full-reseed LEAK: reason={reason} \
                     transition={label} (an interactive transition triggered an O(N) full \
                     reseed where the incremental fast path should have run)"
                );
            }
        }
    }

    /// Per-case read snapshot for the invariant.
    pub fn summary(&self) -> ReseedSummary {
        let inner = self.inner.lock().expect("reseed observer lock");
        ReseedSummary {
            steady_leak_total: inner.steady_leaks.len(),
            steady_leaks: inner.steady_leaks.clone(),
            full_by_reason: inner.full_by_reason.clone(),
            incremental: inner.incremental,
        }
    }
}

/// A `tracing` layer that records every `holon_latency` projection event's
/// `mode`/`reason` into the process-global [`ReseedObserver`]. Attach it with a
/// `Targets` filter pinned to the `holon_latency` target. That filter DOES
/// raise the registry's `max_level_hint` to DEBUG, but per-layer interest
/// caching gates this layer to `holon_latency` callsites only — every other
/// DEBUG callsite (e.g. the hot render-tree spans) reports no interest for it
/// and is never handed to `on_event`.
pub struct ReseedObserverLayer;

#[derive(Default)]
struct ProjectionVisitor {
    stage: Option<String>,
    mode: Option<String>,
    reason: Option<String>,
}

impl tracing::field::Visit for ProjectionVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "stage" => self.stage = Some(value.to_string()),
            "mode" => self.mode = Some(value.to_string()),
            "reason" => self.reason = Some(value.to_string()),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // Fallback for any non-`&str` recording path. String fields recorded via
        // `record_debug` arrive quoted (`"full"`); strip a single pair.
        let raw = format!("{value:?}");
        let v = raw.trim_matches('"').to_string();
        match field.name() {
            "stage" => self.stage = Some(v),
            "mode" => self.mode = Some(v),
            "reason" => self.reason = Some(v),
            _ => {}
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ReseedObserverLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "holon_latency" {
            return;
        }
        let mut v = ProjectionVisitor::default();
        event.record(&mut v);
        if v.stage.as_deref() != Some("projection") {
            return;
        }
        let mode = v.mode.as_deref().unwrap_or("?");
        let reason = v.reason.as_deref().unwrap_or("unlabeled");
        ReseedObserver::global().record_projection(mode, reason);
    }
}

/// Read cap: the per-case reseed attribution snapshot. Ref-less — the data
/// lives in the process-global [`ReseedObserver`], mirroring
/// [`crate::pbt::composed::observed_errors::ObservedProblems`].
#[holon_macros::capmap_adapter]
pub trait ReseedAttribution {
    fn reseed_summary(&self) -> ReseedSummary;
}

/// Provider reading the process-global observer.
#[derive(Default)]
pub struct ComposedReseedObserver;

impl ComposedReseedObserver {
    pub fn new() -> Self {
        Self
    }
}

impl ReseedAttribution for ComposedReseedObserver {
    fn reseed_summary(&self) -> ReseedSummary {
        ReseedObserver::global().summary()
    }
}

/// Observe-only pin: logs the running full-reseed attribution each check and,
/// only when `enforce` is set, fails when an interactive transition triggered a
/// steady-state full reseed.
pub struct InvNoSteadyReseedLeak {
    /// Whether a steady-state leak fails the case. Read ONCE from the
    /// environment at construction (`from_env`, called by `wire()`) and held as
    /// data — parse-don't-validate at the boundary. Keeping it off the `check`
    /// hot path also means the enforce decision can't race a concurrent test
    /// that toggles the env var (plain `cargo test` shares one process).
    enforce: bool,
}

impl InvNoSteadyReseedLeak {
    pub const ID: InvariantId = InvariantId("inv-no-steady-reseed-leak");

    /// Construct with the enforce flag read from `HOLON_PBT_RESEED_ORACLE`.
    /// THE one-line flip: Inc 0 runs observe-only (env unset ⇒ `enforce=false`,
    /// so `check` never fails); a later increment sets
    /// `HOLON_PBT_RESEED_ORACLE= enforce`.
    pub fn from_env() -> Self {
        Self {
            enforce: std::env::var("HOLON_PBT_RESEED_ORACLE").as_deref() == Ok("enforce"),
        }
    }
}

#[allow(async_fn_in_trait)]
impl Invariant<CapMap, CapMap> for InvNoSteadyReseedLeak {
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &CapMap, sut: &CapMap) -> InvariantResult {
        let summary = sut.reseed_summary();
        tracing::debug!(
            target: "holon_latency",
            stage = "reseed_oracle",
            steady_leaks = summary.steady_leak_total as u64,
            "{}",
            summary.report(),
        );
        if summary.steady_leak_total == 0 {
            return InvariantResult::Ok;
        }
        let detail: Vec<String> = summary
            .steady_leaks
            .iter()
            .map(|(t, r)| format!("{t} → reason={r}"))
            .collect();
        if self.enforce {
            InvariantResult::Fail(format!(
                "[inv-no-steady-reseed-leak] {} interactive transition(s) triggered an O(N) \
                 full reseed at steady state (should have taken the incremental fast path):\n  {}",
                summary.steady_leak_total,
                detail.join("\n  "),
            ))
        } else {
            InvariantResult::Skipped(format!(
                "HOLON_PBT_RESEED_ORACLE off — {} observed steady-state reseed leak(s): {}",
                summary.steady_leak_total,
                detail.join("; "),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    struct FixtureSummary(ReseedSummary);
    impl ReseedAttribution for FixtureSummary {
        fn reseed_summary(&self) -> ReseedSummary {
            self.0.clone()
        }
    }

    fn map(summary: ReseedSummary) -> CapMap {
        let mut caps = CapMap::new();
        caps.insert(Arc::new(FixtureSummary(summary)) as Arc<dyn ReseedAttribution>);
        caps
    }

    /// No steady leaks ⇒ Ok regardless of the enforce flag. The flag is a field
    /// (not an env read), so these tests never touch process-global state and
    /// are safe under plain parallel `cargo test`.
    #[tokio::test]
    async fn ok_when_no_steady_leak() {
        let sut = map(ReseedSummary {
            incremental: 5,
            full_by_reason: BTreeMap::from([("coldboot".to_string(), 1)]),
            ..Default::default()
        });
        assert!(matches!(
            InvNoSteadyReseedLeak { enforce: true }
                .check(&CapMap::new(), &sut)
                .await,
            InvariantResult::Ok
        ));
    }

    /// A steady leak, observe mode (`enforce=false`) ⇒ Skipped (disclosed, not
    /// RED).
    #[tokio::test]
    async fn skipped_when_leak_and_not_enforcing() {
        let sut = map(ReseedSummary {
            steady_leak_total: 1,
            steady_leaks: vec![("Split".to_string(), "orphan".to_string())],
            ..Default::default()
        });
        match (InvNoSteadyReseedLeak { enforce: false })
            .check(&CapMap::new(), &sut)
            .await
        {
            InvariantResult::Skipped(msg) => assert!(msg.contains("orphan"), "{msg}"),
            other => panic!("expected Skipped in observe mode, got {other:?}"),
        }
    }

    /// A steady leak with `enforce=true` ⇒ Fail (the flipped pin).
    #[tokio::test]
    async fn fails_when_leak_and_enforcing() {
        let sut = map(ReseedSummary {
            steady_leak_total: 1,
            steady_leaks: vec![("InsertText".to_string(), "unsettled".to_string())],
            ..Default::default()
        });
        match (InvNoSteadyReseedLeak { enforce: true })
            .check(&CapMap::new(), &sut)
            .await
        {
            InvariantResult::Fail(msg) => assert!(msg.contains("unsettled"), "{msg}"),
            other => panic!("expected Fail when enforcing, got {other:?}"),
        }
    }
}
