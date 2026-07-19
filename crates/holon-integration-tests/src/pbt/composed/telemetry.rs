//! Per-case structured telemetry for the ONE composed keystone (weights-spike
//! step 0 — the *measure* for how effective the hard-coded generator weights
//! are at steering trajectories toward bug-rich territory).
//!
//! @pbt kind observability
//!
//! Env-gated by `HOLON_PBT_TELEMETRY=1`. When OFF the harness emits nothing
//! new and takes no telemetry work, so a default run's stderr is byte-identical
//! to before this module existed. When ON, `teardown` emits exactly one
//! machine-parseable line per case:
//!
//! ```text
//! [pbt-telemetry] {"case_index":0,"n_transitions":7,"wiring":{...},
//!   "transition_kinds":{"Indent":2,...},"distinct_bigrams":["Indent>Split",...],
//!   "engagement":{"inv-...":[3,7],...},"wall":{"total_us":123,"per_kind_us":{...}}}
//! ```
//!
//! ## Determinism contract
//! Everything OUTSIDE the `wall` object is a pure function of the drawn
//! transition sequence + wiring + engagement ledger — i.e. of the proptest
//! seed — so two runs of the same seed produce byte-identical non-`wall`
//! content. Wall-clock micros are the sole nondeterministic part and are
//! quarantined under `wall`, so [`build_line`] can be unit-tested for a stable
//! shape without them (the proptest replay ban on wall-clock is about *driving*
//! behaviour off the clock; here we only *report* elapsed time — the RNG and
//! the SUT never observe it). Never read for control flow.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use holon_pbt_core::Wiring;
use serde_json::Value;
use serde_json::json;

use super::harness::EngageTally;

/// Process-global 0-based case ordinal. proptest-state-machine gives the
/// `StateMachineTest` impl no access to its RNG seed, so the ordinal is the
/// stable per-case key; it is drawn only when telemetry is enabled.
static CASE_INDEX: AtomicU64 = AtomicU64::new(0);

/// Whether `HOLON_PBT_TELEMETRY=1` is set. Read once — a run cannot flip mid-way.
pub(crate) fn telemetry_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("HOLON_PBT_TELEMETRY").as_deref() == Ok("1"))
}

/// The next case ordinal (0-based). Only called from `init_test`/`from_parts`
/// when telemetry is enabled, so the disabled path never touches the counter.
pub(crate) fn next_case_index() -> u64 {
    CASE_INDEX.fetch_add(1, Ordering::Relaxed)
}

/// Per-case accumulator threaded through the harness `ComposedSut` behind a
/// `RefCell` (interior mutability — `apply` records here while holding `&mut
/// self` on disjoint fields). Cheap: one `String` push + one `u64` per
/// transition, populated only when telemetry is enabled.
#[derive(Default)]
pub(crate) struct CaseTelemetry {
    /// This case's ordinal, taken once at construction.
    pub case_index: u64,
    /// Ordered transition-kind labels (head token of the transition `Debug`
    /// repr, exactly the `action_label` the latency summary uses).
    pub kinds: Vec<String>,
    /// Per-transition wall micros, parallel to [`kinds`](Self::kinds).
    pub micros: Vec<u64>,
}

/// Construct a fresh per-case accumulator, drawing the next case ordinal only
/// when telemetry is enabled (the disabled path leaves the counter untouched
/// and the accumulator inert).
pub(crate) fn new_case() -> CaseTelemetry {
    let mut t = CaseTelemetry::default();
    if telemetry_enabled() {
        t.case_index = next_case_index();
    }
    t
}

impl CaseTelemetry {
    /// Append one applied transition's kind label and its measured micros.
    pub fn record(&mut self, kind: String, micros: u64) {
        self.kinds.push(kind);
        self.micros.push(micros);
    }
}

/// Build the machine-parseable telemetry [`Value`]. Pure: no clock reads, no
/// env, no globals — every input is passed in, so identical non-`wall` content
/// follows from identical inputs (the determinism the unit test pins).
pub(crate) fn build_line(
    case_index: u64,
    wiring: &Wiring,
    kinds: &[String],
    micros: &[u64],
    engagement: &BTreeMap<&'static str, EngageTally>,
) -> Value {
    let mut kind_counts: BTreeMap<&str, u64> = BTreeMap::new();
    for k in kinds {
        *kind_counts.entry(k.as_str()).or_default() += 1;
    }
    // Distinct adjacent transition-kind pairs — the diversity of two-step
    // trajectories the weights actually produced (the novelty-curve primitive).
    let mut bigrams: BTreeSet<String> = BTreeSet::new();
    for w in kinds.windows(2) {
        bigrams.insert(format!("{}>{}", w[0], w[1]));
    }
    let engagement_json: Value = engagement
        .iter()
        .map(|(id, t)| ((*id).to_string(), json!([t.engaged, t.engaged + t.skipped])))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    // Wall-clock aggregates (nondeterministic — quarantined under `wall`).
    let mut per_kind_micros: BTreeMap<&str, u64> = BTreeMap::new();
    for (k, m) in kinds.iter().zip(micros) {
        *per_kind_micros.entry(k.as_str()).or_default() += *m;
    }
    let total_micros: u64 = micros.iter().sum();

    json!({
        "case_index": case_index,
        "n_transitions": kinds.len(),
        // Wiring is `Serialize`, so this is its canonical stable shape:
        // {"storage_adapters":["Loro",...],"sync_adapters":[...],"actors":[...]}.
        "wiring": serde_json::to_value(wiring).expect("wiring serializes"),
        "transition_kinds": serde_json::to_value(&kind_counts).expect("kind counts serialize"),
        "distinct_bigrams": serde_json::to_value(&bigrams).expect("bigrams serialize"),
        "engagement": engagement_json,
        "wall": {
            "total_us": total_micros,
            "per_kind_us":
                serde_json::to_value(&per_kind_micros).expect("per-kind micros serialize"),
        },
    })
}

/// Emit one telemetry line to stderr under the `[pbt-telemetry]` prefix (the
/// grep key the analysis scripts split on). Compact single-line JSON.
pub(crate) fn emit(value: &Value) {
    eprintln!("[pbt-telemetry] {value}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_wiring() -> Wiring {
        Wiring::custom(
            [
                holon_pbt_core::StorageAdapter::Loro,
                holon_pbt_core::StorageAdapter::Turso,
            ],
            [],
            [holon_pbt_core::Actor::UI],
        )
    }

    fn sample_engagement() -> BTreeMap<&'static str, EngageTally> {
        let mut m = BTreeMap::new();
        m.insert(
            "inv-viewmodel-entity-ids-subset-of-data",
            EngageTally { engaged: 3, skipped: 1 },
        );
        m.insert("inv-block-parent-validity", EngageTally { engaged: 0, skipped: 4 });
        m
    }

    /// The emitted line parses back as JSON and carries the whole stable-shaped
    /// schema — the contract the analysis scripts rely on.
    #[test]
    fn line_parses_and_is_stable_shaped() {
        let kinds: Vec<String> =
            ["Indent", "Split", "Indent", "Outdent"].iter().map(|s| s.to_string()).collect();
        let micros = vec![10u64, 20, 30, 40];
        let value = build_line(7, &sample_wiring(), &kinds, &micros, &sample_engagement());

        // Round-trips through a string exactly like `emit` -> log -> parser.
        let text = value.to_string();
        let parsed: Value = serde_json::from_str(&text).expect("emitted line is valid JSON");

        assert_eq!(parsed["case_index"], 7);
        assert_eq!(parsed["n_transitions"], 4);
        assert_eq!(parsed["wiring"]["storage_adapters"], json!(["Loro", "Turso"]));
        assert_eq!(parsed["wiring"]["actors"], json!(["UI"]));
        assert_eq!(parsed["transition_kinds"]["Indent"], 2);
        assert_eq!(parsed["transition_kinds"]["Split"], 1);
        // Distinct adjacent pairs: Indent>Split, Split>Indent, Indent>Outdent.
        assert_eq!(
            parsed["distinct_bigrams"],
            json!(["Indent>Outdent", "Indent>Split", "Split>Indent"])
        );
        assert_eq!(parsed["engagement"]["inv-viewmodel-entity-ids-subset-of-data"], json!([3, 4]));
        assert_eq!(parsed["engagement"]["inv-block-parent-validity"], json!([0, 4]));
        // Wall aggregates are present and separable from the deterministic body.
        assert_eq!(parsed["wall"]["total_us"], 100);
        assert!(parsed["wall"]["per_kind_us"].is_object());
    }

    /// Non-`wall` content is a pure function of the (seed-determined) inputs:
    /// same transitions + wiring + engagement -> byte-identical body, even when
    /// the wall micros differ between the two "runs".
    #[test]
    fn non_wall_content_is_deterministic_across_differing_timings() {
        let kinds: Vec<String> =
            ["Indent", "Split", "Indent"].iter().map(|s| s.to_string()).collect();
        let run_a = build_line(0, &sample_wiring(), &kinds, &[1, 2, 3], &sample_engagement());
        let run_b =
            build_line(0, &sample_wiring(), &kinds, &[999, 7, 88_000], &sample_engagement());

        for key in ["case_index", "n_transitions", "wiring", "transition_kinds", "distinct_bigrams", "engagement"] {
            assert_eq!(run_a[key], run_b[key], "field {key} must be timing-independent");
        }
        assert_ne!(run_a["wall"], run_b["wall"], "wall fields track the (differing) timings");
    }
}
