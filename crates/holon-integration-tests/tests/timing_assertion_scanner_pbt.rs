//! `scripts/check-timing-assertions.sh` over a copy of this repository with
//! generated Rust files added.
//!
//! A small-duration comparison that decides pass or fail outside
//! `DurationBudget` is refused in every shape a test or test helper can write
//! it, unless a baseline row classes it as liveness, lower-bound,
//! virtual-clock or harness. The same measurement through
//! `DurationBudget::small`, and a load-insensitive count comparison, pass.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

/// The parts of the repository the scanner and its `check` half read.
const MIRRORED: &[&str] = &[
    "scripts",
    "docs",
    ".config",
    "crates",
    "frontends",
    "justfile",
    "Cargo.toml",
];
const SKIPPED: &[&str] = &["target", "node_modules", ".jj", ".git"];
const BASELINE: &str = "scripts/timing-assertions-baseline.txt";

fn repo_root() -> PathBuf {
    Path::new(
        &std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by cargo for a test run"),
    )
    .join("../..")
    .canonicalize()
    .expect("the repository root exists")
}

fn copy_tree(from: &Path, to: &Path) {
    let meta = std::fs::symlink_metadata(from).expect("a mirrored path exists");
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(from).expect("a symlink has a target");
        std::os::unix::fs::symlink(target, to).expect("the mirror takes the symlink");
    } else if meta.is_dir() {
        std::fs::create_dir_all(to).expect("the mirror takes the directory");
        for entry in std::fs::read_dir(from).expect("a mirrored directory is readable") {
            let entry = entry.expect("a directory entry is readable");
            let name = entry.file_name();
            if SKIPPED.iter().any(|s| name == *s) {
                continue;
            }
            copy_tree(&entry.path(), &to.join(&name));
        }
    } else {
        std::fs::copy(from, to).expect("the mirror takes the file");
    }
}

struct Mirror {
    dir: tempfile::TempDir,
    crates: Vec<String>,
    frontends: Vec<String>,
    baseline: String,
}

impl Mirror {
    fn new() -> Self {
        let root = repo_root();
        let dir = tempfile::tempdir().expect("a temp dir");
        for part in MIRRORED {
            copy_tree(&root.join(part), &dir.path().join(part));
        }
        let names = |sub: &str| -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(dir.path().join(sub))
                .expect("the mirror has the directory")
                .map(|e| e.expect("entry").path())
                .filter(|p| p.join("Cargo.toml").is_file())
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        let crates = names("crates");
        let frontends = names("frontends");
        let baseline = std::fs::read_to_string(dir.path().join(BASELINE))
            .expect("the mirror has the baseline");
        let m = Mirror {
            dir,
            crates,
            frontends,
            baseline,
        };
        let (code, out) = m.scan();
        assert_eq!(
            code, 0,
            "the tree under test fails its own duration-assertion scanner before any \
             generated file is added:\n{out}"
        );
        m
    }

    fn scan(&self) -> (i32, String) {
        let out = Command::new("bash")
            .arg("scripts/check-timing-assertions.sh")
            .current_dir(self.dir.path())
            .output()
            .expect("bash runs the scanner");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.code().expect("the scanner exits"), text)
    }

    /// Writes every probe, scans once, and restores the tree: one scan reads
    /// the whole repository, so a scan per probe would not fit the test slot.
    fn scan_with(&self, probes: &[Probe]) -> (i32, String, String) {
        let mut b = self.baseline.clone();
        if !b.ends_with('\n') {
            b.push('\n');
        }
        let mut created_dirs = Vec::new();
        let mut files = Vec::new();
        let mut shown = String::new();
        for probe in probes {
            let rel = probe.path(self);
            let file = self.dir.path().join(&rel);
            assert!(
                !file.exists(),
                "a generated probe must be a new file: {rel}"
            );
            let parent = file.parent().unwrap().to_path_buf();
            if !parent.exists() {
                std::fs::create_dir_all(&parent).unwrap();
                created_dirs.push(parent);
            }
            let (source, line) = probe.source();
            std::fs::write(&file, &source).unwrap();
            if let Some(class) = probe.row {
                b.push_str(&format!("{rel}\t{}\t{}\n", class.name(), line.trim()));
            }
            shown.push_str(&format!("{rel} (row {:?})\n{source}\n", probe.row));
            files.push(file);
        }
        let baseline_path = self.dir.path().join(BASELINE);
        std::fs::write(&baseline_path, b).unwrap();
        let (code, out) = self.scan();
        for file in files {
            std::fs::remove_file(file).unwrap();
        }
        for dir in created_dirs.into_iter().rev() {
            std::fs::remove_dir(dir).unwrap();
        }
        std::fs::write(&baseline_path, &self.baseline).unwrap();
        (code, out, shown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowClass {
    Timing,
    Liveness,
    LowerBound,
    Harness,
    VirtualClock,
}

impl RowClass {
    fn name(self) -> &'static str {
        match self {
            RowClass::Timing => "timing",
            RowClass::Liveness => "liveness",
            RowClass::LowerBound => "lower-bound",
            RowClass::Harness => "harness",
            RowClass::VirtualClock => "virtual-clock",
        }
    }
}

/// Where the generated function lives; the scanner reads `src/` and `tests/`
/// of every crate and frontend.
#[derive(Debug, Clone, Copy)]
enum Place {
    CrateTests,
    CrateTestsSubdir,
    CrateSrcTestModule,
    CrateSrcHelper,
    FrontendTests,
}

/// How the comparison decides pass or fail.
#[derive(Debug, Clone, Copy)]
enum Form {
    Assert,
    AssertWithMessage,
    AssertMultiLine,
    AssertAfterComments,
    PropAssert,
    DebugAssert,
    AssertEqTrue,
    IfPanic,
    LetThenAssert,
    InvariantFail,
}

/// What is compared.
#[derive(Debug, Clone)]
enum Measure {
    /// A raw small-duration comparison: `lhs op rhs`.
    Raw {
        lhs: String,
        op: &'static str,
        rhs: String,
    },
    /// The same measurement through the budget type.
    Budget { value: String },
    /// A load-insensitive count comparison.
    Count {
        lhs: String,
        op: &'static str,
        rhs: String,
    },
}

#[derive(Debug, Clone)]
struct Probe {
    place: Place,
    pick: usize,
    stem: String,
    form: Form,
    measure: Measure,
    row: Option<RowClass>,
}

impl Probe {
    fn path(&self, m: &Mirror) -> String {
        let file = format!("scan_probe_{}.rs", self.stem);
        let krate = |names: &[String]| names[self.pick % names.len()].clone();
        match self.place {
            Place::CrateTests => format!("crates/{}/tests/{file}", krate(&m.crates)),
            Place::CrateTestsSubdir => {
                format!("crates/{}/tests/scan_probe_dir/{file}", krate(&m.crates))
            }
            Place::CrateSrcTestModule | Place::CrateSrcHelper => {
                format!("crates/{}/src/{file}", krate(&m.crates))
            }
            Place::FrontendTests => format!("frontends/{}/tests/{file}", krate(&m.frontends)),
        }
    }

    /// The file and the line a baseline row keys on.
    fn source(&self) -> (String, String) {
        let cmp = match &self.measure {
            Measure::Raw { lhs, op, rhs } | Measure::Count { lhs, op, rhs } => {
                format!("{lhs} {op} {rhs}")
            }
            Measure::Budget { value } => format!(
                "DurationBudget::small(\"probe\", Duration::from_millis(5)).check({value}).is_ok()"
            ),
        };
        let (body, line) = match self.form {
            Form::Assert => {
                let l = format!("assert!({cmp});");
                (l.clone(), l)
            }
            Form::AssertWithMessage => {
                let l = format!("assert!({cmp}, \"over the budget\");");
                (l.clone(), l)
            }
            Form::AssertMultiLine => {
                let l = format!("{cmp},");
                (
                    format!("assert!(\n        {l}\n        \"over the budget\"\n    );"),
                    l,
                )
            }
            Form::AssertAfterComments => {
                let l = format!("{cmp},");
                (
                    format!(
                        "assert!(\n        // the rung\n        // as scoped\n        // by the ruling\n        {l}\n        \"over the budget\"\n    );"
                    ),
                    l,
                )
            }
            Form::PropAssert => {
                let l = format!("prop_assert!({cmp});");
                (l.clone(), l)
            }
            Form::DebugAssert => {
                let l = format!("debug_assert!({cmp});");
                (l.clone(), l)
            }
            Form::AssertEqTrue => {
                let l = format!("assert_eq!({cmp}, true);");
                (l.clone(), l)
            }
            Form::IfPanic => {
                let l = format!("if !({cmp}) {{");
                (
                    format!("{l}\n        panic!(\"over the budget\");\n    }}"),
                    l,
                )
            }
            Form::LetThenAssert => {
                let l = format!("let within = {cmp};");
                (format!("{l}\n    assert!(within, \"over the budget\");"), l)
            }
            Form::InvariantFail => {
                let l = format!("if {cmp} {{");
                (
                    format!(
                        "{l}\n        InvariantResult::Pass\n    }} else {{\n        InvariantResult::Fail(\"over the budget\".into())\n    }};"
                    ),
                    l,
                )
            }
        };
        let fun = format!(
            "fn scan_probe_{}() {{\n    let start = std::time::Instant::now();\n    {body}\n}}\n",
            self.stem
        );
        let source = match self.place {
            Place::CrateSrcTestModule => format!(
                "#[cfg(test)]\nmod tests {{\n    use super::*;\n\n    #[test]\n    {}}}\n",
                fun.replace('\n', "\n    ")
            ),
            Place::CrateSrcHelper => format!("pub {fun}"),
            _ => format!("#[test]\n{fun}"),
        };
        (source, line)
    }

    /// The one expected verdict: a raw small-duration comparison is refused
    /// with no row or a `timing` row; a `small` call is refused unless its row
    /// is `timing` and it sits outside a test fn, which no owner key selects.
    fn refused(&self) -> bool {
        match self.measure {
            Measure::Raw { .. } => matches!(self.row, None | Some(RowClass::Timing)),
            Measure::Budget { .. } => {
                self.row != Some(RowClass::Timing) || !matches!(self.place, Place::CrateSrcHelper)
            }
            Measure::Count { .. } => false,
        }
    }
}

/// Duration-valued operands, as tests in this repository spell them.
const DURATION_LHS: &[&str] = &[
    "start.elapsed()",
    "t0.elapsed()",
    "elapsed",
    "p50",
    "p95",
    "p99",
    "p95_ms",
    "p50_us",
    "gated_ms",
    "latency_ms",
    "compile_ms",
    "wall.as_millis()",
    "dur.as_secs_f64()",
    "settle.as_micros()",
    "sum_p50",
];
const DURATION_RHS: &[&str] = &[
    "Duration::from_millis(5)",
    "Duration::from_secs(2)",
    "Duration::from_micros(750)",
    "Duration::from_secs_f64(0.005)",
    "D165A_GATE_MS",
    "SLO_MS",
    "STEP_US",
    "SETTLE_NS",
    "BOOT_SECS",
    "FLUSH_MILLIS",
    "SETTLE_BUDGET",
    "SYNC_DEADLINE",
    "budget as u128",
    "5.0",
    "1.0",
    "5_000",
];
/// Operands that carry no duration on either side of the comparison.
const COUNT_LHS: &[&str] = &["rows.len()", "sql_count", "statements", "n_queries"];
const COUNT_RHS: &[&str] = &["MAX_ROWS", "10_000", "limit", "STATEMENT_CAP"];
const OPS: &[&str] = &["<", "<=", ">", ">="];

fn pick(xs: &'static [&'static str]) -> impl Strategy<Value = String> {
    proptest::sample::select(xs).prop_map(str::to_string)
}

fn place() -> impl Strategy<Value = Place> {
    prop_oneof![
        Just(Place::CrateTests),
        Just(Place::CrateTestsSubdir),
        Just(Place::CrateSrcTestModule),
        Just(Place::CrateSrcHelper),
        Just(Place::FrontendTests),
    ]
}

fn form() -> impl Strategy<Value = Form> {
    prop_oneof![
        Just(Form::Assert),
        Just(Form::AssertWithMessage),
        Just(Form::AssertMultiLine),
        Just(Form::AssertAfterComments),
        Just(Form::PropAssert),
        Just(Form::DebugAssert),
        Just(Form::AssertEqTrue),
        Just(Form::IfPanic),
        Just(Form::LetThenAssert),
        Just(Form::InvariantFail),
    ]
}

fn measure() -> impl Strategy<Value = Measure> {
    let raw = (
        pick(DURATION_LHS),
        proptest::sample::select(OPS),
        pick(DURATION_RHS),
        any::<bool>(),
    )
        .prop_map(|(a, op, b, swap)| {
            let (lhs, rhs) = if swap { (b, a) } else { (a, b) };
            Measure::Raw { lhs, op, rhs }
        });
    let count = (
        pick(COUNT_LHS),
        proptest::sample::select(OPS),
        pick(COUNT_RHS),
    )
        .prop_map(|(lhs, op, rhs)| Measure::Count { lhs, op, rhs });
    let budget = prop_oneof![
        Just("start.elapsed()".to_string()),
        Just("from_ms(p95)".to_string()),
        Just("from_ms(gated)".to_string()),
    ]
    .prop_map(|value| Measure::Budget { value });
    prop_oneof![3 => raw, 1 => count, 1 => budget]
}

fn row() -> impl Strategy<Value = Option<RowClass>> {
    prop_oneof![
        3 => Just(None),
        1 => Just(Some(RowClass::Timing)),
        1 => Just(Some(RowClass::Liveness)),
        1 => Just(Some(RowClass::LowerBound)),
        1 => Just(Some(RowClass::Harness)),
        1 => Just(Some(RowClass::VirtualClock)),
    ]
}

fn probe() -> impl Strategy<Value = Probe> {
    (
        place(),
        any::<usize>(),
        "[a-z][a-z0-9_]{0,12}",
        form(),
        measure(),
        row(),
    )
        .prop_map(|(place, pick, stem, form, measure, row)| Probe {
            place,
            pick,
            stem,
            form,
            measure,
            row,
        })
}

fn raw(lhs: &str, op: &'static str, rhs: &str) -> Measure {
    Measure::Raw {
        lhs: lhs.into(),
        op,
        rhs: rhs.into(),
    }
}

/// The comparisons the line scanner let through before it learned the
/// small-duration shapes, as their files spell them.
fn known_escapes() -> Vec<Probe> {
    let at = |form, measure| Probe {
        place: Place::CrateTests,
        pick: 0,
        stem: "known_escape".into(),
        form,
        measure,
        row: None,
    };
    vec![
        at(Form::AssertMultiLine, raw("gated", "<=", "D165A_GATE_MS")),
        at(
            Form::AssertWithMessage,
            raw("p95_ms", "<", "budget as u128"),
        ),
        at(Form::AssertMultiLine, raw("p95", "<", "1.0")),
        at(Form::AssertMultiLine, raw("p95", "<", "5_000")),
        at(Form::AssertMultiLine, raw("compile_ms", "<", "SLO_MS")),
        at(Form::IfPanic, raw("completion", "<=", "limit_ms")),
    ]
}

/// Gives each probe of one scan its own file name.
fn numbered(probes: Vec<Probe>) -> Vec<Probe> {
    probes
        .into_iter()
        .enumerate()
        .map(|(i, p)| Probe {
            stem: format!("{}_{i}", p.stem),
            ..p
        })
        .collect()
}

/// One scan judges each probe by its own file: a refused site is named, an
/// accepted one is not, and the exit is 1 iff some site is refused.
fn judge(m: &Mirror, probes: &[Probe]) -> Result<(), TestCaseError> {
    let (code, out, shown) = m.scan_with(probes);
    let wrong: Vec<String> = probes
        .iter()
        .filter(|p| out.contains(&p.path(m)) != p.refused())
        .map(|p| {
            let verdict = if p.refused() { "refuse" } else { "accept" };
            format!(
                "must {verdict} {} (row {:?}):\n{}",
                p.path(m),
                p.row,
                p.source().0
            )
        })
        .collect();
    prop_assert!(
        wrong.is_empty(),
        "{}\n--- scanner (exit {}):\n{}",
        wrong.join("\n"),
        code,
        out
    );
    let want = i32::from(probes.iter().any(Probe::refused));
    prop_assert_eq!(
        code,
        want,
        "scanner exit:\n{}\n--- scanner:\n{}",
        shown,
        out
    );
    Ok(())
}

#[test]
fn a_small_duration_comparison_outside_the_budget_type_is_refused_in_every_shape() {
    let m = Mirror::new();
    judge(&m, &numbered(known_escapes())).unwrap_or_else(|e| panic!("{e}"));
    let mut runner = TestRunner::new(Config {
        cases: 2,
        // A shrink step costs a whole-tree scan; the verdict per file already
        // names each probe the scanner misjudges.
        max_shrink_iters: 0,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(
            &proptest::collection::vec(probe(), 8..=40).prop_map(numbered),
            |ps| judge(&m, &ps),
        )
        .unwrap_or_else(|e| panic!("{e}"));
}
