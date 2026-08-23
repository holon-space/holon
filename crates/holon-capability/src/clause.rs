//! Every CLAUSE a profile can state, as a closed vocabulary.
//!
//! This exists so "driven or marked" can be enforced by CODE. A `#`-comment
//! saying `NOT YET CERTIFIED` is invisible to the compiler and to the
//! certifier, so it could never gate anything — the marker has to be DATA. A
//! profile therefore declares `not_yet_certified: [..]` as a real list, and the
//! certifier FAILS on any clause that is neither probed by the format's impl
//! nor named there.
//!
//! What this prevents is the exact defect 2b.1 shipped: six clauses whose
//! citations read like guarantees, which no case exercised, and which a
//! reviewer could make false without turning the suite red.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

/// One clause of one axis.
///
/// NOT every yaml field is here. `property_keys.reserved_keys` is deliberately
/// absent: it is an INPUT that tells the certifier which keys to exclude from
/// the ordinary-property law, not a claim the law can range over. A wrong entry
/// there shrinks coverage rather than stating a falsehood, so "drive it or mark
/// it" is not the right discipline for it — keeping it minimal and justified
/// per key is (see the org yaml's note on that field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseId {
    // axis 1
    HostedKinds,
    // axis 2
    ContentRepresentation,
    ContentInlineConstructs,
    ContentBlockConstructs,
    // axis 3
    PropertyKeysCharset,
    PropertyKeysCase,
    PropertyKeysReservedPrefixes,
    PropertyKeysEngineOwnedKeys,
    PropertyKeysCollision,
    PropertyKeysSchemaRequired,
    // axis 4
    PropertyValuesTypes,
    PropertyValuesEmptyString,
    PropertyValuesNull,
    PropertyValuesMultiValue,
    PropertyValuesReferenceValues,
    // axis 5
    OrderingSiblingOrder,
    OrderingOrderKeyDurable,
    OrderingConcurrentInsert,
    OrderingPropertyOrder,
    // axis 6
    HierarchyShape,
    HierarchyMaxDepth,
    HierarchyReparent,
    HierarchyConstraints,
    HierarchyCycles,
    // axis 7
    IdentityIdSpace,
    IdentityIdOrigin,
    IdentityIdConstraints,
    IdentityRenameStability,
    IdentityCarriers,
    IdentityCarrierDisagreement,
    // axis 8
    ComputedLive,
    ComputedPersisted,
    ComputedExpressionClosure,
    // axis 9
    MutationWriteLeg,
    MutationUnitOfWrite,
    MutationMergeGranularity,
    MutationConflictSurface,
    // axis 10
    AssetsAttachments,
    AssetsBinaryInline,
    AssetsExtensions,
    // axis 11
    TagsAttachExisting,
    TagsDetachExisting,
    TagsResolutionRefusesUnknown,
    TagsMintNew,
}

impl std::fmt::Display for ClauseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The yaml spelling, so a finding names the line the author must edit.
        let s = serde_yaml::to_string(self).map_err(|_| std::fmt::Error)?;
        f.write_str(s.trim())
    }
}

/// Every clause the vocabulary knows. The coverage law ranges over exactly
/// this list, so adding an axis field without adding it here would silently
/// exempt the new field — which is why the exhaustiveness test below exists.
pub const ALL_CLAUSES: &[ClauseId] = &[
    ClauseId::HostedKinds,
    ClauseId::ContentRepresentation,
    ClauseId::ContentInlineConstructs,
    ClauseId::ContentBlockConstructs,
    ClauseId::PropertyKeysCharset,
    ClauseId::PropertyKeysCase,
    ClauseId::PropertyKeysReservedPrefixes,
    ClauseId::PropertyKeysEngineOwnedKeys,
    ClauseId::PropertyKeysCollision,
    ClauseId::PropertyKeysSchemaRequired,
    ClauseId::PropertyValuesTypes,
    ClauseId::PropertyValuesEmptyString,
    ClauseId::PropertyValuesNull,
    ClauseId::PropertyValuesMultiValue,
    ClauseId::PropertyValuesReferenceValues,
    ClauseId::OrderingSiblingOrder,
    ClauseId::OrderingOrderKeyDurable,
    ClauseId::OrderingConcurrentInsert,
    ClauseId::OrderingPropertyOrder,
    ClauseId::HierarchyShape,
    ClauseId::HierarchyMaxDepth,
    ClauseId::HierarchyReparent,
    ClauseId::HierarchyConstraints,
    ClauseId::HierarchyCycles,
    ClauseId::IdentityIdSpace,
    ClauseId::IdentityIdOrigin,
    ClauseId::IdentityIdConstraints,
    ClauseId::IdentityRenameStability,
    ClauseId::IdentityCarriers,
    ClauseId::IdentityCarrierDisagreement,
    ClauseId::ComputedLive,
    ClauseId::ComputedPersisted,
    ClauseId::ComputedExpressionClosure,
    ClauseId::MutationWriteLeg,
    ClauseId::MutationUnitOfWrite,
    ClauseId::MutationMergeGranularity,
    ClauseId::MutationConflictSurface,
    ClauseId::AssetsAttachments,
    ClauseId::AssetsBinaryInline,
    ClauseId::AssetsExtensions,
    ClauseId::TagsAttachExisting,
    ClauseId::TagsDetachExisting,
    ClauseId::TagsResolutionRefusesUnknown,
    ClauseId::TagsMintNew,
];

/// WHO enforces a clause.
///
/// The vocabulary was missing this dimension until a `hierarchy.constraints`
/// probe reported a real rule as unenforced: the rule lives in
/// `DocumentManager::name_chain` (`crates/holon-filesystem`), one layer above
/// the format crate a certifier can reach. Without a layer, a reader cannot
/// tell a FORMAT LIE from a LAYER GAP — and hiding the second under
/// "not yet certified" would reproduce exactly the defect that made the
/// coverage law necessary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementLayer {
    /// The parse/render code of the format crate itself. Driveable by a
    /// format-crate certification harness — the only layer 2b.2 covers.
    Format,
    /// The sync controller / consolidator: renames, merges, conflict
    /// surfacing, structural refusals that need vault context.
    Sync,
    /// The OPERATION layer — `BlockOperations` and the block-op catalog, which
    /// refuse an edit BEFORE it reaches a file. Distinct from `Sync`: a
    /// pre-flight refusal leaves the tree untouched and never observes a
    /// resolution. It exists because `hierarchy_reparent` was labelled `sync`
    /// with a note admitting the label was approximate, and a label the
    /// coverage law reads must not be approximate.
    Operation,
    /// Type declaration: refused when the type is declared, not when a value
    /// is written (`declare_type`).
    Declaration,
}

/// Which layer's harness may certify a clause, for every clause.
///
/// Stored grouped by layer rather than as 39 per-clause lines: the parser
/// asserts the union is EXACTLY `ALL_CLAUSES`, so the completeness the ruling
/// demands is enforced without the verbosity that would make authors
/// copy-paste it wrong.
/// A deferral must NAME the code that enforces it.
///
/// A deferral without a site is indistinguishable from a clause nobody has
/// looked at: it reads like a considered layer decision while asserting
/// nothing checkable. Requiring `site:` is what makes "another layer owns this"
/// a claim a reader can verify rather than a place to park work.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeferralSite {
    pub clause: ClauseId,
    /// `path:line` (or `path:fn`) of the code that actually enforces it.
    pub site: String,
}

/// A `not_yet_certified` marker must say WHY nothing drives the clause.
///
/// Same argument as `DeferralSite`: a bare marker reads like a considered
/// decision while asserting nothing. The reason is what a later reader needs
/// to know whether the obstacle still stands — "not discriminable, because the
/// separator axis consumes the discriminating shape" is checkable; silence is
/// not.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub clause: ClauseId,
    /// What was measured, and why it did not settle the clause.
    pub reason: String,
}

/// A clause certified against a MOVING upstream.
///
/// LogSeq's datascript pin moved between schema 65.12 and 65.33 and its script
/// oracles became a compiled CLI, so a clause certified there is a snapshot,
/// not a stable fact. `certified_against` names the range the measurement
/// holds for. This is NOT a second escape hatch: a provisional clause must
/// still be DRIVEN — it is driven-with-expiry — and a provisional clause that
/// nothing drives is a gap like any other.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provisional {
    pub clause: ClauseId,
    /// The upstream range the measurement was taken against.
    pub certified_against: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnforcementMap {
    #[serde(default)]
    pub format: BTreeSet<ClauseId>,
    #[serde(default)]
    pub sync: Vec<DeferralSite>,
    #[serde(default)]
    pub operation: Vec<DeferralSite>,
    #[serde(default)]
    pub declaration: Vec<DeferralSite>,
}

impl EnforcementMap {
    /// Every clause must appear EXACTLY once. Missing means a clause with no
    /// stated owner; duplicated means two owners — both are load errors, not
    /// defaults, because a silent default is how the layer dimension would rot
    /// back into invisibility.
    pub fn check_total(&self) -> Result<(), String> {
        // A blank site is the same failure as a missing one.
        for d in self
            .sync
            .iter()
            .chain(self.operation.iter())
            .chain(self.declaration.iter())
        {
            if d.site.trim().is_empty() {
                return Err(format!(
                    "{} is deferred to another layer but names no enforcing site",
                    d.clause
                ));
            }
        }

        let mut seen: Vec<ClauseId> = Vec::new();
        seen.extend(self.format.iter().copied());
        seen.extend(self.sync.iter().map(|d| d.clause));
        seen.extend(self.operation.iter().map(|d| d.clause));
        seen.extend(self.declaration.iter().map(|d| d.clause));

        let unique: BTreeSet<ClauseId> = seen.iter().copied().collect();
        if unique.len() != seen.len() {
            return Err("a clause is listed under more than one enforcement layer".to_string());
        }
        let missing: Vec<String> = ALL_CLAUSES
            .iter()
            .filter(|c| !unique.contains(c))
            .map(|c| c.to_string())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "these clauses name no enforcement layer: {}",
                missing.join(", ")
            ));
        }
        Ok(())
    }

    pub fn layer_of(&self, clause: ClauseId) -> EnforcementLayer {
        if self.sync.iter().any(|d| d.clause == clause) {
            EnforcementLayer::Sync
        } else if self.operation.iter().any(|d| d.clause == clause) {
            EnforcementLayer::Operation
        } else if self.declaration.iter().any(|d| d.clause == clause) {
            EnforcementLayer::Declaration
        } else {
            EnforcementLayer::Format
        }
    }

    /// The enforcing site of a deferred clause.
    pub fn site_of(&self, clause: ClauseId) -> Option<&str> {
        self.sync
            .iter()
            .chain(self.operation.iter())
            .chain(self.declaration.iter())
            .find(|d| d.clause == clause)
            .map(|d| d.site.as_str())
    }
}

/// A clause this harness cannot certify because it belongs to another layer.
///
/// Reported in its OWN category, never as a gap: it is not a TODO for the
/// format author, and letting it sit among the format TODOs is precisely the
/// confusion the layer dimension exists to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredClause {
    pub clause: ClauseId,
    pub layer: EnforcementLayer,
    /// The code that enforces it. Never empty — the loader refuses otherwise.
    pub site: String,
}

impl std::fmt::Display for DeferredClause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is enforced at the {:?} layer by {} — uncertified HERE by design",
            self.clause, self.layer, self.site
        )
    }
}

/// A clause the profile STATES that nothing checks and nothing excuses.
///
/// Not a [`crate::Violation`]: a violation is about a round trip that broke a
/// declaration, and this is about the HARNESS not exercising one. Forcing it
/// into the same struct would mean inventing a leg, a key and a sent value that
/// do not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageGap {
    pub clause: ClauseId,
    pub reason: GapReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GapReason {
    /// A SET-valued clause whose declared members are not all driven. Recorded
    /// per member because a clause-level boolean lets ONE driven member
    /// launder every other: the clause reads certified while most of what it
    /// declares was never touched.
    MembersUndriven(Vec<String>),
    /// Stated, unprobed, and not marked — the F1 defect.
    UnmarkedAndUndriven,
    /// Listed under `not_yet_certified` although it is NOT a format-layer
    /// clause. "Not yet" and "not here" are different statements; blurring
    /// them is what made the layer gap invisible in the first place.
    MarkedButWrongLayer(EnforcementLayer),
    /// A format-crate harness claims to drive a clause another layer enforces
    /// — the probe is measuring something other than the clause.
    DrivenAtWrongLayer(EnforcementLayer),
    /// Marked `not_yet_certified`, but the certifier DOES drive it. Harmless
    /// to correctness and still worth fixing: a stale marker teaches readers to
    /// distrust a clause that is in fact certified.
    MarkedButDriven,
}

impl std::fmt::Display for CoverageGap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reason {
            GapReason::UnmarkedAndUndriven => write!(
                f,
                "{} is declared but NOTHING drives it, and it is not listed under \
                 `not_yet_certified` — either probe it or mark it",
                self.clause
            ),
            GapReason::MembersUndriven(members) => write!(
                f,
                "{} declares {} member(s) that NOTHING drives: {} — a set clause is certified \
                 only when every declared member is",
                self.clause,
                members.len(),
                members.join(", ")
            ),
            GapReason::MarkedButWrongLayer(layer) => write!(
                f,
                "{} is listed under `not_yet_certified`, but it is enforced at the {:?} layer \
                 — state that with `enforced_by`, not as a format TODO",
                self.clause, layer
            ),
            GapReason::DrivenAtWrongLayer(layer) => write!(
                f,
                "{} is enforced at the {:?} layer, but THIS harness drove it — a format-layer \
                 probe cannot certify another layer's rule",
                self.clause, layer
            ),
            GapReason::MarkedButDriven => write!(
                f,
                "{} is listed under `not_yet_certified` but the certifier DOES drive it — \
                 remove the stale marker",
                self.clause
            ),
        }
    }
}

/// Compare what the profile excuses against what the run actually drove.
/// What a set-valued clause DECLARES, against what a run actually drove.
///
/// Keyed by clause; the strings are member names as the report prints them.
pub type MemberCoverage = std::collections::BTreeMap<ClauseId, BTreeSet<String>>;

pub fn coverage_gaps(
    enforced_by: &EnforcementMap,
    not_yet_certified: &BTreeSet<ClauseId>,
    probed: &BTreeSet<ClauseId>,
    declared_members: &MemberCoverage,
    probed_members: &MemberCoverage,
) -> (Vec<CoverageGap>, Vec<DeferredClause>) {
    let mut gaps = Vec::new();
    let mut deferred = Vec::new();
    for &clause in ALL_CLAUSES {
        let marked = not_yet_certified.contains(&clause);
        let driven = probed.contains(&clause);
        let layer = enforced_by.layer_of(clause);

        if layer != EnforcementLayer::Format {
            // Another layer owns it. The only errors possible here are
            // category errors — claiming it as a format TODO, or a
            // format-layer probe pretending to certify it.
            if marked {
                gaps.push(CoverageGap {
                    clause,
                    reason: GapReason::MarkedButWrongLayer(layer),
                });
            }
            if driven {
                gaps.push(CoverageGap {
                    clause,
                    reason: GapReason::DrivenAtWrongLayer(layer),
                });
            }
            deferred.push(DeferredClause {
                clause,
                layer,
                site: enforced_by.site_of(clause).unwrap_or_default().to_string(),
            });
            continue;
        }

        match (marked, driven) {
            (false, false) => gaps.push(CoverageGap {
                clause,
                reason: GapReason::UnmarkedAndUndriven,
            }),
            (true, true) => gaps.push(CoverageGap {
                clause,
                reason: GapReason::MarkedButDriven,
            }),
            _ => {}
        }

        // A set clause counts as driven only when EVERY declared member is.
        if driven {
            if let Some(declared) = declared_members.get(&clause) {
                let empty = BTreeSet::new();
                let hit = probed_members.get(&clause).unwrap_or(&empty);
                let missing: Vec<String> = declared.difference(hit).cloned().collect();
                if !missing.is_empty() {
                    gaps.push(CoverageGap {
                        clause,
                        reason: GapReason::MembersUndriven(missing),
                    });
                }
            }
        }
    }
    (gaps, deferred)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL_CLAUSES` must list every variant. A variant missing from it is
    /// silently exempt from the coverage law — the very loophole this module
    /// closes — so the count is pinned deliberately.
    #[test]
    fn all_clauses_lists_every_variant() {
        let listed: BTreeSet<_> = ALL_CLAUSES.iter().copied().collect();
        assert_eq!(
            listed.len(),
            ALL_CLAUSES.len(),
            "ALL_CLAUSES contains a duplicate"
        );
        assert_eq!(
            listed.len(),
            44,
            "a clause was added to the vocabulary without adding it to ALL_CLAUSES, which \
             would exempt it from the driven-or-marked law"
        );
    }

    /// Every clause owned by the FORMAT layer — the shape a single-layer test
    /// wants, so the layer dimension does not have to be restated per case.
    fn all_format() -> EnforcementMap {
        EnforcementMap {
            format: ALL_CLAUSES.iter().copied().collect(),
            sync: Vec::new(),
            operation: Vec::new(),
            declaration: Vec::new(),
        }
    }

    #[test]
    fn every_clause_must_name_an_enforcement_layer() {
        let mut partial = all_format();
        partial.format.remove(&ClauseId::HostedKinds);
        let err = partial
            .check_total()
            .expect_err("a clause with no owner must not load");
        assert!(
            err.contains("hosted_kinds"),
            "the error must name it: {err}"
        );

        let mut doubled = all_format();
        doubled.sync.push(DeferralSite {
            clause: ClauseId::HostedKinds,
            site: "somewhere.rs:1".to_string(),
        });
        assert!(
            doubled.check_total().is_err(),
            "a clause owned by two layers is ambiguous and must not load"
        );
    }

    #[test]
    fn a_clause_of_another_layer_is_deferred_not_a_gap() {
        let mut map = all_format();
        map.format.remove(&ClauseId::HierarchyConstraints);
        map.sync.push(DeferralSite {
            clause: ClauseId::HierarchyConstraints,
            site: "sync_ports.rs:277".to_string(),
        });
        let (gaps, deferred) = coverage_gaps(
            &map,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &MemberCoverage::new(),
            &MemberCoverage::new(),
        );
        assert!(
            !gaps
                .iter()
                .any(|g| g.clause == ClauseId::HierarchyConstraints),
            "a sync-layer clause is not a format TODO"
        );
        assert!(
            deferred
                .iter()
                .any(|d| d.clause == ClauseId::HierarchyConstraints
                    && d.layer == EnforcementLayer::Sync),
            "it must be reported in its own category instead"
        );
    }

    /// The falsifier for the OPERATION layer: a format probe driving a clause
    /// the profile assigns to it is a category error, and the coverage law must
    /// say so rather than count it as covered.
    ///
    /// Without this the new variant would ship unexercised — a layer label
    /// nothing can contradict is the same defect as a citation nothing checks.
    #[test]
    fn a_format_probe_driving_an_operation_layer_clause_is_a_gap() {
        let mut map = all_format();
        map.format.remove(&ClauseId::HierarchyReparent);
        map.operation.push(DeferralSite {
            clause: ClauseId::HierarchyReparent,
            site: "traits.rs:2429-2440".to_string(),
        });
        let driven: BTreeSet<ClauseId> = [ClauseId::HierarchyReparent].into_iter().collect();
        let (gaps, deferred) = coverage_gaps(
            &map,
            &BTreeSet::new(),
            &driven,
            &MemberCoverage::new(),
            &MemberCoverage::new(),
        );
        assert!(
            gaps.iter().any(|g| g.clause == ClauseId::HierarchyReparent
                && g.reason == GapReason::DrivenAtWrongLayer(EnforcementLayer::Operation)),
            "a format probe must not certify an operation-layer clause: {gaps:?}"
        );
        assert!(
            deferred
                .iter()
                .any(|d| d.clause == ClauseId::HierarchyReparent
                    && d.layer == EnforcementLayer::Operation
                    && d.site == "traits.rs:2429-2440"),
            "and the deferral must still name its site: {deferred:?}"
        );
    }

    /// The category error the layer dimension exists to prevent: calling a
    /// layer gap a format TODO.
    #[test]
    fn marking_another_layers_clause_not_yet_certified_is_a_gap() {
        let mut map = all_format();
        map.format.remove(&ClauseId::HierarchyConstraints);
        map.sync.push(DeferralSite {
            clause: ClauseId::HierarchyConstraints,
            site: "sync_ports.rs:277".to_string(),
        });
        let marked = BTreeSet::from([ClauseId::HierarchyConstraints]);
        let (gaps, _) = coverage_gaps(
            &map,
            &marked,
            &BTreeSet::new(),
            &MemberCoverage::new(),
            &MemberCoverage::new(),
        );
        assert!(
            gaps.iter().any(|g| matches!(
                g.reason,
                GapReason::MarkedButWrongLayer(EnforcementLayer::Sync)
            )),
            "\"not yet\" and \"not here\" are different statements"
        );
    }

    #[test]
    fn an_unmarked_undriven_clause_is_a_gap() {
        let (gaps, _) = coverage_gaps(
            &all_format(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &MemberCoverage::new(),
            &MemberCoverage::new(),
        );
        assert_eq!(gaps.len(), ALL_CLAUSES.len());
        assert!(
            gaps.iter()
                .all(|g| g.reason == GapReason::UnmarkedAndUndriven)
        );
    }

    #[test]
    fn a_marked_clause_that_is_driven_is_reported_as_stale() {
        let marked = BTreeSet::from([ClauseId::PropertyValuesTypes]);
        let probed = BTreeSet::from([ClauseId::PropertyValuesTypes]);
        let (gaps, _) = coverage_gaps(
            &all_format(),
            &marked,
            &probed,
            &MemberCoverage::new(),
            &MemberCoverage::new(),
        );
        let stale = gaps
            .iter()
            .find(|g| g.clause == ClauseId::PropertyValuesTypes)
            .expect("a marked-but-driven clause must be reported");
        assert_eq!(stale.reason, GapReason::MarkedButDriven);
    }

    #[test]
    fn a_clause_that_is_driven_and_unmarked_is_no_gap() {
        let probed = BTreeSet::from([ClauseId::PropertyValuesTypes]);
        let (gaps, _) = coverage_gaps(
            &all_format(),
            &BTreeSet::new(),
            &probed,
            &MemberCoverage::new(),
            &MemberCoverage::new(),
        );
        assert!(
            !gaps
                .iter()
                .any(|g| g.clause == ClauseId::PropertyValuesTypes)
        );
    }
}
