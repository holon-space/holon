//! Runtime read of the known-reds registry, so the keystone can KEEP DRAWING
//! past a draw that hits an already-registered failure.
//!
//! A draw whose panic matches a `known-red` row by wording AND site is
//! **recorded and spent**: the panic stays on stderr (the process panic hook
//! already printed it), the draw does not count toward the configured cases,
//! and the run draws a new case from the same seeded stream, up to
//! `cases +` [`EXTRA_DRAWS`] draws in all. Any other panic still fails the run
//! at once, with its shrink.
//!
//! The registry `docs/Testing/KeystoneKnownReds.md` stays the single source of
//! truth: this module parses the same `| key | status | pattern |` columns as
//! the classifier and matches against the same normalized signature.

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Once;

use anyhow::Context as _;
use proptest::strategy::Strategy;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestError;
use proptest::test_runner::TestRunner;
use regex::Regex;

struct KnownRedRow {
    key: String,
    wording: Regex,
    /// `None` for a row not pinned to a site (`—`), which is not site-checked.
    site: Option<Regex>,
}

/// The `known-red` rows of `docs/Testing/KeystoneKnownReds.md`, compiled.
pub struct KnownReds {
    rows: Vec<KnownRedRow>,
    normalizers: Vec<(Regex, &'static str)>,
    registry: PathBuf,
}

impl KnownReds {
    /// Parse and compile the registry next to this checkout.
    pub fn load() -> anyhow::Result<Self> {
        let registry = super::run_result::repo_root()
            .join("docs")
            .join("Testing")
            .join("KeystoneKnownReds.md");
        let text = std::fs::read_to_string(&registry)
            .with_context(|| format!("read known-reds registry {}", registry.display()))?;
        Self::parse(&text, registry)
    }

    fn parse(text: &str, registry: PathBuf) -> anyhow::Result<Self> {
        let mut rows = Vec::new();
        for line in text.lines() {
            if !line.trim_start().starts_with("| `") {
                continue;
            }
            let cols: Vec<&str> = line.split('|').collect();
            if cols.len() < 5 {
                continue;
            }
            let key = cols[1].trim().trim_matches('`').trim();
            let status = cols[2].trim();
            // A cell cannot hold a literal `|`; the registry writes alternation as
            // `&#124;`.
            let pattern = cols[3]
                .trim()
                .trim_matches('`')
                .trim()
                .replace("&#124;", "|");
            if key.is_empty() || pattern.is_empty() || status != "known-red" {
                continue;
            }
            let re = Regex::new(&pattern).with_context(|| {
                format!(
                    "compile known-red pattern for `{key}` in {}: {pattern}",
                    registry.display()
                )
            })?;
            let site = cols[4]
                .trim()
                .trim_matches('`')
                .trim()
                .replace("&#124;", "|");
            // Read as scripts/check-known-red-evidence.py reads it.
            let site = if site == "—" || site.contains(char::is_whitespace) {
                None
            } else {
                Some(Regex::new(&site).with_context(|| {
                    format!(
                        "compile known-red site for `{key}` in {}: {site}",
                        registry.display()
                    )
                })?)
            };
            rows.push(KnownRedRow {
                key: key.to_string(),
                wording: re,
                site,
            });
        }
        // No rows is the registry's own stated end state (a minimal registry of
        // owned residuals, then none): every panic is then novel and fails its
        // run at once, which is the strictest reading, not a startup failure.
        Ok(Self {
            rows,
            // Mirrors `normalize_sig` in scripts/keystone-known-reds.sh; both
            // sides must keep matching the same volatile payloads, because the
            // patterns are written against the normalized shape.
            normalizers: vec![
                (
                    Regex::new(
                        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
                    )?,
                    "<uuid>",
                ),
                (Regex::new(r"\bId\([0-9]+\)")?, "Id(N)"),
                (Regex::new(r"Mapped: \[[^]]*\]")?, "Mapped: [...]"),
                (Regex::new(r"split-[0-9]+")?, "split-N"),
            ],
            registry,
        })
    }

    pub fn registry(&self) -> &PathBuf {
        &self.registry
    }

    /// The registry key of the row matching this panic message, if any. Only
    /// the first message line is a signature — proptest's own re-report and the
    /// classifier read exactly that line.
    pub fn classify(&self, panic_message: &str) -> Option<&str> {
        self.row_for(panic_message).map(|row| row.key.as_str())
    }

    /// [`Self::classify`], plus the row's `Site` check: a row's wording from a
    /// site the row does not list is a cross-family match, so no known red.
    pub fn classify_at(&self, panic_message: &str, site: Option<&str>) -> Option<&str> {
        let row = self.row_for(panic_message)?;
        match (&row.site, site) {
            (None, _) => Some(&row.key),
            (Some(listed), Some(at)) if listed.is_match(at) => Some(&row.key),
            _ => None,
        }
    }

    fn row_for(&self, panic_message: &str) -> Option<&KnownRedRow> {
        let first = panic_message.lines().next().unwrap_or_default();
        let mut sig = first.to_string();
        for (re, to) in &self.normalizers {
            sig = re.replace_all(&sig, *to).into_owned();
        }
        self.rows.iter().find(|row| row.wording.is_match(&sig))
    }
}

/// One run of a keystone-shaped PBT: the runner (its `Display` is proptest's
/// terminal report), how the run ended, and the sample it drew.
pub struct Sample<V> {
    pub runner: TestRunner,
    pub result: Result<(), TestError<V>>,
    pub tally: SampleTally,
}

/// Draws a run may spend on known reds beyond its configured cases.
pub const EXTRA_DRAWS: u32 = 4;

/// Run `draw` over `strategy` under proptest. A draw whose panic is a
/// registered known red at a listed site is rejected, so proptest draws a new
/// case in its place; the run makes at most `cases +` [`EXTRA_DRAWS`] draws,
/// and its floor check then decides. A novel panic resumes
/// its unwind, so proptest fails and shrinks it as usual.
pub fn draw_sample<S: Strategy>(
    mut config: Config,
    strategy: &S,
    known_reds: &KnownReds,
    draw: impl Fn(S::Value),
) -> Sample<S::Value> {
    record_panic_sites();
    let draw_cap = config.cases + EXTRA_DRAWS;
    // Known reds plus the calls past the draw cap are the rejects; proptest
    // aborts only on the reject after the cap, never before it.
    config.max_global_rejects = draw_cap;
    let mut runner = TestRunner::new(config);
    let tally = RefCell::new(SampleTally::default());
    let result = runner.run(strategy, |value| {
        if tally.borrow().drawn() == draw_cap {
            return Err(TestCaseError::reject("draw cap reached"));
        }
        take_panic_site();
        let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| draw(value))) else {
            tally.borrow_mut().note_completed();
            return Ok(());
        };
        let site = take_panic_site();
        match known_reds.classify_at(&panic_message(&*payload), site.as_deref()) {
            Some(key) => {
                tally.borrow_mut().note_known_red(key);
                Err(TestCaseError::reject(format!("known red {key}")))
            }
            None => {
                tally.borrow_mut().note_novel();
                std::panic::resume_unwind(payload)
            }
        }
    });
    let tally = tally.into_inner();
    let result = match result {
        Err(TestError::Abort(_)) if tally.drawn() == draw_cap => Ok(()),
        other => other,
    };
    Sample {
        runner,
        result,
        tally,
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default()
}

thread_local! {
    static PANIC_SITE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record each panic's `<file>:<line>:<col>` on its own thread, the form a
/// registry `Site` is written against, then run the previous hook.
fn record_panic_sites() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let site = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
            // A thread tearing down its locals has no draw left to classify.
            let _ = PANIC_SITE.try_with(|s| *s.borrow_mut() = site);
            prev(info);
        }));
    });
}

/// The site of this thread's last panic since the previous take.
fn take_panic_site() -> Option<String> {
    PANIC_SITE.with(|s| s.borrow_mut().take())
}

/// The floor of COMPLETED draws a run must reach to be a verdict at all:
/// **half the configured cases, at least one**.
///
/// Half, because the draws a known red eats are draws no other invariant was
/// checked on: below half, the run has less power than a run at half the case
/// count the operator asked for, so calling it a pass misreports what was
/// tested. At least one, so a `PROPTEST_CASES=1` run (the per-weave smoke)
/// whose every draw up to the draw cap ended on a known red is red.
pub fn completed_floor(cases: u32) -> u32 {
    cases.div_ceil(2).max(1)
}

/// Draws attempted vs completed, and the known reds that ate the difference.
#[derive(Default)]
pub struct SampleTally {
    completed: u32,
    hits: BTreeMap<String, u32>,
    ended_on_novel: bool,
}

impl SampleTally {
    pub fn note_completed(&mut self) {
        if self.ended_on_novel {
            return;
        }
        self.completed += 1;
    }

    pub fn note_known_red(&mut self, key: &str) {
        if self.ended_on_novel {
            return;
        }
        *self.hits.entry(key.to_string()).or_default() += 1;
    }

    /// A draw ended on an UNREGISTERED signature: it was attempted, it did not
    /// complete, and it is the last draw of the sample — proptest now shrinks,
    /// re-entering the same closure with replays that are not draws.
    pub fn note_novel(&mut self) {
        self.ended_on_novel = true;
    }

    pub fn completed(&self) -> u32 {
        self.completed
    }

    pub fn hits(&self) -> u32 {
        self.hits.values().sum()
    }

    /// Draws that completed or ended on a known red: the ones the draw cap
    /// counts.
    fn drawn(&self) -> u32 {
        self.completed + self.hits()
    }

    /// The gate summary: ONE machine-readable line the classifier reads, plus
    /// one line per signature. `verdict=FLOOR-BREACH` means the sample is too
    /// small to be a verdict.
    pub fn disclose(&self, cases: u32) -> String {
        let floor = completed_floor(cases);
        let verdict = if self.completed >= floor {
            "OK"
        } else {
            "FLOOR-BREACH"
        };
        let mut out = format!(
            "[keystone-sample] cases={cases} attempted={} completed={} floor={floor} \
             known-red-hits={} verdict={verdict}\n",
            self.completed + self.hits() + u32::from(self.ended_on_novel),
            self.completed,
            self.hits(),
        );
        for (key, count) in &self.hits {
            out.push_str(&format!(
                "[keystone-sample] known-red [{key}] x{count} — draw(s) spent, run continued\n"
            ));
        }
        out
    }

    /// `Err` with the gate's reason when the run is below the floor.
    pub fn check_floor(&self, cases: u32) -> Result<(), String> {
        let floor = completed_floor(cases);
        if self.completed >= floor {
            return Ok(());
        }
        let what = if self.completed == 0 {
            "only known reds drawn"
        } else {
            "too few draws completed"
        };
        Err(format!(
            "[keystone-sample] FLOOR BREACH: {what}: {} of {cases} configured draws completed \
             (floor {floor}); {} draw(s) ended on a registered known red {:?}. A sample this \
             small is not a pass — fix or re-register the known red, or lower PROPTEST_CASES \
             deliberately.",
            self.completed,
            self.hits(),
            self.hits,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_registry_compiles_and_classifies_the_syn_real_mint_panic() {
        let reds = KnownReds::load().expect("load registry");
        let panic = "assertion `left == right` failed: per-tick reconcile: one synthetic per \
                     minted real id (syn=[], real=[EntityUri(\"block:259a9a58-18bb-4559-a589-\
                     87aee571c59e\")], HeldCreates { parked: 0 })\n  left: 0\n right: 1";
        assert_eq!(reds.classify(panic), Some("syn-real-mint"));
        assert_eq!(
            reds.classify("some brand new assertion nobody registered"),
            None
        );
    }

    #[test]
    fn a_draw_that_ended_on_a_novel_panic_is_still_an_attempted_draw() {
        let mut tally = SampleTally::default();
        tally.note_completed();
        tally.note_novel();
        assert!(tally.disclose(4).contains("attempted=2 completed=1"));
        // The novel panic ends the run; proptest's shrink replays re-enter the
        // same closure, and a replay is not another draw of the sample.
        tally.note_completed();
        tally.note_known_red("syn-real-mint");
        tally.note_novel();
        let line = tally.disclose(4);
        assert!(line.contains("attempted=2 completed=1"), "{line}");
        assert!(line.contains("known-red-hits=0"), "{line}");
    }

    #[test]
    fn an_empty_registry_classifies_every_panic_as_novel() {
        let reds = KnownReds::parse("no rows here\n", PathBuf::from("empty.md"))
            .expect("an empty registry is the registry's own end state, not a startup failure");
        assert_eq!(reds.classify("any panic at all"), None);
    }

    #[test]
    fn floor_is_half_the_cases_and_never_zero() {
        assert_eq!(completed_floor(1), 1);
        assert_eq!(completed_floor(2), 1);
        assert_eq!(completed_floor(16), 8);
        assert_eq!(completed_floor(64), 32);
    }

    #[test]
    fn a_run_under_the_floor_is_not_a_pass() {
        let mut tally = SampleTally::default();
        for _ in 0..3 {
            tally.note_completed();
        }
        tally.note_known_red("syn-real-mint");
        assert!(tally.check_floor(16).is_err());
        assert!(tally.disclose(16).contains("verdict=FLOOR-BREACH"));
        assert!(
            tally
                .disclose(16)
                .contains("attempted=4 completed=3 floor=8")
        );

        for _ in 0..5 {
            tally.note_completed();
        }
        assert!(tally.check_floor(16).is_ok());
        assert!(tally.disclose(16).contains("verdict=OK"));
    }

    /// A keystone divergence panics at the harness's one `#[track_caller]`
    /// call, so a row pinned to any other harness line reads as novel in-run.
    #[test]
    fn every_row_pinned_to_the_harness_accepts_the_divergence_site() {
        let reds = KnownReds::load().expect("load registry");
        let site = crate::pbt::composed::harness::divergence_site();
        let stale: Vec<(&str, &str)> = reds
            .rows
            .iter()
            .filter_map(|row| Some((row.key.as_str(), row.site.as_ref()?)))
            .filter(|(_, listed)| listed.as_str().contains("composed/harness"))
            .filter(|(_, listed)| !listed.is_match(&site))
            .map(|(key, listed)| (key, listed.as_str()))
            .collect();
        assert_eq!(
            stale,
            [],
            "rows whose Site rejects the divergence site {site}"
        );
    }

    #[test]
    fn a_duplicate_block_id_verdict_beside_another_invariant_stays_novel() {
        let reds = KnownReds::load().expect("load registry");
        let problem = r#""[inv-no-observed-errors] 1 swallowed problem(s) (ERROR log / panic) during this transition — a green run is hiding a real failure:\n  [ERROR] holon_filesystem::file_sync_controller (crates/holon-filesystem/src/file_sync_controller.rs:3025): [FileSyncController] DUPLICATE BLOCK ID: 'block:kd-1-a' is declared by BOTH /v/Journals/2026-01-16.org and /v/forward-edge-page.org, so /v/forward-edge-page.org is NOT ingested""#;
        let head = "reconciled composed sequence diverged from the oracle: [";
        let alone = format!("{head}(\"inv-no-observed-errors\", {problem})]");
        assert_eq!(reds.classify(&alone), Some("cut-paste-duplicate-block-id"));
        let cofired = format!(
            "{head}(\"inv-copies-stay-on-disk\", \"kd-1 has 0 copies on disk where the model expects 1\"), (\"inv-no-observed-errors\", {problem})]"
        );
        assert_eq!(reds.classify(&cofired), None, "{cofired}");
    }

    mod draw_sample_pbt {
        use std::cell::Cell;
        use std::cell::RefCell;
        use std::collections::BTreeMap;

        use proptest::prelude::*;
        use proptest::test_runner::Config;
        use proptest::test_runner::RngSeed;
        use proptest::test_runner::TestError;
        use proptest::test_runner::TestRunner;

        use super::super::*;
        use crate::pbt::composed::lamport_delta::assert_delta;

        /// Extra draws the redraw ruling grants per run, beyond the configured
        /// cases.
        const EXTRA_DRAWS: u32 = 4;
        const INDENT_ROW: usize = 5;
        const ROW_KEYS: [&str; 6] = [
            "toy-exact-site",
            "toy-line-site",
            "toy-alternation-site",
            "toy-unanchored-site",
            "toy-unpinned",
            "indent-writes-no-loro-op",
        ];

        #[derive(Clone, Copy, Debug)]
        enum Outcome {
            Complete,
            /// The row's wording, raised from the site its `Site` lists.
            KnownRed(usize),
            /// The row's wording, raised from a site its `Site` does not list.
            ElsewhereRed(usize),
            Novel,
            EmptyPayload,
        }

        fn raise_at_home(message: String) -> ! {
            std::panic::panic_any(message)
        }

        fn raise_elsewhere(message: String) -> ! {
            std::panic::panic_any(message)
        }

        fn site_of(raise: fn(String) -> !) -> String {
            record_panic_sites();
            take_panic_site();
            std::panic::catch_unwind(|| raise("measuring the raise site".to_string()))
                .expect_err("a raise panics");
            take_panic_site().expect("the panic hook records the raise site")
        }

        fn indent_message() -> String {
            "assertion `left == right` failed: [lamport-delta] 'Indent' advanced the SUT \
             primary's Lamport clock by 0 (7 -> 7), the model predicts exactly 1: the SUT \
             wrote fewer Loro op(s) the model does not expect\n  left: 0\n right: 1"
                .to_string()
        }

        fn message_of(row: usize) -> String {
            if row == INDENT_ROW {
                indent_message()
            } else {
                format!("toy known red {row}: a registered failure")
            }
        }

        /// One toy row per `Site` form the registry uses, plus the checked-in
        /// Indent row, whose site is the real `assert_delta`.
        fn registry() -> KnownReds {
            let home = site_of(raise_at_home);
            let (file, rest) = home.split_once(':').expect("<file>:<line>:<col>");
            let line = rest.split_once(':').expect("<line>:<col>").0;
            let (file, home) = (regex::escape(file), regex::escape(&home));
            let sites = [
                format!("^{home}$"),
                format!("^{file}:{line}:[0-9]+$"),
                format!("^(crates/elsewhere\\.rs:1:1&#124;{home})$"),
                format!("{file}:{line}:"),
                "—".to_string(),
            ];
            let mut text = String::new();
            for (row, site) in sites.iter().enumerate() {
                text.push_str(&format!(
                    "| `{}` | known-red | `^toy known red {row}: ` | `{site}` | s | e | t | o | — |\n",
                    ROW_KEYS[row]
                ));
            }
            let checked_in = std::fs::read_to_string(KnownReds::load().unwrap().registry())
                .expect("read the checked-in registry");
            let indent = checked_in
                .lines()
                .find(|l| l.starts_with("| `indent-writes-no-loro-op` |"))
                .expect("the registry holds the indent-writes-no-loro-op row");
            text.push_str(indent);
            text.push('\n');
            KnownReds::parse(&text, PathBuf::from("toy.md")).expect("toy registry parses")
        }

        fn raise(outcome: Outcome, draw: u32) {
            match outcome {
                Outcome::Complete => {}
                Outcome::KnownRed(INDENT_ROW) => {
                    assert_delta("Indent", Some(1 + draw % 2), Some(7), Some(7))
                }
                Outcome::KnownRed(row) => raise_at_home(message_of(row)),
                Outcome::ElsewhereRed(row) => raise_elsewhere(message_of(row)),
                Outcome::Novel => raise_elsewhere("an unregistered signature".to_string()),
                Outcome::EmptyPayload => raise_at_home(String::new()),
            }
        }

        enum End {
            Green,
            FloorBreach,
            Novel(String),
        }

        struct Expected {
            attempted: u32,
            completed: u32,
            hits: BTreeMap<&'static str, u32>,
            end: End,
        }

        /// The ruling, walked over the script: a known red is a spent draw, a
        /// run ends at `cases + 4` draws, a novel panic ends it at once.
        fn reference(cases: u32, script: &[Outcome]) -> Expected {
            let (mut attempted, mut completed) = (0, 0);
            let mut hits: BTreeMap<&'static str, u32> = BTreeMap::new();
            while completed < cases {
                let outcome = script
                    .get(attempted as usize)
                    .copied()
                    .unwrap_or(Outcome::Complete);
                attempted += 1;
                let spent = match outcome {
                    Outcome::Complete => {
                        completed += 1;
                        None
                    }
                    Outcome::KnownRed(row) | Outcome::ElsewhereRed(row @ 4) => Some(row),
                    Outcome::ElsewhereRed(row) => {
                        return novel(attempted, completed, hits, message_of(row));
                    }
                    Outcome::Novel => {
                        let message = "an unregistered signature".to_string();
                        return novel(attempted, completed, hits, message);
                    }
                    Outcome::EmptyPayload => {
                        return novel(attempted, completed, hits, String::new());
                    }
                };
                if let Some(row) = spent {
                    *hits.entry(ROW_KEYS[row]).or_default() += 1;
                }
                if attempted == cases + EXTRA_DRAWS {
                    break;
                }
            }
            let floor = cases.div_ceil(2).max(1);
            let end = if completed >= floor {
                End::Green
            } else {
                End::FloorBreach
            };
            Expected {
                attempted,
                completed,
                hits,
                end,
            }
        }

        fn novel(
            attempted: u32,
            completed: u32,
            hits: BTreeMap<&'static str, u32>,
            message: String,
        ) -> Expected {
            Expected {
                attempted,
                completed,
                hits,
                end: End::Novel(message),
            }
        }

        fn run(reds: &KnownReds, config: Config, script: &[Outcome]) -> (Sample<u64>, Vec<u64>) {
            let calls = Cell::new(0u32);
            let drawn = RefCell::new(Vec::new());
            let novel_at = Cell::new(None::<Outcome>);
            let sample = draw_sample(config, &any::<u64>(), reds, |value| {
                let call = calls.get();
                calls.set(call + 1);
                drawn.borrow_mut().push(value);
                // Shrink replays after a novel draw fail the same way.
                if let Some(outcome) = novel_at.get() {
                    raise(outcome, call);
                }
                let outcome = script
                    .get(call as usize)
                    .copied()
                    .unwrap_or(Outcome::Complete);
                if matches!(
                    outcome,
                    Outcome::Novel | Outcome::EmptyPayload | Outcome::ElsewhereRed(0..=3 | 5)
                ) {
                    novel_at.set(Some(outcome));
                }
                raise(outcome, call);
            });
            (sample, drawn.into_inner())
        }

        fn check(
            reds: &KnownReds,
            cases: u32,
            seed: u64,
            max_global_rejects: u32,
            script: &[Outcome],
        ) {
            let config = Config {
                cases,
                max_global_rejects,
                max_shrink_iters: 4,
                failure_persistence: None,
                rng_seed: RngSeed::Fixed(seed),
                ..Config::default()
            };
            let (sample, drawn) = run(reds, config.clone(), script);
            let (_, replayed) = run(reds, config, script);
            let want = reference(cases, script);

            let floor = cases.div_ceil(2).max(1);
            let verdict = if want.completed >= floor {
                "OK"
            } else {
                "FLOOR-BREACH"
            };
            let mut disclosure = format!(
                "[keystone-sample] cases={cases} attempted={} completed={} floor={floor} \
                 known-red-hits={} verdict={verdict}\n",
                want.attempted,
                want.completed,
                want.hits.values().sum::<u32>(),
            );
            for (key, count) in &want.hits {
                disclosure.push_str(&format!(
                    "[keystone-sample] known-red [{key}] x{count} — draw(s) spent, run continued\n"
                ));
            }
            assert_eq!(
                sample.tally.disclose(cases),
                disclosure,
                "script {script:?}"
            );

            let draws = &drawn[..want.attempted as usize];
            assert_eq!(
                draws,
                &replayed[..draws.len()],
                "one seed replays every draw"
            );
            let distinct: std::collections::BTreeSet<_> = draws.iter().collect();
            assert_eq!(
                distinct.len(),
                draws.len(),
                "a redraw is a new value: {draws:?}"
            );

            match (want.end, sample.result) {
                (End::Novel(message), Err(TestError::Fail(reason, _))) => {
                    assert_eq!(reason.message(), message, "script {script:?}")
                }
                (End::Novel(message), other) => {
                    panic!("expected the novel panic {message:?}, got {other:?}; script {script:?}")
                }
                (end, Ok(())) => {
                    let floor = sample.tally.check_floor(cases);
                    match (end, floor) {
                        (End::Green, Ok(())) => {}
                        (End::FloorBreach, Err(breach)) => {
                            assert!(
                                breach.starts_with("[keystone-sample] FLOOR BREACH:"),
                                "{breach}"
                            );
                            assert_eq!(
                                breach.contains("only known reds drawn"),
                                want.completed == 0,
                                "{breach}"
                            );
                            for (key, count) in &want.hits {
                                assert!(breach.contains(&format!("{key:?}: {count}")), "{breach}");
                            }
                        }
                        (_, floor) => panic!("floor check {floor:?} disagrees; script {script:?}"),
                    }
                }
                (_, Err(e)) => panic!("unexpected terminal error {e}; script {script:?}"),
            }
        }

        fn outcome() -> impl Strategy<Value = Outcome> {
            prop_oneof![
                3 => Just(Outcome::Complete),
                3 => (0..ROW_KEYS.len()).prop_map(Outcome::KnownRed),
                1 => (0..ROW_KEYS.len()).prop_map(Outcome::ElsewhereRed),
                1 => Just(Outcome::Novel),
                1 => Just(Outcome::EmptyPayload),
            ]
        }

        #[test]
        fn a_run_redraws_past_known_reds_up_to_four_extra_draws() {
            use Outcome::*;
            let reds = registry();
            let kr = KnownRed(0);
            let static_examples: Vec<(u32, Vec<Outcome>)> = vec![
                (1, vec![kr, Complete]),
                (1, vec![kr; 5]),
                (1, vec![kr, Novel]),
                (1, vec![Novel]),
                (
                    16,
                    [vec![Complete; 7], vec![kr], vec![Complete; 9]].concat(),
                ),
                (1, vec![KnownRed(INDENT_ROW), Complete]),
                (1, vec![ElsewhereRed(INDENT_ROW)]),
                (
                    1,
                    vec![
                        KnownRed(1),
                        KnownRed(2),
                        KnownRed(3),
                        KnownRed(4),
                        ElsewhereRed(4),
                    ],
                ),
                (1, vec![EmptyPayload]),
            ];
            for (cases, script) in &static_examples {
                check(&reds, *cases, 7, 1024, script);
            }
            let mut runner = TestRunner::new(Config {
                cases: 128,
                failure_persistence: None,
                ..Config::default()
            });
            let input = (
                1..=16u32,
                any::<u64>(),
                0..2048u32,
                prop::collection::vec(outcome(), 0..=24),
            );
            runner
                .run(&input, |(cases, seed, rejects, script)| {
                    check(&reds, cases, seed, rejects, &script);
                    Ok(())
                })
                .unwrap();
        }

        /// Today a known red spends one of the configured cases. The redraw
        /// may only add draws, so a run that is green when every known red
        /// spends its case is green with the redraw too.
        #[test]
        fn a_redraw_never_fails_a_run_that_spending_the_case_passes() {
            use Outcome::*;
            let reds = registry();
            let kr = KnownRed(0);
            let property = |cases: u32, script: &[Outcome]| {
                let floor = cases.div_ceil(2).max(1);
                let completed_when_spent = (0..cases as usize)
                    .filter(|&i| matches!(script.get(i).copied().unwrap_or(Complete), Complete))
                    .count() as u32;
                let config = Config {
                    cases,
                    failure_persistence: None,
                    rng_seed: RngSeed::Fixed(7),
                    ..Config::default()
                };
                let (sample, _) = run(&reds, config, script);
                assert!(sample.result.is_ok(), "script {script:?}");
                if completed_when_spent >= floor {
                    assert_eq!(
                        sample.tally.check_floor(cases),
                        Ok(()),
                        "cases={cases}: green when each known red spends a case, red with \
                         the redraw; script {script:?}"
                    );
                }
            };
            property(16, &[kr; 5]);
            let mut runner = TestRunner::new(Config {
                cases: 128,
                failure_persistence: None,
                ..Config::default()
            });
            let input = (
                1..=16u32,
                prop::collection::vec(prop_oneof![Just(Complete), Just(kr)], 0..=24),
            );
            runner
                .run(&input, |(cases, script)| {
                    property(cases, &script);
                    Ok(())
                })
                .unwrap();
        }
    }
}
