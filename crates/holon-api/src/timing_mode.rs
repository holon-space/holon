//! `HOLON_PBT_TIMING` — whether a budget whose quantity is load-perturbed (a
//! duration, an RSS ceiling) fails the run or only reports, and the ledger in
//! which every such check leaves one line.
//!
//! - `measure` (the default when the variable is unset): over budget is `Ok`,
//!   printed as one `MEASURE-ONLY` line.
//! - `enforce`: over budget fails.
//!
//! In both modes every check appends one line to the ledger, within budget
//! too, so `scripts/timing-breaches.py` can tell a gate in which a check ran
//! clean from a gate in which it did not run.
//!
//! This is NOT `HOLON_PBT_INVARIANTS`, which softens a whole invariant by id as
//! a disclosed known-red waiver: a standing load policy must not be spelled
//! like a waiver, and the SQL-count ratchet must keep its teeth at any load.

use std::fmt;
use std::io::Write;
use std::path::PathBuf;

/// Whether a load-perturbed budget breach fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingMode {
    /// Over budget fails.
    Enforce,
    /// Over budget reports and passes. The default.
    Measure,
}

/// The variable [`mode`] reads.
pub const ENV: &str = "HOLON_PBT_TIMING";

/// Overrides the ledger path.
pub const LEDGER_ENV: &str = "HOLON_TIMING_MEASURE_LOG";

/// A gate script exports it (`HOLON_GATE_ID=$GATE`); unset means `local`.
pub const GATE_ENV: &str = "HOLON_GATE_ID";

/// nextest names the running test here, on every thread of the test.
pub const TEST_NAME_ENV: &str = "NEXTEST_TEST_NAME";

/// Thread names that are never a test's.
const NOT_TEST_THREADS: [&str; 2] = ["main", "tokio-runtime-worker"];

/// The ledger when [`LEDGER_ENV`] is unset, relative to `$HOME`.
pub const DEFAULT_LEDGER: &str = ".cache/holon-build/timing-checks.jsonl";

#[derive(Debug, thiserror::Error)]
pub enum TimingError {
    #[error("{ENV}={raw:?} is not a timing mode; it is `measure` (the default) or `enforce`")]
    Mode { raw: String },
    #[error(
        "{GATE_ENV}={raw:?} is not a gate id; it is `local` or `gate<N>` with an optional rerun letter"
    )]
    Gate { raw: String },
    #[error(
        "no test name: {TEST_NAME_ENV} is unset and the thread {thread:?} is not a test's; \
         the caller captures `TimingEnv::from_env()` on the test's thread"
    )]
    NoTest { thread: Option<String> },
    #[error(
        "{what} {raw:?} is refused: it is empty, has leading or trailing whitespace, or a control character"
    )]
    Name { what: &'static str, raw: String },
    #[error("{what} of '{quantity}' is {value}, not a finite number >= 0")]
    Number {
        what: &'static str,
        quantity: String,
        value: f64,
    },
    #[error("'{quantity}': over={over} disagrees with value {value} > budget {budget}")]
    Verdict {
        quantity: String,
        value: f64,
        budget: f64,
        over: bool,
    },
    #[error("{var} is not valid unicode")]
    NotUnicode { var: &'static str },
    #[error("HOME is unset, so the default timing ledger has no location; set {LEDGER_ENV}")]
    NoHome,
    #[error("the 1-min load could not be read: {0}")]
    Load(std::io::Error),
    #[error("the check of '{quantity}' was not appended to the ledger {}: {source}", path.display())]
    Ledger {
        quantity: String,
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Parse a [`ENV`] value. Only the exact spellings `measure` and `enforce`.
pub fn parse(raw: &str) -> Result<TimingMode, TimingError> {
    match raw {
        "enforce" => Ok(TimingMode::Enforce),
        "measure" => Ok(TimingMode::Measure),
        other => Err(TimingError::Mode {
            raw: other.to_string(),
        }),
    }
}

/// [`ENV`] of the process. Unset means [`TimingMode::Measure`].
pub fn mode() -> Result<TimingMode, TimingError> {
    match var(ENV)? {
        None => Ok(TimingMode::Measure),
        Some(raw) => parse(&raw),
    }
}

fn var(name: &'static str) -> Result<Option<String>, TimingError> {
    match std::env::var(name) {
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(TimingError::NotUnicode { var: name }),
    }
}

/// `local`, or `gate<N>` (no leading zero) with an optional lowercase letter
/// for a rerun of gate N. The ledger reader uses the same grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateId(String);

impl GateId {
    pub fn local() -> Self {
        GateId("local".to_string())
    }

    pub fn parse(raw: &str) -> Result<Self, TimingError> {
        let refuse = || TimingError::Gate {
            raw: raw.to_string(),
        };
        if raw == "local" {
            return Ok(Self::local());
        }
        let rest = raw.strip_prefix("gate").ok_or_else(refuse)?;
        let rest = match rest.as_bytes().last() {
            Some(c) if c.is_ascii_lowercase() => &rest[..rest.len() - 1],
            _ => rest,
        };
        let digits_ok = rest.bytes().all(|c| c.is_ascii_digit())
            && rest.as_bytes().first().is_some_and(|&c| c != b'0');
        if !digits_ok {
            return Err(refuse());
        }
        Ok(GateId(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The name of the test a check belongs to, refused like a quantity
/// ([`check_name`]) and never a `main` or runtime worker thread's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestName(String);

impl TestName {
    pub fn parse(raw: &str) -> Result<Self, TimingError> {
        check_name("test name", raw)?;
        Ok(TestName(raw.to_string()))
    }

    /// [`TEST_NAME_ENV`], else the current thread's name: libtest runs each
    /// test on a thread named after it.
    pub fn current() -> Result<Self, TimingError> {
        if let Some(name) = var(TEST_NAME_ENV)? {
            return Self::parse(&name);
        }
        match std::thread::current().name() {
            Some(name) if !NOT_TEST_THREADS.contains(&name) => Self::parse(name),
            other => Err(TimingError::NoTest {
                thread: other.map(str::to_string),
            }),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Refuses a name that would make an unreadable ledger or summary line.
pub fn check_name(what: &'static str, raw: &str) -> Result<(), TimingError> {
    let bad = raw.is_empty() || raw.trim() != raw || raw.chars().any(char::is_control);
    if bad {
        return Err(TimingError::Name {
            what,
            raw: raw.to_string(),
        });
    }
    Ok(())
}

/// What the process environment says about timing: the mode, the ledger, and
/// the gate and test a check belongs to. A check on a thread other than the
/// test's uses a `TimingEnv` captured on the test thread.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingEnv {
    pub mode: TimingMode,
    pub ledger: PathBuf,
    pub gate: GateId,
    pub test: TestName,
}

impl TimingEnv {
    pub fn from_env() -> Result<Self, TimingError> {
        Ok(TimingEnv {
            mode: mode()?,
            ledger: ledger_path()?,
            gate: match var(GATE_ENV)? {
                None => GateId::local(),
                Some(raw) => GateId::parse(&raw)?,
            },
            test: TestName::current()?,
        })
    }
}

fn ledger_path() -> Result<PathBuf, TimingError> {
    if let Some(p) = var(LEDGER_ENV)? {
        return Ok(PathBuf::from(p));
    }
    let home = var("HOME")?.ok_or(TimingError::NoHome)?;
    Ok(PathBuf::from(home).join(DEFAULT_LEDGER))
}

/// The host's 1-minute load average.
pub fn load_1min() -> Result<f64, TimingError> {
    extern "C" {
        fn getloadavg(loadavg: *mut f64, nelem: i32) -> i32;
    }
    let mut load = [0.0f64; 1];
    // SAFETY: `load` holds the one element requested.
    let got = unsafe { getloadavg(load.as_mut_ptr(), 1) };
    if got != 1 {
        return Err(TimingError::Load(std::io::Error::other(format!(
            "getloadavg returned {got} samples, wanted 1"
        ))));
    }
    Ok(load[0])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Ms,
    Bytes,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Ms => "ms",
            Unit::Bytes => "B",
        }
    }
}

/// One check of a load-perturbed quantity against its budget.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub test: String,
    /// A stable key per measured thing: the check plus what it was measured on
    /// (`inv-settle-budget/interaction/SharePage`, `hop_step_cost/p50_sum`).
    pub quantity: String,
    pub value: f64,
    pub budget: f64,
    pub unit: Unit,
    pub load: f64,
    /// Decided by the caller on the exact quantity: two durations an `f64`
    /// millisecond value cannot tell apart may still differ.
    pub over: bool,
}

impl Check {
    fn ledger_line(&self, gate: &GateId) -> String {
        format!(
            r#"{{"gate":"{}","test":"{}","quantity":"{}","value":{},"budget":{},"unit":"{}","load":{},"over":{}}}"#,
            gate.as_str(),
            escape(&self.test),
            escape(&self.quantity),
            self.value,
            self.budget,
            self.unit.as_str(),
            self.load,
            self.over,
        )
    }
}

impl fmt::Display for Check {
    /// `MEASURE-ONLY` is the grep handle of the disclosure line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let u = self.unit.as_str();
        write!(
            f,
            "[MEASURE-ONLY] {} {} {}{u} over the {}{u} budget (load {:.1}) — measured, not judged ({ENV}=measure)",
            self.test, self.quantity, self.value, self.budget, self.load
        )
    }
}

/// Names are refused by [`check_name`], so only these two need escaping.
fn escape(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

/// Append one ledger line for a check of `quantity` and return it; under
/// [`TimingMode::Measure`] an over-budget check also prints its `MEASURE-ONLY`
/// line. The caller decides the verdict from `over` and `env.mode`.
pub fn record(
    env: &TimingEnv,
    quantity: &str,
    value: f64,
    budget: f64,
    unit: Unit,
    over: bool,
) -> Result<Check, TimingError> {
    check_name("quantity", quantity)?;
    // A byte delta shrinks when a transition frees memory; a time never does.
    let value_signed = unit == Unit::Bytes;
    for (what, x, signed) in [("value", value, value_signed), ("budget", budget, false)] {
        if !(x.is_finite() && (signed || x >= 0.0)) {
            return Err(TimingError::Number {
                what,
                quantity: quantity.to_string(),
                value: x,
            });
        }
    }
    if over != (value > budget) {
        return Err(TimingError::Verdict {
            quantity: quantity.to_string(),
            value,
            budget,
            over,
        });
    }
    let check = Check {
        test: env.test.as_str().to_string(),
        quantity: quantity.to_string(),
        value,
        budget,
        unit,
        load: load_1min()?,
        over,
    };
    append(env, &check).map_err(|source| TimingError::Ledger {
        quantity: quantity.to_string(),
        path: env.ledger.clone(),
        source,
    })?;
    if check.over && env.mode == TimingMode::Measure {
        eprintln!("{check}");
    }
    Ok(check)
}

/// One `write` of the whole line to an `O_APPEND` file, so concurrent checks
/// do not interleave.
fn append(env: &TimingEnv, check: &Check) -> std::io::Result<()> {
    if let Some(dir) = env.ledger.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&env.ledger)?;
    f.write_all(format!("{}\n", check.ledger_line(&env.gate)).as_bytes())
}
