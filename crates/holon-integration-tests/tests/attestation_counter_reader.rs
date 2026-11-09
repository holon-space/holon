//! `attest_drew` reads the suite's process-wide draw counter: it accepts a
//! kind exactly when this process drew it, on any thread, and refuses every
//! other kind with the refusal text.

use std::panic::catch_unwind;

use holon_integration_tests::pbt::alphabet_coverage::attest_drew;
use proptest::prelude::*;
use proptest::test_runner::TestRunner;

use crate::attestation_oracle::Tally;
use crate::attestation_oracle::TallyOp;
use crate::attestation_oracle::fresh_kind;
use crate::attestation_oracle::leak;
use crate::attestation_oracle::refusal_text;
use crate::attestation_oracle::tally_op;

const POOL: usize = 3;

#[derive(Clone, Debug)]
enum Op {
    Tally(TallyOp),
    Attest(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => tally_op(POOL).prop_map(Op::Tally),
        1 => (0..POOL).prop_map(Op::Attest),
    ]
}

fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(text) => *text,
        Err(payload) => (*payload
            .downcast::<&'static str>()
            .expect("a panic payload is a String or a &str"))
        .to_owned(),
    }
}

#[test]
fn attest_drew_accepts_exactly_the_kinds_this_process_drew() {
    let case = (
        any::<String>(),
        any::<String>(),
        any::<String>(),
        prop::collection::vec(op(), 0..40),
    );
    let outcome = TestRunner::default().run(&case, |(base, s1, s2, ops)| {
        let base = fresh_kind(&base);
        let kinds = [
            base,
            leak(format!("{base}{s1}")),
            leak(format!("{base}{s2}")),
        ];
        let mut tally = Tally::default();
        for op in &ops {
            match op {
                Op::Tally(op) => tally.apply(op, &kinds),
                Op::Attest(k) => {
                    let kind = kinds[*k];
                    let outcome = catch_unwind(|| attest_drew(kind)).map_err(panic_text);
                    let expected = if tally.drawn(kind) > 0 {
                        Ok(())
                    } else {
                        Err(refusal_text(kind))
                    };
                    prop_assert_eq!(outcome, expected, "attest_drew({:?})", kind);
                }
            }
        }
        Ok(())
    });
    if let Err(e) = outcome {
        panic!("{e}");
    }
}
