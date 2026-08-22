//! The certifier: proves every declared restriction is REAL, and flags the
//! ones that are stricter than the format needs.
//!
//! The law, stated once (design draft §3.4): for every generated case,
//! **either the round trip is lossless, or the boundary refused the input.**
//! "Accepted and then quietly changed" is the one outcome that is always red.
//!
//! Two verdicts, and only one of them fails a gate:
//!
//! - **declared-but-broken** → [`Violation`], RED.
//! - **works-but-undeclared** → [`TighteningPrompt`], never a failure. The
//!   format is better than the profile admits; the profile is under-selling it.
//!   Failing on that would make the suite red for GOOD news, and it is
//!   inherently generator-dependent (CV-C).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use holon_api::Value;

use crate::axes::Attachments;
use crate::axes::BinaryInline;
use crate::axes::BlockConstruct;
use crate::axes::CarrierDisagreement;
use crate::axes::Collision;
use crate::axes::ConstraintId;
use crate::axes::ContentRepresentation;
use crate::axes::Cycles;
use crate::axes::Extension;
use crate::axes::HierarchyShape;
use crate::axes::HostedKind;
use crate::axes::IdOrigin;
use crate::axes::IdSpace;
use crate::axes::InlineConstruct;
use crate::axes::KeyCase;
use crate::axes::KeyCharset;
use crate::axes::MaxDepth;
use crate::axes::MultiValue;
use crate::axes::MultiValueScope;
use crate::axes::MultiValueSemantics;
use crate::axes::OrderKeyDurability;
use crate::axes::PropertyOrder;
use crate::axes::ReferenceValues;
use crate::axes::Representability;
use crate::axes::SchemaRequirement;
use crate::axes::SiblingOrder;
use crate::axes::ValueKind;
use crate::axes::WriteLeg;
use crate::axes::WriteUnit;
use crate::clause::ClauseId;
use crate::clause::CoverageGap;
use crate::clause::DeferredClause;
use crate::clause::GapReason;
use crate::clause::MemberCoverage;
use crate::clause::coverage_gaps;
use crate::profile::CapabilityProfile;
use crate::profile::CapabilityProfileId;
use crate::supports::Feature;
use crate::violation::Axis;
use crate::violation::Clause;
use crate::violation::Leg;
use crate::violation::Outcome;
use crate::violation::TighteningPrompt;
use crate::violation::Violation;

/// One property carrier of a format — a distinct code path a property can
/// travel. Formats have more than one, and they fail differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carrier {
    /// Names the leg in every finding, so a red points at a function.
    pub leg: Leg,
    /// Prose for the report: what this carrier IS.
    pub description: &'static str,
}

/// What came back for a key after the format's real write → read round trip.
///
/// Three outcomes, not two, because the certification law has three
/// (draft §3.4): lossless, refused, or — the always-red one — accepted and
/// quietly changed. Collapsing `Refused` into `Absent` would make "the
/// boundary rejected this input" indistinguishable from "the boundary took it
/// and lost it", which is precisely the distinction the law turns on.
#[derive(Debug, Clone, PartialEq)]
pub enum Readback {
    /// The key is gone: accepted, then silently lost.
    Absent,
    /// The key is present, carrying this value.
    Present(Value),
    /// The format REFUSED the input at its boundary — a loud, honest `Err`
    /// rather than a silent loss. `:WIDGET_ONLY:` outside `t`/`true` is the
    /// live example (`crates/holon-org-format/src/parser.rs:1006-1013`).
    Refused { reason: String },
}

/// A format that can be certified against a profile.
///
/// Deliberately NOT `FileFormatAdapter`: `holon-logseq-db` is an importer, not
/// a file adapter, and the certifier must reach both. Implementations live in
/// the format crate's `tests/` directory — never in `src/` — so a format crate
/// never gains a non-test dependency on this one and the profile stays an
/// independent statement ABOUT the format.
pub trait CertifiableFormat {
    fn profile(&self) -> &CapabilityProfile;

    /// Every distinct property carrier this format has.
    fn carriers(&self) -> &'static [Carrier];

    /// Put `key`/`value` into a fresh entity through `carrier`, run the
    /// format's REAL write → read round trip, and report what came back for
    /// that key.
    ///
    /// `Err` is reserved for a broken harness (the fixture would not render or
    /// would not parse). A value the format loses is `Ok(Readback::Absent)`,
    /// not an error — losing data is a finding, not a crash.
    fn round_trip_property(
        &self,
        carrier: Carrier,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback>;

    /// Write properties in `authored` key order, read them back, and report the
    /// order that returned.
    ///
    /// `Ok(None)` means this harness CANNOT drive the clause. It is not a pass:
    /// the certifier then records the clause as unprobed, and the coverage law
    /// demands `not_yet_certified` name it. That is how "driven or marked" is
    /// enforced by code rather than by the author's memory — a format cannot
    /// quietly skip an axis by leaving the method unimplemented.
    fn round_trip_property_order(&self, _: &[&str]) -> anyhow::Result<Option<Vec<String>>> {
        Ok(None)
    }

    /// Ingest an entity whose identity carriers name DIFFERENT identities.
    ///
    /// Same `Ok(None)` contract as above.
    fn carriers_disagree(&self) -> anyhow::Result<Option<DisagreementOutcome>> {
        Ok(None)
    }

    /// Put a specimen of `construct` through the real write-back and report
    /// whether it came back intact. Same `Ok(None)` contract as above.
    fn round_trip_block_construct(
        &self,
        _: BlockConstruct,
    ) -> anyhow::Result<Option<ConstructOutcome>> {
        Ok(None)
    }

    /// As above, for an inline mark.
    fn round_trip_inline_construct(
        &self,
        _: InlineConstruct,
    ) -> anyhow::Result<Option<ConstructOutcome>> {
        Ok(None)
    }

    /// Round-trip a tree nested `depth` levels deep. Same `Ok(None)` contract.
    fn round_trip_depth(&self, _: u32) -> anyhow::Result<Option<ConstructOutcome>> {
        Ok(None)
    }

    /// Author a structure that VIOLATES `constraint` and report what happened.
    /// A declared constraint must be REFUSED, never quietly accepted.
    fn violate_constraint(&self, _: ConstraintId) -> anyhow::Result<Option<ConstructOutcome>> {
        Ok(None)
    }

    /// Author a parent cycle. `cycles: rejected` must REFUSE it — flattening a
    /// cycle silently reparents blocks the author never moved.
    fn introduce_cycle(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        Ok(None)
    }

    /// Attach a file with this extension and report whether it survives.
    fn round_trip_attachment(&self, _: &Extension) -> anyhow::Result<Option<ConstructOutcome>> {
        Ok(None)
    }

    /// Write the SAME key twice with different values; report which value came
    /// back. Same `Ok(None)` contract.
    fn collide_key(&self, _: &Value, _: &Value) -> anyhow::Result<Option<Readback>> {
        Ok(None)
    }

    /// Ingest an entity carrying `authored_id` and report the id it ended up
    /// with — the difference between `authored` and `minted_on_write`.
    fn id_after_ingest(&self, _: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    /// Whether an entity written through `carrier` keeps its identity.
    fn identity_via(&self, _: crate::axes::IdCarrier) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Sibling titles in the order they came back, given the authored order.
    fn round_trip_sibling_order(&self, _: &[&str]) -> anyhow::Result<Option<Vec<String>>> {
        Ok(None)
    }

    /// Whether EVERY entity this format produces is hierarchical (has a place
    /// in a tree). Answers axis 1 from what ingest actually yields.
    fn all_entities_hierarchical(&self) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Whether ingest yields MARK DATA for a marked-up span, rather than
    /// carrying the markup as opaque text.
    fn marks_are_parsed(&self) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Whether the written form carries an EXPLICIT order key. `derived`
    /// claims it does not.
    fn writes_explicit_order_key(&self) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Whether the format can hold several ROOTS at once (forest vs tree).
    fn holds_multiple_roots(&self) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Ingest an entity whose id is `id` and report whether the boundary
    /// refused it. `Some(None)` = accepted; `Some(Some(reason))` = refused.
    #[allow(clippy::type_complexity)]
    fn id_refused(&self, _: &str) -> anyhow::Result<Option<Option<String>>> {
        Ok(None)
    }

    /// Render after a SINGLE-block change and report whether the output is the
    /// whole document (byte-identical to a full render) rather than a fragment.
    ///
    /// This is the falsifier for `unit_of_write`: a format that emitted a patch
    /// would produce something SHORTER than the full render, and asserting it
    /// from the render function's signature would prove nothing.
    fn single_change_emits_whole_document(&self) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Whether an attachment is carried as a REFERENCE (a path in the text)
    /// rather than as embedded bytes.
    fn attachment_is_reference(&self) -> anyhow::Result<Option<bool>> {
        Ok(None)
    }

    /// Put `values` into a MULTI-VALUED edge field joined by `separator`, round
    /// trip, and report the values that came back IN ORDER.
    ///
    /// Splitting is the only place a `separator` is observable and ORDER is the
    /// only place `semantics` is: an ordinary property never splits under
    /// `edge_fields_only`, so probing one certified every separator and both
    /// semantics alike (silent flips S1 and S2).
    fn round_trip_multi_value(
        &self,
        _: &[&str],
        _: &str,
    ) -> anyhow::Result<Option<MultiValueReadback>> {
        Ok(None)
    }

    /// Put `value` into a REFERENCE-typed property and report what the format
    /// parsed it into.
    ///
    /// Driven with two shapes: one that is a legal ID and one that is a legal
    /// NAME but not a legal id. A format that takes the first and refuses the
    /// second refers `by_id`; one that takes both, or only the second, does
    /// not.
    fn round_trip_reference(&self, _: &str) -> anyhow::Result<Option<ReferenceReadback>> {
        Ok(None)
    }

    /// Attempt a real WRITE through the format's own write path and report
    /// whether it succeeded and round-tripped.
    ///
    /// Asked of the FORMAT, never of the profile: `supports()` derives its
    /// answer FROM `write_leg`, so comparing the two would compare the profile
    /// with itself and pass for any declaration.
    fn attempt_write(&self) -> anyhow::Result<Option<WriteAttempt>> {
        Ok(None)
    }
}

/// What a REFERENCE-valued property gave back, at the TYPE level.
///
/// The string coming back proves nothing about references — an ordinary
/// property carries a string too. The discriminating question is what the
/// format parsed the value INTO, which is why this reports the typed shape
/// rather than the readback text.
#[derive(Debug, Clone, PartialEq)]
pub enum ReferenceReadback {
    /// Parsed as typed references — the ids or names they resolved to.
    Refs(Vec<String>),
    /// Carried, but as an ordinary string: no reference typing happened.
    Plain(String),
    /// The boundary refused the value as a reference.
    Refused { reason: String },
}

/// What a multi-valued field gave back.
///
/// `Refused` is not a harness failure: joining on a delimiter the format does
/// not split leaves ONE value that may itself be illegal (org's `:REQUIRES:`
/// takes bare ids, and `beta|alpha` is not one). That refusal is evidence the
/// field did NOT split, so the negative arm must be able to read it.
#[derive(Debug, Clone, PartialEq)]
pub enum MultiValueReadback {
    /// The values that came back, in order.
    Values(Vec<String>),
    /// The format rejected the joined field at its boundary.
    Refused { reason: String },
}

/// What happened when the certifier tried to WRITE through the format.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteAttempt {
    /// The write happened and the content came back, through THIS mechanism.
    ///
    /// Carrying the leg is what makes `write_leg` answer "which", not merely
    /// "whether" — the clause is named for the mechanism, and a probe that
    /// reports only success certifies `file`, `api` and `in_process` alike
    /// (silent flip S6).
    Wrote { leg: WriteLeg },
    /// The format refused the write, loudly.
    Refused { reason: String },
    /// No write path exists to call at all.
    NoWriteApi,
}

/// What became of one content construct across the round trip.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstructOutcome {
    /// Came back exactly as authored.
    Survived,
    /// Gone.
    Lost,
    /// Came back as something else — accepted and quietly altered.
    Changed { got: String },
    /// The boundary refused the input, loudly.
    Refused { reason: String },
}

/// What a format does when two identity carriers disagree.
///
/// The distinction is the whole point of the clause: refusing is a loud,
/// recoverable parse error, while picking one silently means the entity's
/// identity depends on which carrier the reader happened to trust — and every
/// inbound link rides on that choice.
#[derive(Debug, Clone, PartialEq)]
pub enum DisagreementOutcome {
    Refused { reason: String },
    Picked { carrier: crate::axes::IdCarrier },
}

/// Everything one certification run learned.
#[derive(Debug, Default)]
pub struct CertificationReport {
    pub violations: Vec<Violation>,
    pub prompts: Vec<TighteningPrompt>,
    /// Clauses stated but neither driven nor excused — see [`CoverageGap`].
    /// A gap FAILS the run: the yaml discipline is a gate, not a convention.
    pub gaps: Vec<CoverageGap>,
    /// Clauses another LAYER enforces — reported in their own category so a
    /// layer gap can never hide among the format TODOs.
    pub deferred: Vec<DeferredClause>,
    /// Which clauses this run actually drove. Recorded so coverage is measured
    /// from what happened, never from what the author believed.
    pub probed: BTreeSet<ClauseId>,
    /// Which MEMBERS of each set-valued clause were driven. Recorded at member
    /// granularity because a clause-level boolean lets one driven member
    /// launder the rest.
    pub probed_members: MemberCoverage,
    /// Clauses the profile excuses with `not_yet_certified`. Carried into the
    /// report so the escape hatch is VISIBLE in the run's output: a marker that
    /// only exists in the yaml reads, from the output, exactly like a clause a
    /// probe covers.
    pub marked: BTreeMap<ClauseId, String>,
    /// The yaml this run certified. A stale `HOLON_CAPABILITY_PROFILE` points
    /// the harness at a DIFFERENT valid profile, whose report looks just as
    /// clean; printing the input is what tells the two apart.
    pub profile_path: Option<PathBuf>,
    /// Clauses certified against a MOVING upstream, with the range each was
    /// measured against. Driven-with-expiry, never an excuse.
    pub provisional: BTreeMap<ClauseId, String>,
    /// Cases that behaved exactly as declared. Counted so a run that generated
    /// NOTHING cannot masquerade as a pass.
    pub confirmed: usize,
}

impl CertificationReport {
    /// The machine-readable report the ledger tooling consumes.
    ///
    /// The certifier NEVER writes under `docs/`. A proptest run that creates
    /// files in the source tree dirties the working copy nondeterministically
    /// and feeds the verify-files-contaminate-commits hazard, so the run emits
    /// JSON under `target/` and a HUMAN runs `capability-ledger.py sync` to
    /// materialize entries (CV-C: the ledger is hand-written, like the bug
    /// funnel).
    pub fn write_report(
        &self,
        profile: &CapabilityProfileId,
        dir: &Path,
    ) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating the report dir {}", dir.display()))?;
        let path = dir.join(format!("{profile}.json"));
        let records: Vec<serde_json::Value> = self
            .prompts
            .iter()
            .map(|p| {
                serde_json::json!({
                    "profile": p.profile.to_string(),
                    "axis": p.axis.to_string(),
                    "leg": p.leg.to_string(),
                    "construct": p.key,
                    "note": p.note,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "profile": profile.to_string(),
            "profile_path": self.profile_path.as_ref().map(|p| p.display().to_string()),
            "report_path": path.display().to_string(),
            "confirmed": self.confirmed,
            "violations": self.violations.len(),
            "coverage_gaps": self.gaps.len(),
            "clauses_driven": self.probed.len(),
            "clauses_provisional": self
                .provisional
                .iter()
                .map(|(clause, range)| {
                    serde_json::json!({"clause": clause.to_string(), "certified_against": range})
                })
                .collect::<Vec<_>>(),
            "clauses_marked_not_yet_certified": self
                .marked
                .iter()
                .map(|(clause, reason)| {
                    serde_json::json!({"clause": clause.to_string(), "reason": reason})
                })
                .collect::<Vec<_>>(),
            "tightening_prompts": records,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc)?)
            .with_context(|| format!("writing the certification report {}", path.display()))?;
        Ok(path)
    }

    pub fn is_clean(&self) -> bool {
        self.violations.is_empty() && self.gaps.is_empty()
    }

    /// A human-readable dump — the red log's content.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "profile read from: {}\n",
            match &self.profile_path {
                Some(p) => p.display().to_string(),
                None => "<in-memory yaml>".to_string(),
            }
        ));
        out.push_str(&format!(
            "confirmed: {}  violations: {}  coverage gaps: {}  tightening prompts: {}  \
             clauses driven: {}  deferred to another layer: {}  marked not-yet-certified: {}  \
             provisional: {}\n",
            self.confirmed,
            self.violations.len(),
            self.gaps.len(),
            self.prompts.len(),
            self.probed.len(),
            self.deferred.len(),
            self.marked.len(),
            self.provisional.len(),
        ));
        for v in &self.violations {
            out.push_str(&format!("VIOLATION  {v}\n"));
        }
        for g in &self.gaps {
            out.push_str(&format!("GAP        {g}\n"));
        }
        for p in &self.prompts {
            out.push_str(&format!("TIGHTENING {p}\n"));
        }
        for d in &self.deferred {
            out.push_str(&format!("DEFERRED   {d}\n"));
        }
        for (clause, range) in &self.provisional {
            out.push_str(&format!(
                "PROVISIONAL {clause} is certified against {range} — re-certify when that moves\n"
            ));
        }
        for (clause, reason) in &self.marked {
            out.push_str(&format!(
                "MARKER     {clause} is NOT certified — {reason}\n"
            ));
        }
        out
    }
}

/// What the law says about ONE generated case.
///
/// The same three-way judgement serves every axis, because the law is one law
/// (draft §3.4). Keeping it in one place is what stops each axis inventing its
/// own slightly-different notion of "close enough".
pub(crate) enum Judgement {
    /// Behaved exactly as declared.
    Confirmed,
    /// Declared-but-broken — RED.
    Broken(Outcome),
    /// Works-but-undeclared — a tightening prompt, never a failure (CV-C).
    ///
    /// The payload is unread today: every caller writes a clause-specific note
    /// instead of the generic one, because a prompt a human must act on should
    /// name the clause rather than the law. Kept as documentation of what the
    /// judgement MEANS.
    #[allow(dead_code)]
    UnderClaimed(String),
}

/// Judge a round trip against a declaration that the value IS carried intact.
///
/// `carried == false` means the profile declares this value does NOT survive
/// (a reserved prefix, an undeclared type). Losing it is then honest; carrying
/// it is the surprise.
pub(crate) fn judge(carried: bool, sent: &Value, readback: &Readback) -> Judgement {
    let intact = matches!(readback, Readback::Present(got) if got == sent);
    match (carried, readback) {
        (true, _) if intact => Judgement::Confirmed,
        (true, Readback::Present(got)) => Judgement::Broken(Outcome::Changed { got: got.clone() }),
        (true, Readback::Absent) => Judgement::Broken(Outcome::Dropped),
        (true, Readback::Refused { reason }) => Judgement::Broken(Outcome::Refused {
            reason: reason.clone(),
        }),
        // Declared not-carried and duly not carried — including a REFUSAL,
        // which is the law's other legal branch and strictly better than a
        // silent drop.
        (false, _) if !intact => Judgement::Confirmed,
        (false, _) => Judgement::UnderClaimed(
            "declared not carried, but this leg carried it intact — the profile is narrower \
             than the format"
                .to_string(),
        ),
    }
}

/// One inhabitant per `ValueKind`, used to drive the value-type cases.
fn specimen(kind: ValueKind) -> Value {
    match kind {
        ValueKind::String => Value::String("plain".to_string()),
        ValueKind::Integer => Value::Integer(42),
        ValueKind::Float => Value::Float(1.5),
        ValueKind::Boolean => Value::Boolean(true),
        ValueKind::DateTime => Value::DateTime("2026-08-22T10:00:00Z".to_string()),
        ValueKind::Json => Value::Json(r#"{"a":1}"#.to_string()),
        ValueKind::Array => Value::Array(vec![Value::String("one".to_string())]),
        ValueKind::Object => Value::Object(Default::default()),
        ValueKind::Null => Value::Null,
    }
}

const ALL_KINDS: &[ValueKind] = &[
    ValueKind::String,
    ValueKind::Integer,
    ValueKind::Float,
    ValueKind::Boolean,
    ValueKind::DateTime,
    ValueKind::Json,
    ValueKind::Array,
    ValueKind::Object,
    ValueKind::Null,
];

/// Certify a format against its own profile, for axes 3 and 4.
pub fn certify(format: &dyn CertifiableFormat) -> anyhow::Result<CertificationReport> {
    let profile = format.profile();
    let mut report = CertificationReport::default();

    for &carrier in format.carriers() {
        certify_property_keys(format, profile, carrier, &mut report)?;
        certify_property_values(format, profile, carrier, &mut report)?;
    }

    certify_content(format, profile, &mut report)?;
    certify_misc(format, profile, &mut report)?;
    certify_remainder(format, profile, &mut report)?;
    certify_hierarchy(format, profile, &mut report)?;
    certify_mutation(format, profile, &mut report)?;
    certify_assets(format, profile, &mut report)?;
    certify_ordering(format, profile, &mut report)?;
    certify_identity(format, profile, &mut report)?;

    // LAST, and it reads only what the run recorded: a clause the profile
    // states must be either driven above or named in `not_yet_certified`.
    // Anything else is the F1 defect — a citation that reads like a guarantee
    // and gates nothing.
    // What each SET clause declares, so the law can demand every member.
    let mut declared: MemberCoverage = MemberCoverage::new();
    declared.insert(
        ClauseId::ContentBlockConstructs,
        profile
            .content()
            .block_constructs
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
    );
    declared.insert(
        ClauseId::ContentInlineConstructs,
        profile
            .content()
            .inline_constructs
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
    );
    declared.insert(
        ClauseId::AssetsExtensions,
        profile
            .assets()
            .extensions
            .iter()
            .map(|e| e.as_str().to_string())
            .collect(),
    );
    if let MultiValue::Delimited { separators, .. } = &profile.property_values().multi_value {
        declared.insert(
            ClauseId::PropertyValuesMultiValue,
            separators.iter().map(|s| s.as_str().to_string()).collect(),
        );
    }
    declared.insert(
        ClauseId::IdentityCarriers,
        profile
            .identity()
            .carriers
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
    );
    declared.insert(
        ClauseId::IdentityIdConstraints,
        profile
            .identity()
            .id_constraints
            .iter()
            .map(|c| format!("{c:?}"))
            .collect(),
    );

    let (gaps, deferred) = coverage_gaps(
        profile.enforced_by(),
        &profile.marked_clauses(),
        &report.probed,
        &declared,
        &report.probed_members,
    );
    report.gaps = gaps;
    report.deferred = deferred;
    report.marked = profile.not_yet_certified().clone();
    report.profile_path = profile.source().map(|p| p.to_path_buf());
    report.provisional = profile.provisional().clone();
    // Driven-with-expiry: `provisional` annotates a MEASURED clause. One that
    // nothing drives is a citation with a date on it, which is the very thing
    // the coverage law refuses.
    for clause in report.provisional.keys() {
        if !report.probed.contains(clause) && !report.marked.contains_key(clause) {
            report.gaps.push(CoverageGap {
                clause: *clause,
                reason: GapReason::UnmarkedAndUndriven,
            });
        }
    }

    Ok(report)
}

/// Axis 3 — a key the profile does NOT reserve must survive with its value; a
/// key it DOES reserve is expected not to, and its loss is honest rather than
/// a violation.
/// Is this carrier's property boundary closed to ORDINARY writes?
///
/// A format whose write path refuses every property change (LogSeq-DB's push
/// is title-only) refuses the control too, and then NOTHING about key shape or
/// value kind is observable through it: the refusal is about the operation,
/// not about the key or the value. Confirming a clause on such a refusal is
/// the false-witness pattern — `folded_lower` would "confirm" because two
/// spellings both failed to arrive, and `empty_string: error` would "confirm"
/// because the write never got as far as the value.
///
/// So the axis-3 and axis-4 clauses are NOT DRIVEN through a closed boundary,
/// and the profile must MARK them with the reason instead.
fn boundary_is_closed(
    format: &dyn CertifiableFormat,
    carrier: Carrier,
) -> anyhow::Result<Option<String>> {
    let control = format.round_trip_property(carrier, "Plain", &Value::String("carried".into()))?;
    Ok(match control {
        Readback::Refused { reason } => Some(reason),
        _ => None,
    })
}

fn certify_property_keys(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    carrier: Carrier,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let axis = profile.property_keys();
    // Probe keys spanning the reserved/unreserved boundary. `Plain` is the
    // control: it must survive under EVERY profile, including a lying one, so
    // a red that also kills the control is a broken harness, not a finding.
    // `ID` is here to be SKIPPED visibly — see the `is_owned` arm.
    if boundary_is_closed(format, carrier)?.is_some() {
        return Ok(());
    }
    let probes = ["Plain", "_underscored", "ID"];
    let carried = Value::String("carried".to_string());
    report.probed.insert(ClauseId::PropertyKeysReservedPrefixes);
    certify_key_shape(format, profile, carrier, report, &carried)?;

    for key in probes {
        // A key the format OWNS is outside the ordinary-property law entirely:
        // it is not claimed to vanish and not an ordinary carrier, so neither
        // its survival nor its loss is evidence about this axis.
        if axis.is_owned(key) {
            continue;
        }
        let readback = format.round_trip_property(carrier, key, &carried)?;
        // A prefix-reserved key is declared NOT carried.
        let expected_carried = !axis.is_prefix_reserved(key);
        match judge(expected_carried, &carried, &readback) {
            Judgement::Confirmed => report.confirmed += 1,
            Judgement::Broken(outcome) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyKeys,
                clause: Clause::KeyNotReserved,
                leg: carrier.leg,
                key: key.to_string(),
                sent: carried.clone(),
                outcome,
            }),
            Judgement::UnderClaimed(_) => report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::PropertyKeys,
                leg: carrier.leg,
                key: key.to_string(),
                sent: carried.clone(),
                note: "declared under a reserved PREFIX, but this leg carried it — the \
                       erasure is not a property of the format, only of this leg"
                    .to_string(),
            }),
        }
    }
    Ok(())
}

/// Axis 4 — a declared type must round-trip preserving BOTH kind and
/// inhabitant. A kind that survives only by being re-typed is CHANGED, which
/// the law makes red: the value was accepted and quietly altered.
fn certify_property_values(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    carrier: Carrier,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let axis = profile.property_values();
    // A key the KEY axis does not reserve, so an axis-4 case can never fail
    // for an axis-3 reason.
    if boundary_is_closed(format, carrier)?.is_some() {
        return Ok(());
    }
    let key = "Probe";
    report.probed.insert(ClauseId::PropertyValuesTypes);
    report.probed.insert(ClauseId::PropertyValuesEmptyString);
    certify_value_shape(format, profile, carrier, report)?;

    for &kind in ALL_KINDS {
        let sent = specimen(kind);
        let readback = format.round_trip_property(carrier, key, &sent)?;
        match judge(axis.types.contains(&kind), &sent, &readback) {
            Judgement::Confirmed => report.confirmed += 1,
            Judgement::Broken(outcome) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyValues,
                clause: Clause::TypeDeclared(kind),
                leg: carrier.leg,
                key: key.to_string(),
                sent: sent.clone(),
                outcome,
            }),
            Judgement::UnderClaimed(_) => report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::PropertyValues,
                leg: carrier.leg,
                key: key.to_string(),
                sent: sent.clone(),
                note: format!(
                    "{kind:?} is undeclared but round-trips intact — property_values.types is \
                     narrower than the format"
                ),
            }),
        }
    }

    // `empty_string` is its own clause: an INHABITANT question, not a kind
    // question, and the two disagree in org — an empty value survives the
    // renderer and is lost at ingest.
    //
    // All THREE arms are driven. Probing only `Representable` would make a
    // false `dropped` uncertifiable, and ignoring `Error` would leave a
    // declaration nothing can contradict — the same defect in two places.
    let empty = Value::String(String::new());
    let readback = format.round_trip_property(carrier, key, &empty)?;
    let violation = |outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis: Axis::PropertyValues,
        clause: Clause::EmptyString,
        leg: carrier.leg,
        key: key.to_string(),
        sent: empty.clone(),
        outcome,
    };
    match axis.empty_string {
        // Declared refused: only an actual refusal confirms it. Being carried
        // OR being silently dropped both contradict the declaration, and the
        // silent drop is the worse of the two because it denies the loudness
        // the declaration promises.
        Representability::Error => match &readback {
            Readback::Refused { .. } => report.confirmed += 1,
            _ => report.violations.push(violation(Outcome::NotRefused)),
        },
        Representability::Representable | Representability::Dropped => {
            let carried = axis.empty_string == Representability::Representable;
            match judge(carried, &empty, &readback) {
                Judgement::Confirmed => report.confirmed += 1,
                Judgement::Broken(outcome) => report.violations.push(violation(outcome)),
                Judgement::UnderClaimed(_) => report.prompts.push(TighteningPrompt {
                    profile: profile.id().clone(),
                    axis: Axis::PropertyValues,
                    leg: carrier.leg,
                    key: key.to_string(),
                    sent: empty.clone(),
                    note: "declared dropped, but this leg carried the empty value intact — \
                           property_values.empty_string under-claims the format"
                        .to_string(),
                }),
            }
        }
    }

    Ok(())
}

/// Axis 5 — `property_order`. Does the AUTHOR's key order come back?
///
/// The load-bearing case for org: the `_drawer_order` carrier claims to
/// preserve it, and that carrier is itself `_`-prefixed — the prefix this same
/// profile declares erased. The clause is only true because the carrier lives
/// in the STORED bag and never in the drawer, so it is exactly the kind of
/// claim that deserves a probe rather than a citation.
fn certify_ordering(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    // Deliberately NOT alphabetical: a format that sorts would return the same
    // list for an already-sorted input, so a sorted probe cannot tell
    // `preserved` from `canonical`.
    let authored = ["Zeta", "Alpha", "Mu"];
    let Some(returned) = format.round_trip_property_order(&authored)? else {
        return Ok(());
    };
    report.probed.insert(ClauseId::OrderingPropertyOrder);

    let declared = profile.ordering().property_order;
    let preserved = returned == authored.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    let mut sorted = authored.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    sorted.sort();
    let canonical = returned == sorted;

    let finding = |outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis: Axis::Ordering,
        clause: Clause::PropertyOrder,
        leg: Leg("drawer"),
        key: authored.join(","),
        sent: Value::String(authored.join(",")),
        outcome,
    };

    match declared {
        PropertyOrder::Preserved if preserved => report.confirmed += 1,
        PropertyOrder::Preserved => report.violations.push(finding(Outcome::Changed {
            got: Value::String(returned.join(",")),
        })),
        // `canonical` promises a DETERMINISTIC order that is not the author's.
        // Returning the authored order satisfies determinism but under-claims.
        PropertyOrder::Canonical if canonical && !preserved => report.confirmed += 1,
        PropertyOrder::Canonical if preserved => report.prompts.push(TighteningPrompt {
            profile: profile.id().clone(),
            axis: Axis::Ordering,
            leg: Leg("drawer"),
            key: authored.join(","),
            sent: Value::String(authored.join(",")),
            note: "declared canonical, but the AUTHORED order came back — the format \
                   preserves more than the profile admits"
                .to_string(),
        }),
        PropertyOrder::Canonical => report.violations.push(finding(Outcome::Changed {
            got: Value::String(returned.join(",")),
        })),
        // `unspecified` claims nothing about order — so if the AUTHORED order
        // comes back exactly, the format preserves more than the profile
        // admits. Confirming this arm unconditionally (as before) made
        // `unspecified` the one declaration that could never be wrong: silent
        // flip S5, and the same shape already fixed for sibling_order.
        PropertyOrder::Unspecified if !preserved => report.confirmed += 1,
        PropertyOrder::Unspecified => report.prompts.push(TighteningPrompt {
            profile: profile.id().clone(),
            axis: Axis::Ordering,
            leg: Leg("drawer"),
            key: authored.join(","),
            sent: Value::String(authored.join(",")),
            note: "declared unspecified, but the AUTHORED key order came back exactly — the \
                   format preserves order the profile declines to promise"
                .to_string(),
        }),
    }
    Ok(())
}

/// Axis 7 — `carrier_disagreement`. Two carriers naming different identities
/// must produce the DECLARED outcome.
fn certify_identity(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let Some(outcome) = format.carriers_disagree()? else {
        return Ok(());
    };
    report.probed.insert(ClauseId::IdentityCarrierDisagreement);

    let declared = profile.identity().carrier_disagreement;
    let finding = |o: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis: Axis::Identity,
        clause: Clause::CarrierDisagreement,
        leg: Leg("carriers"),
        key: "ID".to_string(),
        sent: Value::String("two carriers, two identities".to_string()),
        outcome: o,
    };

    match (declared, &outcome) {
        (CarrierDisagreement::Error, DisagreementOutcome::Refused { .. }) => report.confirmed += 1,
        (CarrierDisagreement::Error, DisagreementOutcome::Picked { carrier }) => {
            report.violations.push(finding(Outcome::Changed {
                got: Value::String(format!("silently picked {carrier:?}")),
            }))
        }
        (CarrierDisagreement::PrecedenceWins, DisagreementOutcome::Picked { .. }) => {
            report.confirmed += 1
        }
        (CarrierDisagreement::PrecedenceWins, DisagreementOutcome::Refused { reason }) => {
            report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::Identity,
                leg: Leg("carriers"),
                key: "ID".to_string(),
                sent: Value::String("two carriers, two identities".to_string()),
                note: format!(
                    "declared a silent precedence pick, but the format REFUSED ({reason}) — \
                     the profile under-claims a loud, safer behaviour"
                ),
            })
        }
    }
    Ok(())
}

/// Axis 3's remaining clauses: `charset`, `case`, `collision`,
/// `schema_required`.
///
/// All four ride the same round trip as `reserved_prefixes` — they are
/// questions about the KEY, and the key is what the round trip already
/// carries.
fn certify_key_shape(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    carrier: Carrier,
    report: &mut CertificationReport,
    carried: &Value,
) -> anyhow::Result<()> {
    let axis = profile.property_keys();
    let broken = |clause: Clause, key: &str, outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis: Axis::PropertyKeys,
        clause,
        leg: carrier.leg,
        key: key.to_string(),
        sent: carried.clone(),
        outcome,
    };

    // charset — a key the declared charset FORBIDS must not be carried. The
    // interesting probe is the one just outside the boundary, not a wild one.
    let hostile = match axis.charset {
        KeyCharset::Any => None,
        KeyCharset::NoWhitespace => Some("has space"),
        KeyCharset::Identifier => Some("has-hyphen"),
        KeyCharset::KeywordNamespaced => Some("unnamespaced"),
    };
    if let Some(key) = hostile {
        report.probed.insert(ClauseId::PropertyKeysCharset);
        // A forbidden key must NOT come back as an ordinary property. Refusal
        // and loss are both honest; carrying it means the charset is wider
        // than declared.
        match format.round_trip_property(carrier, key, carried)? {
            Readback::Present(got) if got == *carried => report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::PropertyKeys,
                leg: carrier.leg,
                key: key.to_string(),
                sent: carried.clone(),
                note: format!(
                    "{:?} forbids this key shape, but the leg carried it — the charset is \
                     wider than declared",
                    axis.charset
                ),
            }),
            _ => report.confirmed += 1,
        }
    }

    // case — `sensitive` means two spellings keep their OWN slots. Probed as a
    // pair, because one key alone cannot show folding.
    report.probed.insert(ClauseId::PropertyKeysCase);
    let lower = format.round_trip_property(carrier, "effort", carried)?;
    let upper = format.round_trip_property(carrier, "Effort", carried)?;
    let both_kept =
        matches!(&lower, Readback::Present(_)) && matches!(&upper, Readback::Present(_));
    match axis.case {
        KeyCase::Sensitive if both_kept => report.confirmed += 1,
        KeyCase::Sensitive => {
            report
                .violations
                .push(broken(Clause::KeyCase, "effort/Effort", Outcome::Dropped))
        }
        // A folding format claims the two spellings COLLAPSE into one slot.
        // Confirming this arm unconditionally (as the first version did) made
        // `folded_upper` unfalsifiable: the declaration could be wrong and
        // nothing would say so.
        KeyCase::FoldedUpper | KeyCase::FoldedLower if !both_kept => report.confirmed += 1,
        KeyCase::FoldedUpper | KeyCase::FoldedLower => report.violations.push(broken(
            Clause::KeyCase,
            "effort/Effort",
            Outcome::NotRefused,
        )),
    }

    // schema_required — `open` claims an undeclared key needs no declaration.
    report.probed.insert(ClauseId::PropertyKeysSchemaRequired);
    let novel = format.round_trip_property(carrier, "NeverDeclaredBefore", carried)?;
    match (axis.schema_required, &novel) {
        (SchemaRequirement::Open, Readback::Present(got)) if got == carried => {
            report.confirmed += 1
        }
        (SchemaRequirement::Open, _) => report.violations.push(broken(
            Clause::SchemaRequired,
            "NeverDeclaredBefore",
            Outcome::Dropped,
        )),
        (SchemaRequirement::Declared, Readback::Refused { .. }) => report.confirmed += 1,
        (SchemaRequirement::Declared, _) => report.violations.push(broken(
            Clause::SchemaRequired,
            "NeverDeclaredBefore",
            Outcome::NotRefused,
        )),
    }
    Ok(())
}

/// Axis 4's remaining clauses: `null`, `multi_value`, `reference_values`.
fn certify_value_shape(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    carrier: Carrier,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let axis = profile.property_values();
    let key = "Probe";
    let broken = |clause: Clause, sent: Value, outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis: Axis::PropertyValues,
        clause,
        leg: carrier.leg,
        key: key.to_string(),
        sent,
        outcome,
    };

    // null — the INHABITANT question, separate from the `Null` KIND that
    // `types` drives.
    report.probed.insert(ClauseId::PropertyValuesNull);
    let back = format.round_trip_property(carrier, key, &Value::Null)?;
    let survived = matches!(&back, Readback::Present(Value::Null));
    match axis.null {
        Representability::Representable if survived => report.confirmed += 1,
        Representability::Representable => {
            report
                .violations
                .push(broken(Clause::Null, Value::Null, Outcome::Dropped))
        }
        Representability::Dropped if !survived => report.confirmed += 1,
        Representability::Dropped => report.prompts.push(TighteningPrompt {
            profile: profile.id().clone(),
            axis: Axis::PropertyValues,
            leg: carrier.leg,
            key: key.to_string(),
            sent: Value::Null,
            note: "declared dropped, but this leg carried null — under-claimed".to_string(),
        }),
        Representability::Error => match &back {
            Readback::Refused { .. } => report.confirmed += 1,
            _ => report
                .violations
                .push(broken(Clause::Null, Value::Null, Outcome::NotRefused)),
        },
    }

    // multi_value — with `scope: edge_fields_only`, an ORDINARY property
    // containing the separator must stay ONE value. Splitting it would silently
    // turn one authored string into a list.
    // The SEPARATOR and the SEMANTICS are only observable where the format
    // actually splits — an ordinary property never splits under
    // `edge_fields_only`, so probing one certified every separator and both
    // semantics alike (silent flips S1, S2).
    if let MultiValue::Delimited {
        separators,
        semantics,
        ..
    } = &axis.multi_value
    {
        let authored = ["beta", "alpha"];
        // EVERY declared delimiter is driven, and each is recorded as a MEMBER:
        // a clause-level boolean let one working separator launder the rest.
        for separator in separators {
            let sep = separator.as_str();
            let Some(readback) = format.round_trip_multi_value(&authored, sep)? else {
                continue;
            };
            report.probed.insert(ClauseId::PropertyValuesMultiValue);
            report
                .probed_members
                .entry(ClauseId::PropertyValuesMultiValue)
                .or_default()
                .insert(sep.to_string());
            let back = match readback {
                MultiValueReadback::Values(values) => values,
                // Declared to split, and the boundary rejected the joined field
                // instead — the declaration is wrong about this delimiter.
                MultiValueReadback::Refused { reason } => {
                    report.violations.push(broken(
                        Clause::MultiValue,
                        Value::String(authored.join(sep)),
                        Outcome::Refused { reason },
                    ));
                    continue;
                }
            };
            if back.len() != authored.len() {
                // The declared separator did not split the field.
                report.violations.push(broken(
                    Clause::MultiValue,
                    Value::String(authored.join(sep)),
                    Outcome::Changed {
                        got: Value::String(back.join("|")),
                    },
                ));
                continue;
            }
            let order_kept = back == authored.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            match (semantics, order_kept) {
                (MultiValueSemantics::List, true) => report.confirmed += 1,
                (MultiValueSemantics::List, false) => report.violations.push(broken(
                    Clause::MultiValue,
                    Value::String(authored.join(sep)),
                    Outcome::Changed {
                        got: Value::String(back.join(sep)),
                    },
                )),
                (MultiValueSemantics::Set, false) => report.confirmed += 1,
                (MultiValueSemantics::Set, true) => report.prompts.push(TighteningPrompt {
                    profile: profile.id().clone(),
                    axis: Axis::PropertyValues,
                    leg: carrier.leg,
                    key: key.to_string(),
                    sent: Value::String(authored.join(sep)),
                    note: "declared a SET, but the authored order came back — the format \
                           preserves an order the profile calls insignificant"
                        .to_string(),
                }),
            }
        }

        // The NEGATIVE arm, and without it the set is unfalsifiable upward: a
        // profile could name every delimiter in the world and each one that
        // happened to work would confirm it. A delimiter the profile does NOT
        // name must NOT split.
        for candidate in DELIMITER_CANDIDATES {
            if separators.iter().any(|s| s.as_str() == *candidate) {
                continue;
            }
            let Some(MultiValueReadback::Values(back)) =
                format.round_trip_multi_value(&authored, candidate)?
            else {
                // Not driveable, or REFUSED — and a refusal means the field did
                // not split, which is what an undeclared delimiter should do.
                continue;
            };
            if back.len() > 1 {
                report.prompts.push(TighteningPrompt {
                    profile: profile.id().clone(),
                    axis: Axis::PropertyValues,
                    leg: carrier.leg,
                    key: key.to_string(),
                    sent: Value::String(authored.join(candidate)),
                    note: format!(
                        "{candidate:?} splits the field but is NOT declared in \
                         multi_value.separators — a reader joining on a declared separator \
                         would produce a value this format silently splits"
                    ),
                });
            }
        }
    }

    if let MultiValue::Delimited {
        separators, scope, ..
    } = &axis.multi_value
    {
        // `scope` is one question about the format, so one declared delimiter
        // answers it; WHICH delimiters split is the `separators` clause above.
        let Some(separator) = separators.iter().next().map(|s| s.as_str().to_string()) else {
            return Ok(());
        };
        report.probed.insert(ClauseId::PropertyValuesMultiValue);
        let joined = Value::String(format!("alpha{separator}beta"));
        let back = format.round_trip_property(carrier, key, &joined)?;
        let intact = matches!(&back, Readback::Present(got) if *got == joined);
        match scope {
            MultiValueScope::EdgeFieldsOnly if intact => report.confirmed += 1,
            MultiValueScope::EdgeFieldsOnly => report.violations.push(broken(
                Clause::MultiValue,
                joined,
                Outcome::Changed {
                    got: match back {
                        Readback::Present(v) => v,
                        _ => Value::Null,
                    },
                },
            )),
            // `all_properties` splits by design; the round trip cannot show
            // that through a single string, so it is not driven here.
            MultiValueScope::AllProperties => {
                report.probed.remove(&ClauseId::PropertyValuesMultiValue);
            }
        }
    }

    certify_reference_values(format, profile, carrier, key, report)
}

/// An id-shaped value: legal as an id in every format the vocabulary models.
const ID_SHAPED: &str = "certify-target";

/// Axis 4 — `reference_values`, as far as a format probe can honestly go.
///
/// MEASURED, and the answer bounds the clause: only `none` is discriminable
/// here. `by_id` versus `by_name` is not, because the wire form is the same
/// bare slug for both and every name shape that DIFFERS from an id shape
/// contains a separator, which the `multi_value` axis consumes first — the
/// first version of this probe sent "Some Page Title" and read back
/// `Refs(["Page", "Some", "Title"])`, i.e. it measured the split, not the
/// naming. A profile declaring a naming mode therefore carries a
/// `not_yet_certified` marker WITH that reason.
fn certify_reference_values(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    carrier: Carrier,
    key: &str,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    // Only the `none` arm is decidable from a round trip: either the format
    // parses a value into a typed reference or it does not.
    if profile.property_values().reference_values != ReferenceValues::None {
        return Ok(());
    }
    let Some(readback) = format.round_trip_reference(ID_SHAPED)? else {
        return Ok(());
    };
    report
        .probed
        .insert(ClauseId::PropertyValuesReferenceValues);
    if matches!(readback, ReferenceReadback::Refs(_)) {
        report.prompts.push(TighteningPrompt {
            profile: profile.id().clone(),
            axis: Axis::PropertyValues,
            leg: carrier.leg,
            key: key.to_string(),
            sent: Value::String(ID_SHAPED.to_string()),
            note: format!(
                "declared to carry no references, but the format parsed one into a typed \
                 reference — the profile under-claims; got {readback:?}"
            ),
        });
    } else {
        report.confirmed += 1;
    }
    Ok(())
}

/// The remaining format-layer clauses whose probe is a single observation.
///
/// Grouped rather than split into five near-identical functions: each is one
/// question with one answer, and five one-case functions would be more
/// ceremony than content.
fn certify_misc(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let broken = |axis: Axis, clause: Clause, key: &str, outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis,
        clause,
        leg: Leg("structure"),
        key: key.to_string(),
        sent: Value::String(key.to_string()),
        outcome,
    };

    // axis 3 — collision. Two writes of one key; `last_wins` means the SECOND
    // value comes back.
    let first = Value::String("first".to_string());
    let second = Value::String("second".to_string());
    if let Some(back) = format.collide_key(&first, &second)? {
        report.probed.insert(ClauseId::PropertyKeysCollision);
        let got = match &back {
            Readback::Present(v) => Some(v.clone()),
            _ => None,
        };
        let ok = match profile.property_keys().collision {
            Collision::LastWins => got.as_ref() == Some(&second),
            Collision::FirstWins => got.as_ref() == Some(&first),
            Collision::Error => matches!(back, Readback::Refused { .. }),
            // `multi_valued` keeps both; a single readback cannot show that, so
            // it is not driven here.
            Collision::MultiValued => {
                report.probed.remove(&ClauseId::PropertyKeysCollision);
                true
            }
        };
        if !ok {
            report.violations.push(broken(
                Axis::PropertyKeys,
                Clause::Collision,
                "colliding key",
                Outcome::Changed {
                    got: got.unwrap_or(Value::Null),
                },
            ));
        } else {
            report.confirmed += 1;
        }
    }

    // axis 7 — id_origin. `authored` means the id the author wrote SURVIVES;
    // a minted id would silently detach every inbound link.
    // OBSERVE which origin the format has, then compare to the declared one.
    // The previous version confirmed `derived_from_position` unconditionally
    // (silent flip S7): it asked only whether the authored id survived, which
    // cannot tell a position-derived id from a freshly minted one. Running the
    // SAME fixture twice discriminates them — a position-derived id is stable
    // across runs, a minted one is not.
    let authored = "authored-identity";
    if let Some(first) = format.id_after_ingest(authored)? {
        report.probed.insert(ClauseId::IdentityIdOrigin);
        let second = format
            .id_after_ingest(authored)?
            .unwrap_or_else(|| first.clone());
        let observed = if first.contains(authored) {
            IdOrigin::Authored
        } else if first == second {
            IdOrigin::DerivedFromPosition
        } else {
            IdOrigin::MintedOnWrite
        };
        if observed == profile.identity().id_origin {
            report.confirmed += 1;
        } else if observed == IdOrigin::Authored {
            report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::Identity,
                leg: Leg("structure"),
                key: authored.to_string(),
                sent: Value::String(authored.to_string()),
                note: format!(
                    "declared {:?}, but the AUTHORED id survived — the format preserves \
                     identity better than the profile admits",
                    profile.identity().id_origin
                ),
            });
        } else {
            report.violations.push(broken(
                Axis::Identity,
                Clause::IdOrigin,
                authored,
                Outcome::Changed {
                    got: Value::String(format!("observed {observed:?}, got id {first}")),
                },
            ));
        }
    }

    // axis 7 — carriers. Every DECLARED carrier must actually carry identity.
    let mut drove_carriers = false;
    // Ranges over the CLOSED vocabulary, never the declared subset: a law over
    // what a profile declares is satisfied by DELETING a member, so a real
    // carrier could be dropped to clear a gap and the suite would stay green
    // (silent flip S8).
    for &carrier in crate::axes::ALL_ID_CARRIERS {
        let Some(works) = format.identity_via(carrier)? else {
            continue;
        };
        drove_carriers = true;
        report
            .probed_members
            .entry(ClauseId::IdentityCarriers)
            .or_default()
            .insert(format!("{carrier:?}"));
        let declared = profile.identity().carriers.contains(&carrier);
        match (declared, works) {
            (true, true) | (false, false) => report.confirmed += 1,
            (true, false) => report.violations.push(broken(
                Axis::Identity,
                Clause::Carriers,
                &format!("{carrier:?}"),
                Outcome::Dropped,
            )),
            // Carries identity but is not declared — the profile is hiding a
            // carrier, which is how a deletion used to pass.
            (false, true) => report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::Identity,
                leg: Leg("structure"),
                key: format!("{carrier:?}"),
                sent: Value::String(format!("{carrier:?}")),
                note: "carries identity but is NOT declared in identity.carriers — an \
                       undeclared carrier is a carrier a reader will not know to preserve"
                    .to_string(),
            }),
        }
    }
    if drove_carriers {
        report.probed.insert(ClauseId::IdentityCarriers);
    }

    // axis 5 — sibling_order. MAPS the observation onto one value.
    //
    // The previous version asked only "did the order come back?" and confirmed
    // every key-based value on the true side, so declaring org — which has NO
    // order key on disk — `fractional_index` certified happily (silent flip
    // S4). Order returning WITHOUT a key on disk means the order IS the file
    // position; a key-based mechanism requires a key to exist.
    let siblings = ["First", "Second", "Third"];
    if let Some(back) = format.round_trip_sibling_order(&siblings)? {
        report.probed.insert(ClauseId::OrderingSiblingOrder);
        let same = back == siblings.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let has_key = format.writes_explicit_order_key()?.unwrap_or(false);
        let declared = profile.ordering().sibling_order;
        // Only `file_position` is compatible with "ordered, but no key".
        if same && !has_key && declared != SiblingOrder::FilePosition {
            report.violations.push(broken(
                Axis::Ordering,
                Clause::SiblingOrder,
                "First,Second,Third",
                Outcome::Changed {
                    got: Value::String(format!(
                        "order survives with NO explicit key on disk, which is \
                         file_position, not {declared:?}"
                    )),
                },
            ));
        } else {
            match (declared, same) {
                // `unordered` claims order is NOT modelled. If the authored order
                // comes back exactly, the format models MORE than declared — an
                // under-claim, and a prompt rather than silent confirmation, which
                // is what the first version did and what made this arm
                // unfalsifiable.
                (SiblingOrder::Unordered, true) => report.prompts.push(TighteningPrompt {
                    profile: profile.id().clone(),
                    axis: Axis::Ordering,
                    leg: Leg("structure"),
                    key: "First,Second,Third".to_string(),
                    sent: Value::String("First,Second,Third".to_string()),
                    note: "declared unordered, but the AUTHORED sibling order came back exactly \
                       — the format models order the profile denies"
                        .to_string(),
                }),
                (SiblingOrder::Unordered, false) => report.confirmed += 1,
                (_, true) => report.confirmed += 1,
                (_, false) => report.violations.push(broken(
                    Axis::Ordering,
                    Clause::SiblingOrder,
                    "First,Second,Third",
                    Outcome::Changed {
                        got: Value::String(back.join(",")),
                    },
                )),
            }
        }
    }

    // axis 1 — hosted_kinds. Answered from what ingest YIELDS: a format that
    // only ever produces hierarchical entities cannot host a free-standing one.
    if let Some(all_hier) = format.all_entities_hierarchical()? {
        report.probed.insert(ClauseId::HostedKinds);
        let declares_free = profile.hosted_kinds().contains(&HostedKind::FreeStanding);
        if all_hier && declares_free {
            report.violations.push(broken(
                Axis::Content,
                Clause::HostedKinds,
                "free_standing",
                Outcome::Dropped,
            ));
        } else {
            report.confirmed += 1;
        }
    }
    Ok(())
}

/// The last format-layer clauses. Each is one observation with one falsifier.
fn certify_remainder(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let broken = |axis: Axis, clause: Clause, key: &str, outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis,
        clause,
        leg: Leg("structure"),
        key: key.to_string(),
        sent: Value::String(key.to_string()),
        outcome,
    };

    // axis 2 — representation. `marked_text` claims marks are PARSED, not
    // carried as opaque markup.
    if let Some(parsed_marks) = format.marks_are_parsed()? {
        report.probed.insert(ClauseId::ContentRepresentation);
        let declares_marks = profile.content().representation == ContentRepresentation::MarkedText;
        if declares_marks == parsed_marks {
            report.confirmed += 1;
        } else if declares_marks {
            report.violations.push(broken(
                Axis::Content,
                Clause::Representation,
                "marked_text",
                Outcome::Dropped,
            ));
        } else {
            report.prompts.push(TighteningPrompt {
                profile: profile.id().clone(),
                axis: Axis::Content,
                leg: Leg("structure"),
                key: "representation".to_string(),
                sent: Value::String("marked span".to_string()),
                note: "declared opaque, but marks ARE parsed — the profile under-claims"
                    .to_string(),
            });
        }
    }

    // axis 5 — order_key_durable. `derived` claims NO key reaches disk.
    if let Some(writes_key) = format.writes_explicit_order_key()? {
        report.probed.insert(ClauseId::OrderingOrderKeyDurable);
        let declares_derived = profile.ordering().order_key_durable == OrderKeyDurability::Derived;
        if declares_derived != writes_key {
            report.confirmed += 1;
        } else {
            report.violations.push(broken(
                Axis::Ordering,
                Clause::OrderKeyDurable,
                "order key",
                if writes_key {
                    Outcome::NotRefused
                } else {
                    Outcome::Dropped
                },
            ));
        }
    }

    // axis 6 — shape. `forest` claims several roots coexist.
    if let Some(multi) = format.holds_multiple_roots()? {
        report.probed.insert(ClauseId::HierarchyShape);
        let declares_forest = profile.hierarchy().shape == HierarchyShape::Forest;
        if declares_forest == multi {
            report.confirmed += 1;
        } else {
            report.violations.push(broken(
                Axis::Hierarchy,
                Clause::Shape,
                "multiple roots",
                Outcome::Dropped,
            ));
        }
    }

    // axis 7 — id_space and id_constraints, driven by HOSTILE ids. An EMPTY
    // constraint list is still a claim: that NONE of these is refused. A
    // refusal is a finding for the yaml, not a failure of the format.
    let hostile = [
        ("empty", ""),
        ("whitespace", "   "),
        ("scheme-prefixed", "block:already-scheme"),
        ("very-long", "x"),
        ("control-chars", "id\u{7}with\u{1}control"),
    ];
    let mut drove_ids = false;
    for (name, raw) in hostile {
        let id = if name == "very-long" {
            raw.repeat(10_240)
        } else {
            raw.to_string()
        };
        let Some(refusal) = format.id_refused(&id)? else {
            continue;
        };
        drove_ids = true;
        if let Some(reason) = refusal {
            // The profile says no id shape is constrained; one was refused.
            if profile.identity().id_constraints.is_empty() {
                report.prompts.push(TighteningPrompt {
                    profile: profile.id().clone(),
                    axis: Axis::Identity,
                    leg: Leg("structure"),
                    key: name.to_string(),
                    sent: Value::String(name.to_string()),
                    note: format!(
                        "id_constraints is empty — a claim that NO id shape is refused — but \
                         the `{name}` id WAS refused ({reason}). The constraint is real and \
                         unnamed."
                    ),
                });
            } else {
                report.confirmed += 1;
            }
        } else {
            report.confirmed += 1;
        }
    }
    if drove_ids {
        report.probed.insert(ClauseId::IdentityIdConstraints);
    }

    // axis 7 — id_space, JUDGED rather than merely attested. `opaque_string`
    // claims an arbitrary id that is neither a uuid nor path-shaped still
    // round-trips; a `uuid` space claims the opposite. The previous version
    // inserted this clause into `probed` and then read it nowhere, which is an
    // attestation with no measurement behind it.
    let opaque = "not-a-uuid-just-text";
    if let Some(got) = format.id_after_ingest(opaque)? {
        report.probed.insert(ClauseId::IdentityIdSpace);
        let kept = got.contains(opaque);
        let ok = match profile.identity().id_space {
            IdSpace::OpaqueString => kept,
            IdSpace::Uuid => !kept,
            // The remaining spaces make no claim this probe can settle.
            _ => {
                report.probed.remove(&ClauseId::IdentityIdSpace);
                true
            }
        };
        if ok {
            report.confirmed += 1;
        } else {
            report.violations.push(broken(
                Axis::Identity,
                Clause::IdSpace,
                opaque,
                Outcome::Changed {
                    got: Value::String(got),
                },
            ));
        }
    }

    // axis 7 — each DECLARED id constraint driven by an id that violates
    // exactly IT. Recorded under the identity clause: `violate_constraint` is
    // shared with hierarchy, but the CLAIM lives wherever the constraint is
    // declared, and attributing it to the wrong clause would leave the real one
    // uncovered — which is what the member law caught.
    for &constraint in &profile.identity().id_constraints {
        let Some(outcome) = format.violate_constraint(constraint)? else {
            continue;
        };
        report.probed.insert(ClauseId::IdentityIdConstraints);
        report
            .probed_members
            .entry(ClauseId::IdentityIdConstraints)
            .or_default()
            .insert(format!("{constraint:?}"));
        match &outcome {
            ConstructOutcome::Refused { .. } | ConstructOutcome::Lost => report.confirmed += 1,
            _ => report.violations.push(broken(
                Axis::Identity,
                Clause::IdConstraint(constraint),
                &format!("{constraint:?}"),
                Outcome::NotRefused,
            )),
        }
    }

    // axis 9 — unit_of_write. The falsifier is a PARTIAL write.
    if let Some(whole) = format.single_change_emits_whole_document()? {
        report.probed.insert(ClauseId::MutationUnitOfWrite);
        let declares_file = profile.mutation().unit_of_write == WriteUnit::File;
        if declares_file == whole {
            report.confirmed += 1;
        } else {
            report.violations.push(broken(
                Axis::Mutation,
                Clause::UnitOfWrite,
                "single-block change",
                Outcome::Changed {
                    got: Value::String(if whole {
                        "whole document".to_string()
                    } else {
                        "a fragment".to_string()
                    }),
                },
            ));
        }
    }

    // axis 10 — attachments as REFERENCE vs embedded bytes.
    if let Some(is_ref) = format.attachment_is_reference()? {
        report.probed.insert(ClauseId::AssetsAttachments);
        report.probed.insert(ClauseId::AssetsBinaryInline);
        let declares_ref = profile.assets().attachments == Attachments::InlineReference;
        let declares_no_binary = profile.assets().binary_inline == BinaryInline::None;
        if declares_ref == is_ref && (!is_ref || declares_no_binary) {
            report.confirmed += 1;
        } else {
            report.violations.push(broken(
                Axis::Assets,
                Clause::Attachments,
                "attachment carriage",
                Outcome::Changed {
                    got: Value::String(if is_ref {
                        "a reference".to_string()
                    } else {
                        "embedded bytes".to_string()
                    }),
                },
            ));
        }
    }
    Ok(())
}

/// Axis 2 — every construct in the CLOSED vocabulary is driven, declared or
/// not.
///
/// Driving the undeclared ones is the point: a construct the profile omits
/// because nobody looked will round-trip and raise a prompt, which is how the
/// draft's UNKNOWNs (org `table`, `logbook`) get RESOLVED rather than
/// inherited. 2b.5 may not refuse content on the strength of a clause nobody
/// drove.
fn certify_content(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let axis = profile.content();

    let mut drove_block = false;
    for &construct in ALL_BLOCK_CONSTRUCTS {
        let Some(outcome) = format.round_trip_block_construct(construct)? else {
            continue;
        };
        drove_block = true;
        report
            .probed_members
            .entry(ClauseId::ContentBlockConstructs)
            .or_default()
            .insert(format!("{construct:?}"));
        judge_construct(
            profile,
            report,
            Axis::Content,
            Clause::BlockConstruct(construct),
            &format!("{construct:?}"),
            axis.block_constructs.contains(&construct),
            &outcome,
        );
    }
    if drove_block {
        report.probed.insert(ClauseId::ContentBlockConstructs);
    }

    let mut drove_inline = false;
    for &construct in ALL_INLINE_CONSTRUCTS {
        let Some(outcome) = format.round_trip_inline_construct(construct)? else {
            continue;
        };
        drove_inline = true;
        report
            .probed_members
            .entry(ClauseId::ContentInlineConstructs)
            .or_default()
            .insert(format!("{construct:?}"));
        judge_construct(
            profile,
            report,
            Axis::Content,
            Clause::InlineConstruct(construct),
            &format!("{construct:?}"),
            axis.inline_constructs.contains(&construct),
            &outcome,
        );
    }
    if drove_inline {
        report.probed.insert(ClauseId::ContentInlineConstructs);
    }
    Ok(())
}

/// The construct form of [`judge`] — same law, different carrier of evidence.
#[allow(clippy::too_many_arguments)]
fn judge_construct(
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
    axis: Axis,
    clause: Clause,
    name: &str,
    declared: bool,
    outcome: &ConstructOutcome,
) {
    let sent = Value::String(name.to_string());
    let broken = |o: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis,
        clause: clause.clone(),
        leg: Leg("content"),
        key: name.to_string(),
        sent: sent.clone(),
        outcome: o,
    };
    match (declared, outcome) {
        (true, ConstructOutcome::Survived) => report.confirmed += 1,
        (true, ConstructOutcome::Lost) => report.violations.push(broken(Outcome::Dropped)),
        (true, ConstructOutcome::Changed { got }) => {
            report.violations.push(broken(Outcome::Changed {
                got: Value::String(got.clone()),
            }))
        }
        (true, ConstructOutcome::Refused { reason }) => {
            report.violations.push(broken(Outcome::Refused {
                reason: reason.clone(),
            }))
        }
        // Undeclared and not carried: the profile told the truth.
        (false, ConstructOutcome::Lost)
        | (false, ConstructOutcome::Changed { .. })
        | (false, ConstructOutcome::Refused { .. }) => report.confirmed += 1,
        // Undeclared and it WORKS — the UNKNOWN resolving itself.
        (false, ConstructOutcome::Survived) => report.prompts.push(TighteningPrompt {
            profile: profile.id().clone(),
            axis,
            leg: Leg("content"),
            key: name.to_string(),
            sent,
            note: format!(
                "{name} is undeclared but round-trips intact — the profile omits a construct \
                 the format carries"
            ),
        }),
    }
}

/// Extensions probed alongside whatever a profile declares — common enough
/// that carrying one silently is a real under-declaration, and varied enough
/// that a format carrying ALL of them is saying something too.
/// Delimiters the negative arm tries against `multi_value.separators`.
///
/// The delimiters an author would plausibly reach for. Every one the profile
/// does NOT declare must leave the field unsplit, so a real separator missing
/// from the declaration shows up here rather than passing unnoticed.
///
/// `\n` and `\r` are deliberately ABSENT, by measurement rather than by
/// assumption: a line-terminating character inside a one-line drawer value is
/// not CONSTRUCTIBLE — the probe read back an EMPTY set, meaning the field was
/// destroyed rather than split, which says nothing about delimiting. NBSP is
/// present and it earns its place: it splits.
pub(crate) const DELIMITER_CANDIDATES: &[&str] = &[" ", ",", ";", "|", "/", "+", "\t", "\u{a0}"];

pub(crate) const NEIGHBOUR_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "tiff", "tif", "pdf", "mp4", "exe",
];

pub(crate) const ALL_BLOCK_CONSTRUCTS: &[BlockConstruct] = &[
    BlockConstruct::Heading,
    BlockConstruct::Paragraph,
    BlockConstruct::SourceBlock,
    BlockConstruct::Quote,
    BlockConstruct::Table,
    BlockConstruct::List,
    BlockConstruct::Image,
    BlockConstruct::Logbook,
    BlockConstruct::PlanningTimestamp,
    BlockConstruct::TodoKeyword,
    BlockConstruct::Priority,
];

pub(crate) const ALL_INLINE_CONSTRUCTS: &[InlineConstruct] = &[
    InlineConstruct::Bold,
    InlineConstruct::Italic,
    InlineConstruct::Underline,
    InlineConstruct::Strikethrough,
    InlineConstruct::Verbatim,
    InlineConstruct::Code,
    InlineConstruct::Subscript,
    InlineConstruct::Superscript,
    InlineConstruct::LinkByName,
    InlineConstruct::LinkById,
    InlineConstruct::LinkExternal,
    InlineConstruct::Tag,
    InlineConstruct::EscapeSequence,
];

/// Axis 6 — the structural refusals.
///
/// Every clause here is about a REFUSAL, so the law's second branch is the
/// whole subject: an over-deep tree, a violated constraint and a cycle must be
/// turned away, never accepted-and-mangled.
fn certify_hierarchy(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let axis = profile.hierarchy();

    // max_depth — an in-bounds tree must survive; `unbounded` means a deep one
    // does too. A limit means one level PAST it must be refused.
    if let Some(outcome) = format.round_trip_depth(6)? {
        report.probed.insert(ClauseId::HierarchyMaxDepth);
        let in_bounds = match axis.max_depth {
            MaxDepth::Unbounded => true,
            MaxDepth::Limit(n) => 6 <= n,
        };
        judge_construct(
            profile,
            report,
            Axis::Hierarchy,
            Clause::MaxDepth,
            "depth-6 tree",
            in_bounds,
            &outcome,
        );
    }

    // Each NAMED constraint must actually refuse its violation. A constraint
    // that is declared but not enforced is the most dangerous kind of clause:
    // a consumer would offer the move believing the boundary will catch it.
    let mut drove_constraint = false;
    for &constraint in &axis.constraints {
        let Some(outcome) = format.violate_constraint(constraint)? else {
            continue;
        };
        drove_constraint = true;
        let name = format!("{constraint:?}");
        report
            .probed_members
            .entry(ClauseId::HierarchyConstraints)
            .or_default()
            .insert(name.clone());
        match &outcome {
            ConstructOutcome::Refused { .. } => report.confirmed += 1,
            ConstructOutcome::Lost => report.confirmed += 1,
            other => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::Hierarchy,
                clause: Clause::HierarchyConstraint(constraint),
                leg: Leg("structure"),
                key: name.clone(),
                sent: Value::String(name),
                outcome: match other {
                    ConstructOutcome::Survived => Outcome::NotRefused,
                    ConstructOutcome::Changed { got } => Outcome::Changed {
                        got: Value::String(got.clone()),
                    },
                    _ => unreachable!("refused and lost are handled above"),
                },
            }),
        }
    }
    if drove_constraint {
        report.probed.insert(ClauseId::HierarchyConstraints);
    }

    if let Some(outcome) = format.introduce_cycle()? {
        report.probed.insert(ClauseId::HierarchyCycles);
        match (axis.cycles, &outcome) {
            (Cycles::Rejected, ConstructOutcome::Refused { .. }) => report.confirmed += 1,
            (Cycles::Rejected, _) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::Hierarchy,
                clause: Clause::Cycles,
                leg: Leg("structure"),
                key: "parent cycle".to_string(),
                sent: Value::String("parent cycle".to_string()),
                outcome: Outcome::NotRefused,
            }),
            (Cycles::Representable, ConstructOutcome::Survived) => report.confirmed += 1,
            (Cycles::Representable, _) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::Hierarchy,
                clause: Clause::Cycles,
                leg: Leg("structure"),
                key: "parent cycle".to_string(),
                sent: Value::String("parent cycle".to_string()),
                outcome: Outcome::Dropped,
            }),
        }
    }
    Ok(())
}

/// Axis 9 — `write_leg`, asked of the FORMAT.
///
/// The previous version compared `write_leg == Absent` against
/// `supports(Mutate)`, and `supports` answers FROM `write_leg` — a tautology
/// that passed for any declaration, including declaring a writable format
/// read-only. The probe now ATTEMPTS A WRITE.
///
/// A declared `absent` on a format that CAN write is a violation, not a
/// tightening prompt: under-claiming is usually harmless, but "this home is
/// read-only" is a claim the user acts on — it un-offers every mutating
/// affordance. Telling someone their data cannot be edited here when it can is
/// a lie they cannot detect.
fn certify_mutation(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let Some(attempt) = format.attempt_write()? else {
        return Ok(());
    };
    report.probed.insert(ClauseId::MutationWriteLeg);

    let declared_absent = profile.mutation().write_leg == WriteLeg::Absent;
    let broken = |outcome: Outcome| Violation {
        profile: profile.id().clone(),
        rev: profile.revision().clone(),
        axis: Axis::Mutation,
        clause: Clause::WriteLeg,
        leg: Leg("write_path"),
        key: "Mutate".to_string(),
        sent: Value::String(format!("write_leg = {:?}", profile.mutation().write_leg)),
        outcome,
    };

    match (declared_absent, &attempt) {
        // Declared writable and it wrote: the offer set must agree.
        (false, WriteAttempt::Wrote { leg }) => {
            if *leg != profile.mutation().write_leg {
                // Declared one mechanism, observed another.
                report.violations.push(broken(Outcome::Changed {
                    got: Value::String(format!("{leg:?}")),
                }));
            } else if profile.supports(&Feature::Mutate).is_offered() {
                report.confirmed += 1;
            } else {
                report.violations.push(broken(Outcome::Dropped));
            }
        }
        (false, _) => report.violations.push(broken(Outcome::Dropped)),
        // Declared read-only: the format must actually refuse, AND every
        // mutating feature must be un-offered.
        (true, WriteAttempt::Refused { .. } | WriteAttempt::NoWriteApi) => {
            if profile.supports(&Feature::Mutate).is_offered() {
                report.violations.push(broken(Outcome::NotRefused));
            } else {
                report.confirmed += 1;
            }
        }
        (true, WriteAttempt::Wrote { .. }) => report.violations.push(broken(Outcome::NotRefused)),
    }
    Ok(())
}

/// Axis 10 — a declared extension must survive; an undeclared one must not be
/// silently accepted and orphaned.
fn certify_assets(
    format: &dyn CertifiableFormat,
    profile: &CapabilityProfile,
    report: &mut CertificationReport,
) -> anyhow::Result<()> {
    let axis = profile.assets();
    let mut drove = false;
    // Every declared extension, PLUS a fixed set of plausible neighbours.
    //
    // Probing only the declared set can never find an UNDER-declaration: an
    // extension the format carries but the profile omits is exactly the case
    // the neighbours exist to surface, and one obviously-wrong outsider
    // (`exe`) is not enough to find it.
    let probes: Vec<Extension> = axis
        .extensions
        .iter()
        .cloned()
        .chain(NEIGHBOUR_EXTENSIONS.iter().map(|e| Extension::new(*e)))
        .collect();
    for ext in &probes {
        let Some(outcome) = format.round_trip_attachment(ext)? else {
            continue;
        };
        drove = true;
        report
            .probed_members
            .entry(ClauseId::AssetsExtensions)
            .or_default()
            .insert(ext.as_str().to_string());
        judge_construct(
            profile,
            report,
            Axis::Assets,
            Clause::AssetExtension,
            ext.as_str(),
            axis.extensions.contains(ext),
            &outcome,
        );
    }
    if drove {
        report.probed.insert(ClauseId::AssetsExtensions);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A format that carries EVERYTHING back unchanged.
    ///
    /// Exists to drive the under-claim arms against a profile that says a
    /// value is lost. No real format in 2b.1 carries the empty value on any
    /// leg, so without this stub the `dropped`-but-survives branch would ship
    /// unexercised — which is the exact defect (a clause nothing drives) that
    /// this symmetry was added to fix.
    struct LosslessStub {
        profile: CapabilityProfile,
    }

    const STUB_LEG: Carrier = Carrier {
        leg: Leg("stub"),
        description: "an ideal carrier that loses nothing",
    };

    impl CertifiableFormat for LosslessStub {
        fn profile(&self) -> &CapabilityProfile {
            &self.profile
        }

        fn carriers(&self) -> &'static [Carrier] {
            &[STUB_LEG]
        }

        fn round_trip_property(
            &self,
            _: Carrier,
            _: &str,
            value: &Value,
        ) -> anyhow::Result<Readback> {
            Ok(Readback::Present(value.clone()))
        }
    }

    fn stub_with(empty_string: &str) -> LosslessStub {
        let yaml = crate::fixture::minimal_with(
            "empty_string: representable",
            &format!("empty_string: {empty_string}"),
        );
        LosslessStub {
            profile: CapabilityProfile::from_yaml(&yaml).expect("stub profile parses"),
        }
    }

    /// The under-claim arm: the profile says the empty value is lost, the
    /// format carries it. That is a PROMPT, never a failure (CV-C).
    #[test]
    fn an_empty_string_declared_dropped_but_carried_raises_a_prompt() {
        let report = certify(&stub_with("dropped")).expect("certification runs");
        assert!(
            report.is_clean(),
            "under-claiming must never fail the gate: {}",
            report.render()
        );
        let prompt = report
            .prompts
            .iter()
            .find(|p| p.sent == Value::String(String::new()))
            .unwrap_or_else(|| {
                panic!(
                    "a format that carries the empty value must contradict a `dropped` \
                     declaration:\n{}",
                    report.render()
                )
            });
        assert!(
            prompt.note.contains("under-claims"),
            "the prompt must say the profile under-claims; got: {}",
            prompt.note
        );
    }

    /// The other arm still holds: declared representable AND carried is simply
    /// confirmed, with nothing raised.
    #[test]
    fn an_empty_string_declared_representable_and_carried_is_confirmed() {
        let report = certify(&stub_with("representable")).expect("certification runs");
        assert!(report.is_clean(), "{}", report.render());
        assert!(
            !report
                .prompts
                .iter()
                .any(|p| p.sent == Value::String(String::new())),
            "a correct declaration must raise nothing: {}",
            report.render()
        );
    }

    /// A format whose boundary REFUSES everything, loudly.
    ///
    /// Drives the `Representability::Error` arm and the declared-but-refused
    /// violation. Without it those branches would ship unexercised — the F1
    /// defect, repeated.
    /// A format whose boundary refuses a VALUE, loudly — but not the operation.
    ///
    /// The control key must survive: a stub that refused everything would have
    /// a CLOSED boundary, and `boundary_is_closed` correctly drives nothing
    /// through one, because a blanket refusal is evidence about the operation
    /// and not about the value. Selective refusal is the case this stub exists
    /// to drive.
    struct RefusingStub {
        profile: CapabilityProfile,
    }

    impl CertifiableFormat for RefusingStub {
        fn profile(&self) -> &CapabilityProfile {
            &self.profile
        }

        fn carriers(&self) -> &'static [Carrier] {
            &[STUB_LEG]
        }

        fn round_trip_property(
            &self,
            _: Carrier,
            key: &str,
            value: &Value,
        ) -> anyhow::Result<Readback> {
            if key == "Plain" {
                return Ok(Readback::Present(value.clone()));
            }
            Ok(Readback::Refused {
                reason: format!("stub refuses {key}"),
            })
        }
    }

    fn refusing_with(empty_string: &str) -> RefusingStub {
        RefusingStub {
            profile: stub_with(empty_string).profile,
        }
    }

    /// `empty_string: error` CONFIRMS only against an actual refusal.
    #[test]
    fn an_empty_string_declared_error_is_confirmed_by_a_real_refusal() {
        let report = certify(&refusing_with("error")).expect("certification runs");
        assert!(
            report
                .violations
                .iter()
                .all(|v| v.clause != Clause::EmptyString),
            "a boundary that refuses the empty value satisfies `error`:\n{}",
            report.render()
        );
    }

    /// The arm that makes `error` falsifiable: declared refused, actually
    /// carried. A silent take-and-lose would fail the same way.
    #[test]
    fn an_empty_string_declared_error_but_carried_is_a_violation() {
        let report = certify(&stub_with("error")).expect("certification runs");
        let v = report
            .violations
            .iter()
            .find(|v| v.clause == Clause::EmptyString)
            .unwrap_or_else(|| {
                panic!(
                    "declaring a refusal the boundary does not perform must be RED:\n{}",
                    report.render()
                )
            });
        assert_eq!(v.outcome, Outcome::NotRefused, "{}", report.render());
    }

    /// A refusal of something the profile DECLARES carried is a violation —
    /// the law's two legal branches are lossless OR refused, and `types:
    /// [string]` picked the first one.
    #[test]
    fn refusing_a_declared_type_is_a_violation() {
        let report = certify(&refusing_with("dropped")).expect("certification runs");
        let v = report
            .violations
            .iter()
            .find(|v| v.clause == Clause::TypeDeclared(ValueKind::String))
            .unwrap_or_else(|| {
                panic!(
                    "a declared-carried type that is REFUSED must be red:\n{}",
                    report.render()
                )
            });
        assert!(
            matches!(v.outcome, Outcome::Refused { .. }),
            "the outcome must name the refusal, not collapse to Dropped: {:?}",
            v.outcome
        );
    }

    /// A format that SCRAMBLES property order and picks an identity carrier
    /// silently.
    ///
    /// Org does neither, so without this stub the RED arms of both new axes
    /// would ship unexercised — a clause that can only be confirmed and never
    /// broken is not a falsifiable clause.
    struct SloppyStub {
        profile: CapabilityProfile,
    }

    impl CertifiableFormat for SloppyStub {
        fn profile(&self) -> &CapabilityProfile {
            &self.profile
        }

        fn carriers(&self) -> &'static [Carrier] {
            &[STUB_LEG]
        }

        fn round_trip_property(
            &self,
            _: Carrier,
            _: &str,
            value: &Value,
        ) -> anyhow::Result<Readback> {
            Ok(Readback::Present(value.clone()))
        }

        fn round_trip_property_order(
            &self,
            authored: &[&str],
        ) -> anyhow::Result<Option<Vec<String>>> {
            let mut scrambled: Vec<String> = authored.iter().map(|k| k.to_string()).collect();
            scrambled.reverse();
            Ok(Some(scrambled))
        }

        fn carriers_disagree(&self) -> anyhow::Result<Option<DisagreementOutcome>> {
            Ok(Some(DisagreementOutcome::Picked {
                carrier: crate::axes::IdCarrier::DrawerId,
            }))
        }
    }

    /// The sloppy stub DRIVES the two order/identity clauses, so its profile
    /// must not also excuse them — a marker that has gone stale is itself a
    /// finding, which is the point of `MarkedButDriven`.
    fn sloppy() -> SloppyStub {
        // The leading newline ANCHORS the match to a 2-space `not_yet_certified`
        // entry. Without it, `"  - x"` is a substring of the 4-space
        // `enforced_by` entry `"    - x"` and the strip silently corrupts the
        // enforcement map instead — which is exactly what it did on the first
        // attempt, and the loud parse error is what caught it.
        let yaml = crate::fixture::MINIMAL
            .replace("\n  - ordering_property_order\n", "\n")
            .replace("\n  - identity_carrier_disagreement\n", "\n");
        SloppyStub {
            profile: CapabilityProfile::from_yaml(&yaml).expect("sloppy profile parses"),
        }
    }

    /// `property_order: preserved` is RED when the order comes back changed.
    #[test]
    fn a_scrambled_property_order_breaks_a_preserved_declaration() {
        let report = certify(&sloppy()).expect("certification runs");
        let v = report
            .violations
            .iter()
            .find(|v| v.clause == Clause::PropertyOrder)
            .unwrap_or_else(|| {
                panic!(
                    "declaring `preserved` against a format that reorders must be RED:\n{}",
                    report.render()
                )
            });
        assert!(
            matches!(v.outcome, Outcome::Changed { .. }),
            "the finding must show what came back: {:?}",
            v.outcome
        );
    }

    /// `carrier_disagreement: error` is RED when the format silently picks —
    /// the case where an authored identity is discarded without a word.
    #[test]
    fn a_silent_carrier_pick_breaks_an_error_declaration() {
        let report = certify(&sloppy()).expect("certification runs");
        let v = report
            .violations
            .iter()
            .find(|v| v.clause == Clause::CarrierDisagreement)
            .unwrap_or_else(|| {
                panic!(
                    "declaring a loud error against a format that picks silently must be \
                     RED:\n{}",
                    report.render()
                )
            });
        assert!(
            matches!(&v.outcome, Outcome::Changed { got } if format!("{got:?}").contains("picked")),
            "the finding must name the carrier that silently won: {:?}",
            v.outcome
        );
    }

    /// Sanity: the stub is not vacuously green. It carries an Integer intact
    /// while the profile declares `types: [string]`, so the undeclared-but-
    /// works arm must fire.
    #[test]
    fn the_stub_still_reports_undeclared_kinds_that_survive() {
        let report = certify(&stub_with("representable")).expect("certification runs");
        assert!(
            report
                .prompts
                .iter()
                .any(|p| matches!(p.sent, Value::Integer(_))),
            "a lossless format must contradict `types: [string]`: {}",
            report.render()
        );
    }

    /// A format that keeps sibling order WITHOUT an order key, and hosts only
    /// hierarchical entities.
    ///
    /// Both observations live in `certify_misc`, in that order. The stub exists
    /// to prove the first one does not swallow the second.
    struct OrderedStub {
        profile: CapabilityProfile,
    }

    impl CertifiableFormat for OrderedStub {
        fn profile(&self) -> &CapabilityProfile {
            &self.profile
        }

        fn carriers(&self) -> &'static [Carrier] {
            &[STUB_LEG]
        }

        fn round_trip_property(
            &self,
            _: Carrier,
            _: &str,
            value: &Value,
        ) -> anyhow::Result<Readback> {
            Ok(Readback::Present(value.clone()))
        }

        fn round_trip_sibling_order(
            &self,
            siblings: &[&str],
        ) -> anyhow::Result<Option<Vec<String>>> {
            Ok(Some(siblings.iter().map(|s| s.to_string()).collect()))
        }

        fn writes_explicit_order_key(&self) -> anyhow::Result<Option<bool>> {
            Ok(Some(false))
        }

        fn all_entities_hierarchical(&self) -> anyhow::Result<Option<bool>> {
            Ok(Some(true))
        }
    }

    /// A sibling-order violation must not abort the rest of its pass.
    ///
    /// The arm used to `return Ok(())`, so a wrong `sibling_order` skipped the
    /// `hosted_kinds` probe further down and the run reported a coverage GAP
    /// against a clause the format does drive — a false accusation produced by
    /// an unrelated red.
    #[test]
    fn a_sibling_order_violation_still_leaves_the_later_clauses_probed() {
        let yaml = crate::fixture::MINIMAL
            .replace(
                "\n  - clause: ordering_sibling_order\n    reason: no stub in this crate drives it; the org harness does",
                "",
            )
            .replace(
                "\n  - clause: hosted_kinds\n    reason: no stub in this crate drives it; the org harness does",
                "",
            )
            // The order-key observation drives this one as a side effect.
            .replace(
                "\n  - clause: ordering_order_key_durable\n    reason: no stub in this crate drives it; the org harness does",
                "",
            )
            .replace(
                "sibling_order: file_position",
                "sibling_order: fractional_index",
            );
        let stub = OrderedStub {
            profile: CapabilityProfile::from_yaml(&yaml).expect("stub profile parses"),
        };
        let report = certify(&stub).expect("certification runs");

        assert_eq!(
            report
                .violations
                .iter()
                .filter(|v| v.clause == Clause::SiblingOrder)
                .count(),
            1,
            "order without a key on disk is file_position, not fractional_index:\n{}",
            report.render()
        );
        assert!(
            report.gaps.is_empty(),
            "the run drives hosted_kinds too — a gap here means the sibling-order red \
             aborted the pass:\n{}",
            report.render()
        );
        assert!(
            report.probed.contains(&ClauseId::HostedKinds),
            "hosted_kinds must be recorded as driven:\n{}",
            report.render()
        );
    }
}
