//! Names a caller or a net author passes: near misses of the names a net
//! declares, the names the executor generates, and any other text.

use proptest::prelude::*;
use proptest::sample::select;

pub fn other_name() -> impl Strategy<Value = String> {
    prop_oneof![
        select(vec![
            "Nope", "", " ", "t0", "T0 ", " T0", "T", "T00", "p0", "P0 ", "Ghost", "D9", "W9",
        ])
        .prop_map(str::to_owned),
        select(vec!["T0", "W0", "P0"]).prop_flat_map(|n| {
            select(vec![
                "Running",
                "RetryPending",
                "OutcomeUnknown",
                "UndoPending",
                "x",
            ])
            .prop_map(move |r| format!("{n}.{r}"))
        }),
        select(vec![
            "Answer",
            "Asks",
            "Answered",
            "Expired",
            "WorkFailed",
            "ClearFailed"
        ])
        .prop_map(|r| format!("{r}.T0")),
        ".{0,8}",
    ]
}
