//! The self-attestation's verdict as a function of a draw count: it must not
//! depend on the process-wide tally, which sibling tests share under
//! `cargo test`.

use holon_integration_tests::pbt::alphabet_coverage::attest_drew;
use holon_integration_tests::pbt::alphabet_coverage::attested;
use holon_integration_tests::pbt::alphabet_coverage::note_drawn;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

use crate::attestation_oracle::Tally;
use crate::attestation_oracle::any_kind_text;
use crate::attestation_oracle::fresh_kind;
use crate::attestation_oracle::refusal_text;
use crate::attestation_oracle::tally_op;

fn expected(drawn: u64, kind: &str) -> Result<(), String> {
    if drawn == 0 {
        Err(refusal_text(kind))
    } else {
        Ok(())
    }
}

#[test]
fn a_zero_draw_attestation_is_refused_whatever_the_process_tally_says() {
    let kind = fresh_kind("StructuralKeyBurst");
    note_drawn(kind);
    attest_drew(kind);
    assert_eq!(attested(0, kind), Err(refusal_text(kind)));
}

fn drawn_count() -> impl Strategy<Value = u64> {
    prop_oneof![Just(0), Just(1), Just(u64::MAX), any::<u64>()]
}

fn check_verdict(drawn: u64, kind: &str) -> Result<(), TestCaseError> {
    prop_assert_eq!(attested(drawn, kind), expected(drawn, kind));
    Ok(())
}

#[test]
fn an_attestation_holds_exactly_when_its_run_drew_the_kind() {
    for (drawn, kind) in [
        (0, "StructuralKeyBurst"),
        (1, "StructuralKeyBurst"),
        (u64::MAX, ""),
        (0, ""),
    ] {
        check_verdict(drawn, kind).unwrap_or_else(|e| panic!("static ({drawn}, {kind:?}): {e}"));
    }
    const POOL: usize = 3;
    let case = (
        drawn_count(),
        any_kind_text(),
        prop::collection::vec(tally_op(POOL), 0..12),
    );
    let outcome = TestRunner::default().run(&case, |(drawn, kind, ops)| {
        check_verdict(drawn, &kind)?;
        let own = fresh_kind(&kind);
        let kinds = [own; POOL];
        let mut tally = Tally::default();
        for op in &ops {
            tally.apply(op, &kinds);
            check_verdict(drawn, own)?;
        }
        Ok(())
    });
    if let Err(e) = outcome {
        panic!("{e}");
    }
}
