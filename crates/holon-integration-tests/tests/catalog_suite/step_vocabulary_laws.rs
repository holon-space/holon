//! The laws the generated step vocabulary must obey, over the WHOLE catalog.
//!
//! - (a) ambiguity — the registration-time structural refusal runs here, and
//!   the round-trip property below is its empirical half: every rendered step
//!   must resolve to exactly one variant.
//! - (b) round trip — `parse(render(t)) == t`, compared on serde values
//!   (transitions are not `PartialEq`).
//! - (c)/(e) coverage — the derive refuses uncovered fields and unknown
//!   placeholders at COMPILE time; `check_step_vocabulary` re-checks the serde
//!   key set, which the derive cannot see.
//! - (d) quoting by type — exercised by the adversarial `String` examples
//!   (quotes, backslashes, and a fragment that mimics another template's
//!   literal segment) that every string-ish field draws.

#![cfg(feature = "pbt")]

use holon_integration_tests::pbt::transitions::E2ETransition;
use proptest::prelude::*;

/// (a), layer 1 + (c)/(e), runtime half.
#[test]
fn the_catalog_registers_without_ambiguity_or_uncovered_fields() {
    E2ETransition::check_step_vocabulary().expect("the step vocabulary must register");
    let catalog = E2ETransition::step_catalog();
    assert!(
        catalog.len() >= 60,
        "expected every declared variant to carry a template, saw {}",
        catalog.len()
    );
    for (variant, template) in &catalog {
        assert!(
            !template.trim().is_empty(),
            "{variant} declares an empty step template"
        );
    }
}

#[test]
fn every_variant_contributes_examples() {
    let examples = E2ETransition::step_catalog_examples();
    assert!(
        examples.len() >= E2ETransition::step_catalog().len(),
        "every variant must contribute at least one example value, saw {} for {} variants",
        examples.len(),
        E2ETransition::step_catalog().len()
    );
}

/// A docstring under a step whose transition reads no document is author
/// intent the vocabulary cannot honour — refused, never dropped.
#[test]
fn a_stray_docstring_is_a_loud_refusal() {
    let step = r#"I focus block "block:blk-a" in region "main""#;
    E2ETransition::parse_step(step, None).expect("the step alone must parse");
    let err = E2ETransition::parse_step(step, Some("* stray\n"))
        .expect_err("a docstring this step cannot carry must be refused");
    assert!(err.contains("takes no docstring"), "{err}");

    // The one document-carrying step still REQUIRES its docstring.
    let org_step = r#"an org file "x.org":"#;
    E2ETransition::parse_step(org_step, Some("* Hello\n")).expect("org step with docstring");
    E2ETransition::parse_step(org_step, None)
        .expect_err("the org-file step without its document must be refused");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// (b) + (a), layer 2: rendering a transition and reading the step back
    /// yields the SAME value, resolved through exactly one variant.
    #[test]
    fn parse_of_render_is_the_identity(
        transition in proptest::sample::select(E2ETransition::step_catalog_examples())
    ) {
        let rendered = transition.render_step();
        let parsed = E2ETransition::parse_step(&rendered.text, rendered.docstring.as_deref())
            .map_err(|e| TestCaseError::fail(format!(
                "{} rendered {:?} which does not read back: {e}",
                transition.variant_name(), rendered.text,
            )))?;
        prop_assert_eq!(
            parsed.variant_name(),
            transition.variant_name(),
            "step {:?} read back as a different variant",
            rendered.text
        );
        prop_assert_eq!(
            holon_pbt_core::step_vocabulary::comparable_step_value(
                serde_json::to_value(&parsed).unwrap()
            ),
            holon_pbt_core::step_vocabulary::comparable_step_value(
                serde_json::to_value(&transition).unwrap()
            ),
            "step {:?} read back as a different value",
            rendered.text
        );
    }
}
