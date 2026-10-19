//! `HOLON_PBT_TIMING` — whether a budget whose quantity is a DURATION (or
//! another load-perturbed measurement) fails the run or only reports.
//!
//! The composed harness arms `inv-settle-budget` on every transition and, under
//! `HOLON_PERF_BUDGET`, the wall/query/RSS ceilings of `inv-sql-budget`. Those
//! two oracles are why suites that assert nothing about time — the
//! hand-authored replay, the windowed gate, the wide nextest sweep — had to run
//! on a quiet host: 90% of a landing gate's quiet hold asserted no duration of
//! its own (#475).
//!
//! `measure` separates MEASURING a duration from JUDGING it. The measurement
//! and its disclosure happen in both modes; only the verdict changes.
//!
//! - `enforce` (the default, so omitting the variable changes nothing): over
//!   budget fails.
//! - `measure`: over budget is `Ok`, printed as one `MEASURE-ONLY` line naming
//!   the quantity, the value and the budget, and appended to the breach ledger
//!   so a trend can show a regression the enforced rung missed.
//!
//! This is NOT [`crate::pbt::invariant_mode_override`]
//! (`HOLON_PBT_INVARIANTS`). That softens a WHOLE invariant, by id, as a
//! disclosed temporary green while a known red is fixed. Two differences
//! decide against reusing it here: it would also soften the deterministic
//! SQL-count ratchet inside `inv-sql-budget`, which load does not perturb and
//! which must keep its teeth at any load; and a standing load policy must not
//! be spelled the same way as a known-red waiver, or the known-reds classifier
//! cannot tell them apart.
//!
//! The mode is read in the invariant BODY rather than in a runner, so it
//! applies to a plain `cargo nextest run` from a lane exactly as it does to a
//! gate recipe.
//!
//! @pbt kind infra
//! @pbt covers timing-mode-knob — HOLON_PBT_TIMING measure/enforce for the
//! duration-valued budgets (#475)

use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// Whether a load-perturbed budget breach fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingMode {
    /// Over budget fails. The default.
    Enforce,
    /// Over budget reports and passes.
    Measure,
}

/// The variable this module reads.
pub const ENV: &str = "HOLON_PBT_TIMING";

/// Where breaches observed in `measure` are appended.
/// `HOLON_TIMING_MEASURE_LOG` overrides; the default sits beside the gate's
/// step timings.
const LEDGER_ENV: &str = "HOLON_TIMING_MEASURE_LOG";

/// Stamped into every ledger line so a trend can be read per gate.
/// A gate script exports it (`HOLON_GATE_ID=$GATE`); a lane run leaves it unset
/// and the line reads `local`.
const GATE_ENV: &str = "HOLON_GATE_ID";

/// Parse [`ENV`]. Unset means [`TimingMode::Enforce`], so no run loses its
/// teeth by omission. An unrecognized value PANICS rather than defaulting:
/// a typo that silently enforced would be a false green on a measure run, and
/// a typo that silently measured would be a false green on an enforced one.
pub fn mode() -> TimingMode {
    match std::env::var(ENV) {
        Err(_) => TimingMode::Enforce,
        Ok(raw) => parse(&raw),
    }
}

/// [`mode`]'s decision, split out so it is testable without touching the
/// process environment.
pub fn parse(raw: &str) -> TimingMode {
    match raw.trim() {
        "enforce" => TimingMode::Enforce,
        "measure" => TimingMode::Measure,
        other => panic!(
            "{ENV}={other:?} is not a timing mode; it is `enforce` (the default) or `measure`. \
             See docs/Testing/TimingCritical.md."
        ),
    }
}

/// One load-perturbed budget breach, as both modes observe it.
///
/// The unit is carried rather than fixed to milliseconds: the same ledger holds
/// the settle budget's durations and `inv-sql-budget`'s RSS ceilings, which are
/// bytes. Both are load-perturbed, both are measured in `measure` mode, and a
/// trend over one schema is readable where a trend over two is not.
#[derive(Debug, Clone, PartialEq)]
pub struct Breach {
    /// What was measured, as a stable key: the invariant id plus the quantity
    /// (`inv-settle-budget/interaction`, `inv-sql-budget/wall_ms`).
    pub quantity: String,
    /// The transition or action the measurement belongs to.
    pub subject: String,
    pub value: f64,
    pub budget: f64,
    pub unit: String,
}

impl fmt::Display for Breach {
    /// The disclosure line. `MEASURE-ONLY` is the grep handle a recipe asserts
    /// the presence of: a measure-mode step that emitted none measured nothing,
    /// which is a false green.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{} MEASURE-ONLY] '{}' {}{} over the {}{} budget — measured, not judged ({}=measure)",
            self.quantity, self.subject, self.value, self.unit, self.budget, self.unit, ENV
        )
    }
}

impl Breach {
    /// A duration breach, in milliseconds.
    pub fn ms(quantity: &str, subject: &str, value: Duration, budget: Duration) -> Self {
        Breach::new(
            quantity,
            subject,
            value.as_millis() as f64,
            budget.as_millis() as f64,
            "ms",
        )
    }

    pub fn new(quantity: &str, subject: &str, value: f64, budget: f64, unit: &str) -> Self {
        Breach {
            quantity: quantity.to_string(),
            subject: subject.to_string(),
            value,
            budget,
            unit: unit.to_string(),
        }
    }

    /// The ledger line: one JSON object, no dependency on a serializer, so the
    /// fields are visible here and cannot drift from the reader.
    pub fn ledger_line(&self, gate: &str) -> String {
        format!(
            r#"{{"gate":"{}","quantity":"{}","subject":"{}","value":{},"budget":{},"unit":"{}"}}"#,
            escape(gate),
            escape(&self.quantity),
            escape(&self.subject),
            self.value,
            self.budget,
            escape(&self.unit),
        )
    }
}

/// The two characters a quantity, subject or gate id could carry that would
/// break the line. Nothing else can appear in them (invariant ids, transition
/// keys and gate ids are ASCII identifiers).
fn escape(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

fn ledger_path() -> PathBuf {
    if let Ok(p) = std::env::var(LEDGER_ENV) {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME must be set to locate the timing ledger");
    PathBuf::from(home).join(".cache/holon-build/timing-measure.jsonl")
}

/// Disclose a breach observed in [`TimingMode::Measure`]: print it, and append
/// it to the ledger so a regression the enforced rung missed is still visible
/// as a trend.
///
/// A ledger that cannot be written is reported on stderr and does not fail the
/// run — the breach itself is already disclosed on stdout, so losing the trend
/// line must not turn a measure run into a red.
pub fn disclose(breach: &Breach) {
    eprintln!("{breach}");
    // This crate's OWN unit tests feed synthetic durations through the verdict,
    // and a synthetic value in the trend ledger is indistinguishable from a
    // measured one. Integration tests consume this module as a library
    // dependency, compiled without `cfg(test)`, so they still append.
    if cfg!(test) {
        return;
    }
    let gate = std::env::var(GATE_ENV).unwrap_or_else(|_| "local".to_string());
    let path = ledger_path();
    let appended = std::fs::create_dir_all(path.parent().expect("the ledger path has a parent"))
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        })
        .and_then(|mut f| writeln!(f, "{}", breach.ledger_line(&gate)));
    if let Err(e) = appended {
        eprintln!(
            "[timing-mode] the breach above was NOT appended to {}: {e}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_variable_enforces() {
        // `mode()` reads the process environment, which the test harness shares;
        // the parse is where the decision lives and is tested directly.
        assert_eq!(parse("enforce"), TimingMode::Enforce);
        assert_eq!(parse("measure"), TimingMode::Measure);
        assert_eq!(parse("  measure  "), TimingMode::Measure);
    }

    #[test]
    #[should_panic(expected = "is not a timing mode")]
    fn a_typo_panics_rather_than_picking_a_side() {
        parse("measur");
    }

    /// The grep handle every measure-mode recipe asserts is present, and the
    /// three numbers a reader needs.
    #[test]
    fn a_disclosure_names_the_quantity_the_value_and_the_budget() {
        let b = Breach::ms(
            "inv-settle-budget/interaction",
            "SharePage",
            Duration::from_millis(7292),
            Duration::from_millis(5000),
        );
        let line = b.to_string();
        assert!(line.contains("MEASURE-ONLY"), "{line}");
        assert!(line.contains("inv-settle-budget/interaction"), "{line}");
        assert!(line.contains("SharePage"), "{line}");
        assert!(line.contains("7292ms"), "{line}");
        assert!(line.contains("5000ms"), "{line}");
    }

    #[test]
    fn a_ledger_line_carries_the_gate_and_the_numbers() {
        let b = Breach::ms(
            "inv-sql-budget/wall_ms",
            "SetField",
            Duration::from_millis(31_000),
            Duration::from_millis(30_000),
        );
        assert_eq!(
            b.ledger_line("gate45"),
            r#"{"gate":"gate45","quantity":"inv-sql-budget/wall_ms","subject":"SetField","value":31000,"budget":30000,"unit":"ms"}"#
        );
    }
}
