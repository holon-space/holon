//! `DurationBudget` over (class, mode, value, budget, names, gate, ledger).
//!
//! Only a `small` budget under `measure` turns an over-budget value into Ok.
//! Every `small` check writes exactly one ledger line (`over` true or false)
//! in both modes; `liveness` and `lower_bound` write none and fail on their
//! own bound in every mode. Inputs a caller can pass that the ledger cannot
//! hold (a bad gate id, an empty or control-character name, an unwritable
//! ledger, an unparsable `HOLON_PBT_TIMING`, no test name) are an Err, never
//! a panic and never a guessed value.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use holon_api::duration_budget::DurationBudget;
use holon_api::duration_budget::from_ms;
use holon_api::timing_mode::GateId;
use holon_api::timing_mode::TestName;
use holon_api::timing_mode::TimingEnv;
use holon_api::timing_mode::TimingMode;
use holon_integration_tests::pbt::transition_budgets::Metric;
use holon_integration_tests::pbt::transition_budgets::MetricSample;
use holon_integration_tests::pbt::transition_budgets::Severity;
use holon_integration_tests::pbt::transition_budgets::Violation;
use holon_integration_tests::pbt::transition_budgets::evaluate_in;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

const KEYS: &[&str] = &[
    "gate", "test", "quantity", "value", "budget", "unit", "load", "over",
];
const PROBE_SPEC: &str = "DURATION_BUDGET_PROBE_SPEC";
const PROBE_OUT: &str = "DURATION_BUDGET_PROBE_OUT";
const PROBE_TEST: &str = "duration_budget_pbt::check_from_the_process_environment_probe";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Class {
    Small,
    Liveness,
    LowerBound,
}

fn budget_of(class: Class, quantity: &str, budget: Duration) -> DurationBudget {
    match class {
        Class::Small => DurationBudget::small(quantity, budget),
        Class::Liveness => DurationBudget::liveness(quantity, budget),
        Class::LowerBound => DurationBudget::lower_bound(quantity, budget),
    }
}

/// `None` where the contract leaves the verdict to the written line: a
/// `small` excess too small to survive rounding both to f64 milliseconds.
fn breached(class: Class, value: Duration, budget: Duration) -> Option<bool> {
    match class {
        Class::Small if value > budget => {
            let gap = (value - budget).as_nanos() as f64;
            (gap > budget.as_nanos() as f64 * 2f64.powi(-49)).then_some(true)
        }
        Class::Small => Some(false),
        Class::Liveness => Some(value > budget),
        Class::LowerBound => Some(value < budget),
    }
}

fn over_field(line: Option<&String>) -> Option<bool> {
    serde_json::from_str::<serde_json::Value>(line?).ok()?["over"].as_bool()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn close(got: f64, want: f64) -> bool {
    (got - want).abs() <= 1e-6 + want.abs() * 1e-9
}

fn os_load() -> f64 {
    #[cfg(target_os = "macos")]
    {
        let mut l = [0f64; 3];
        // SAFETY: getloadavg writes at most `nelem` doubles into `l`.
        let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 1) };
        assert_eq!(n, 1, "getloadavg reads the 1-min load");
        l[0]
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/loadavg")
            .expect("/proc/loadavg is readable")
            .split_whitespace()
            .next()
            .expect("a 1-min load")
            .parse()
            .expect("a number")
    }
}

fn ledger_lines(path: &Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => s.lines().map(str::to_string).collect(),
        Err(_) => Vec::new(),
    }
}

/// The one line a `small` check wrote, checked field by field.
#[allow(clippy::too_many_arguments)]
fn judge_line(
    line: &str,
    gate: &str,
    test: &str,
    quantity: &str,
    value: Duration,
    budget: Duration,
    loads: (f64, f64),
) -> Result<(), TestCaseError> {
    let v: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| TestCaseError::fail(format!("ledger line is no JSON ({e}): {line}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| TestCaseError::fail(format!("ledger line is no object: {line}")))?;
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    let mut want: Vec<&str> = KEYS.to_vec();
    want.sort();
    prop_assert_eq!(keys, want, "ledger keys of {}", line);
    prop_assert_eq!(obj["gate"].as_str(), Some(gate), "gate of {}", line);
    prop_assert_eq!(obj["test"].as_str(), Some(test), "test of {}", line);
    prop_assert_eq!(
        obj["quantity"].as_str(),
        Some(quantity),
        "quantity of {}",
        line
    );
    prop_assert_eq!(obj["unit"].as_str(), Some("ms"), "unit of {}", line);
    if let Some(over) = breached(Class::Small, value, budget) {
        prop_assert_eq!(obj["over"].as_bool(), Some(over), "over of {}", line);
    }
    let num = |k: &str| obj[k].as_f64().unwrap_or(f64::NAN);
    prop_assert_eq!(
        obj["over"].as_bool(),
        Some(num("value") > num("budget")),
        "over of {} disagrees with value > budget as the summary reads it (Q19), so the summary refuses the line",
        line
    );
    prop_assert!(
        close(num("value"), ms(value)),
        "value {} ms of {}",
        ms(value),
        line
    );
    prop_assert!(
        close(num("budget"), ms(budget)),
        "budget {} ms of {}",
        ms(budget),
        line
    );
    let load = num("load");
    let (lo, hi) = (loads.0.min(loads.1), loads.0.max(loads.1));
    prop_assert!(
        load >= lo * 0.9 - 0.5 && load <= hi * 1.1 + 0.5,
        "load {} outside the OS 1-min load [{}, {}] around the check: {}",
        load,
        lo,
        hi,
        line
    );
    Ok(())
}

// --- generators -------------------------------------------------------------

#[derive(Debug, Clone)]
enum Text {
    Valid(String),
    Invalid(String),
}

impl Text {
    fn as_str(&self) -> &str {
        match self {
            Text::Valid(s) | Text::Invalid(s) => s,
        }
    }

    fn valid(&self) -> bool {
        matches!(self, Text::Valid(_))
    }
}

fn name_char() -> impl Strategy<Value = char> {
    prop_oneof![
        6 => proptest::char::range(' ', '~'),
        1 => any::<char>().prop_filter("no control character", |c| !c.is_control()),
        1 => proptest::sample::select(vec!['"', '\\', ':', '/', '\u{2028}', '\u{200b}', '\u{feff}', 'é', '日']),
    ]
}

/// A name the ledger holds: non-empty, no edge whitespace, no control char.
pub(crate) fn valid_name() -> impl Strategy<Value = String> {
    proptest::collection::vec(name_char(), 0..40).prop_map(|cs| {
        let s: String = cs.into_iter().collect();
        format!("a{}z", s.trim())
    })
}

fn control_char() -> impl Strategy<Value = char> {
    prop_oneof![
        proptest::char::range('\u{0}', '\u{1f}'),
        proptest::char::range('\u{7f}', '\u{9f}'),
    ]
}

pub(crate) fn invalid_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (
            valid_name(),
            proptest::sample::select(vec![" ", "\t", "\u{3000}", "\u{a0}", "\n"]),
            any::<bool>()
        )
            .prop_map(|(s, ws, lead)| if lead {
                format!("{ws}{s}")
            } else {
                format!("{s}{ws}")
            }),
        (valid_name(), control_char(), any::<prop::sample::Index>()).prop_map(|(s, c, at)| {
            let cs: Vec<char> = s.chars().collect();
            let i = at.index(cs.len() + 1);
            cs[..i]
                .iter()
                .chain(std::iter::once(&c))
                .chain(&cs[i..])
                .collect()
        }),
    ]
}

fn text() -> impl Strategy<Value = Text> {
    prop_oneof![
        5 => valid_name().prop_map(Text::Valid),
        1 => invalid_name().prop_map(Text::Invalid),
    ]
}

fn valid_gate() -> impl Strategy<Value = String> {
    prop_oneof![
        1 => Just("local".to_string()),
        3 => (1u32..200_000).prop_map(|n| format!("gate{n}")),
        1 => (1u32..200_000, proptest::char::range('a', 'z')).prop_map(|(n, c)| format!("gate{n}{c}")),
        1 => (1u128..u128::MAX).prop_map(|n| format!("gate{n}")),
    ]
}

pub(crate) fn invalid_gate() -> impl Strategy<Value = String> {
    proptest::sample::select(vec![
        "",
        "foo",
        "gate07",
        "gate-3",
        "gate",
        "GATE5",
        "gate5 ",
        " gate5",
        "gate5cc",
        "gate5C",
        "gate 5",
        "Local",
        "gate5\n",
        "w12",
        "gate0",
        "gate00",
        "gate0a",
        "gate\u{ff15}",
        "gate\u{663}",
        "gate5\u{e9}",
        "gate+5",
        "gate5.0",
    ])
    .prop_map(str::to_string)
}

fn gate() -> impl Strategy<Value = Text> {
    prop_oneof![
        5 => valid_gate().prop_map(Text::Valid),
        1 => invalid_gate().prop_map(Text::Invalid),
    ]
}

fn duration() -> impl Strategy<Value = Duration> {
    prop_oneof![
        3 => (0u64..20_000_000).prop_map(Duration::from_nanos),
        2 => any::<u64>().prop_map(Duration::from_nanos),
        1 => (any::<u64>(), 0u32..1_000_000_000).prop_map(|(s, n)| Duration::new(s, n)),
        1 => Just(Duration::ZERO),
        1 => Just(Duration::MAX),
    ]
}

#[derive(Debug, Clone, Copy)]
enum Relation {
    Under,
    Equal,
    Over,
}

fn value_for(budget: Duration, rel: Relation, step: Duration) -> Duration {
    let step = step.max(Duration::from_nanos(1));
    match rel {
        Relation::Under => budget.saturating_sub(step),
        Relation::Equal => budget,
        Relation::Over => budget.saturating_add(step),
    }
}

fn relation() -> impl Strategy<Value = Relation> {
    prop_oneof![
        Just(Relation::Under),
        Just(Relation::Equal),
        Just(Relation::Over)
    ]
}

fn class() -> impl Strategy<Value = Class> {
    prop_oneof![
        Just(Class::Small),
        Just(Class::Liveness),
        Just(Class::LowerBound)
    ]
}

fn mode() -> impl Strategy<Value = TimingMode> {
    prop_oneof![Just(TimingMode::Enforce), Just(TimingMode::Measure)]
}

#[derive(Debug, Clone, Copy)]
enum LedgerKind {
    Absent,
    Existing,
    Directory,
    UnderRegularFile,
    ReadOnly,
    MissingParent,
}

impl LedgerKind {
    fn writable(self) -> bool {
        matches!(
            self,
            LedgerKind::Absent | LedgerKind::Existing | LedgerKind::MissingParent
        )
    }

    fn make(self, dir: &Path) -> PathBuf {
        let p = dir.join("timing-breaches.jsonl");
        match self {
            LedgerKind::Absent => {}
            LedgerKind::Existing => {
                std::fs::write(&p, "{\"earlier\":\"line\"}\n").unwrap();
            }
            LedgerKind::Directory => std::fs::create_dir(&p).unwrap(),
            LedgerKind::UnderRegularFile => {
                std::fs::write(dir.join("plain"), "x").unwrap();
                return dir.join("plain").join("timing-breaches.jsonl");
            }
            LedgerKind::MissingParent => {
                return dir.join("not").join("yet").join("timing-breaches.jsonl");
            }
            LedgerKind::ReadOnly => {
                std::fs::write(&p, "").unwrap();
                let mut perm = std::fs::metadata(&p).unwrap().permissions();
                perm.set_readonly(true);
                std::fs::set_permissions(&p, perm).unwrap();
            }
        }
        p
    }
}

fn ledger_kind() -> impl Strategy<Value = LedgerKind> {
    prop_oneof![
        4 => Just(LedgerKind::Absent),
        2 => Just(LedgerKind::Existing),
        1 => Just(LedgerKind::Directory),
        1 => Just(LedgerKind::UnderRegularFile),
        1 => Just(LedgerKind::ReadOnly),
        1 => Just(LedgerKind::MissingParent),
    ]
}

#[derive(Debug, Clone)]
struct Case {
    class: Class,
    mode: TimingMode,
    budget: Duration,
    relation: Relation,
    step: Duration,
    quantity: Text,
    test: Text,
    gate: Text,
    ledger: LedgerKind,
}

impl Case {
    fn value(&self) -> Duration {
        value_for(self.budget, self.relation, self.step)
    }

    /// Gate and test name are typed in the env, so only the quantity
    /// reaches the check unparsed.
    fn refused(&self) -> bool {
        !self.quantity.valid()
    }
}

fn case() -> impl Strategy<Value = Case> {
    (
        class(),
        mode(),
        duration(),
        relation(),
        duration(),
        text(),
        text(),
        gate(),
        ledger_kind(),
    )
        .prop_map(
            |(class, mode, budget, relation, step, quantity, test, gate, ledger)| Case {
                class,
                mode,
                budget,
                relation,
                step,
                quantity,
                test,
                gate,
                ledger,
            },
        )
}

// --- properties -------------------------------------------------------------

fn runner(cases: u32) -> TestRunner {
    TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    })
}

fn judge_check(c: Case) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let ledger = c.ledger.make(dir.path());
    let before = ledger_lines(&ledger);
    let value = c.value();
    let gate = match (&c.gate, GateId::parse(c.gate.as_str())) {
        (Text::Valid(raw), Ok(g)) => {
            prop_assert_eq!(g.as_str(), raw.as_str());
            g
        }
        (Text::Invalid(_), Err(_)) => GateId::local(),
        (_, got) => {
            return Err(TestCaseError::fail(format!(
                "GateId::parse({:?}) = {got:?}",
                c.gate
            )));
        }
    };
    let test = match (&c.test, TestName::parse(c.test.as_str())) {
        (Text::Valid(raw), Ok(t)) => {
            prop_assert_eq!(t.as_str(), raw.as_str());
            t
        }
        (Text::Invalid(_), Err(_)) => TestName::parse("suite::fallback").unwrap(),
        (_, got) => {
            return Err(TestCaseError::fail(format!(
                "TestName::parse({:?}) = {got:?}",
                c.test
            )));
        }
    };
    let (gate_label, test_label) = (gate.as_str().to_string(), test.as_str().to_string());
    let env = TimingEnv {
        mode: c.mode,
        ledger: ledger.clone(),
        gate,
        test,
    };
    let budget = budget_of(c.class, c.quantity.as_str(), c.budget);
    prop_assert_eq!(budget.quantity(), c.quantity.as_str());
    prop_assert_eq!(budget.budget(), c.budget);
    let load0 = os_load();
    let got = budget.check_in(&env, value);
    let load1 = os_load();
    let after = ledger_lines(&ledger);

    let small = c.class == Class::Small;
    let breach = breached(c.class, value, c.budget)
        .unwrap_or_else(|| over_field(after.get(before.len())).unwrap_or(false));
    let refused = c.refused();
    let write_fails = small && !c.ledger.writable();
    let want_err = refused || write_fails || (breach && (!small || c.mode == TimingMode::Enforce));
    let want_lines = usize::from(small && !refused && !write_fails);
    let shown = format!("{c:?} value={value:?} ledger={}", ledger.display());

    prop_assert_eq!(
        got.is_err(),
        want_err,
        "check_in result {:?} for {}",
        got,
        shown
    );
    if c.ledger.writable() {
        prop_assert_eq!(
            &after[..before.len()],
            &before[..],
            "earlier ledger lines kept: {}",
            shown
        );
        prop_assert_eq!(
            after.len() - before.len(),
            want_lines,
            "new ledger lines {:?} for {}",
            &after[before.len()..],
            shown
        );
    }
    if want_lines == 1 {
        judge_line(
            &after[before.len()],
            &gate_label,
            &test_label,
            c.quantity.as_str(),
            value,
            c.budget,
            (load0, load1),
        )?;
    }
    if let Err(e) = &got {
        let msg = e.to_string();
        if write_fails && !refused && !(breach && c.mode == TimingMode::Enforce) {
            prop_assert!(
                msg.contains(&ledger.display().to_string()) && msg.contains("os error"),
                "a ledger write failure names the path and the OS error: {msg}"
            );
        } else if breach && !refused && !write_fails {
            prop_assert!(
                msg.contains(c.quantity.as_str()) && msg.contains(&test_label),
                "a budget breach names its quantity and test: {msg}"
            );
        }
    }
    Ok(())
}

#[test]
fn only_a_small_budget_under_measure_turns_a_breach_into_ok_and_every_small_check_writes_one_line()
{
    let fixed = |class, mode, relation| Case {
        class,
        mode,
        budget: Duration::from_millis(5),
        relation,
        step: Duration::from_micros(250),
        quantity: Text::Valid("hop/p50-sum".into()),
        test: Text::Valid("hop_step_cost::hop_cost".into()),
        gate: Text::Valid("gate158c".into()),
        ledger: LedgerKind::Absent,
    };
    for class in [Class::Small, Class::Liveness, Class::LowerBound] {
        for mode in [TimingMode::Enforce, TimingMode::Measure] {
            for relation in [Relation::Under, Relation::Equal, Relation::Over] {
                judge_check(fixed(class, mode, relation)).unwrap_or_else(|e| panic!("{e}"));
            }
        }
    }
    // A 1 ns excess past 2^53 ns rounds to the budget's own f64 milliseconds.
    for budget in [
        Duration::new(1_125_899_906_109_159, 62_500_001),
        Duration::new(38_031_619, 967_904_378),
    ] {
        for mode in [TimingMode::Enforce, TimingMode::Measure] {
            judge_check(Case {
                budget,
                step: Duration::from_nanos(1),
                ..fixed(Class::Small, mode, Relation::Over)
            })
            .unwrap_or_else(|e| panic!("{e}"));
        }
    }
    runner(512)
        .run(&case(), judge_check)
        .unwrap_or_else(|e| panic!("{e}"));
}

/// Several checks on one ledger, from threads at once, with names long
/// enough that a split write would interleave: one whole line per check.
#[test]
fn concurrent_small_checks_on_one_ledger_write_one_whole_line_each() {
    let strategy = (
        2usize..8,
        1usize..6,
        proptest::collection::vec(valid_name(), 1..4),
        0usize..4000,
    );
    runner(24)
        .run(&strategy, |(threads, per, names, pad)| {
            let dir = tempfile::tempdir().unwrap();
            let ledger = dir.path().join("timing-breaches.jsonl");
            let mut want: Vec<(String, String)> = Vec::new();
            std::thread::scope(|s| {
                for t in 0..threads {
                    let quantity = format!("{}{}#{t}", names[t % names.len()], "q".repeat(pad));
                    let test = format!("suite::writer{t}");
                    for _ in 0..per {
                        want.push((test.clone(), quantity.clone()));
                    }
                    let env = TimingEnv {
                        mode: if t % 2 == 0 {
                            TimingMode::Measure
                        } else {
                            TimingMode::Enforce
                        },
                        ledger: ledger.clone(),
                        gate: GateId::parse("gate7").unwrap(),
                        test: TestName::parse(&test).unwrap(),
                    };
                    s.spawn(move || {
                        let b = budget_of(Class::Small, &quantity, Duration::from_millis(5));
                        for i in 0..per {
                            let r = b.check_in(
                                &env,
                                Duration::from_millis(if i % 2 == 0 { 1 } else { 9 }),
                            );
                            assert_eq!(
                                r.is_err(),
                                i % 2 == 1 && env.mode == TimingMode::Enforce,
                                "{r:?}"
                            );
                        }
                    });
                }
            });
            let mut got: Vec<(String, String)> = Vec::new();
            for line in ledger_lines(&ledger) {
                let v: serde_json::Value = serde_json::from_str(&line).map_err(|e| {
                    TestCaseError::fail(format!("interleaved or broken line ({e}): {line}"))
                })?;
                got.push((
                    v["test"].as_str().unwrap_or("").into(),
                    v["quantity"].as_str().unwrap_or("").into(),
                ));
            }
            got.sort();
            want.sort();
            prop_assert_eq!(got, want);
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
}

#[derive(Debug, Clone, Copy)]
enum ValueClass {
    InRange,
    NegativeZero,
    Negative,
    NaN,
    PosInf,
    NegInf,
    TooLarge,
}

/// Duration::MAX in milliseconds is about 1.8446744e22; values within a
/// relative 1e-3 of it are left out, where f64 rounding decides the side.
fn from_ms_input() -> impl Strategy<Value = (ValueClass, f64)> {
    prop_oneof![
        3 => (0.0f64..1e7).prop_map(|x| (ValueClass::InRange, x)),
        2 => (-30.0f64..22.25).prop_map(|e| (ValueClass::InRange, 10f64.powf(e))),
        1 => Just((ValueClass::InRange, 0.0)),
        1 => Just((ValueClass::InRange, 5e-324)),
        1 => Just((ValueClass::NegativeZero, -0.0)),
        2 => (-1e300f64..-1e-300).prop_map(|x| (ValueClass::Negative, x)),
        1 => Just((ValueClass::Negative, -5e-324)),
        1 => prop_oneof![Just(f64::NAN), Just(-f64::NAN), Just(f64::from_bits(0x7ff0_0000_0000_0001))]
            .prop_map(|x| (ValueClass::NaN, x)),
        1 => Just((ValueClass::PosInf, f64::INFINITY)),
        1 => Just((ValueClass::NegInf, f64::NEG_INFINITY)),
        2 => (22.27f64..308.0).prop_map(|e| (ValueClass::TooLarge, 10f64.powf(e))),
        1 => Just((ValueClass::TooLarge, f64::MAX)),
    ]
}

#[test]
fn from_ms_converts_every_representable_millisecond_count_and_refuses_the_rest() {
    runner(512)
        .run(&from_ms_input(), |(class, x)| {
            let got = from_ms(x);
            match class {
                ValueClass::InRange | ValueClass::NegativeZero => {
                    let d = got
                        .map_err(|e| TestCaseError::fail(format!("from_ms({x:?}) refused: {e}")))?;
                    let want_ns = x.max(0.0) * 1e6;
                    let got_ns = d.as_nanos() as f64;
                    prop_assert!(
                        (got_ns - want_ns).abs() <= 1.0 + want_ns * 1e-12,
                        "from_ms({:?}) = {:?}, want {} ns",
                        x,
                        d,
                        want_ns
                    );
                }
                _ => {
                    let e = match got {
                        Ok(d) => {
                            return Err(TestCaseError::fail(format!(
                                "from_ms({x:?}) = Ok({d:?}), want Err ({class:?})"
                            )));
                        }
                        Err(e) => e.to_string(),
                    };
                    prop_assert!(
                        e.contains(&format!("{x}")) || e.contains(&format!("{x:?}")),
                        "the refusal names the value {:?}: {}",
                        x,
                        e
                    );
                }
            }
            Ok(())
        })
        .unwrap_or_else(|e| panic!("{e}"));
}

// --- the process environment ------------------------------------------------

#[derive(Debug, Clone)]
enum ThreadKind {
    Test,
    Unnamed,
    Named(&'static str),
}

/// Runs inside a child process this binary starts: one `check` from the
/// process environment on the requested thread, its Result into a file.
#[test]
#[ignore = "a child process of check_reads_mode_ledger_gate_and_test_name_from_the_process_environment"]
fn check_from_the_process_environment_probe() {
    let spec: serde_json::Value =
        serde_json::from_str(&std::env::var(PROBE_SPEC).expect("the probe spec"))
            .expect("JSON spec");
    let out = PathBuf::from(std::env::var(PROBE_OUT).expect("the probe out path"));
    let class = match spec["class"].as_str().unwrap() {
        "small" => Class::Small,
        "liveness" => Class::Liveness,
        _ => Class::LowerBound,
    };
    let b = budget_of(
        class,
        spec["quantity"].as_str().unwrap(),
        Duration::from_nanos(spec["budget_ns"].as_u64().unwrap()),
    );
    let value = Duration::from_nanos(spec["value_ns"].as_u64().unwrap());
    let run = move || match b.check(value) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("err\n{e}"),
    };
    let result = match spec["thread"].as_str().unwrap() {
        "test" => run(),
        "unnamed" => std::thread::spawn(run)
            .join()
            .expect("the check thread ends"),
        name => std::thread::Builder::new()
            .name(name.into())
            .spawn(run)
            .unwrap()
            .join()
            .expect("the check thread ends"),
    };
    std::fs::write(out, result).unwrap();
}

#[derive(Debug, Clone)]
enum Var {
    Unset,
    Set(String),
    NotUnicode(&'static [u8]),
}

impl Var {
    fn apply(&self, cmd: &mut Command, key: &str) {
        use std::os::unix::ffi::OsStrExt;
        match self {
            Var::Unset => {}
            Var::Set(v) => {
                cmd.env(key, v);
            }
            Var::NotUnicode(b) => {
                cmd.env(key, std::ffi::OsStr::from_bytes(b));
            }
        }
    }
}

#[derive(Debug, Clone)]
enum LedgerVar {
    Set,
    HomeDefault,
    NoHome,
}

#[derive(Debug, Clone)]
struct EnvCase {
    timing: Var,
    ledger: LedgerVar,
    gate: Var,
    /// NEXTEST_TEST_NAME, drawn apart from the probe's own name so the
    /// line shows which source the test field came from (Q24).
    nextest: Option<String>,
    cache_dir_exists: bool,
    thread: ThreadKind,
    class: Class,
    budget: Duration,
    relation: Relation,
    quantity: String,
}

fn timing_var() -> impl Strategy<Value = Var> {
    prop_oneof![
        2 => Just(Var::Unset),
        2 => Just(Var::Set("enforce".into())),
        2 => Just(Var::Set("measure".into())),
        2 => proptest::sample::select(vec![
            "", "Measure", "MEASURE", " measure", "measure ", "enforce\n", "on", "1", "measure,enforce", "warn",
        ])
        .prop_map(|s| Var::Set(s.into())),
        1 => Just(Var::NotUnicode(b"meas\xffure")),
    ]
}

fn env_case() -> impl Strategy<Value = EnvCase> {
    (
        (
        timing_var(),
        prop_oneof![3 => Just(LedgerVar::Set), 1 => Just(LedgerVar::HomeDefault), 1 => Just(LedgerVar::NoHome)],
        prop_oneof![
            2 => Just(Var::Unset),
            2 => valid_gate().prop_map(Var::Set),
            1 => invalid_gate().prop_filter("an env value holds no NUL", |g| !g.contains('\0')).prop_map(Var::Set),
            1 => Just(Var::NotUnicode(b"gate5\xff")),
        ],
        prop_oneof![
            2 => Just(None),
            1 => Just(Some(PROBE_TEST.to_string())),
            1 => valid_name().prop_map(Some),
        ],
        any::<bool>(),
        ),
        (
        prop_oneof![
            Just(ThreadKind::Test),
            Just(ThreadKind::Unnamed),
            Just(ThreadKind::Named("tokio-runtime-worker")),
            Just(ThreadKind::Named("main")),
        ],
        class(),
        (0u64..50_000_000).prop_map(Duration::from_nanos),
        relation(),
        valid_name(),
        ),
    )
        .prop_map(|(
            (timing, ledger, gate, nextest, cache_dir_exists),
            (thread, class, budget, relation, quantity),
        )| EnvCase {
            timing,
            ledger,
            gate,
            nextest,
            cache_dir_exists,
            thread,
            class,
            budget,
            relation,
            quantity,
        })
}

fn judge_env(c: EnvCase) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    let cache = home.join(".cache/holon-build");
    if c.cache_dir_exists {
        std::fs::create_dir_all(&cache).unwrap();
    } else {
        std::fs::create_dir_all(&home).unwrap();
    }
    let set_ledger = root.join("ledger.jsonl");
    let out = root.join("probe-result");
    let value = value_for(c.budget, c.relation, Duration::from_micros(300));
    let spec = serde_json::json!({
        "thread": match c.thread { ThreadKind::Test => "test", ThreadKind::Unnamed => "unnamed", ThreadKind::Named(n) => n },
        "class": match c.class { Class::Small => "small", Class::Liveness => "liveness", Class::LowerBound => "lower_bound" },
        "budget_ns": c.budget.as_nanos() as u64,
        "value_ns": value.as_nanos() as u64,
        "quantity": c.quantity,
    });
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", PROBE_TEST, "--ignored", "--nocapture"]);
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().into_owned();
        if k.starts_with("NEXTEST")
            || k.starts_with("HOLON_")
            || k == "RUST_TEST_THREADS"
            || k == "HOME"
        {
            cmd.env_remove(k);
        }
    }
    cmd.env(PROBE_SPEC, spec.to_string()).env(PROBE_OUT, &out);
    c.timing.apply(&mut cmd, "HOLON_PBT_TIMING");
    c.gate.apply(&mut cmd, "HOLON_GATE_ID");
    match c.ledger {
        LedgerVar::Set => {
            cmd.env("HOLON_TIMING_MEASURE_LOG", &set_ledger)
                .env("HOME", &home);
        }
        LedgerVar::HomeDefault => {
            cmd.env("HOME", &home);
        }
        LedgerVar::NoHome => {}
    }
    if let Some(name) = &c.nextest {
        cmd.env("NEXTEST_TEST_NAME", name);
    }
    let status = cmd.output().unwrap();
    let shown = format!(
        "{c:?} value={value:?}\n--- child stderr:\n{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let result = std::fs::read_to_string(&out).map_err(|_| {
        TestCaseError::fail(format!("the check panicked instead of returning: {shown}"))
    })?;

    let mode = match &c.timing {
        Var::Unset => Some(TimingMode::Measure),
        Var::Set(v) if v == "enforce" => Some(TimingMode::Enforce),
        Var::Set(v) if v == "measure" => Some(TimingMode::Measure),
        Var::Set(_) | Var::NotUnicode(_) => None,
    };
    let gate_ok = match &c.gate {
        Var::Unset => true,
        Var::NotUnicode(_) => false,
        Var::Set(g) => {
            g == "local"
                || g.strip_prefix("gate").is_some_and(|n| {
                    let n = n
                        .strip_suffix(|ch: char| ch.is_ascii_lowercase())
                        .unwrap_or(n);
                    !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
                })
        }
    };
    let name_ok = c.nextest.is_some() || matches!(c.thread, ThreadKind::Test);
    let ledger_ok = !matches!(c.ledger, LedgerVar::NoHome);
    let small = c.class == Class::Small;
    let written: Vec<String> = [set_ledger.clone()]
        .into_iter()
        .chain(
            std::fs::read_dir(&cache)
                .into_iter()
                .flatten()
                .map(|e| e.unwrap().path()),
        )
        .flat_map(|p| ledger_lines(&p))
        .collect();
    let breach = breached(c.class, value, c.budget)
        .unwrap_or_else(|| over_field(written.first()).unwrap_or(false));
    // Gate, ledger path and test name only matter where a line is written.
    let usable = mode.is_some() && (!small || (gate_ok && name_ok && ledger_ok));
    let want_err = !usable || (breach && (!small || mode == Some(TimingMode::Enforce)));
    prop_assert_eq!(
        result.starts_with("err"),
        want_err,
        "child result {:?} for {}",
        result,
        shown
    );
    if mode.is_none() {
        let named_value = match &c.timing {
            Var::Set(bad) => result.contains(bad.as_str()) || result.contains(&format!("{bad:?}")),
            _ => true,
        };
        prop_assert!(
            result.contains("HOLON_PBT_TIMING") && named_value,
            "an unparsable HOLON_PBT_TIMING is named with its value: {:?}",
            result
        );
    }
    let want_lines = usize::from(small && usable);
    prop_assert_eq!(
        written.len(),
        want_lines,
        "ledger lines {:?} for {}",
        written,
        shown
    );
    if want_lines == 1 {
        let v: serde_json::Value = serde_json::from_str(&written[0]).unwrap_or_default();
        let gate = match &c.gate {
            Var::Unset => "local",
            Var::Set(g) => g.as_str(),
            Var::NotUnicode(_) => unreachable!("a non-Unicode gate writes no line"),
        };
        prop_assert_eq!(v["gate"].as_str(), Some(gate), "gate of {}", written[0]);
        prop_assert_eq!(
            v["test"].as_str(),
            Some(c.nextest.as_deref().unwrap_or(PROBE_TEST)),
            "test of {}",
            written[0]
        );
        if matches!(c.ledger, LedgerVar::HomeDefault) {
            prop_assert!(
                !cache.join("timing-measure.jsonl").exists(),
                "the old-format ledger file is never written"
            );
        }
    }
    Ok(())
}

#[test]
fn check_reads_mode_ledger_gate_and_test_name_from_the_process_environment() {
    runner(64)
        .run(&env_case(), judge_env)
        .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_test_field_names_the_running_test() {
    let env = TimingEnv::from_env().expect("the harness environment is usable");
    assert_eq!(
        env.test.as_str(),
        "duration_budget_pbt::the_test_field_names_the_running_test",
        "under this harness the ledger's test field is the test's own name"
    );
}

// --- transition budgets ------------------------------------------------------

const METRICS: [Metric; 9] = [
    Metric::SqlReads,
    Metric::SqlReadRepeat,
    Metric::SqlWrites,
    Metric::SqlDdl,
    Metric::MaxQueryMs,
    Metric::WallMs,
    Metric::SettleMs,
    Metric::RssDeltaBytes,
    Metric::RssCumulativeBytes,
];

fn is_rss(m: Metric) -> bool {
    matches!(m, Metric::RssDeltaBytes | Metric::RssCumulativeBytes)
}

#[derive(Debug, Clone)]
struct Sample {
    metric: Metric,
    severity: Severity,
    actual: f64,
    limit: f64,
}

impl Sample {
    fn records(&self) -> bool {
        self.severity == Severity::Error && self.metric.unit().is_some()
    }

    fn real(&self, i: usize) -> MetricSample {
        MetricSample {
            metric: self.metric,
            actual: self.actual,
            limit: self.limit,
            severity: self.severity,
            message: format!("sample {i}"),
        }
    }
}

/// `build_samples` gives every metric a whole-number actual from a Duration
/// or a count, except the RSS metrics: an `isize` delta that is negative
/// when a transition frees memory. Pinned is drawn only for the SQL counts,
/// the one kind `build_samples` pins (a pinned wall clock is an assert).
fn sample() -> impl Strategy<Value = Sample> {
    let limit = prop_oneof![
        1 => Just(0.0),
        3 => (1u64..=30_000).prop_map(|n| n as f64),
        3 => (1u64..=4_000_000_000).prop_map(|n| n as f64),
        2 => (1u64..=4_000_000_000, prop_oneof![Just(0.5), Just(1.5), Just(3.7)])
            .prop_map(|(n, m)| n as f64 * m),
    ];
    (
        0..METRICS.len(),
        0u8..3,
        limit,
        0u8..5,
        1u64..=4_000_000_000,
    )
        .prop_map(|(m, sev, limit, rel, step)| {
            let metric = METRICS[m];
            let severity = match sev {
                0 => Severity::Warn,
                2 if metric.deterministic() => Severity::Pinned,
                _ => Severity::Error,
            };
            let step = step as f64;
            let actual = match rel {
                0 => (limit - step).max(0.0).floor(),
                1 => limit,
                2 => limit + step,
                3 => 0.0,
                _ if is_rss(metric) => -step,
                _ => (limit / 2.0).floor(),
            };
            Sample {
                metric,
                severity,
                actual,
                limit,
            }
        })
}

#[derive(Debug, Clone)]
struct TransitionCase {
    mode: TimingMode,
    gate: String,
    test: String,
    key: String,
    ledger: LedgerKind,
    samples: Vec<Sample>,
}

fn transition_case() -> impl Strategy<Value = TransitionCase> {
    (
        mode(),
        valid_gate(),
        valid_name(),
        valid_name(),
        ledger_kind(),
        proptest::collection::vec(sample(), 0..8),
    )
        .prop_map(|(mode, gate, test, key, ledger, samples)| TransitionCase {
            mode,
            gate,
            test,
            key,
            ledger,
            samples,
        })
}

fn expected_violation(mode: TimingMode, s: &Sample, i: usize) -> Option<String> {
    if s.actual <= s.limit {
        return None;
    }
    Some(match s.severity {
        Severity::Warn => format!("Warning(sample {i})"),
        Severity::Error if mode == TimingMode::Measure && !s.metric.deterministic() => {
            format!("Measured({:?} sample {i})", s.metric)
        }
        Severity::Error => format!("Error(sample {i})"),
        Severity::Pinned => format!("PinnedError(sample {i})"),
    })
}

fn shown(v: &Violation) -> String {
    match v {
        Violation::Warning(m) => format!("Warning({m})"),
        Violation::Error(m) => format!("Error({m})"),
        Violation::PinnedError(m) => format!("PinnedError({m})"),
        Violation::Measured {
            metric,
            actual: _,
            limit: _,
            message,
        } => format!("Measured({metric:?} {message})"),
    }
}

fn judge_transition(c: TransitionCase) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let ledger = c.ledger.make(dir.path());
    let before = ledger_lines(&ledger);
    let env = TimingEnv {
        mode: c.mode,
        ledger: ledger.clone(),
        gate: GateId::parse(&c.gate).unwrap(),
        test: TestName::parse(&c.test).unwrap(),
    };
    let real: Vec<MetricSample> = c
        .samples
        .iter()
        .enumerate()
        .map(|(i, s)| s.real(i))
        .collect();
    let load_lo = os_load();
    let got = evaluate_in(&env, &c.key, &real);
    let load_hi = os_load();
    let recorded: Vec<&Sample> = c.samples.iter().filter(|s| s.records()).collect();

    if !c.ledger.writable() && !recorded.is_empty() {
        prop_assert!(
            got.is_err(),
            "an unwritable ledger {:?} gave {:?}",
            c.ledger,
            got
        );
        return Ok(());
    }
    let got = got.map_err(|e| {
        TestCaseError::fail(format!(
            "evaluate_in refused samples a transition produces: {e}"
        ))
    })?;
    let want: Vec<String> = c
        .samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| expected_violation(c.mode, s, i))
        .collect();
    prop_assert_eq!(got.iter().map(shown).collect::<Vec<_>>(), want);

    let after = ledger_lines(&ledger);
    prop_assert!(after.starts_with(&before));
    let lines = &after[before.len()..];
    prop_assert_eq!(
        lines.len(),
        recorded.len(),
        "one ledger line per load-perturbed Error sample (L8)"
    );
    for (line, s) in lines.iter().zip(&recorded) {
        let obj: serde_json::Value =
            serde_json::from_str(line).map_err(|e| TestCaseError::fail(format!("{line}: {e}")))?;
        let num = |k: &str| obj[k].as_f64().unwrap_or(f64::NAN);
        let unit = if is_rss(s.metric) { "B" } else { "ms" };
        prop_assert_eq!(obj["gate"].as_str(), Some(c.gate.as_str()));
        prop_assert_eq!(obj["test"].as_str(), Some(c.test.as_str()));
        prop_assert_eq!(
            obj["quantity"].as_str().map(str::to_string),
            Some(format!("inv-sql-budget/{}/{}", s.metric.key(), c.key))
        );
        prop_assert_eq!(obj["unit"].as_str(), Some(unit));
        prop_assert!(close(num("budget"), s.limit), "{}", line);
        prop_assert!(close(num("value"), s.actual), "{}", line);
        prop_assert_eq!(obj["over"].as_bool(), Some(s.actual > s.limit), "{}", line);
        prop_assert!(
            num("load") >= load_lo - 0.5 && num("load") <= load_hi + 0.5,
            "{}",
            line
        );
    }
    if !lines.is_empty() {
        // An `Existing` ledger starts with a non-ledger line; only the lines
        // evaluate_in wrote are its output.
        let fresh = dir.path().join("written.jsonl");
        std::fs::write(
            &fresh,
            lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
        )
        .unwrap();
        let run = crate::breach_summary_pbt::summarize(&fresh, dir.path(), None);
        prop_assert_eq!(
            run.code,
            0,
            "the summary refuses the ledger evaluate_in wrote: {}",
            run.stderr
        );
    }
    Ok(())
}

/// A transition's budget samples: under measure only a non-deterministic
/// Error breach is softened; every Error sample with a unit writes one line
/// the summary reads, a freed-memory (negative) RSS delta included.
#[test]
fn evaluate_in_records_each_load_perturbed_error_sample_and_fails_only_what_the_mode_fails() {
    runner(128)
        .run(&transition_case(), judge_transition)
        .unwrap_or_else(|e| panic!("{e}"));
}

const SETTLE_UNIT_TESTS: &str = "pbt::composed::invariants::settle_budget::tests::";

/// The crate's own unit-test binary: next to this one in `deps/`, or in a
/// sibling `<hash>/out/` of cargo's per-unit build-dir layout.
// shortcut: the newest `holon_integration_tests-*` executable is taken; a stale
// build with another hash could be newer, upgrade when nextest names lib
// binaries.
fn lib_test_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let here = exe.parent().unwrap();
    let dirs: Vec<PathBuf> = if here.file_name().unwrap() == "out" {
        let units = here.parent().unwrap().parent().unwrap();
        std::fs::read_dir(units)
            .unwrap()
            .map(|e| e.unwrap().path().join("out"))
            .filter(|d| d.is_dir())
            .collect()
    } else {
        vec![here.to_path_buf()]
    };
    dirs.iter()
        .flat_map(|d| std::fs::read_dir(d).unwrap().map(|e| e.unwrap().path()))
        .filter(|p| {
            p.extension().is_none()
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("holon_integration_tests-")
                && std::fs::metadata(p).unwrap().permissions().mode() & 0o111 != 0
        })
        .max_by_key(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .unwrap_or_else(|| panic!("no holon_integration_tests-* test binary near {exe:?}"))
}

/// Example-based: the input space is the two modes the unit tests branch on,
/// both enumerated. Unit tests that feed made-up settle samples to the
/// production verdict must leave the gate's ledger untouched, or their 9 s
/// sample is an `over` row of every gate block.
#[test]
fn unit_tests_with_made_up_settle_samples_write_nothing_into_the_gate_ledger() {
    let bin = lib_test_binary();
    let listed = Command::new(&bin)
        .args([SETTLE_UNIT_TESTS, "--list"])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert_eq!(
        listed.matches(": test").count(),
        3,
        "{bin:?} must hold the three settle-budget unit tests:\n{listed}"
    );
    for mode in ["enforce", "measure"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let ledger = root.join("gate-ledger.jsonl");
        let mut cmd = Command::new(&bin);
        cmd.arg(SETTLE_UNIT_TESTS);
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy().into_owned();
            if k.starts_with("NEXTEST") || k.starts_with("HOLON_") || k == "RUST_TEST_THREADS" {
                cmd.env_remove(k);
            }
        }
        cmd.env("HOME", &home)
            .env("HOLON_TIMING_MEASURE_LOG", &ledger)
            .env("HOLON_GATE_ID", "gate1")
            .env("HOLON_PBT_TIMING", mode);
        let out = cmd.output().unwrap();
        let shown = format!(
            "mode {mode}\n--- stdout:\n{}\n--- stderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "the unit tests failed: {shown}");
        let written = std::fs::read_to_string(&ledger).unwrap_or_default();
        assert!(
            written.trim().is_empty(),
            "the gate ledger got lines from made-up samples:\n{written}\n{shown}"
        );
        assert!(
            ledger_lines(&home.join(".cache/holon-build/timing-checks.jsonl")).is_empty(),
            "the home ledger got lines from made-up samples: {shown}"
        );
    }
}
