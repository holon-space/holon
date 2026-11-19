//! The case budget of the slow composed-harness property tests: a lane run
//! draws fewer cases than the integrate gate. `scripts/cargo-shape-env.sh`
//! sets [`BUDGET_VAR`] per shape; no build script reads it, so switching it
//! rebuilds nothing.

use std::env;
use std::env::VarError;

pub const BUDGET_VAR: &str = "HOLON_PBT_BUDGET";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseBudget {
    Lane,
    Gate,
}

impl CaseBudget {
    /// Unset is the gate budget, so a run outside the shape env draws the full
    /// count.
    pub fn from_env() -> Self {
        match env::var(BUDGET_VAR) {
            Err(VarError::NotPresent) => Self::Gate,
            Ok(v) => Self::parse(&v),
            Err(e) => panic!("{BUDGET_VAR}: {e}"),
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "lane" => Self::Lane,
            "gate" => Self::Gate,
            other => panic!("{BUDGET_VAR} must be `lane` or `gate`, got {other:?}"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Lane => "lane",
            Self::Gate => "gate",
        }
    }
}

/// The case count of `test`: `override_var` when it is set, else the count of
/// the budget. Prints one `[pbt-budget]` line naming the count and its source.
pub fn composed_cases(test: &str, override_var: &str, lane: u32, gate: u32) -> u32 {
    assert!(
        0 < lane && lane <= gate,
        "{test}: lane count {lane} must be in 1..={gate}"
    );
    let (cases, source) = match env::var(override_var) {
        Ok(v) => (parse_cases(override_var, &v), format!("{override_var}={v}")),
        Err(VarError::NotPresent) => {
            let budget = CaseBudget::from_env();
            let cases = match budget {
                CaseBudget::Lane => lane,
                CaseBudget::Gate => gate,
            };
            (cases, format!("{BUDGET_VAR} budget {}", budget.name()))
        }
        Err(e) => panic!("{override_var}: {e}"),
    };
    eprintln!("[pbt-budget] {test}: {cases} cases ({source})");
    cases
}

/// The case count `var` names, else `default` when it is unset. A value that
/// is not a positive decimal count panics naming `var` and the text.
pub fn cases_or(var: &str, default: u32) -> u32 {
    cases_from(var, env::var(var), default)
}

fn cases_from(var: &str, value: Result<String, VarError>, default: u32) -> u32 {
    match value {
        Ok(v) => parse_cases(var, &v),
        Err(VarError::NotPresent) => default,
        Err(e) => panic!("{var}: {e}"),
    }
}

fn parse_cases(var: &str, value: &str) -> u32 {
    match value.parse::<u32>() {
        // `u32::from_str` takes a leading `+`; a case count is bare digits.
        Ok(n) if n > 0 && value.bytes().all(|b| b.is_ascii_digit()) => n,
        Ok(_) => panic!("{var} must be a positive case count, got {value:?}"),
        Err(e) => panic!("{var} must be a positive case count, got {value:?}: {e}"),
    }
}

/// The budget and `parse_cases` cases are examples, not a PBT: the input is
/// one setting with two values and an invalid class.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_budget_values_parse() {
        assert_eq!(CaseBudget::parse("lane"), CaseBudget::Lane);
        assert_eq!(CaseBudget::parse("gate"), CaseBudget::Gate);
    }

    #[test]
    #[should_panic(expected = "HOLON_PBT_BUDGET must be `lane` or `gate`, got \"Lane\"")]
    fn a_budget_of_another_spelling_panics_naming_the_variable() {
        CaseBudget::parse("Lane");
    }

    #[test]
    fn a_positive_case_count_parses() {
        assert_eq!(parse_cases("PAIR_CASES", "24"), 24);
    }

    #[test]
    #[should_panic(expected = "PAIR_CASES must be a positive case count, got \"bogus\"")]
    fn a_case_count_that_does_not_parse_panics_naming_the_variable() {
        parse_cases("PAIR_CASES", "bogus");
    }

    #[test]
    #[should_panic(expected = "PROPTEST_CASES must be a positive case count, got \"0\"")]
    fn a_zero_case_count_panics() {
        parse_cases("PROPTEST_CASES", "0");
    }
    mod cases_or_pbt {
        use std::env::VarError;
        use std::panic::catch_unwind;

        use proptest::prelude::*;

        use super::super::cases_from;

        #[derive(Clone, Debug)]
        enum Drawn {
            Unset,
            Count(u32),
            NotACount(String),
        }

        const NOT_COUNTS: &[&str] = &[
            "",
            "0",
            "-3",
            "+3",
            "3 ",
            " 3",
            "1_000",
            "0x10",
            "4294967296",
            "abc",
        ];

        fn drawn() -> impl Strategy<Value = Drawn> {
            prop_oneof![
                Just(Drawn::Unset),
                (1..=u32::MAX).prop_map(Drawn::Count),
                prop::sample::select(NOT_COUNTS).prop_map(|s| Drawn::NotACount(s.to_owned())),
                "[a-z0-9]{1,8}"
                    .prop_filter("not all digits", |s| !s.bytes().all(|b| b.is_ascii_digit()))
                    .prop_map(Drawn::NotACount),
            ]
        }

        fn verdict_follows_the_drawn_class(
            var: &str,
            default: u32,
            drawn: &Drawn,
        ) -> Result<(), String> {
            let value = match drawn {
                Drawn::Unset => Err(VarError::NotPresent),
                Drawn::Count(n) => Ok(n.to_string()),
                Drawn::NotACount(s) => Ok(s.clone()),
            };
            let outcome = catch_unwind(|| cases_from(var, value, default));
            match (drawn, outcome) {
                (Drawn::Unset, Ok(n)) if n == default => Ok(()),
                (Drawn::Count(want), Ok(n)) if n == *want => Ok(()),
                (Drawn::NotACount(text), Err(payload)) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .ok_or_else(|| format!("{drawn:?}: the panic payload is not a String"))?;
                    let quoted = format!("{text:?}");
                    if message.contains(var) && message.contains(&quoted) {
                        Ok(())
                    } else {
                        Err(format!(
                            "{drawn:?}: message {message:?} lacks {var} or {quoted}"
                        ))
                    }
                }
                (_, Ok(n)) => Err(format!("{drawn:?}: read {n} cases, default {default}")),
                (_, Err(_)) => Err(format!("{drawn:?}: panicked on a value that holds a count")),
            }
        }

        #[test]
        fn fixed_texts_follow_their_class() {
            let failures: Vec<String> = ["abc", "0", "4294967296", "", "+3"]
                .into_iter()
                .filter_map(|text| {
                    verdict_follows_the_drawn_class(
                        "PROPTEST_CASES",
                        16,
                        &Drawn::NotACount(text.to_owned()),
                    )
                    .err()
                })
                .collect();
            assert!(failures.is_empty(), "{}", failures.join("\n"));
        }

        proptest! {
            #[test]
            fn a_case_count_variable_reads_its_count_its_default_or_panics_naming_itself(
                var in prop::sample::select(&["PROPTEST_CASES", "PAIR_CASES"][..]),
                default in 1..=64u32,
                drawn in drawn(),
            ) {
                verdict_follows_the_drawn_class(var, default, &drawn).map_err(TestCaseError::fail)?;
            }
        }
    }
}
