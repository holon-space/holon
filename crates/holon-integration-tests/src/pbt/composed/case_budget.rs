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

fn parse_cases(var: &str, value: &str) -> u32 {
    match value.parse::<u32>() {
        Ok(n) if n > 0 => n,
        Ok(_) => panic!("{var} must be a positive case count, got {value:?}"),
        Err(e) => panic!("{var} must be a positive case count, got {value:?}: {e}"),
    }
}

/// Example cases, not a PBT: the input is one setting with two values and an
/// invalid class.
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
}
