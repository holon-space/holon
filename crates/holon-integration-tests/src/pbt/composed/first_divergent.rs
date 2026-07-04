//! First-divergent-layer verdict for the composed keystone failure header.
//!
//! When the keystone reds, several invariants across the stack usually fail at
//! once: a store/CRDT divergence re-projects into the matview, which re-derives
//! into the ViewModel, which re-renders — so the raw `{hard:?}` list buries the
//! ROOT layer under its downstream echoes. This module computes the EARLIEST
//! (most upstream) layer that diverged, so triage starts at the root instead of
//! hand-localizing.
//!
//! The layer ordering is the single-edit data-flow pipeline — the same order
//! `docs/Architecture/Model.md`'s five layers settle in for one interaction:
//! store/CRDT → matview/SQL → viewmodel → render → org round-trip. A divergence
//! at layer N forces every layer > N to diverge too, so the min-layer failure
//! is the one to chase.
//!
//! Every id arrives already attributed: a
//! [`CapInvariant`](holon_pbt_core::composition::CapInvariant) cannot be built
//! without an [`Attribution`], and `run_selected` records one per registry
//! entry. There is no map to consult, so there is no unmapped id to guess at.
//!
//! COVERAGE, not assumption: "first divergent" only means anything if the
//! layers beneath were actually observed. Absence from the failing list is NOT
//! evidence of health — an invariant can be absent because its cap was
//! DESELECTED (windowed `CapMap`s carry no store/SQL/org caps at all), because
//! its body ran but observed nothing (SKIPPED), or because its failure was
//! SOFTENED out via `HOLON_PBT_INVARIANTS=<id>:warn`. [`RunCoverage`] carries
//! those dispositions in, and the verdict names every unverified layer as a
//! blind spot instead of calling it green.

use holon_pbt_core::attribution::ALL_LAYERS;
use holon_pbt_core::attribution::Attribution;
use holon_pbt_core::attribution::Layer;

/// One dispositioned id together with the attribution its wiring declared.
type Attributed = (&'static str, Attribution);

/// Why an invariant produced no evidence about its layer this run. Every
/// variant means "we did NOT observe this layer", never "this layer is fine".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Unverified {
    /// The cap wasn't in the `CapMap`, so selection never ran the body — the
    /// windowed `CapMap` deselects the whole store/SQL/org family this way.
    Deselected,
    /// The body ran but suppressed itself: it observed nothing.
    Skipped,
    /// The body ran AND failed, but `HOLON_PBT_INVARIANTS=<id>:warn|skip`
    /// demoted the failure. The layer is known-red, not green.
    SoftenedOut,
    /// No id at this layer appears in the run at all — the registry that ran
    /// carried none. No evidence.
    NotInRun,
}

impl Unverified {
    fn label(self) -> &'static str {
        match self {
            Unverified::Deselected => "deselected (cap absent)",
            Unverified::Skipped => "skipped (observed nothing)",
            Unverified::SoftenedOut => "failure SOFTENED out",
            Unverified::NotInRun => "absent from the run report",
        }
    }
}

/// How thoroughly one layer was observed. Only [`LayerCoverage::Verified`] —
/// EVERY dispositioned id at the layer measured, none softened — licenses the
/// word "green".
#[derive(Debug, Clone, PartialEq, Eq)]
enum LayerCoverage {
    Verified,
    /// Some ids measured clean, others never produced evidence. NOT green:
    /// the unmeasured ones are the blind spot.
    Partial {
        measured: usize,
        total: usize,
        reasons: Vec<Unverified>,
    },
    /// Nothing at this layer produced evidence.
    Unverified(Vec<Unverified>),
}

/// What this run actually observed, per invariant id. Built from the
/// [`RunReport`](holon_pbt_core::composition::RunReport) plus the ids whose
/// failures the disclosed-softening knob filtered out, so the verdict can tell
/// "measured and clean" apart from "never looked".
#[derive(Debug, Default, Clone)]
pub struct RunCoverage {
    /// Ran with a non-`Skipped` verdict: this id genuinely observed its layer.
    pub measured: Vec<Attributed>,
    pub skipped: Vec<Attributed>,
    pub deselected: Vec<Attributed>,
    /// Failed, then demoted by `HOLON_PBT_INVARIANTS`.
    pub softened_out: Vec<Attributed>,
}

impl RunCoverage {
    /// Derive coverage from a run. `softened_out` is the ids whose `Fail` the
    /// caller filtered out of `hard` — they must be passed in, not dropped,
    /// or their layer would masquerade as clean.
    pub fn from_report(
        report: &holon_pbt_core::composition::RunReport,
        softened_out: Vec<&'static str>,
    ) -> Self {
        use holon_pbt_core::composition::InvariantResult;
        // `run_selected` records one attribution per registry entry, so every
        // dispositioned id resolves; a miss is a runner bug, not a missing map.
        let attribution_of = |id: &'static str| -> Attribution {
            report
                .attributions
                .iter()
                .find(|(known, _)| known.0 == id)
                .unwrap_or_else(|| {
                    panic!("dispositioned id {id} carries no attribution in the run report")
                })
                .1
        };
        let mut measured = Vec::new();
        let mut skipped = Vec::new();
        let mut softened = Vec::new();
        for (id, result) in &report.ran {
            let entry = (id.0, attribution_of(id.0));
            if softened_out.contains(&id.0) {
                softened.push(entry);
                continue;
            }
            match result {
                InvariantResult::Skipped(_) => skipped.push(entry),
                InvariantResult::Ok | InvariantResult::Fail(_) => measured.push(entry),
            }
        }
        Self {
            measured,
            skipped,
            deselected: report
                .deselected
                .iter()
                .map(|id| (id.0, attribution_of(id.0)))
                .collect(),
            softened_out: softened,
        }
    }

    /// The attribution a failing id declared at its wiring site. Every id in
    /// `hard` came from `report.ran`, so a miss is a caller bug.
    fn attribution_of(&self, id: &str) -> Attribution {
        [
            &self.measured,
            &self.skipped,
            &self.deselected,
            &self.softened_out,
        ]
        .into_iter()
        .flatten()
        .find(|(known, _)| *known == id)
        .unwrap_or_else(|| panic!("failing id {id} was not dispositioned by this run"))
        .1
    }

    /// How well this run covered one layer. ONE measured invariant is not a
    /// clean bill of health for a layer that has five: the four that never ran
    /// are exactly where the divergence could be hiding, so partial coverage is
    /// its own verdict and never folds into [`LayerCoverage::Verified`].
    ///
    /// The denominator is the layer's ids that this run has ANY disposition for
    /// (`ran` ∪ `deselected`), and that set IS the whole registry at the layer:
    /// `run_selected` (`holon-pbt-core/src/composition.rs`) walks every entry
    /// it is handed, records its attribution, and pushes it into exactly
    /// one of `ran` / `deselected`. Each id carries its own layer here, so
    /// no lookup can miss and no denominator can be understated.
    ///
    /// A layer with NO disposition at all is [`Unverified::NotInRun`].
    fn coverage_of(&self, layer: Layer) -> LayerCoverage {
        let count = |ids: &[Attributed]| -> usize {
            ids.iter().filter(|(_, a)| a.layer() == Some(layer)).count()
        };
        let measured = count(&self.measured);
        let softened = count(&self.softened_out);
        let deselected = count(&self.deselected);
        let skipped = count(&self.skipped);
        let total = measured + softened + deselected + skipped;

        let mut reasons = Vec::new();
        if softened > 0 {
            reasons.push(Unverified::SoftenedOut);
        }
        if deselected > 0 {
            reasons.push(Unverified::Deselected);
        }
        if skipped > 0 {
            reasons.push(Unverified::Skipped);
        }
        if total == 0 {
            return LayerCoverage::Unverified(vec![Unverified::NotInRun]);
        }
        if reasons.is_empty() {
            return LayerCoverage::Verified;
        }
        if measured == 0 {
            return LayerCoverage::Unverified(reasons);
        }
        LayerCoverage::Partial {
            measured,
            total,
            reasons,
        }
    }
}

/// Build the divergent-layer verdict + wiring line(s) for a set of hard
/// (unsoftened) invariant failures. `hard` is `(id, message)` — the same list
/// the keystone asserts is empty. `coverage` is what the run actually observed;
/// it decides whether the failing layer may be called FIRST-divergent (every
/// layer below verified) or only the lowest MEASURED failing one (a layer below
/// was never observed). Returns an empty string when `hard` is empty.
pub fn first_divergent_verdict(hard: &[(&str, &str)], coverage: &RunCoverage) -> String {
    if hard.is_empty() {
        return String::new();
    }
    let mut layered: Vec<(Layer, &str, &str)> = Vec::new(); // (layer, id, source)
    let mut cross_cutting: Vec<&str> = Vec::new();
    for (id, _) in hard {
        let attribution = coverage.attribution_of(id);
        match attribution.layer() {
            Some(layer) => layered.push((layer, id, attribution.wiring())),
            None => cross_cutting.push(id),
        }
    }

    let mut out = String::new();
    if let Some(first) = layered.iter().map(|(l, _, _)| *l).min() {
        let in_layer: Vec<&str> = layered
            .iter()
            .filter(|(l, _, _)| *l == first)
            .map(|(_, id, _)| *id)
            .collect();
        // Every co-equal min-layer id gets its own wiring pointer — with N ids
        // sharing the layer, one pointer sends triage to an arbitrary 1-of-N.
        let sources: Vec<&str> = layered
            .iter()
            .filter(|(l, _, _)| *l == first)
            .map(|(_, _, s)| *s)
            .collect();
        assert!(
            !sources.is_empty(),
            "min layer came from `layered`, so an entry exists",
        );
        // Below the failing layer, split FULLY verified from partially covered
        // from never observed. Only full coverage licenses "green"; partial
        // coverage carries its counts so it can't be read as full.
        let reason_list = |reasons: &[Unverified]| -> String {
            reasons
                .iter()
                .map(|r| r.label())
                .collect::<Vec<_>>()
                .join(" + ")
        };
        let mut green: Vec<&'static str> = Vec::new();
        let mut partial: Vec<String> = Vec::new();
        let mut blind: Vec<String> = Vec::new();
        for layer in ALL_LAYERS.iter().filter(|l| **l < first) {
            match coverage.coverage_of(*layer) {
                LayerCoverage::Verified => green.push(layer.label()),
                LayerCoverage::Partial {
                    measured,
                    total,
                    reasons,
                } => partial.push(format!(
                    "{} ({measured}/{total} measured; rest {})",
                    layer.label(),
                    reason_list(&reasons),
                )),
                LayerCoverage::Unverified(reasons) => {
                    blind.push(format!("{} [{}]", layer.label(), reason_list(&reasons)))
                }
            }
        }
        if blind.is_empty() && partial.is_empty() {
            let below = if green.is_empty() {
                "nothing below it".to_string()
            } else {
                format!("verified green below: {}", green.join(", "))
            };
            out.push_str(&format!(
                "first-divergent-layer: {} ({}); {below}",
                first.label(),
                in_layer.join(", "),
            ));
        } else {
            let mut notes = Vec::new();
            if !blind.is_empty() {
                notes.push(format!("UNVERIFIED below it: {}", blind.join(", ")));
            }
            if !partial.is_empty() {
                notes.push(format!(
                    "PARTIALLY verified below it (NOT green): {}",
                    partial.join(", "),
                ));
            }
            if !green.is_empty() {
                notes.push(format!("fully verified green below: {}", green.join(", ")));
            }
            out.push_str(&format!(
                "lowest MEASURED failing layer: {} ({}) — NOT proven first-divergent: {}",
                first.label(),
                in_layer.join(", "),
                notes.join("; "),
            ));
        }
        out.push_str(&format!("\n  ↳ wiring: {}", sources.join("; ")));
    } else {
        out.push_str(
            "first-divergent-layer: none — every diverged invariant is \
             cross-cutting (no pipeline layer)",
        );
    }
    if !cross_cutting.is_empty() {
        out.push_str(&format!(
            "\n  cross-cutting (no pipeline layer): {}",
            cross_cutting.join(", "),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dispositioned id at a pipeline layer. These tests pin the verdict's
    /// LOGIC, so the wiring string is a fixture, not a catalog fact.
    fn at(id: &'static str, layer: Layer) -> Attributed {
        (id, Attribution::at(layer, "test/wiring.rs"))
    }

    /// A dispositioned id with a distinguishable wiring pointer, for asserting
    /// that co-equal failures each get their own.
    fn wired(id: &'static str, layer: Layer, wiring: &'static str) -> Attributed {
        (id, Attribution::at(layer, wiring))
    }

    fn cross(id: &'static str) -> Attributed {
        (id, Attribution::cross_cutting("test/wiring.rs"))
    }

    /// Coverage where every pipeline layer was genuinely measured — one
    /// representative id per layer, all with a non-`Skipped` verdict.
    fn fully_measured() -> RunCoverage {
        RunCoverage {
            measured: vec![
                at("inv-no-orphan-blocks", Layer::StoreCrdt),
                at("inv-matview-consistent-with-recompute", Layer::Projection),
                at("inv-viewmodel-snapshot", Layer::ViewModel),
                at("inv-frontend-bounds-rendered", Layer::Render),
                at("inv-org-render-fixed-point", Layer::OrgRoundTrip),
            ],
            ..RunCoverage::default()
        }
    }

    #[test]
    fn empty_hard_yields_empty_verdict() {
        assert_eq!(first_divergent_verdict(&[], &fully_measured()), "");
    }

    /// The windowed counterexample: the windowed `CapMap` has no
    /// `SutBlocks`/SQL/Loro/org caps, so the whole store + matview family
    /// DESELECTS and a render failure is the only thing that could red. The
    /// verdict must not call the never-observed layers green.
    #[test]
    fn deselected_lower_layers_are_a_disclosed_blind_spot_not_green() {
        let coverage = RunCoverage {
            measured: vec![
                at("inv-frontend-bounds-rendered", Layer::Render),
                at("inv-displayed-text/widget", Layer::Render),
                at("inv-viewmodel-snapshot", Layer::ViewModel),
            ],
            deselected: vec![
                at("inv-no-orphan-blocks", Layer::StoreCrdt),
                at("inv-matview-consistent-with-recompute", Layer::Projection),
                at("inv-org-render-fixed-point", Layer::OrgRoundTrip),
            ],
            ..RunCoverage::default()
        };
        let hard = &[
            ("inv-frontend-bounds-rendered", "geometry mismatch"),
            ("inv-displayed-text/widget", "text mismatch"),
        ][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(!v.contains("layers below green"), "got: {v}");
        assert!(
            v.starts_with("lowest MEASURED failing layer: render ("),
            "got: {v}"
        );
        assert!(v.contains("NOT proven first-divergent"), "got: {v}");
        for blind in ["store/CRDT [deselected", "matview/SQL [deselected"] {
            assert!(v.contains(blind), "missing {blind} in: {v}");
        }
        // The one layer below that DID run stays reported as green.
        assert!(v.contains("verified green below: viewmodel"), "got: {v}");
        // …and is never listed as unverified.
        assert!(!v.contains("viewmodel ["), "got: {v}");
    }

    /// A store-layer failure demoted by `HOLON_PBT_INVARIANTS=<id>:warn` leaves
    /// the store layer KNOWN-RED, not clean — the matview verdict above it must
    /// not claim otherwise.
    #[test]
    fn softened_out_layer_is_never_reported_green() {
        let coverage = RunCoverage {
            measured: vec![at(
                "inv-matview-consistent-with-recompute",
                Layer::Projection,
            )],
            softened_out: vec![at("inv-no-orphan-blocks", Layer::StoreCrdt)],
            ..RunCoverage::default()
        };
        let hard = &[("inv-matview-consistent-with-recompute", "row drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(!v.contains("layers below green"), "got: {v}");
        assert!(
            !v.contains("verified green below: store/CRDT"),
            "softened store layer claimed green: {v}"
        );
        assert!(v.contains("store/CRDT [failure SOFTENED out]"), "got: {v}");
        assert!(v.contains("NOT proven first-divergent"), "got: {v}");
    }

    /// The signal is not merely deleted: when every lower layer really ran
    /// clean, the verdict still says so and keeps the "first-divergent" claim.
    #[test]
    fn genuinely_measured_clean_lower_layers_are_reported_green() {
        let hard = &[("inv-frontend-bounds-rendered", "geometry mismatch")][..];
        let v = first_divergent_verdict(hard, &fully_measured());
        assert!(
            v.starts_with("first-divergent-layer: render (inv-frontend-bounds-rendered);"),
            "got: {v}"
        );
        assert!(
            v.contains("verified green below: store/CRDT, matview/SQL, viewmodel"),
            "got: {v}"
        );
        assert!(!v.contains("UNVERIFIED"), "got: {v}");
    }

    /// A skipped (vacuous) body observed nothing — same blind spot as a
    /// deselect, and it must be named as such.
    #[test]
    fn skipped_lower_layer_is_unverified() {
        let coverage = RunCoverage {
            measured: vec![
                at("inv-viewmodel-snapshot", Layer::ViewModel),
                at("inv-matview-consistent-with-recompute", Layer::Projection),
            ],
            skipped: vec![at("inv-no-orphan-blocks", Layer::StoreCrdt)],
            ..RunCoverage::default()
        };
        let hard = &[("inv-viewmodel-snapshot", "vm drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            v.contains("store/CRDT [skipped (observed nothing)]"),
            "got: {v}"
        );
        assert!(v.contains("verified green below: matview/SQL"), "got: {v}");
    }

    /// A layer that appears in neither `ran` nor `deselected` yields no
    /// evidence at all — disclosed, never assumed green.
    #[test]
    fn layer_absent_from_the_run_report_is_unverified() {
        let coverage = RunCoverage {
            measured: vec![at("inv-viewmodel-snapshot", Layer::ViewModel)],
            ..RunCoverage::default()
        };
        let hard = &[("inv-viewmodel-snapshot", "vm drift")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            v.contains("store/CRDT [absent from the run report]"),
            "got: {v}"
        );
        assert!(
            v.contains("matview/SQL [absent from the run report]"),
            "got: {v}"
        );
    }

    /// `RunCoverage::from_report` is the only builder the call sites use:
    /// `Ok`/`Fail` count as measured, `Skipped` does not, softened ids move out
    /// of `measured` into their own bucket — each carrying the attribution the
    /// runner recorded for it.
    #[test]
    fn coverage_from_report_classifies_every_disposition() {
        use holon_pbt_core::composition::InvariantId;
        use holon_pbt_core::composition::InvariantResult;
        use holon_pbt_core::composition::RunReport;
        let report = RunReport {
            ran: vec![
                (InvariantId("inv-viewmodel-snapshot"), InvariantResult::Ok),
                (
                    InvariantId("inv-matview-consistent-with-recompute"),
                    InvariantResult::Fail("drift".into()),
                ),
                (
                    InvariantId("inv-org-render-fixed-point"),
                    InvariantResult::Skipped("no org files".into()),
                ),
                (
                    InvariantId("inv-no-orphan-blocks"),
                    InvariantResult::Fail("softened".into()),
                ),
            ],
            deselected: vec![InvariantId("inv-frontend-bounds-rendered")],
            attributions: vec![
                (
                    InvariantId("inv-viewmodel-snapshot"),
                    Attribution::at(Layer::ViewModel, "test/wiring.rs"),
                ),
                (
                    InvariantId("inv-matview-consistent-with-recompute"),
                    Attribution::at(Layer::Projection, "test/wiring.rs"),
                ),
                (
                    InvariantId("inv-org-render-fixed-point"),
                    Attribution::at(Layer::OrgRoundTrip, "test/wiring.rs"),
                ),
                (
                    InvariantId("inv-no-orphan-blocks"),
                    Attribution::at(Layer::StoreCrdt, "test/wiring.rs"),
                ),
                (
                    InvariantId("inv-frontend-bounds-rendered"),
                    Attribution::at(Layer::Render, "test/wiring.rs"),
                ),
            ],
        };
        let coverage = RunCoverage::from_report(&report, vec!["inv-no-orphan-blocks"]);
        let ids = |b: &[Attributed]| -> Vec<&'static str> { b.iter().map(|(id, _)| *id).collect() };
        assert_eq!(
            ids(&coverage.measured),
            vec![
                "inv-viewmodel-snapshot",
                "inv-matview-consistent-with-recompute"
            ]
        );
        assert_eq!(ids(&coverage.skipped), vec!["inv-org-render-fixed-point"]);
        assert_eq!(
            ids(&coverage.deselected),
            vec!["inv-frontend-bounds-rendered"]
        );
        assert_eq!(ids(&coverage.softened_out), vec!["inv-no-orphan-blocks"]);
        assert_eq!(
            coverage.coverage_of(Layer::ViewModel),
            LayerCoverage::Verified
        );
        assert_eq!(
            coverage.coverage_of(Layer::StoreCrdt),
            LayerCoverage::Unverified(vec![Unverified::SoftenedOut])
        );
        assert_eq!(
            coverage.coverage_of(Layer::Render),
            LayerCoverage::Unverified(vec![Unverified::Deselected])
        );
        assert_eq!(
            coverage.coverage_of(Layer::OrgRoundTrip),
            LayerCoverage::Unverified(vec![Unverified::Skipped])
        );
    }

    /// ONE measured invariant does not clear a layer that has more. A layer
    /// with a clean measured id AND a deselected id is PARTIAL — it must stay
    /// out of the green list, disclose its counts, and (because a layer below
    /// is only partly covered) strip the "first-divergent" claim.
    #[test]
    fn partially_covered_layer_is_not_green() {
        let coverage = RunCoverage {
            measured: vec![
                // store/CRDT: 1 of 2 dispositioned ids measured…
                at("inv-no-orphan-blocks", Layer::StoreCrdt),
                at("inv-matview-consistent-with-recompute", Layer::Projection),
                at("inv-viewmodel-snapshot", Layer::ViewModel),
                at("inv-frontend-bounds-rendered", Layer::Render),
            ],
            // …the other one — the one that would have caught the divergence —
            // never ran.
            deselected: vec![at("inv-no-parent-cycles", Layer::StoreCrdt)],
            ..RunCoverage::default()
        };
        assert_eq!(
            coverage.coverage_of(Layer::StoreCrdt),
            LayerCoverage::Partial {
                measured: 1,
                total: 2,
                reasons: vec![Unverified::Deselected],
            }
        );
        let hard = &[("inv-frontend-bounds-rendered", "geometry mismatch")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            !v.contains("green below: store/CRDT"),
            "partially covered layer claimed green: {v}"
        );
        assert!(
            v.contains(
                "PARTIALLY verified below it (NOT green): store/CRDT (1/2 measured; rest \
                        deselected (cap absent))"
            ),
            "got: {v}"
        );
        assert!(v.contains("NOT proven first-divergent"), "got: {v}");
        // The fully-covered layers below are still reported as green.
        assert!(
            v.contains("fully verified green below: matview/SQL, viewmodel"),
            "got: {v}"
        );
    }

    #[test]
    fn picks_the_most_upstream_layer() {
        // A store divergence echoing up into projection + viewmodel: the verdict
        // must name store/CRDT (the root), not the downstream echoes.
        let coverage = RunCoverage {
            measured: vec![
                wired(
                    "inv-no-orphan-blocks",
                    Layer::StoreCrdt,
                    "crates/holon-integration-tests/src/pbt/composed/invariants/no_orphan.rs",
                ),
                at("inv-matview-consistent-with-recompute", Layer::Projection),
                at("inv-viewmodel-entity-ids-subset-of-data", Layer::ViewModel),
            ],
            ..RunCoverage::default()
        };
        let hard = &[
            ("inv-viewmodel-entity-ids-subset-of-data", "downstream echo"),
            ("inv-matview-consistent-with-recompute", "projection echo"),
            ("inv-no-orphan-blocks", "the root divergence"),
        ][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            v.starts_with("first-divergent-layer: store/CRDT (inv-no-orphan-blocks)"),
            "got: {v}"
        );
        assert!(v.contains("nothing below it"), "got: {v}");
        assert!(
            v.contains(
                "↳ wiring: crates/holon-integration-tests/src/pbt/composed/invariants/\
                 no_orphan.rs"
            ),
            "got: {v}"
        );
    }

    #[test]
    fn groups_co_equal_layer_failures_and_discloses_cross_cutting() {
        let coverage = RunCoverage {
            measured: vec![
                at("inv-no-orphan-blocks", Layer::StoreCrdt),
                wired(
                    "inv-matview-consistent-with-recompute",
                    Layer::Projection,
                    "matview_recompute_matches.rs",
                ),
                wired(
                    "inv-block-content/sql",
                    Layer::Projection,
                    "turso/correspondences.rs",
                ),
                cross("inv-no-errors"),
            ],
            ..RunCoverage::default()
        };
        let hard = &[
            ("inv-matview-consistent-with-recompute", "m1"),
            ("inv-block-content/sql", "m2"),
            ("inv-no-errors", "a swallowed error"),
        ][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(
            v.starts_with("first-divergent-layer: matview/SQL ("),
            "got: {v}"
        );
        // Every co-equal min-layer id gets its own wiring pointer, not just the
        // first one found.
        assert!(
            v.contains("↳ wiring: matview_recompute_matches.rs; turso/correspondences.rs"),
            "got: {v}"
        );
        assert!(
            v.contains("inv-matview-consistent-with-recompute"),
            "got: {v}"
        );
        assert!(v.contains("inv-block-content/sql"), "got: {v}");
        assert!(
            v.contains("cross-cutting (no pipeline layer): inv-no-errors"),
            "got: {v}"
        );
    }

    #[test]
    fn only_cross_cutting_reports_no_pipeline_layer() {
        let coverage = RunCoverage {
            measured: vec![cross("inv-no-errors")],
            ..RunCoverage::default()
        };
        let hard = &[("inv-no-errors", "x")][..];
        let v = first_divergent_verdict(hard, &coverage);
        assert!(v.starts_with("first-divergent-layer: none"), "got: {v}");
        assert!(
            v.contains("cross-cutting (no pipeline layer): inv-no-errors"),
            "got: {v}"
        );
    }
}
