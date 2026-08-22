//! Certifies `crates/holon-org-format/profile.yaml` against the REAL org
//! round trip — Increment 2b.1, axes 3 (`property_keys`) and 4
//! (`property_values`).
//!
//! The `CertifiableFormat` impl lives HERE, in an integration-test target,
//! never in `src/`. `holon-org-format` must not gain a non-test dependency on
//! `holon-capability`, or the format crate would start reading its own profile
//! at runtime and the profile would stop being an independent statement ABOUT
//! it. Pinned by `crates/holon-architecture-tests/tests/architecture_rules.rs`.
//!
//! ## Org has TWO property carriers and they fail DIFFERENTLY
//!
//! That is why every finding names its leg:
//!
//! * `org_properties_json` — the drawer is rendered straight from the
//!   `org_properties` JSON string. A non-string is STRINGIFIED
//!   (`models.rs:186-189` and `:199-202`, `_ => value.to_string()`), so an
//!   integer reaches disk as the text `42` and returns as `Value::String`.
//! * `flat_properties` — no `org_properties` is set, so the renderer rebuilds
//!   the drawer from `drawer_properties()`, whose flat leg gates on
//!   `Value::as_string()` (`models.rs:910`). `as_string` is `None` for every
//!   non-`String` variant (`crates/holon-pattern/src/value.rs:85-90`), so the
//!   value is DROPPED.
//!
//! Both legs erase `_`-prefixed keys (`models.rs:886`, `:894`, `:909`).

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Value;
use holon_capability::CapabilityProfile;
use holon_capability::Carrier;
use holon_capability::CertifiableFormat;
use holon_capability::Leg;
use holon_capability::Readback;
use holon_capability::certify;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/certify";
const FILE: &str = "/certify/profile.org";
const BLOCK_ID: &str = "certify-block";
const PROBE_HEADLINE: &str = "Probe headline";

/// A one-headline document carrying nothing but its identity. Every probe
/// starts from this, ingested, so the property under test is the ONLY thing
/// that varies.
const BASE_FIXTURE: &str = "#+TITLE: Certify\n\n* Probe headline\n:PROPERTIES:\n:ID: \
                            certify-block\n:END:\n";

const JSON_LEG: Carrier = Carrier {
    leg: Leg("org_properties_json"),
    description: "drawer rendered from the org_properties JSON string",
};
const FLAT_LEG: Carrier = Carrier {
    leg: Leg("flat_properties"),
    description: "drawer rebuilt from the flat properties bag",
};

struct OrgFormat {
    profile: CapabilityProfile,
}

impl OrgFormat {
    fn load() -> Self {
        let yaml = include_str!("../profile.yaml");
        Self {
            profile: CapabilityProfile::from_yaml(yaml).expect("the org profile yaml must parse"),
        }
    }
}

/// A `Value` as the `org_properties` JSON carrier would hold it — the shape
/// the renderer's `serde_json::Value` match arms actually see.
fn as_json(value: &Value) -> serde_json::Value {
    match value {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Integer(i) => serde_json::Value::from(*i),
        Value::Float(f) => serde_json::Value::from(*f),
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
        }
        Value::Array(items) => serde_json::Value::Array(items.iter().map(as_json).collect()),
        Value::Object(map) => {
            serde_json::Value::Object(map.iter().map(|(k, v)| (k.clone(), as_json(v))).collect())
        }
        Value::Null => serde_json::Value::Null,
    }
}

impl CertifiableFormat for OrgFormat {
    fn profile(&self) -> &CapabilityProfile {
        &self.profile
    }

    fn carriers(&self) -> &'static [Carrier] {
        &[JSON_LEG, FLAT_LEG]
    }

    fn round_trip_property(
        &self,
        carrier: Carrier,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        let path = Path::new(FILE);
        // The base comes from a real INGEST, not a hand-built Block: the
        // round trip under test is the production loop (parse → mutate →
        // render → parse), so the fixture must enter it the way a vault file
        // does.
        let base = parse_org_file(path, BASE_FIXTURE, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| anyhow::anyhow!("the base fixture must parse: {e}"))?;
        let doc = base.document.clone();
        let mut block = base
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the base fixture must carry the probe block"))?;

        match carrier.leg {
            Leg("org_properties_json") => {
                let mut props = serde_json::Map::new();
                props.insert("ID".to_string(), serde_json::Value::String(BLOCK_ID.into()));
                props.insert(key.to_string(), as_json(value));
                block.set_org_properties(Some(serde_json::to_string(&props)?));
            }
            Leg("flat_properties") => block.set_property(key, value.clone()),
            other => anyhow::bail!("org has no carrier named {other}"),
        }

        let rendered = OrgRenderer::render_document(&doc, &[block], path, &doc.id);
        let parsed = parse_org_file(path, &rendered, &EntityUri::no_parent(), Path::new(ROOT))
            .map_err(|e| {
                anyhow::anyhow!("the rendered fixture must parse back: {e}\n{rendered}")
            })?;

        // Located by HEADLINE, not by id: a probe key is allowed to be one the
        // format owns, and writing `ID` changes the block's identity. Looking
        // it up by id would turn that legitimate case into a harness failure.
        let back = parsed
            .blocks
            .iter()
            .find(|b| b.org_title() == PROBE_HEADLINE)
            .ok_or_else(|| {
                anyhow::anyhow!("the probe block vanished from the round trip:\n{rendered}")
            })?;

        Ok(match back.get_property(key) {
            Some(v) => Readback::Present(v),
            None => Readback::Absent,
        })
    }
}

/// The whole increment in one assertion: every clause the org profile declares
/// is REAL.
///
/// A red here means either the adapter or the profile is lying, and the
/// `Violation` payload names which axis, which clause, which leg and which
/// value — enough to act on without a debugger.
#[test]
fn the_org_profile_declares_only_restrictions_that_are_real() {
    let format = OrgFormat::load();
    let report = certify(&format).expect("the certification harness must run");

    println!("{}", report.render());

    assert!(
        report.confirmed > 0,
        "a run that generated NOTHING must not pass as clean:\n{}",
        report.render()
    );
    assert!(
        report.is_clean(),
        "the org profile declares {} restriction(s) the format does not honour:\n{}",
        report.violations.len(),
        report.render()
    );
}

/// The CONTROL, and it must hold under a LYING profile too.
///
/// A certifier that fails on everything proves nothing. This pins the other
/// side of the discrimination: an ordinary string property on an unreserved
/// key survives both legs intact, so a red in the test above is attributable
/// to the clause it names rather than to a broken harness.
#[test]
fn a_plain_string_property_survives_both_legs() {
    let format = OrgFormat::load();
    let sent = Value::String("carried".to_string());

    for carrier in format.carriers() {
        let back = format
            .round_trip_property(*carrier, "Plain", &sent)
            .expect("the harness must run");
        assert_eq!(
            back,
            Readback::Present(sent.clone()),
            "the control must survive the {} leg — if it does not, the harness is broken and \
             every other verdict in this file is worthless",
            carrier.leg,
        );
    }
}
