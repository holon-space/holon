//! Machine-readable run-result emission for the observability report.
//!
//! Every keystone / GPUI PBT run writes a `<run_id>.result.json` into
//! `HOLON_RESULT_DIR` (default `<repo>/.observability/runs/`) as a side effect
//! of the run itself — green **and** red — with no separate orchestrated run.
//! The JSON is the harness-native format (the `pbt` feature already carries
//! `serde` + `serde_json`, but not `serde_yaml`); `scripts/holon_obs.py`
//! converts it to canonical YAML at upload time.
//!
//! # Semantics
//!
//! - A [`RunResultGuard`] is created at the top of a test body. Its verdict is
//!   **red by default** — only an explicit [`RunResultGuard::set_green`] flips
//!   it — so a test that panics before finishing is recorded red, never
//!   silently green.
//! - Panics are captured by a process-wide panic hook (chained to the previous
//!   hook, so stderr output is unchanged) into a **thread-local** buffer. The
//!   guard drains that buffer on finalize, so under libtest's parallel test
//!   threads each test records only its own panics.
//! - The file is written in `Drop`, so it is emitted on the success path and on
//!   the unwind path alike.
//!
//! # Opt-outs
//!
//! - `HOLON_RESULT_DISABLE=1` — emit nothing (the guard becomes a no-op).
//! - `HOLON_RESULT_DIR=/abs/path` — redirect the output directory.
//! - `HOLON_RESULT_GIT_REV=<rev>` — stamp the record with a git rev; when unset
//!   the field is empty and `holon_obs.py` fills it from `git rev-parse HEAD`.

use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use serde::Serialize;

/// Bump when the shape of [`RunResult`] changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// One panic captured on this thread, in the shape the known-reds classifier
/// normalizes: `file:line` + first message line.
#[derive(Serialize, Clone, Debug)]
pub struct PanicRecord {
    pub message: String,
    pub location: String,
}

/// Green or red. Red is the default; see the module docs.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Green,
    Red,
}

/// The emitted record. Enriched (latency, known-red classification, bug-funnel
/// counters) by `scripts/holon_obs.py` at upload time; this is only what the
/// harness knows authoritatively.
#[derive(Serialize, Clone, Debug)]
pub struct RunResult {
    pub schema: u32,
    pub run_id: String,
    pub kind: String,
    pub git_rev: String,
    pub started_at_ms: u128,
    pub duration_ms: u128,
    pub cases: u32,
    pub verdict: Verdict,
    pub panics: Vec<PanicRecord>,
}

thread_local! {
    static PANICS: RefCell<Vec<PanicRecord>> = const { RefCell::new(Vec::new()) };
}

fn payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Install the panic-recording hook exactly once per process. The hook records
/// into the panicking thread's `PANICS` buffer and then delegates to the
/// previous hook (so default stderr output is preserved).
fn install_panic_hook_once() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = payload_to_string(info.payload());
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_default();
            PANICS.with(|p| p.borrow_mut().push(PanicRecord { message, location }));
            prev(info);
        }));
    });
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Sortable, collision-free run id: `<started_ms>-<kind>-<pid>-<seq>`.
fn make_run_id(kind: &str, started_ms: u128) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{started_ms}-{kind}-{}-{seq}", std::process::id())
}

fn disabled() -> bool {
    std::env::var("HOLON_RESULT_DISABLE")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
}

/// Resolve the output directory: `HOLON_RESULT_DIR` if set, else walk up from
/// `CARGO_MANIFEST_DIR` to the repo root (the ancestor containing `.git`) and
/// use `<repo>/.observability/runs/`. Falls back to two levels up when no
/// `.git` is present (e.g. a copied tree).
fn default_dir() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cur = manifest.parent();
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return dir.join(".observability").join("runs");
        }
        cur = dir.parent();
    }
    manifest
        .join("..")
        .join("..")
        .join(".observability")
        .join("runs")
}

/// RAII guard: writes `<dir>/<run_id>.result.json` on drop (success or unwind).
pub struct RunResultGuard {
    result: RunResult,
    dir: PathBuf,
    disabled: bool,
    finalized: bool,
}

impl RunResultGuard {
    /// Create a guard with default env-driven configuration.
    pub fn new(kind: &str, cases: u32) -> Self {
        let dir = if disabled() {
            PathBuf::new()
        } else {
            std::env::var("HOLON_RESULT_DIR")
                .ok()
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(default_dir)
        };
        Self::with_dir(kind, cases, dir, disabled())
    }

    /// Create a guard writing to an explicit directory (always enabled). Used
    /// by tests and by harnesses that resolve their own output location.
    pub fn with_dir(kind: &str, cases: u32, dir: PathBuf, disabled: bool) -> Self {
        if !disabled {
            install_panic_hook_once();
        }
        let started_at_ms = now_ms();
        let result = RunResult {
            schema: SCHEMA_VERSION,
            run_id: make_run_id(kind, started_at_ms),
            kind: kind.to_string(),
            git_rev: std::env::var("HOLON_RESULT_GIT_REV").unwrap_or_default(),
            started_at_ms,
            duration_ms: 0,
            cases,
            verdict: Verdict::Red,
            panics: Vec::new(),
        };
        Self {
            result,
            dir,
            disabled,
            finalized: false,
        }
    }

    pub fn set_green(&mut self) {
        self.result.verdict = Verdict::Green;
    }

    pub fn set_red(&mut self) {
        self.result.verdict = Verdict::Red;
    }

    pub fn run_id(&self) -> &str {
        &self.result.run_id
    }

    /// Drain this thread's captured panics into the record.
    fn note_panics(&mut self) {
        PANICS.with(|p| {
            let mut buf = p.borrow_mut();
            self.result.panics.append(&mut buf);
        });
    }

    fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        self.note_panics();
        self.result.duration_ms = now_ms().saturating_sub(self.result.started_at_ms);
        if self.disabled {
            return;
        }
        let _ = std::fs::create_dir_all(&self.dir);
        let path = self.dir.join(format!("{}.result.json", self.result.run_id));
        if let Ok(json) = serde_json::to_string_pretty(&self.result) {
            let _ = std::fs::write(path, json);
        }
    }
}

impl Drop for RunResultGuard {
    fn drop(&mut self) {
        self.finalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn green_guard_writes_a_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut guard = RunResultGuard::with_dir("unit", 1, dir.path().to_path_buf(), false);
        guard.set_green();
        let run_id = guard.run_id().to_string();
        drop(guard);

        let path = dir.path().join(format!("{run_id}.result.json"));
        let json = std::fs::read_to_string(&path).expect("result file exists");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["schema"], 1);
        assert_eq!(parsed["kind"], "unit");
        assert_eq!(parsed["cases"], 1);
        assert_eq!(parsed["verdict"], "green");
        assert_eq!(parsed["panics"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn default_verdict_is_red() {
        let dir = tempfile::tempdir().expect("tempdir");
        let guard = RunResultGuard::with_dir("unit", 1, dir.path().to_path_buf(), false);
        let run_id = guard.run_id().to_string();
        drop(guard);

        let path = dir.path().join(format!("{run_id}.result.json"));
        let json = std::fs::read_to_string(&path).expect("result file exists");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert_eq!(parsed["verdict"], "red");
    }
}
