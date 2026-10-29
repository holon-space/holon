//! Runtime read of the known-reds registry, so the keystone can KEEP DRAWING
//! past a draw that hits an already-registered failure.
//!
//! Without this, the first draw that trips a registered known red fails the
//! proptest case, proptest spends its shrink budget on that one known bug and
//! the run ends — a `PROPTEST_CASES=16` run that completed three draws while
//! `scripts/keystone-known-reds.sh` still reported PASS-WITH-NOTE. The sample
//! was almost empty and the gate said pass.
//!
//! Here a draw whose panic matches a `known-red` row is **recorded and spent**:
//! the panic stays on stderr (the process panic hook already printed it, so the
//! log and the classifier see it unchanged) and the run draws the next case. A
//! signature matching NO row still fails the run at once, with its shrink.
//!
//! The registry `docs/Testing/KeystoneKnownReds.md` stays the single source of
//! truth: this module parses the same `| key | status | pattern |` columns as
//! the classifier and matches against the same normalized signature.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context as _;
use regex::Regex;

/// The `known-red` rows of `docs/Testing/KeystoneKnownReds.md`, compiled.
pub struct KnownReds {
    rows: Vec<(String, Regex)>,
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
            if cols.len() < 4 {
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
            rows.push((key.to_string(), re));
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
        let first = panic_message.lines().next().unwrap_or_default();
        let mut sig = first.to_string();
        for (re, to) in &self.normalizers {
            sig = re.replace_all(&sig, *to).into_owned();
        }
        self.rows
            .iter()
            .find(|(_, re)| re.is_match(&sig))
            .map(|(key, _)| key.as_str())
    }
}

/// The floor of COMPLETED draws a run must reach to be a verdict at all:
/// **half the configured cases, at least one**.
///
/// Half, because the draws a known red eats are draws no other invariant was
/// checked on: below half, the run has less power than a run at half the case
/// count the operator asked for, so calling it a pass misreports what was
/// tested. At least one, so `PROPTEST_CASES=1` (the per-weave smoke) stays red
/// on a known red exactly as it was before the run learned to keep drawing.
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
        Err(format!(
            "[keystone-sample] FLOOR BREACH: only {} of {cases} configured draws completed \
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
}
