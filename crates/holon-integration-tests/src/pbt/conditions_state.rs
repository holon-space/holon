//! What the reference model expects the app to be DISCLOSING.
//!
//! Every degradation Holon raises is a sticky condition on the
//! [`ConditionBus`](holon_api::ConditionBus), keyed by `(subject, kind)`. Until
//! the model tracked them, a transition that fails on purpose proved only that
//! the write was refused — never that the user was TOLD. Those are different
//! defects, and the silent one is the failure the bus exists to prevent: from
//! inside the store it looks exactly like a write that succeeded.
//!
//! The model records the identity, never the prose. The kind is the stable
//! `&'static str` every all-clear site already names; asserting the sentence
//! would make the model a hand-copy of `ConditionKind::detail` and stop being
//! an independent oracle.
//!
//! **Subjects are matched by FILE NAME, anchored to a path component.** Most
//! raise sites carry an absolute path inside the run's temp vault, which no
//! constant can know. The model pins the part it owns — the file name — and the
//! comparison is identity at the final path component, never a raw string
//! suffix: see [`holon_pbt_core::capabilities::subject_matches_expected`] for
//! why (a different file whose name merely ends with the pinned one must not
//! satisfy the expectation).
//!
//! **The model claims authority over KINDS, not over the whole bus.** A booted
//! app legitimately discloses things no transition caused (secrets held in
//! memory, undo history cleared at boot). Comparing the whole set would fail on
//! those, so the invariant compares exactly within the kinds this state has
//! spoken about, and ignores the rest. The exclusion is deliberate and visible
//! here rather than buried in the invariant body.

use std::collections::BTreeSet;

use holon_frontend::crash_history::Kept;

/// One condition the model expects to be in effect: the file NAME that owns its
/// subject, and its stable kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExpectedCondition {
    /// The file name the raise site's subject must END IN — as its final path
    /// component, so a bare file name where the subject is a path, and the
    /// whole subject where it already is a name. Never a raw string suffix: see
    /// [`holon_pbt_core::capabilities::subject_matches_expected`].
    pub subject_name: String,
    pub kind: &'static str,
    /// The file names the condition names, in its order, when the model
    /// states them.
    pub files: Option<Vec<String>>,
    /// How many files the condition counts, when the model states it.
    pub count: Option<usize>,
    /// The message the condition carries, when the model states it.
    pub message: Option<String>,
}

impl ExpectedCondition {
    pub fn matches(&self, subject: &str) -> bool {
        holon_pbt_core::capabilities::subject_matches_expected(subject, &self.subject_name)
    }
}

/// The conditions the reference model believes are raised right now, plus the
/// kinds it claims authority over.
#[derive(Debug, Default, Clone)]
pub struct ConditionsRefState {
    expected: BTreeSet<ExpectedCondition>,
    /// Every kind this state has ever raised or cleared. The invariant judges
    /// only these; a kind never named here is the app's own business.
    governed: BTreeSet<&'static str>,
    /// Every crash record on disk, oldest first.
    crash_records: Vec<(Kept, Panicked)>,
}

/// A panic the model knows of, by its location and message.
#[derive(Debug, Clone)]
pub struct Panicked {
    pub location: String,
    pub message: String,
}

impl Panicked {
    /// The subject a panic condition carries is its location, so the model
    /// pins its final path component: the file name with `:line:column`.
    fn subject_name(&self) -> String {
        std::path::Path::new(&self.location)
            .file_name()
            .unwrap_or_else(|| panic!("a panic location names no file: {}", self.location))
            .to_string_lossy()
            .into_owned()
    }
}

impl ConditionsRefState {
    /// A transition that failed on purpose expects its condition to be in
    /// effect. Idempotent, matching the bus, where a re-raise upserts.
    ///
    /// `subject_name` is the FILE NAME the raise site's subject will carry as
    /// its final path component — not a string suffix.
    pub fn raise(&mut self, subject_name: impl Into<String>, kind: &'static str) {
        self.raise_expecting(subject_name.into(), kind, None, None);
    }

    /// [`raise`](Self::raise) for a condition that names files: `files` are
    /// the file names it must name, in its order.
    pub fn raise_naming_files(
        &mut self,
        subject_name: impl Into<String>,
        kind: &'static str,
        files: Vec<String>,
    ) {
        self.raise_expecting(subject_name.into(), kind, Some(files), None);
    }

    /// [`raise_naming_files`](Self::raise_naming_files) for a condition that
    /// also counts the files it stands for, beyond the ones it names.
    pub fn raise_counting_files(
        &mut self,
        subject_name: impl Into<String>,
        kind: &'static str,
        count: usize,
        files: Vec<String>,
    ) {
        self.raise_expecting(subject_name.into(), kind, Some(files), Some(count));
    }

    fn raise_expecting(
        &mut self,
        subject_name: String,
        kind: &'static str,
        files: Option<Vec<String>>,
        count: Option<usize>,
    ) {
        self.governed.insert(kind);
        self.expected
            .retain(|c| !(c.kind == kind && c.subject_name == subject_name));
        self.expected.insert(ExpectedCondition {
            subject_name,
            kind,
            files,
            count,
            message: None,
        });
    }

    /// The previous run left a panic record: it is kept unshown and disclosed
    /// at start.
    pub fn prior_crash(&mut self, panic: &Panicked) {
        self.crash_records.push((Kept::Unshown, panic.clone()));
        self.previous_run_panicked(panic);
    }

    /// The previous run's panic record is disclosed at start.
    pub fn previous_run_panicked(&mut self, panic: &Panicked) {
        self.raise_panic(holon_api::ConditionKind::PREVIOUS_RUN_PANICKED, panic);
    }

    /// A task panicked: it is disclosed now, and its record is disclosed at
    /// the next start.
    pub fn task_panicked(&mut self, panic: Panicked) {
        self.raise_panic(holon_api::ConditionKind::TASK_PANICKED, &panic);
        self.crash_records.push((Kept::ThisRun, panic));
    }

    /// A restart ends this process's panic conditions; this run's record joins
    /// the unshown ones, and the start discloses the newest unshown record.
    /// A record stays unshown until a drawn frame shows it, which a headless
    /// run never does, so every start discloses it again.
    pub fn restart(&mut self) {
        self.clear_kind(holon_api::ConditionKind::TASK_PANICKED);
        self.clear_kind(holon_api::ConditionKind::PREVIOUS_RUN_PANICKED);
        for (kept, _) in &mut self.crash_records {
            if *kept == Kept::ThisRun {
                *kept = Kept::Unshown;
            }
        }
        let newest_unshown = self
            .crash_records
            .iter()
            .rev()
            .find(|(kept, _)| *kept == Kept::Unshown)
            .map(|(_, panic)| panic.clone());
        if let Some(panic) = newest_unshown {
            self.previous_run_panicked(&panic);
        }
    }

    fn raise_panic(&mut self, kind: &'static str, panic: &Panicked) {
        let subject_name = panic.subject_name();
        self.governed.insert(kind);
        self.expected
            .retain(|c| !(c.kind == kind && c.subject_name == subject_name));
        self.expected.insert(ExpectedCondition {
            subject_name,
            kind,
            files: None,
            count: None,
            message: Some(panic.message.clone()),
        });
    }

    /// The named all-clear happened, so the condition must be gone.
    ///
    /// Governing the kind survives the clear: "this kind must now be absent" is
    /// exactly as much of a claim as "it must be present", and dropping it
    /// would make a failure to clear invisible.
    pub fn clear(&mut self, subject_name: &str, kind: &'static str) {
        self.governed.insert(kind);
        self.expected
            .retain(|c| !(c.kind == kind && c.subject_name == subject_name));
    }

    /// Every condition of `kind`, cleared — for an all-clear that ends a whole
    /// class at once (a clean re-scan, a reconnect of everything).
    pub fn clear_kind(&mut self, kind: &'static str) {
        self.governed.insert(kind);
        self.expected.retain(|c| c.kind != kind);
    }

    /// This state with every subject `resolve` maps replaced, for a view
    /// whose ids live in the SUT's id space.
    pub fn with_subjects_resolved(&self, resolve: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            expected: self
                .expected
                .iter()
                .map(|c| ExpectedCondition {
                    subject_name: resolve(&c.subject_name)
                        .unwrap_or_else(|| c.subject_name.clone()),
                    ..c.clone()
                })
                .collect(),
            governed: self.governed.clone(),
            crash_records: self.crash_records.clone(),
        }
    }

    /// The crash records on disk, newest first. Nothing in a headless run
    /// draws a bus, so none is ever marked seen.
    pub fn crash_history(&self) -> Vec<(Kept, Panicked)> {
        self.crash_records.iter().rev().cloned().collect()
    }

    pub fn expected(&self) -> &BTreeSet<ExpectedCondition> {
        &self.expected
    }

    /// The kinds the invariant may judge. Empty until a transition speaks, so
    /// a run that exercises no failing transition asserts nothing rather than
    /// asserting the app is quiet.
    pub fn governed(&self) -> &BTreeSet<&'static str> {
        &self.governed
    }
}
