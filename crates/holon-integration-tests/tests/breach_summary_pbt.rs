//! `scripts/timing-breaches.py` over generated ledgers.
//!
//! A ledger holds the lines of `small` duration checks. The script prints one
//! block per gate number: a header, a row per over-budget line, a TREND line
//! per series that is over budget in its last 3 gates and at least 1.5x the
//! median of its earlier gate values, and a footer. A malformed or missing
//! ledger never reads as "no breaches".

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

use crate::duration_budget_pbt::invalid_gate;
use crate::duration_budget_pbt::invalid_name;
use crate::duration_budget_pbt::valid_name;

const SCRIPT: &str = "scripts/timing-breaches.py";
const DEFAULT_LEDGER: &str = ".cache/holon-build/timing-checks.jsonl";
const KEYS: &[&str] = &[
    "gate", "test", "quantity", "value", "budget", "unit", "load", "over",
];
const VALUE: usize = 3;
const BUDGET: usize = 4;
const LOAD: usize = 6;

fn repo_root() -> PathBuf {
    Path::new(
        &std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by cargo for a test run"),
    )
    .join("../..")
    .canonicalize()
    .expect("the repository root exists")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    Local,
    Numbered { n: u128, rerun: Option<char> },
}

impl Gate {
    fn label(self) -> String {
        match self {
            Gate::Local => "local".into(),
            Gate::Numbered { n, rerun: None } => format!("gate{n}"),
            Gate::Numbered { n, rerun: Some(c) } => format!("gate{n}{c}"),
        }
    }

    fn number(self) -> Option<u128> {
        match self {
            Gate::Local => None,
            Gate::Numbered { n, .. } => Some(n),
        }
    }
}

#[derive(Debug, Clone)]
struct Line {
    gate: Gate,
    test: String,
    quantity: String,
    value: f64,
    budget: f64,
    unit: &'static str,
    load: f64,
    /// Key order of the JSON object, a permutation of `KEYS`.
    order: Vec<usize>,
    integral_numbers: bool,
}

fn number(x: f64, integral: bool) -> serde_json::Value {
    if integral && x.fract() == 0.0 && x.abs() < 9.0e15 {
        serde_json::Value::from(x as i64)
    } else {
        serde_json::Value::from(x)
    }
}

impl Line {
    fn over(&self) -> bool {
        self.value > self.budget
    }

    fn field(&self, key: &str) -> serde_json::Value {
        match key {
            "gate" => self.gate.label().into(),
            "test" => self.test.clone().into(),
            "quantity" => self.quantity.clone().into(),
            "value" => number(self.value, self.integral_numbers),
            "budget" => number(self.budget, self.integral_numbers),
            "unit" => self.unit.into(),
            "load" => number(self.load, self.integral_numbers),
            "over" => self.over().into(),
            other => unreachable!("no ledger key {other}"),
        }
    }

    fn object(&self) -> serde_json::Map<String, serde_json::Value> {
        self.order
            .iter()
            .map(|&i| (KEYS[i].to_string(), self.field(KEYS[i])))
            .collect()
    }

    fn render(&self) -> String {
        serde_json::Value::Object(self.object()).to_string()
    }
}

/// A JSON number token that is not a finite float once read: what Python's
/// `json` accepts for it, and the float it reads.
#[derive(Debug, Clone)]
struct NonFiniteToken {
    text: String,
    read_as: f64,
}

fn non_finite_tokens() -> Vec<NonFiniteToken> {
    let t = |text: &str, read_as: f64| NonFiniteToken {
        text: text.into(),
        read_as,
    };
    vec![
        t("NaN", f64::NAN),
        t("Infinity", f64::INFINITY),
        t("-Infinity", f64::NEG_INFINITY),
        t("1e999", f64::INFINITY),
        t("-1e999", f64::NEG_INFINITY),
        t(&format!("1{}", "0".repeat(400)), f64::INFINITY),
    ]
}

/// One way a ledger line breaks the line grammar.
#[derive(Debug, Clone)]
enum Breakage {
    NotJson(&'static str),
    Truncated,
    MissingKey(usize),
    ExtraKey,
    WrongType(usize),
    BoolNumber(usize),
    BadGate(String),
    BadUnit(&'static str),
    InvalidUtf8,
    OverDisagrees,
    BadName { quantity: bool, name: String },
    Negative(usize),
    NonFinite(usize, NonFiniteToken),
}

#[derive(Debug, Clone)]
enum Entry {
    Good(Line),
    Bad(Line, Breakage),
    Blank,
}

/// `over` as the reader computes it from the line's numbers.
fn consistent_over(obj: &mut serde_json::Map<String, serde_json::Value>, value: f64, budget: f64) {
    obj.insert("over".into(), (value > budget).into());
}

impl Entry {
    fn bytes(&self) -> Vec<u8> {
        match self {
            Entry::Good(l) => l.render().into_bytes(),
            Entry::Blank => Vec::new(),
            Entry::Bad(l, b) => {
                let mut obj = l.object();
                match b {
                    Breakage::NotJson(text) => return text.as_bytes().to_vec(),
                    Breakage::Truncated => {
                        let s = l.render();
                        return s.as_bytes()[..s.len() / 2].to_vec();
                    }
                    Breakage::MissingKey(i) => {
                        obj.remove(KEYS[*i]);
                    }
                    Breakage::ExtraKey => {
                        obj.insert("subject".into(), "extra".into());
                    }
                    Breakage::WrongType(i) => {
                        let wrong = match KEYS[*i] {
                            "value" | "budget" | "load" => serde_json::Value::from("5"),
                            "over" => serde_json::Value::from("true"),
                            _ => serde_json::Value::from(5),
                        };
                        obj.insert(KEYS[*i].into(), wrong);
                    }
                    Breakage::BoolNumber(i) => {
                        obj.insert(KEYS[*i].into(), true.into());
                    }
                    Breakage::BadGate(g) => {
                        obj.insert("gate".into(), g.clone().into());
                    }
                    Breakage::BadUnit(u) => {
                        obj.insert("unit".into(), (*u).into());
                    }
                    Breakage::OverDisagrees => {
                        obj.insert("over".into(), (!l.over()).into());
                    }
                    Breakage::BadName { quantity, name } => {
                        let key = if *quantity { "quantity" } else { "test" };
                        obj.insert(key.into(), name.clone().into());
                    }
                    Breakage::Negative(i) => {
                        let neg = -(l.field(KEYS[*i]).as_f64().unwrap().max(0.0)) - 0.25;
                        obj.insert(KEYS[*i].into(), neg.into());
                        // A negative byte value is a freed-memory delta (L15).
                        if *i == VALUE {
                            obj.insert("unit".into(), "ms".into());
                        }
                        match *i {
                            VALUE => consistent_over(&mut obj, neg, l.budget),
                            BUDGET => consistent_over(&mut obj, l.value, neg),
                            _ => {}
                        }
                    }
                    Breakage::NonFinite(i, token) => {
                        match *i {
                            VALUE => consistent_over(&mut obj, token.read_as, l.budget),
                            BUDGET => consistent_over(&mut obj, l.value, token.read_as),
                            _ => {}
                        }
                        obj.insert(KEYS[*i].into(), "@NONFINITE@".into());
                        return serde_json::Value::Object(obj)
                            .to_string()
                            .replace("\"@NONFINITE@\"", &token.text)
                            .into_bytes();
                    }
                    Breakage::InvalidUtf8 => {
                        let mut v = serde_json::Value::Object(obj).to_string().into_bytes();
                        let at = v.iter().position(|&c| c == b':').unwrap() + 2;
                        v.insert(at, 0xff);
                        return v;
                    }
                }
                serde_json::Value::Object(obj).to_string().into_bytes()
            }
        }
    }
}

struct Ledger {
    dir: tempfile::TempDir,
    path: PathBuf,
    entries: Vec<Entry>,
}

impl Ledger {
    /// A trailing blank entry forces the final newline: without it the blank
    /// line would not be in the file.
    fn write(entries: Vec<Entry>, trailing_newline: bool) -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir
            .path()
            .canonicalize()
            .expect("the temp dir resolves")
            .join("timing-checks.jsonl");
        let mut bytes = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            if i > 0 {
                bytes.push(b'\n');
            }
            bytes.extend(e.bytes());
        }
        let ends_blank = matches!(entries.last(), Some(Entry::Blank));
        if (trailing_newline && !entries.is_empty()) || ends_blank {
            bytes.push(b'\n');
        }
        std::fs::write(&path, bytes).expect("the ledger is written");
        Ledger { dir, path, entries }
    }

    /// 1-based line numbers of the malformed lines.
    fn malformed(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !matches!(e, Entry::Good(_)))
            .map(|(i, _)| i + 1)
            .collect()
    }

    fn good(&self) -> Vec<&Line> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                Entry::Good(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&std::fs::read(&self.path).unwrap_or_default()).into_owned()
    }
}

#[derive(Debug)]
pub(crate) struct Run {
    pub(crate) code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Where the script is told to look for the ledger.
struct Source<'a> {
    flag: Option<&'a Path>,
    var: Option<&'a Path>,
    home: &'a Path,
}

fn run_script(source: Source<'_>, gate: Option<&str>) -> Run {
    let mut cmd = Command::new("python3");
    cmd.arg(SCRIPT)
        .current_dir(repo_root())
        .env("HOME", source.home)
        .env_remove("HOLON_TIMING_MEASURE_LOG")
        .env_remove("HOLON_GATE_ID");
    if let Some(p) = source.flag {
        cmd.arg("--ledger").arg(p);
    }
    if let Some(p) = source.var {
        cmd.env("HOLON_TIMING_MEASURE_LOG", p);
    }
    if let Some(g) = gate {
        cmd.arg(g);
    }
    let out = cmd.output().expect("python3 runs the summary script");
    Run {
        code: out.status.code().expect("the script exits"),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

pub(crate) fn summarize(ledger: &Path, home: &Path, gate: Option<&str>) -> Run {
    run_script(
        Source {
            flag: Some(ledger),
            var: None,
            home,
        },
        gate,
    )
}

// --- reference model --------------------------------------------------------

/// Python's `repr(float(x))`: shortest round-trip digits, exponent form
/// outside [1e-4, 1e16) with a signed exponent of at least two digits.
fn py_repr(x: f64) -> String {
    let s = format!("{x:?}");
    match s.split_once('e') {
        None => s,
        Some((mantissa, exp)) => {
            let e: i32 = exp.parse().expect("an f64 exponent");
            let sign = if e < 0 { '-' } else { '+' };
            format!("{mantissa}e{sign}{:02}", e.abs())
        }
    }
}

/// Per gate number: the largest value of the series there and whether any
/// of its lines there is over budget.
type Series = BTreeMap<u128, (f64, bool)>;

/// Series keyed by (test, quantity); the unit is the series' unit.
fn series(lines: &[&Line]) -> BTreeMap<(String, String), (Series, &'static str)> {
    let mut out: BTreeMap<(String, String), (Series, &'static str)> = BTreeMap::new();
    for l in lines {
        let Some(n) = l.gate.number() else { continue };
        let (s, _) = out
            .entry((l.test.clone(), l.quantity.clone()))
            .or_insert((Series::new(), l.unit));
        let e = s.entry(n).or_insert((f64::NEG_INFINITY, false));
        e.0 = e.0.max(l.value);
        e.1 |= l.over();
    }
    out
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Block {
    header: String,
    rows: Vec<String>,
    trends: Vec<String>,
    footer: String,
}

fn expected_block(lines: &[&Line], n: u128) -> Block {
    let all = series(lines);
    let of_gate: Vec<&Line> = lines
        .iter()
        .copied()
        .filter(|l| l.gate.number() == Some(n))
        .collect();
    let over: Vec<&Line> = of_gate.iter().copied().filter(|l| l.over()).collect();
    let rows = over
        .iter()
        .map(|l| {
            let (s, _) = &all[&(l.test.clone(), l.quantity.clone())];
            let mut last5: Vec<String> = s
                .range(..=n)
                .rev()
                .take(5)
                .map(|(_, &(v, _))| py_repr(v))
                .collect();
            last5.reverse();
            [
                "over".to_string(),
                l.test.clone(),
                l.quantity.clone(),
                py_repr(l.value),
                py_repr(l.budget),
                l.unit.to_string(),
                py_repr(l.load),
                last5.join(","),
            ]
            .join("\t")
        })
        .collect();
    let mut trends = Vec::new();
    for ((test, quantity), (s, unit)) in &all {
        if !s.contains_key(&n) {
            continue;
        }
        let ran: Vec<(u128, f64, bool)> = s.range(..=n).map(|(&g, &(v, o))| (g, v, o)).collect();
        let k = ran.len();
        if k < 3 || !ran[k - 3..].iter().all(|r| r.2) {
            continue;
        }
        let history: Vec<f64> = ran[k.saturating_sub(11)..k - 1]
            .iter()
            .map(|r| r.1)
            .collect();
        let m = median(&history);
        if ran[k - 1].1 >= 1.5 * m {
            let gates: Vec<String> = ran[k - 3..]
                .iter()
                .map(|r| format!("gate{}", r.0))
                .collect();
            trends.push(
                [
                    "TREND".to_string(),
                    test.clone(),
                    quantity.clone(),
                    py_repr(ran[k - 1].1),
                    py_repr(m),
                    unit.to_string(),
                    gates.join(","),
                ]
                .join("\t"),
            );
        }
    }
    Block {
        header: format!(
            "== timing gate{n}: {} over of {} checks ==",
            over.len(),
            of_gate.len()
        ),
        rows,
        trends,
        footer: format!("== end gate{n} =="),
    }
}

fn expected_blocks(lines: &[&Line]) -> Vec<Block> {
    let mut gates: Vec<u128> = lines.iter().filter_map(|l| l.gate.number()).collect();
    gates.sort();
    gates.dedup();
    gates
        .into_iter()
        .map(|n| expected_block(lines, n))
        .collect()
}

/// The blocks of the script's stdout; blank lines are allowed anywhere, any
/// other line outside a block or inside one that is neither a row nor a
/// TREND line is an error.
fn parse_blocks(stdout: &str) -> Result<Vec<Block>, String> {
    let mut out = Vec::new();
    let mut lines = stdout.split('\n').filter(|l| !l.is_empty());
    while let Some(header) = lines.next() {
        if !header.starts_with("== timing gate") {
            return Err(format!("a line outside any block: {header:?}"));
        }
        let mut block = Block {
            header: header.into(),
            rows: Vec::new(),
            trends: Vec::new(),
            footer: String::new(),
        };
        loop {
            let Some(l) = lines.next() else {
                return Err(format!("the block {header:?} has no footer"));
            };
            if l.starts_with("== end gate") {
                block.footer = l.into();
                break;
            } else if l.starts_with("over\t") {
                block.rows.push(l.into());
            } else if l.starts_with("TREND\t") {
                block.trends.push(l.into());
            } else {
                return Err(format!("a line in {header:?} that is no row: {l:?}"));
            }
        }
        out.push(block);
    }
    Ok(out)
}

fn judge_output(run: &Run, want: &[Block], ledger: &str) -> Result<(), TestCaseError> {
    prop_assert_eq!(
        run.code,
        0,
        "a ledger of valid lines must summarize:\n{}\n--- stderr:\n{}",
        ledger,
        run.stderr
    );
    let got = parse_blocks(&run.stdout)
        .map_err(|e| TestCaseError::fail(format!("{e}\n--- stdout:\n{}", run.stdout)))?;
    prop_assert_eq!(
        &got,
        &want.to_vec(),
        "blocks (L9-L12)\nledger:\n{}\n--- stdout:\n{}",
        ledger,
        run.stdout
    );
    Ok(())
}

// --- generators -------------------------------------------------------------

fn gate_number() -> impl Strategy<Value = u128> {
    prop_oneof![
        8 => 1u128..40,
        1 => 1u128..200_000,
        1 => (u128::MAX - 1000)..=u128::MAX,
    ]
}

fn rerun() -> impl Strategy<Value = Option<char>> {
    prop_oneof![4 => Just(None), 1 => proptest::char::range('a', 'z').prop_map(Some)]
}

fn unit() -> impl Strategy<Value = &'static str> {
    proptest::sample::select(vec!["ms", "B"])
}

/// Values on a quarter grid, so sums, halves and `1.5 * median` are exact in
/// every evaluation order.
fn grid(q: u32) -> f64 {
    f64::from(q) / 4.0
}

/// Powers of two keep grid values exact and reach Python's exponent form
/// on both sides.
fn scale() -> impl Strategy<Value = f64> {
    proptest::sample::select(vec![1.0, 2f64.powi(60), 2f64.powi(-40)])
}

fn wide_value() -> impl Strategy<Value = f64> {
    prop_oneof![
        3 => (0u32..4000).prop_map(grid),
        1 => Just(0.0),
        1 => Just(5e-324),
        1 => Just(1e300),
        1 => Just(1e16),
        1 => Just(1e-4),
        1 => 0.0f64..1e9,
    ]
}

fn order() -> impl Strategy<Value = Vec<usize>> {
    Just((0..KEYS.len()).collect::<Vec<_>>()).prop_shuffle()
}

#[derive(Debug, Clone)]
struct SeriesName {
    test: String,
    quantity: String,
    budget_q: u32,
    unit: &'static str,
    scale: f64,
}

fn series_names() -> impl Strategy<Value = Vec<SeriesName>> {
    proptest::collection::vec(
        (valid_name(), valid_name(), 1u32..400, unit(), scale()),
        1..5,
    )
    .prop_map(|mut v| {
        v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        v.dedup_by(|a, b| (&a.0, &a.1) == (&b.0, &b.1));
        v.into_iter()
            .map(|(test, quantity, budget_q, unit, scale)| SeriesName {
                test,
                quantity,
                budget_q,
                unit,
                scale,
            })
            .collect()
    })
}

/// A ledger of valid lines: per series and gate one to three lines (a rerun
/// label counts as its gate), plus `local` lines, in shuffled file order.
fn trend_ledger() -> impl Strategy<Value = Vec<Line>> {
    (
        series_names(),
        proptest::collection::vec(gate_number(), 1..14),
    )
        .prop_flat_map(|(names, gates)| {
            let mut gates = gates;
            gates.sort();
            gates.dedup();
            let n = names.len();
            let slots = gates.len() * n;
            (
                Just(names),
                Just(gates),
                proptest::collection::vec(
                    (
                        prop_oneof![1 => Just(false), 3 => Just(true)],
                        proptest::collection::vec(
                            (
                                rerun(),
                                prop_oneof![2 => (0u32..4000).prop_map(Some), 3 => Just(None)],
                                (0u32..400).prop_map(|q| f64::from(q) / 8.0),
                                order(),
                                any::<bool>(),
                                prop_oneof![5 => Just(false), 1 => Just(true)],
                            ),
                            1..4,
                        ),
                    ),
                    slots,
                ),
                proptest::collection::vec((0..n, 0u32..4000, order()), 0..4),
            )
        })
        .prop_map(|(names, gates, slots, locals)| {
            let mut lines = Vec::new();
            for (gi, &g) in gates.iter().enumerate() {
                for (si, s) in names.iter().enumerate() {
                    let (ran, draws) = slots[gi * names.len() + si].clone();
                    if !ran {
                        continue;
                    }
                    for (rerun, q, load, order, integral, freed) in draws {
                        // No grid draw means over budget by a growing step.
                        let value = match q {
                            Some(q) => signed(s.unit, freed, grid(q)),
                            None => grid(s.budget_q) + 0.25 * (gi as f64 + 1.0),
                        };
                        lines.push(Line {
                            gate: Gate::Numbered { n: g, rerun },
                            test: s.test.clone(),
                            quantity: s.quantity.clone(),
                            value: value * s.scale,
                            budget: grid(s.budget_q) * s.scale,
                            unit: s.unit,
                            load,
                            order,
                            integral_numbers: integral,
                        });
                    }
                }
            }
            for (si, q, order) in locals {
                let s = &names[si];
                lines.push(Line {
                    gate: Gate::Local,
                    test: s.test.clone(),
                    quantity: s.quantity.clone(),
                    value: grid(q) * s.scale,
                    budget: grid(s.budget_q) * s.scale,
                    unit: s.unit,
                    load: 30.5,
                    order,
                    integral_numbers: false,
                });
            }
            lines
        })
        .prop_shuffle()
}

/// A byte quantity may be a negative delta (freed memory); a time never is.
fn signed(unit: &str, freed: bool, value: f64) -> f64 {
    if unit == "B" && freed && value > 0.0 {
        -value
    } else {
        value
    }
}

/// The unit follows from the quantity, so one series has one unit.
fn unit_of(quantity: &str) -> &'static str {
    if quantity.len().is_multiple_of(2) {
        "ms"
    } else {
        "B"
    }
}

fn any_line() -> impl Strategy<Value = Line> {
    (
        prop_oneof![
            1 => Just(Gate::Local),
            4 => (gate_number(), rerun()).prop_map(|(n, rerun)| Gate::Numbered { n, rerun }),
        ],
        valid_name(),
        valid_name(),
        wide_value(),
        wide_value(),
        wide_value(),
        order(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(gate, test, quantity, value, budget, load, order, integral_numbers, freed)| Line {
                gate,
                unit: unit_of(&quantity),
                value: signed(unit_of(&quantity), freed, value),
                test,
                quantity,
                budget,
                load,
                order,
                integral_numbers,
            },
        )
}

fn breakage() -> impl Strategy<Value = Breakage> {
    let n = KEYS.len();
    let number_key = || prop_oneof![Just(VALUE), Just(BUDGET), Just(LOAD)];
    prop_oneof![
        proptest::sample::select(vec![
            "{", "garbage", "[]", "\"line\"", "null", "1", "{}", " "
        ])
        .prop_map(Breakage::NotJson),
        Just(Breakage::Truncated),
        (0..n).prop_map(Breakage::MissingKey),
        Just(Breakage::ExtraKey),
        (0..n).prop_map(Breakage::WrongType),
        number_key().prop_map(Breakage::BoolNumber),
        invalid_gate().prop_map(Breakage::BadGate),
        proptest::sample::select(vec!["", "s", "MS", "b", "bytes", " ms", "ms "])
            .prop_map(Breakage::BadUnit),
        Just(Breakage::InvalidUtf8),
        Just(Breakage::OverDisagrees),
        (any::<bool>(), invalid_name())
            .prop_map(|(quantity, name)| Breakage::BadName { quantity, name }),
        number_key().prop_map(Breakage::Negative),
        (number_key(), proptest::sample::select(non_finite_tokens()))
            .prop_map(|(i, t)| Breakage::NonFinite(i, t)),
    ]
}

fn entries_with_breakage() -> impl Strategy<Value = Vec<Entry>> {
    proptest::collection::vec(
        prop_oneof![
            6 => any_line().prop_map(Entry::Good),
            1 => Just(Entry::Blank),
            1 => (any_line(), breakage()).prop_map(|(l, b)| Entry::Bad(l, b)),
        ],
        0..20,
    )
}

/// A GATE argument: none, a gate number (present in the ledger or not) with
/// or without a rerun letter, or a refused label.
#[derive(Debug, Clone)]
enum GateArg {
    All,
    Number(prop::sample::Index, u128, Option<char>, bool),
    Refused(String),
}

fn gate_arg() -> impl Strategy<Value = GateArg> {
    prop_oneof![
        2 => Just(GateArg::All),
        3 => (any::<prop::sample::Index>(), gate_number(), rerun(), any::<bool>())
            .prop_map(|(i, n, r, present)| GateArg::Number(i, n, r, present)),
        1 => prop_oneof![Just("local".to_string()), invalid_gate()].prop_map(GateArg::Refused),
    ]
}

// --- properties -------------------------------------------------------------

fn runner(cases: u32) -> TestRunner {
    TestRunner::new(Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    })
}

fn judge_exit(entries: Vec<Entry>, trailing: bool) -> Result<(), TestCaseError> {
    let ledger = Ledger::write(entries, trailing);
    let run = summarize(&ledger.path, ledger.dir.path(), None);
    let bad = ledger.malformed();
    let path = ledger.path.display().to_string();
    if bad.is_empty() {
        return judge_output(&run, &expected_blocks(&ledger.good()), &ledger.text());
    }
    prop_assert_eq!(
        run.code,
        2,
        "a malformed line at {}:{:?} must exit 2; stdout:\n{}\n--- stderr:\n{}",
        path,
        bad,
        run.stdout,
        run.stderr
    );
    let named: Vec<usize> = run
        .stderr
        .match_indices(&format!("{path}:"))
        .filter_map(|(i, m)| {
            let rest = &run.stderr[i + m.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            rest[digits.len()..]
                .starts_with(": ")
                .then(|| digits.parse().ok())
                .flatten()
        })
        .collect();
    prop_assert!(
        !named.is_empty() && named.iter().all(|n| bad.contains(n)),
        "stderr must name a malformed line as `path:line: reason` (malformed: {:?}, named: {:?}):\n{}\n--- ledger:\n{}",
        bad,
        named,
        run.stderr,
        ledger.text()
    );
    Ok(())
}

#[test]
fn a_malformed_ledger_line_anywhere_exits_two_naming_it_and_a_valid_ledger_prints_its_blocks() {
    let blank_only = vec![Entry::Blank];
    let valid_then_blank = vec![
        Entry::Good(at(1, 6.0, 5.0)),
        Entry::Blank,
        Entry::Good(at(2, 6.0, 5.0)),
    ];
    for (e, t) in [
        (Vec::new(), false),
        (blank_only, false),
        (valid_then_blank, true),
    ] {
        judge_exit(e, t).unwrap_or_else(|e| panic!("{e}"));
    }
    runner(128)
        .run(&(entries_with_breakage(), any::<bool>()), |(e, t)| {
            judge_exit(e, t)
        })
        .unwrap_or_else(|e| panic!("{e}"));
}

/// The ledger is the `--ledger` file, else `$HOLON_TIMING_MEASURE_LOG`, else
/// `$HOME/.cache/holon-build/timing-checks.jsonl`; a missing one exits 2 with
/// `<path>: no ledger`. All 8 combinations of the three sources.
#[test]
fn the_ledger_comes_from_the_flag_then_the_variable_then_home() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path().canonicalize().unwrap();
    let write = |path: &Path, gate: u128| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{}\n", at(gate, 1.0, 5.0).render())).unwrap();
    };
    let flag = root.join("flag.jsonl");
    let var = root.join("var.jsonl");
    write(&flag, 1);
    write(&var, 2);
    for home_has_ledger in [false, true] {
        let home = root.join(if home_has_ledger {
            "home"
        } else {
            "empty-home"
        });
        std::fs::create_dir_all(&home).unwrap();
        let home_ledger = home.join(DEFAULT_LEDGER);
        if home_has_ledger {
            write(&home_ledger, 3);
        }
        for (use_flag, use_var) in [(false, false), (false, true), (true, false), (true, true)] {
            let run = run_script(
                Source {
                    flag: use_flag.then_some(flag.as_path()),
                    var: use_var.then_some(var.as_path()),
                    home: &home,
                },
                None,
            );
            let case = format!("flag {use_flag}, var {use_var}, home ledger {home_has_ledger}");
            let gate = if use_flag {
                Some(1)
            } else if use_var {
                Some(2)
            } else if home_has_ledger {
                Some(3)
            } else {
                None
            };
            match gate {
                Some(g) => {
                    let want = expected_blocks(&[&at(g, 1.0, 5.0)]);
                    judge_output(&run, &want, &case).unwrap_or_else(|e| panic!("{case}: {e}"));
                }
                None => assert!(
                    run.code == 2
                        && run
                            .stderr
                            .contains(&format!("{}: no ledger", home_ledger.display())),
                    "{case}: a missing ledger must exit 2 with `{}: no ledger`:\n{run:?}",
                    home_ledger.display()
                ),
            }
        }
    }
}

#[test]
fn a_missing_or_unreadable_ledger_exits_non_zero_naming_its_path() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path().canonicalize().unwrap();
    let missing = root.join("absent.jsonl");
    let run = summarize(&missing, &root, None);
    assert!(
        run.code == 2
            && run
                .stderr
                .contains(&format!("{}: no ledger", missing.display())),
        "a missing ledger must exit 2 with `<path>: no ledger`:\n{run:?}"
    );
    let directory = root.join("a-directory");
    std::fs::create_dir(&directory).unwrap();
    let no_read = root.join("no-read.jsonl");
    std::fs::write(&no_read, b"\n").unwrap();
    std::fs::set_permissions(&no_read, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut unreadable = vec![directory];
    // root reads a mode-000 file, so the case is unreachable there
    if std::fs::read(&no_read).is_err() {
        unreadable.push(no_read);
    }
    for path in unreadable {
        let run = summarize(&path, &root, None);
        assert!(
            run.code == 2
                && run.stderr.contains(&path.display().to_string())
                && !run.stderr.contains("Traceback"),
            "an unreadable ledger must exit 2 naming it (L6 exits only 0 or 2):\n{run:?}"
        );
    }
}

fn judge_blocks(lines: Vec<Line>, arg: GateArg) -> Result<(), TestCaseError> {
    let entries: Vec<Entry> = lines.into_iter().map(Entry::Good).collect();
    let ledger = Ledger::write(entries, true);
    let good = ledger.good();
    let text = ledger.text();
    match arg {
        GateArg::All => {
            let run = summarize(&ledger.path, ledger.dir.path(), None);
            judge_output(&run, &expected_blocks(&good), &text)
        }
        GateArg::Number(i, other, rerun, present) => {
            let ran: Vec<u128> = good.iter().filter_map(|l| l.gate.number()).collect();
            let n = if present && !ran.is_empty() {
                ran[i.index(ran.len())]
            } else {
                other
            };
            let label = Gate::Numbered { n, rerun }.label();
            let run = summarize(&ledger.path, ledger.dir.path(), Some(&label));
            judge_output(
                &run,
                &[expected_block(&good, n)],
                &format!("GATE {label}\n{text}"),
            )
        }
        GateArg::Refused(label) => {
            let run = summarize(&ledger.path, ledger.dir.path(), Some(&label));
            prop_assert_eq!(
                run.code,
                2,
                "the GATE argument {:?} must exit 2:\n{:?}",
                label,
                run
            );
            Ok(())
        }
    }
}

fn at(gate: u128, value: f64, budget: f64) -> Line {
    Line {
        gate: Gate::Numbered {
            n: gate,
            rerun: None,
        },
        test: "suite::slow_step".into(),
        quantity: "step/p95".into(),
        value,
        budget,
        unit: "ms",
        load: 31.0,
        order: (0..KEYS.len()).collect(),
        integral_numbers: false,
    }
}

/// Fixed ledgers the block grammar must print: 3 over-budget gates in a row
/// under 1.5x the median, a gate the test did not run in between, a
/// duplicated line, a rerun gate, file order against gate order (also
/// `gate9` before `gate10`), the 1.5x boundary itself, and values Python
/// prints in exponent form.
fn block_examples() -> Vec<Vec<Line>> {
    let gap = {
        let mut other = at(4, 1.0, 5.0);
        other.test = "suite::other".into();
        vec![
            at(1, 2.0, 5.0),
            at(2, 2.0, 5.0),
            at(3, 6.0, 5.0),
            other,
            at(5, 6.0, 5.0),
            at(6, 6.0, 5.0),
        ]
    };
    let duplicate = vec![
        at(1, 4.0, 5.0),
        at(2, 6.0, 5.0),
        at(3, 6.0, 5.0),
        at(4, 10.0, 5.0),
        at(4, 10.0, 5.0),
    ];
    let rerun = {
        let mut r = at(3, 9.0, 5.0);
        r.gate = Gate::Numbered {
            n: 3,
            rerun: Some('c'),
        };
        vec![at(1, 6.0, 5.0), at(2, 6.0, 5.0), at(3, 5.5, 5.0), r]
    };
    let reversed = vec![at(3, 4.0, 1.0), at(1, 2.0, 1.0), at(2, 2.0, 1.0)];
    let decimal_order = vec![at(10, 4.0, 1.0), at(9, 2.0, 1.0), at(11, 6.0, 1.0)];
    let exponent = vec![
        at(1, 2f64.powi(60), 1e16),
        at(2, 2f64.powi(61), 1e16),
        at(3, 1e-5, 5e-324),
    ];
    vec![
        vec![at(1, 6.0, 5.0), at(2, 6.0, 5.0), at(3, 6.0, 5.0)],
        gap,
        duplicate,
        rerun,
        reversed,
        decimal_order,
        exponent,
        vec![at(1, 2.0, 1.0), at(2, 2.0, 1.0), at(3, 3.0, 1.0)],
        vec![at(1, 2.0, 1.0), at(2, 2.0, 1.0), at(3, 2.75, 1.0)],
        vec![
            at(1, 9.0, 1.0),
            at(2, 0.5, 1.0),
            at(3, 9.0, 1.0),
            at(4, 9.0, 1.0),
        ],
    ]
}

#[test]
fn every_gate_prints_its_block_of_over_rows_and_trend_lines() {
    for lines in block_examples() {
        judge_blocks(lines, GateArg::All).unwrap_or_else(|e| panic!("{e}"));
    }
    runner(128)
        .run(&(trend_ledger(), gate_arg()), |(l, a)| judge_blocks(l, a))
        .unwrap_or_else(|e| panic!("{e}"));
}
