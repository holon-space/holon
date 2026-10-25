//! The probes of a net file (`docs/Architecture/NetFileFormat.md`): commands
//! the host runs on an interval, whose stdout is facts, one JSON object per
//! line.
//!
//! Sans-IO, like [`crate::exec`]: [`Probes::due`] names the runs to start at
//! `now`, the host runs each command and hands its [`ProbeOutcome`] to
//! [`Probes::settle`], which commits the facts into the executor.
//!
//! A good run is the whole truth about its place: a subject it no longer
//! reports is retracted. A run that fails, or whose stdout is not facts,
//! changes no fact: the facts of the last good run stay, and a
//! [`ProbeCondition`] discloses them as stale until a good run clears it.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use holon_pattern::Value;
use indexmap::IndexMap;

use crate::exec::Content;
use crate::exec::EffectRequest;
use crate::exec::ExecError;
use crate::exec::Executor;
use crate::exec::Place;
use crate::exec::Subject;
use crate::file::CommandSpec;
use crate::file::ProbeSpec;

/// How one run of a probe's command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The command exited 0 with this stdout.
    Output(String),
    /// The command exited non-zero, timed out or did not start; the text
    /// says which.
    Failed(String),
}

/// A probe whose last run failed: its facts are the last good run's, kept
/// and stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeCondition {
    pub probe: String,
    pub place: Place,
    pub cause: String,
    /// When the first failed run of the current streak settled.
    pub since: Duration,
    pub stale: Vec<Subject>,
}

impl fmt::Display for ProbeCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "probe {} failed: {}; its {} facts in {} are stale",
            self.probe,
            self.cause,
            self.stale.len(),
            self.place
        )
    }
}

/// The run state of every probe of one net.
pub struct Probes {
    probes: IndexMap<String, State>,
}

struct State {
    spec: ProbeSpec,
    next: Duration,
    running: bool,
    /// The subjects the last good run wrote.
    subjects: BTreeSet<Subject>,
    /// Subjects a good run dropped while a firing (running or parked) held
    /// their token: retracted once the token is back, forgotten once no
    /// firing holds it.
    held: BTreeSet<Subject>,
    failure: Option<(String, Duration)>,
}

impl Probes {
    /// Every probe is due at `now`.
    pub fn new(specs: &IndexMap<String, ProbeSpec>, now: Duration) -> Self {
        let probes = specs
            .iter()
            .map(|(name, spec)| {
                let state = State {
                    spec: spec.clone(),
                    next: now,
                    running: false,
                    subjects: BTreeSet::new(),
                    held: BTreeSet::new(),
                    failure: None,
                };
                (name.clone(), state)
            })
            .collect();
        Self { probes }
    }

    /// The probes to start at `now`, with their commands. Each is running
    /// until its [`Probes::settle`].
    pub fn due(&mut self, now: Duration) -> Vec<(String, CommandSpec)> {
        self.probes
            .iter_mut()
            .filter(|(_, s)| !s.running && s.next <= now)
            .map(|(name, s)| {
                s.running = true;
                (name.clone(), s.spec.command.clone())
            })
            .collect()
    }

    /// Commits the facts of a run of `probe` that ended at `now`, and
    /// schedules its next run one interval later. Returns the effects the
    /// changed facts enabled.
    pub fn settle(
        &mut self,
        exec: &mut Executor,
        probe: &str,
        outcome: ProbeOutcome,
        now: Duration,
    ) -> Result<Vec<EffectRequest>, ExecError> {
        let state = self
            .probes
            .get_mut(probe)
            .unwrap_or_else(|| panic!("a settle of the undeclared probe '{probe}'"));
        assert!(
            state.running,
            "a settle of probe '{probe}', which is not running"
        );
        state.running = false;
        state.next = now + state.spec.interval.duration();
        let facts = match outcome {
            ProbeOutcome::Output(stdout) => facts(&stdout, &state.spec.subject_field),
            ProbeOutcome::Failed(cause) => Err(cause),
        };
        let facts = match facts {
            Ok(facts) => facts,
            Err(cause) => {
                let since = state.failure.as_ref().map_or(now, |(_, since)| *since);
                state.failure = Some((cause, since));
                return Ok(Vec::new());
            }
        };
        state.failure = None;
        let place = &state.spec.facts_into;
        let mut requests = Vec::new();
        let gone: Vec<Subject> = state
            .subjects
            .union(&state.held)
            .filter(|s| !facts.contains_key(*s))
            .cloned()
            .collect();
        state.held.clear();
        for gone in gone {
            if exec.tokens().token(place, &gone).is_some() {
                requests.extend(exec.retract(place, &gone, now)?.requests);
            } else if exec.holder(&Place::new(place), &gone).is_some() {
                state.held.insert(gone);
            }
        }
        state.subjects = facts.keys().cloned().collect();
        for (subject, content) in facts {
            requests.extend(exec.write(place, &subject, content, now)?.requests);
        }
        Ok(requests)
    }

    /// One condition per probe whose last run failed.
    pub fn conditions(&self) -> Vec<ProbeCondition> {
        self.probes
            .iter()
            .filter_map(|(name, s)| {
                let (cause, since) = s.failure.as_ref()?;
                Some(ProbeCondition {
                    probe: name.clone(),
                    place: Place::new(&s.spec.facts_into),
                    cause: cause.clone(),
                    since: *since,
                    stale: s.subjects.iter().cloned().collect(),
                })
            })
            .collect()
    }
}

/// Every line of `stdout` as one fact under the subject its `subject_field`
/// names, or why the stdout is not facts.
fn facts(stdout: &str, subject_field: &str) -> Result<BTreeMap<Subject, Content>, String> {
    let mut facts = BTreeMap::new();
    for (i, line) in stdout.lines().enumerate() {
        let refused = |why: String| format!("line {} of the output {why}: {line}", i + 1);
        let json: serde_json::Value =
            serde_json::from_str(line).map_err(|e| refused(format!("is not JSON ({e})")))?;
        let Value::Object(fields) = Value::from_json_value(json) else {
            return Err(refused("is not a JSON object".to_owned()));
        };
        let subject = match fields.get(subject_field) {
            Some(Value::String(s)) => Subject::new(s),
            Some(Value::Integer(n)) => Subject::new(&n.to_string()),
            Some(other) => {
                return Err(refused(format!(
                    "has a {subject_field} that is neither a string nor an integer: {other:?}"
                )));
            }
            None => return Err(refused(format!("has no {subject_field}"))),
        };
        let content: Content = fields.into_iter().collect();
        if facts.insert(subject.clone(), content).is_some() {
            return Err(refused(format!("repeats the subject {subject}")));
        }
    }
    Ok(facts)
}
