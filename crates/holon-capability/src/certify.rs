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

use holon_api::Value;

use crate::axes::Representability;
use crate::axes::ValueKind;
use crate::profile::CapabilityProfile;
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
#[derive(Debug, Clone, PartialEq)]
pub enum Readback {
    /// The key is gone.
    Absent,
    /// The key is present, carrying this value.
    Present(Value),
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
}

/// Everything one certification run learned.
#[derive(Debug, Default)]
pub struct CertificationReport {
    pub violations: Vec<Violation>,
    pub prompts: Vec<TighteningPrompt>,
    /// Cases that behaved exactly as declared. Counted so a run that generated
    /// NOTHING cannot masquerade as a pass.
    pub confirmed: usize,
}

impl CertificationReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// A human-readable dump — the red log's content.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "confirmed: {}  violations: {}  tightening prompts: {}\n",
            self.confirmed,
            self.violations.len(),
            self.prompts.len(),
        ));
        for v in &self.violations {
            out.push_str(&format!("VIOLATION  {v}\n"));
        }
        for p in &self.prompts {
            out.push_str(&format!("TIGHTENING {p}\n"));
        }
        out
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

    Ok(report)
}

/// Axis 3 — a key the profile does NOT reserve must survive with its value; a
/// key it DOES reserve is expected not to, and its loss is honest rather than
/// a violation.
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
    let probes = ["Plain", "_underscored", "ID"];
    let carried = Value::String("carried".to_string());

    for key in probes {
        // A key the format OWNS is outside the ordinary-property law entirely:
        // it is not claimed to vanish and not an ordinary carrier, so neither
        // its survival nor its loss is evidence about this axis.
        if axis.is_owned(key) {
            continue;
        }
        let readback = format.round_trip_property(carrier, key, &carried)?;
        let reserved = axis.is_prefix_reserved(key);
        match (&readback, reserved) {
            (Readback::Present(got), false) if *got == carried => report.confirmed += 1,
            (Readback::Present(got), false) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyKeys,
                clause: Clause::KeyNotReserved,
                leg: carrier.leg,
                key: key.to_string(),
                sent: carried.clone(),
                outcome: Outcome::Changed { got: got.clone() },
            }),
            (Readback::Absent, false) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyKeys,
                clause: Clause::KeyNotReserved,
                leg: carrier.leg,
                key: key.to_string(),
                sent: carried.clone(),
                outcome: Outcome::Dropped,
            }),
            (Readback::Absent, true) => report.confirmed += 1,
            (Readback::Present(_), true) => report.prompts.push(TighteningPrompt {
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
    let key = "Probe";

    for &kind in ALL_KINDS {
        let sent = specimen(kind);
        let readback = format.round_trip_property(carrier, key, &sent)?;
        let declared = axis.types.contains(&kind);
        let intact = matches!(&readback, Readback::Present(got) if *got == sent);

        match (declared, intact, &readback) {
            (true, true, _) => report.confirmed += 1,
            (true, false, Readback::Absent) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyValues,
                clause: Clause::TypeDeclared(kind),
                leg: carrier.leg,
                key: key.to_string(),
                sent: sent.clone(),
                outcome: Outcome::Dropped,
            }),
            (true, false, Readback::Present(got)) => report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyValues,
                clause: Clause::TypeDeclared(kind),
                leg: carrier.leg,
                key: key.to_string(),
                sent: sent.clone(),
                outcome: Outcome::Changed { got: got.clone() },
            }),
            // Undeclared and lost or altered: exactly what the profile says.
            (false, false, _) => report.confirmed += 1,
            (false, true, _) => report.prompts.push(TighteningPrompt {
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

    // `empty_string` is its own clause: it is an INHABITANT question, not a
    // kind question, and the two disagree in org — an empty value survives the
    // renderer and is lost at ingest.
    //
    // SYMMETRIC on purpose. Probing only the `Representable` arm would make a
    // FALSE `dropped` uncertifiable: a format that actually carries the empty
    // value could declare it lost and nothing would ever contradict that.
    // Under-claiming is exactly what CV-C rules must surface as a tightening
    // prompt, so both arms are driven and each declaration can be wrong.
    let empty = Value::String(String::new());
    let readback = format.round_trip_property(carrier, key, &empty)?;
    let survived = matches!(&readback, Readback::Present(got) if *got == empty);
    match (axis.empty_string, &readback) {
        (Representability::Representable, _) if survived => report.confirmed += 1,
        (Representability::Representable, Readback::Present(got)) => {
            report.violations.push(Violation {
                profile: profile.id().clone(),
                rev: profile.revision().clone(),
                axis: Axis::PropertyValues,
                clause: Clause::EmptyString,
                leg: carrier.leg,
                key: key.to_string(),
                sent: empty.clone(),
                outcome: Outcome::Changed { got: got.clone() },
            })
        }
        (Representability::Representable, Readback::Absent) => report.violations.push(Violation {
            profile: profile.id().clone(),
            rev: profile.revision().clone(),
            axis: Axis::PropertyValues,
            clause: Clause::EmptyString,
            leg: carrier.leg,
            key: key.to_string(),
            sent: empty.clone(),
            outcome: Outcome::Dropped,
        }),
        // Declared lost and lost (or altered — an altered empty is not the
        // empty value coming back, so the declaration still holds).
        (Representability::Dropped, _) if !survived => report.confirmed += 1,
        (Representability::Dropped, _) => report.prompts.push(TighteningPrompt {
            profile: profile.id().clone(),
            axis: Axis::PropertyValues,
            leg: carrier.leg,
            key: key.to_string(),
            sent: empty.clone(),
            note: "declared dropped, but this leg carried the empty value intact — \
                   property_values.empty_string under-claims the format"
                .to_string(),
        }),
        // `Error` means the boundary REFUSES an empty value. Certifying that
        // needs a refusal signal the round trip cannot express today; see the
        // plan's 2b.2 note on `Readback::Refused`.
        (Representability::Error, _) => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::Representability;

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
        let yaml = format!(
            "profile: stub\nfidelity_axes:\n  property_keys:\n    charset: any\n    case: \
             sensitive\n    reserved_prefixes: []\n    reserved_keys: []\n    collision: \
             last_wins\n    schema_required: open\n  property_values:\n    types: [string]\n    \
             empty_string: {empty_string}\n    null: dropped\n    multi_value:\n      kind: \
             none\n    reference_values: none\n"
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
}
