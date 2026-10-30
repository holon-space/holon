//! Where a shell effect's log goes: inside its working dir for every value a
//! request renders into the pattern, rendered in one pass, and never over a
//! file that is already there, so every attempt keeps its own log.
//!
//! Oracles come from `docs/Architecture/NetFileFormat.md` (`log_path`) and the
//! runner's contract: a value never refuses a run and maps to exactly one path
//! component; a value of printable ASCII without `/` and `%` that is not ``,
//! `.` or `..` renders byte for byte, every other byte is percent-encoded, so
//! no value depends on what the host's filesystem accepts; only the author's
//! pattern is refused, when it is absolute, has a literal `..` segment, or its
//! last segment names no file.

use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon_net::exec::AttemptId;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Subject;
use holon_net::file::ShellEffect;
use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Finished;
use holon_net_shell::effect::Runner;
use holon_net_shell::effect::ShellError;
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::Config;

/// Stands for an absolute path into the sandbox, outside the working dir; the
/// strategy cannot know the sandbox, so the case substitutes it.
const ABS: &str = "\u{E000}abs\u{E000}";

/// The working dir sits this deep in the sandbox, so every `..` climb a
/// generated request can spell stays observable inside the sandbox.
const DEPTH: usize = 16;

/// Longest file name and path the host accepts, in bytes.
const NAME_MAX: usize = 255;
const PATH_MAX: usize = 1023;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placeholder {
    Transition,
    Subject,
    Attempt,
    Occurrence,
}

impl Placeholder {
    fn spelled(self) -> &'static str {
        match self {
            Self::Transition => "{transition}",
            Self::Subject => "{subject}",
            Self::Attempt => "{attempt}",
            Self::Occurrence => "{occurrence}",
        }
    }
}

#[derive(Debug, Clone)]
enum Piece {
    Text(String),
    Value(Placeholder),
}

/// A `log_path` pattern as its author wrote it.
#[derive(Debug, Clone)]
struct Pattern(String);

impl Pattern {
    fn text(&self) -> &str {
        &self.0
    }

    /// The segments the pattern names, or `None` when the runner refuses it:
    /// the loader's rule (an unknown placeholder, an unbalanced brace, no
    /// `{attempt}`), an absolute pattern, a literal `..` segment, or a last
    /// segment that names no file.
    fn segments(&self) -> Option<Vec<Vec<Piece>>> {
        if self.0.starts_with('/') {
            return None;
        }
        let mut names_attempt = false;
        let mut segments = Vec::new();
        for segment in self.0.split('/') {
            let mut pieces = Vec::new();
            let mut rest = segment;
            while !rest.is_empty() {
                let (text, after) = rest.split_at(rest.find('{').unwrap_or(rest.len()));
                if text.contains('}') {
                    return None;
                }
                if !text.is_empty() {
                    pieces.push(Piece::Text(text.to_owned()));
                }
                if after.is_empty() {
                    break;
                }
                let close = after.find('}')?;
                let placeholder = match &after[1..close] {
                    "transition" => Placeholder::Transition,
                    "subject" => Placeholder::Subject,
                    "attempt" => Placeholder::Attempt,
                    "occurrence" => Placeholder::Occurrence,
                    _ => return None,
                };
                names_attempt |= placeholder == Placeholder::Attempt;
                pieces.push(Piece::Value(placeholder));
                rest = &after[close + 1..];
            }
            segments.push(pieces);
        }
        let last = segments.last().expect("a split has a part");
        let refused = !names_attempt
            || segments
                .iter()
                .any(|segment| literal_text(segment).as_deref() == Some(".."))
            || matches!(literal_text(last).as_deref(), Some("" | "."));
        (!refused).then_some(segments)
    }
}

/// The text of a segment that names no placeholder.
fn literal_text(segment: &[Piece]) -> Option<String> {
    segment
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => Some(text.as_str()),
            Piece::Value(_) => None,
        })
        .collect()
}

/// Path components a log has below the working dir: one per segment, except
/// a literal `` or `.` segment, which names no directory.
fn components(segments: &[Vec<Piece>]) -> usize {
    segments
        .iter()
        .filter(|segment| !matches!(literal_text(segment).as_deref(), Some("" | ".")))
        .count()
}

#[derive(Debug, Clone)]
enum Occurrence {
    Caller(String),
    Minted(String, u64),
    Work(String, String),
}

/// The caller-chosen names of one request, before the sandbox is known.
#[derive(Debug, Clone)]
struct Names {
    transition: String,
    subject: String,
    occurrence: Occurrence,
}

/// One temporary tree: the working dir deep inside it, the pid file and the
/// spawn markers beside it, and nothing else unless a run put it there.
struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    pid_dir: PathBuf,
    spawned: PathBuf,
    runs: usize,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary dir");
        let root = dir
            .path()
            .canonicalize()
            .expect("the temporary dir resolves");
        let work = (0..DEPTH)
            .fold(root.join("up"), |path, level| path.join(level.to_string()))
            .join("work");
        std::fs::create_dir_all(&work).expect("the working dir is created");
        let pid_dir = root.join("pid");
        let spawned = root.join("spawned");
        std::fs::create_dir(&pid_dir).expect("the pid dir is created");
        std::fs::create_dir(&spawned).expect("the spawn marker dir is created");
        Self {
            _dir: dir,
            root,
            work,
            pid_dir,
            spawned,
            runs: 0,
        }
    }

    fn pid_file(&self) -> PathBuf {
        self.pid_dir.join("groups.pid")
    }

    fn resolve(&self, text: &str) -> String {
        let abs = self.root.join("abs");
        text.replace(ABS, abs.to_str().expect("the sandbox path is utf-8"))
    }

    fn request(&self, names: &Names, attempt: u64) -> EffectRequest {
        let occurrence = match &names.occurrence {
            Occurrence::Caller(id) => OccurrenceId::Caller(Arc::from(self.resolve(id))),
            Occurrence::Minted(transition, n) => OccurrenceId::Minted {
                transition: Arc::from(self.resolve(transition)),
                n: *n,
            },
            Occurrence::Work(transition, answer) => OccurrenceId::Work {
                transition: Arc::from(self.resolve(transition)),
                answer: Arc::from(self.resolve(answer)),
            },
        };
        EffectRequest {
            attempt: AttemptId(attempt),
            occurrence,
            transition: Arc::from(self.resolve(&names.transition)),
            subject: Subject::new(&self.resolve(&names.subject)),
            input: Content::new(),
            probe: false,
        }
    }

    /// A command that writes `marker` to its log, then leaves a spawn marker
    /// outside the working dir, then ends after `tail`.
    fn effect(&mut self, pattern: &str, tail: &str, timeout: &str) -> (ShellEffect, Run) {
        self.runs += 1;
        let marker = format!("output of run {}", self.runs);
        let spawned = self.spawned.join(self.runs.to_string());
        let script = format!("printf %s \"$1\"; : > \"$2\"; {tail}");
        let effect = serde_json::from_value(serde_json::json!({
            "command": {
                "argv": ["/bin/sh", "-c", script, "sh", marker, spawned.to_str().expect("utf-8")],
                "working_dir": self.work.to_str().expect("the sandbox path is utf-8"),
                "env_allow": [],
                "timeout": timeout,
            },
            "log_path": pattern,
            "exit_codes": { "done": [0], "failed": [3], "other": "outcome_unknown" },
        }))
        .expect("the descriptor is a valid shell effect");
        (effect, Run { marker, spawned })
    }

    /// Every entry of the sandbox outside the working dir that the sandbox
    /// did not start with.
    fn outside(&self) -> Vec<PathBuf> {
        entries(&self.root)
            .into_iter()
            .filter(|path| {
                !path.starts_with(&self.work)
                    && !path.starts_with(&self.pid_dir)
                    && !path.starts_with(&self.spawned)
                    && !self.work.starts_with(path)
            })
            .collect()
    }

    fn assert_nothing_outside(&self, context: &str) {
        let outside = self.outside();
        assert!(
            outside.is_empty(),
            "expected nothing outside the working dir {} after {context}, but found {outside:?}",
            self.work.display()
        );
    }
}

/// What one run left to check: the text its log must hold and its spawn marker.
struct Run {
    marker: String,
    spawned: PathBuf,
}

fn entries(dir: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("a sandbox dir is readable") {
            let path = entry.expect("a sandbox entry is readable").path();
            let meta = std::fs::symlink_metadata(&path).expect("a sandbox entry has metadata");
            if meta.is_dir() {
                stack.push(path.clone());
            }
            found.insert(path);
        }
    }
    found
}

fn value_of(request: &EffectRequest, placeholder: Placeholder) -> String {
    match placeholder {
        Placeholder::Transition => request.transition.to_string(),
        Placeholder::Subject => request.subject.as_str().to_owned(),
        Placeholder::Attempt => request.attempt.0.to_string(),
        Placeholder::Occurrence => request.occurrence.to_string(),
    }
}

/// Whether a name answers on disk, the host's case and normalization
/// folding included.
fn on_disk(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Whether the host's filesystem refuses a file name holding the non-ASCII
/// characters of the pattern's literal text (APFS: a code point its Unicode
/// version lacks).
fn refused_by_host(value: &str) -> bool {
    let wide: String = value.chars().filter(|c| !c.is_ascii()).collect();
    if wide.is_empty() {
        return false;
    }
    let dir = tempfile::tempdir().expect("a probe dir");
    match std::fs::File::create(dir.path().join(format!("p{wide}"))) {
        Ok(_) => false,
        Err(e) if e.raw_os_error() == Some(libc::EILSEQ) => true,
        Err(e) => panic!("expected the host to take or refuse the name {wide:?}, got {e}"),
    }
}

fn literal_byte(byte: u8) -> bool {
    (0x20..=0x7E).contains(&byte) && byte != b'/' && byte != b'%'
}

/// The one component a value renders: each byte outside the literal class
/// percent-encoded with uppercase hex, and `` `.` `..` encoded whole.
fn encoded(value: &str) -> String {
    match value {
        "" => "%".to_owned(),
        "." => "%2E".to_owned(),
        ".." => "%2E%2E".to_owned(),
        _ => value
            .bytes()
            .map(|b| {
                if literal_byte(b) {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect(),
    }
}

/// What a run of `pattern` for `request` must end in.
#[derive(Debug, Clone, PartialEq)]
enum Expect {
    /// `Err(ShellError::LogPath)` and the command never runs.
    Refused,
    /// `Ok`, the log `components` deep below the working dir, at `exact` when
    /// nothing is at that path yet.
    Logged {
        exact: Option<PathBuf>,
        components: usize,
    },
    /// `Err(ShellError::Log)`: the host refuses the name, because a component
    /// or the path is longer than it allows, or the pattern's literal text
    /// holds a character its filesystem refuses.
    HostRefuses,
    /// `Err(ShellError::Log)`: an entry that is not a dir sits where the path
    /// needs one, which only two nesting patterns sharing a working dir reach.
    Conflict,
    /// Whether a reused path's suffix still fits decides between `Logged` and
    /// `HostRefuses`; the suffix is the runner's, and only its bound is known.
    LoggedOrHostRefuses { components: usize },
}

fn expect(pattern: &Pattern, request: &EffectRequest, work: &Path) -> Expect {
    let Some(segments) = pattern.segments() else {
        return Expect::Refused;
    };
    let mut rendered = Vec::new();
    let (mut lower_path, mut upper_path) = (work.as_os_str().len(), work.as_os_str().len());
    let (mut lower_too_long, mut upper_too_long) = (false, false);
    let mut host_refuses = false;
    for segment in &segments {
        let (mut text, mut lower, mut upper) = (String::new(), 0, 0);
        for piece in segment {
            match piece {
                Piece::Text(literal_text) => {
                    host_refuses |= literal_text.contains('\0') || refused_by_host(literal_text);
                    text.push_str(literal_text);
                    lower += literal_text.chars().count();
                    upper += literal_text.len();
                }
                Piece::Value(p) => {
                    let value = encoded(&value_of(request, *p));
                    lower += value.len();
                    upper += value.len();
                    text.push_str(&value);
                }
            }
        }
        // The suffix a reused path gets.
        upper += 8;
        lower_too_long |= lower > NAME_MAX;
        upper_too_long |= upper > NAME_MAX;
        lower_path += lower + 1;
        upper_path += upper + 1;
        rendered.push(text);
    }
    let components = components(&segments);
    if host_refuses || lower_too_long || lower_path > PATH_MAX {
        Expect::HostRefuses
    } else if upper_too_long || upper_path > PATH_MAX {
        Expect::LoggedOrHostRefuses { components }
    } else {
        Expect::Logged {
            exact: Some(work.join(rendered.join("/"))),
            components,
        }
    }
}

fn variant(error: &ShellError) -> String {
    let debug = format!("{error:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .expect("a debug form has a first word")
        .to_owned()
}

/// Checks one finished run against its expectation and returns the log it
/// wrote with the bytes it holds.
fn check_run(
    sandbox: &Sandbox,
    pattern: &Pattern,
    request: &EffectRequest,
    expected: &Expect,
    run: &Run,
    result: &Result<Finished, ShellError>,
) -> Option<(PathBuf, Vec<u8>)> {
    let context = format!("pattern {:?}, request {request:?}", pattern.text());
    let logged = match (expected, result) {
        (Expect::Refused, Err(e)) => {
            assert_eq!(
                variant(e),
                "LogPath",
                "expected a LogPath refusal for {context}, got {e:?}"
            );
            None
        }
        (Expect::HostRefuses | Expect::Conflict | Expect::LoggedOrHostRefuses { .. }, Err(e)) => {
            assert_eq!(
                variant(e),
                "Log",
                "expected a Log error for a name the host refuses or a dir taken by another entry, {context}, got {e:?}"
            );
            None
        }
        (
            Expect::Logged { components, .. } | Expect::LoggedOrHostRefuses { components },
            Ok(finished),
        ) => Some((finished, *components)),
        (expected, result) => {
            panic!("expected {expected:?} for {context}, but the run returned {result:?}")
        }
    };
    let Some((finished, components)) = logged else {
        assert!(
            !run.spawned.exists(),
            "expected the command not to run for {context}, but it ran"
        );
        sandbox.assert_nothing_outside(&context);
        return None;
    };
    let log = &finished.log_path;
    let below = log.strip_prefix(&sandbox.work).unwrap_or_else(|_| {
        panic!(
            "expected the log under {} for {context}, but it is {}",
            sandbox.work.display(),
            log.display()
        )
    });
    assert!(
        below
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "expected only plain components below the working dir for {context}, but the log is {}",
        log.display()
    );
    assert_eq!(
        below.components().count(),
        components,
        "expected each pattern segment to be one component for {context}, but the log is {}",
        log.display()
    );
    let meta = std::fs::symlink_metadata(log).unwrap_or_else(|e| {
        panic!(
            "expected the log {} to exist for {context}: {e}",
            log.display()
        )
    });
    assert!(
        meta.is_file(),
        "expected the log to be a file, not a link, for {context}"
    );
    let resolved = log.canonicalize().expect("an existing log resolves");
    assert!(
        resolved.starts_with(&sandbox.work),
        "expected the log to resolve inside the working dir for {context}, but it is {}",
        resolved.display()
    );
    let bytes = std::fs::read(log).unwrap_or_else(|e| {
        panic!(
            "expected to read the log {} for {context}: {e}",
            log.display()
        )
    });
    match finished.ended {
        Ended::Exited { .. } => assert_eq!(
            bytes,
            run.marker.as_bytes(),
            "expected the log {} to hold this run's output for {context}",
            log.display()
        ),
        // A kill can come before the child's first write.
        Ended::Killed(_) | Ended::Signalled(_) => assert!(
            run.marker.as_bytes().starts_with(&bytes),
            "expected the log {} to hold a prefix of this run's output for {context}, but it holds {bytes:?}",
            log.display()
        ),
    }
    sandbox.assert_nothing_outside(&context);
    Some((log.clone(), bytes))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a test runtime")
}

// ---- strategies -------------------------------------------------------------

fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[a-z]{1,4}",
        3 => "(?s).{0,5}",
        1 => Just(String::new()),
        2 => Just(".".to_owned()),
        3 => Just("..".to_owned()),
        3 => Just("/".to_owned()),
        1 => Just("%".to_owned()),
        1 => Just("%2F".to_owned()),
        1 => Just("%2E".to_owned()),
        1 => Just("%25".to_owned()),
        1 => Just("\0".to_owned()),
        1 => Just("%00".to_owned()),
        2 => Just("{attempt}".to_owned()),
        1 => Just("{subject}".to_owned()),
        1 => Just("{transition}".to_owned()),
        1 => Just("{occurrence}".to_owned()),
        1 => Just("{".to_owned()),
        1 => Just("}".to_owned()),
        1 => Just(ABS.to_owned()),
        1 => Just("x".repeat(300)),
        1 => Just("A".to_owned()),
        1 => Just("\u{e9}".to_owned()),
        1 => Just("e\u{301}".to_owned()),
        2 => "[ -~]{1,4}",
        1 => "[\x01-\x1f\x7f]{1,2}",
    ]
}

fn value() -> impl Strategy<Value = String> {
    prop::collection::vec(fragment(), 0..4).prop_map(|fragments| fragments.concat())
}

fn occurrence() -> impl Strategy<Value = Occurrence> {
    prop_oneof![
        value().prop_map(Occurrence::Caller),
        (value(), any::<u64>()).prop_map(|(t, n)| Occurrence::Minted(t, n)),
        (value(), value()).prop_map(|(t, a)| Occurrence::Work(t, a)),
    ]
}

fn names() -> impl Strategy<Value = Names> {
    (value(), value(), occurrence()).prop_map(|(transition, subject, occurrence)| Names {
        transition,
        subject,
        occurrence,
    })
}

fn placeholder() -> impl Strategy<Value = Placeholder> {
    prop_oneof![
        Just(Placeholder::Transition),
        Just(Placeholder::Subject),
        Just(Placeholder::Attempt),
        Just(Placeholder::Occurrence),
    ]
}

fn piece() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => "[a-z0-9_.-]{0,4}",
        4 => prop_oneof![Just(""), Just("."), Just("..")].prop_map(str::to_owned),
        12 => placeholder().prop_map(|p| p.spelled().to_owned()),
        1 => prop_oneof!["\\{[a-z]{0,6}\\}", Just("{".to_owned()), Just("}".to_owned())],
        1 => "(?s).{0,3}",
    ]
}

/// Every pattern a caller can pass: segments of literal text, placeholders
/// and stray braces, most of them naming `{attempt}`, some absolute.
fn pattern() -> impl Strategy<Value = Pattern> {
    (
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.9),
        prop::collection::vec(prop::collection::vec(piece(), 1..4), 1..5),
    )
        .prop_map(|(absolute, attempt, mut segments)| {
            if attempt && !segments.iter().flatten().any(|piece| piece == "{attempt}") {
                segments
                    .last_mut()
                    .expect("a pattern has a segment")
                    .push("{attempt}".to_owned());
            }
            let text = segments
                .iter()
                .map(|segment| segment.concat())
                .collect::<Vec<_>>()
                .join("/");
            Pattern(if absolute { format!("/{text}") } else { text })
        })
}

fn accepted_pattern() -> impl Strategy<Value = Pattern> {
    pattern().prop_filter("the runner accepts it", |pattern| {
        pattern.segments().is_some()
    })
}

// ---- one run per case -------------------------------------------------------

fn run_once(pattern: &Pattern, names: &Names, attempt: u64) {
    let mut sandbox = Sandbox::new();
    let (runner, _) = Runner::open(sandbox.pid_file()).expect("a fresh pid file opens");
    let request = sandbox.request(names, attempt);
    let expected = expect(pattern, &request, &sandbox.work);
    let (effect, run) = sandbox.effect(pattern.text(), "exit 0", "30s");
    let before = entries(&sandbox.work);
    let result = runtime().block_on(runner.run(&effect, &request));
    let logged = check_run(&sandbox, pattern, &request, &expected, &run, &result);
    if logged.is_none() {
        assert_eq!(
            std::fs::read_to_string(sandbox.pid_file()).expect("the pid file reads"),
            "",
            "expected no pid-file entry for a run that did not run, pattern {:?}",
            pattern.text()
        );
        let new: Vec<PathBuf> = entries(&sandbox.work)
            .difference(&before)
            .cloned()
            .collect();
        // The host refuses a name only once the runner has made its parent dirs.
        let left = |path: &PathBuf| expected == Expect::Refused || !path.is_dir();
        assert!(
            !new.iter().any(left),
            "expected no log for a run that did not run, pattern {:?}, request {request:?}, but found {new:?}",
            pattern.text()
        );
    }
    if let Expect::Logged {
        exact: Some(exact), ..
    } = &expected
    {
        let log = &result.as_ref().expect("checked above").log_path;
        assert_eq!(
            log,
            exact,
            "expected the log at its rendered path, each value encoded in one pass, pattern {:?}, request {request:?}",
            pattern.text()
        );
    }
}

proptest! {
    #![proptest_config(Config { cases: 192, failure_persistence: None, ..Config::default() })]

    #[test]
    fn every_value_a_request_renders_keeps_the_log_inside_the_working_dir(
        pattern in accepted_pattern(),
        names in names(),
        attempt in any::<u64>(),
    ) {
        run_once(&pattern, &names, attempt);
    }

    #[test]
    fn every_pattern_a_caller_passes_runs_inside_the_working_dir_or_is_refused_before_it_runs(
        pattern in pattern(),
        names in names(),
        attempt in any::<u64>(),
    ) {
        run_once(&pattern, &names, attempt);
    }
}

// ---- different values, different logs --------------------------------------

/// Values from a small alphabet of the bytes an encoding must keep apart, so
/// equal and near-equal pairs are frequent.
fn close_value() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just(""),
        Just("."),
        Just(".."),
        Just("/"),
        Just("%"),
        Just("%2F"),
        Just("%2E"),
        Just("%25"),
        Just("\0"),
        Just("%00"),
        Just("a"),
        Just("A"),
        Just("\u{e9}"),
        Just("e\u{301}"),
    ];
    prop::collection::vec(piece, 0..3).prop_map(|pieces| pieces.concat())
}

fn close_names() -> impl Strategy<Value = Names> {
    (close_value(), close_value(), close_value()).prop_map(|(transition, subject, id)| Names {
        transition,
        subject,
        occurrence: Occurrence::Caller(id),
    })
}

/// A pattern with each placeholder alone in its own segment between fixed
/// literal text, so the log path names each value apart.
fn separating_pattern() -> impl Strategy<Value = Pattern> {
    (
        Just(vec![
            Placeholder::Transition,
            Placeholder::Subject,
            Placeholder::Occurrence,
            Placeholder::Attempt,
        ])
        .prop_shuffle(),
        prop::collection::vec(("[a-z.]{0,2}", "[a-z.]{0,2}"), 4),
    )
        .prop_map(|(order, around)| {
            let segments: Vec<String> = order
                .into_iter()
                .zip(around)
                .map(|(p, (before, after))| format!("{before}{}{after}", p.spelled()))
                .collect();
            Pattern(segments.join("/"))
        })
}

fn log_below_work(pattern: &Pattern, names: &Names, attempt: u64) -> PathBuf {
    let mut sandbox = Sandbox::new();
    let (runner, _) = Runner::open(sandbox.pid_file()).expect("a fresh pid file opens");
    let request = sandbox.request(names, attempt);
    let expected = expect(pattern, &request, &sandbox.work);
    let (effect, run) = sandbox.effect(pattern.text(), "exit 0", "30s");
    let result = runtime().block_on(runner.run(&effect, &request));
    let (log, _) = check_run(&sandbox, pattern, &request, &expected, &run, &result)
        .expect("a separating pattern with short values logs");
    log.strip_prefix(&sandbox.work)
        .expect("checked above")
        .to_owned()
}

proptest! {
    #![proptest_config(Config { cases: 192, failure_persistence: None, ..Config::default() })]

    #[test]
    fn different_values_get_different_logs_and_equal_values_the_same_log(
        pattern in separating_pattern(),
        a in close_names(),
        b in close_names(),
        attempt_a in 1u64..3,
        attempt_b in 1u64..3,
    ) {
        let first = log_below_work(&pattern, &a, attempt_a);
        let second = log_below_work(&pattern, &b, attempt_b);
        let tuple = |n: &Names, attempt: u64| {
            let Occurrence::Caller(id) = &n.occurrence else { unreachable!("close names are caller occurrences") };
            (n.transition.clone(), n.subject.clone(), id.clone(), attempt)
        };
        prop_assert_eq!(
            tuple(&a, attempt_a) == tuple(&b, attempt_b),
            first == second,
            "pattern {:?}: {:?} gives {:?}, {:?} gives {:?}",
            pattern.text(), tuple(&a, attempt_a), first, tuple(&b, attempt_b), second
        );
    }
}

// ---- many runs, restarts and runners on one working dir -------------------

#[derive(Debug, Clone, Copy)]
enum Plant {
    File,
    Dir,
    DanglingLink,
}

#[derive(Debug, Clone)]
enum Op {
    /// A run that is awaited at once.
    Run {
        runner: Index,
        attempt: u64,
        names: Index,
        pattern: Index,
        code: i32,
    },
    /// A run that stays open while later operations happen.
    Start {
        runner: Index,
        attempt: u64,
        names: Index,
        pattern: Index,
        timeout: bool,
    },
    /// Ends an open run: abandons it, then waits for it.
    Finish {
        open: Index,
    },
    Abandon {
        runner: Index,
        attempt: u64,
    },
    InFlight {
        runner: Index,
    },
    /// A new runner on the same pid file takes the place of the newest one.
    Restart,
    /// A second runner on the same pid file next to the live ones.
    SecondRunner,
    /// Puts an entry where a reused path of an earlier log would go.
    PlantBesideLog {
        log: Index,
        suffix: u8,
        plant: Plant,
    },
    /// Puts an entry where a later run's log would go.
    PlantAhead {
        attempt: u64,
        names: Index,
        pattern: Index,
        plant: Plant,
    },
}

fn plant() -> impl Strategy<Value = Plant> {
    prop_oneof![
        Just(Plant::File),
        Just(Plant::Dir),
        Just(Plant::DanglingLink)
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let attempt = 1u64..4;
    prop_oneof![
        5 => (any::<Index>(), attempt.clone(), any::<Index>(), any::<Index>(), prop_oneof![Just(0), Just(3)])
            .prop_map(|(runner, attempt, names, pattern, code)| Op::Run { runner, attempt, names, pattern, code }),
        2 => (any::<Index>(), attempt.clone(), any::<Index>(), any::<Index>(), any::<bool>())
            .prop_map(|(runner, attempt, names, pattern, timeout)| Op::Start { runner, attempt, names, pattern, timeout }),
        2 => any::<Index>().prop_map(|open| Op::Finish { open }),
        1 => (any::<Index>(), 0u64..5).prop_map(|(runner, attempt)| Op::Abandon { runner, attempt }),
        1 => any::<Index>().prop_map(|runner| Op::InFlight { runner }),
        2 => Just(Op::Restart),
        1 => Just(Op::SecondRunner),
        1 => (any::<Index>(), 1u8..4, plant()).prop_map(|(log, suffix, plant)| Op::PlantBesideLog { log, suffix, plant }),
        1 => (attempt, any::<Index>(), any::<Index>(), plant())
            .prop_map(|(attempt, names, pattern, plant)| Op::PlantAhead { attempt, names, pattern, plant }),
    ]
}

/// A run still open, with what its end is checked against.
struct Open {
    handle: tokio::task::JoinHandle<Result<Finished, ShellError>>,
    runner: Arc<Runner>,
    attempt: u64,
    started: Started,
}

struct Started {
    pattern: Pattern,
    request: EffectRequest,
    expected: Expect,
    run: Run,
}

enum Planted {
    File(PathBuf, Vec<u8>),
    Dir(PathBuf),
    Link(PathBuf),
}

impl Planted {
    fn path(&self) -> &Path {
        match self {
            Self::File(path, _) | Self::Dir(path) | Self::Link(path) => path,
        }
    }
}

struct World {
    sandbox: Sandbox,
    names: Vec<Names>,
    patterns: Vec<Pattern>,
    runners: Vec<Arc<Runner>>,
    open: Vec<Open>,
    logs: Vec<(PathBuf, Vec<u8>)>,
    planted: Vec<Planted>,
}

impl World {
    fn new(names: Vec<Names>, patterns: Vec<Pattern>) -> Self {
        let sandbox = Sandbox::new();
        let (runner, _) = Runner::open(sandbox.pid_file()).expect("a fresh pid file opens");
        Self {
            sandbox,
            names,
            patterns,
            runners: vec![Arc::new(runner)],
            open: Vec::new(),
            logs: Vec::new(),
            planted: Vec::new(),
        }
    }

    fn taken(&self, path: &Path) -> bool {
        self.logs.iter().any(|(log, _)| log == path)
            || self.planted.iter().any(|p| p.path() == path)
    }

    /// Every earlier log and planted entry is as it was.
    fn assert_kept(&self, context: &str) {
        for (log, bytes) in &self.logs {
            let now = std::fs::read(log).unwrap_or_else(|e| {
                panic!(
                    "expected the earlier log {} to stay after {context}: {e}",
                    log.display()
                )
            });
            assert_eq!(
                &now,
                bytes,
                "expected the earlier log {} to keep its bytes after {context}",
                log.display()
            );
        }
        for planted in &self.planted {
            let meta = std::fs::symlink_metadata(planted.path()).unwrap_or_else(|e| {
                panic!(
                    "expected the planted {} to stay after {context}: {e}",
                    planted.path().display()
                )
            });
            match planted {
                Planted::File(path, bytes) => {
                    assert!(
                        meta.is_file(),
                        "expected {} to stay a file after {context}",
                        path.display()
                    );
                    assert_eq!(
                        &std::fs::read(path).expect("a planted file reads"),
                        bytes,
                        "expected the planted file {} to keep its bytes after {context}",
                        path.display()
                    );
                }
                Planted::Dir(path) => {
                    assert!(
                        meta.is_dir(),
                        "expected {} to stay a dir after {context}",
                        path.display()
                    );
                    assert_eq!(
                        std::fs::read_dir(path)
                            .expect("a planted dir reads")
                            .count(),
                        0,
                        "expected the planted dir {} to stay empty after {context}",
                        path.display()
                    );
                }
                Planted::Link(path) => {
                    assert!(
                        meta.is_symlink(),
                        "expected {} to stay a link after {context}",
                        path.display()
                    );
                }
            }
        }
        self.sandbox.assert_nothing_outside(context);
    }

    async fn start(
        &mut self,
        runner: usize,
        attempt: u64,
        names: usize,
        pattern: usize,
        tail: &str,
        timeout: &str,
    ) -> Open {
        let runner = Arc::clone(&self.runners[runner]);
        let names = self.names[names].clone();
        let pattern = self.patterns[pattern].clone();
        let request = self.sandbox.request(&names, attempt);
        let mut expected = expect(&pattern, &request, &self.sandbox.work);
        if let Expect::Logged { exact: Some(e), .. } = &expected
            && e.ancestors()
                .skip(1)
                .take_while(|a| *a != self.sandbox.work)
                .any(|a| on_disk(a) && !a.is_dir())
        {
            expected = Expect::Conflict;
        }
        if let Expect::Logged { exact, .. } = &mut expected {
            // Another log or entry is there, or an open run may take the path first.
            if exact
                .as_deref()
                .is_some_and(|e| self.taken(e) || on_disk(e))
                || !self.open.is_empty()
            {
                *exact = None;
            }
        }
        let (effect, run) = self.sandbox.effect(pattern.text(), tail, timeout);
        let handle = {
            let runner = Arc::clone(&runner);
            let request = request.clone();
            tokio::spawn(async move { runner.run(&effect, &request).await })
        };
        // The run creates its log on its first poll, before anything else is planted.
        tokio::task::yield_now().await;
        Open {
            handle,
            runner,
            attempt,
            started: Started {
                pattern,
                request,
                expected,
                run,
            },
        }
    }

    async fn end(&mut self, open: Open, abandon: bool) {
        if abandon {
            open.runner.abandon(AttemptId(open.attempt));
        }
        let result = open.handle.await.expect("a run task does not panic");
        let Started {
            pattern,
            request,
            expected,
            run,
        } = open.started;
        let context = format!(
            "the run of pattern {:?}, request {request:?}",
            pattern.text()
        );
        if let Ok(finished) = &result {
            assert!(
                !self.taken(&finished.log_path),
                "expected a new log for {context}, but {} was already there",
                finished.log_path.display()
            );
        }
        if let (
            Expect::Logged {
                exact: Some(exact), ..
            },
            Ok(finished),
        ) = (&expected, &result)
        {
            assert_eq!(
                &finished.log_path, exact,
                "expected the log at its rendered path, each value encoded in one pass for {context}"
            );
        }
        if let Some(log) = check_run(&self.sandbox, &pattern, &request, &expected, &run, &result) {
            self.logs.push(log);
        }
        self.assert_kept(&context);
    }

    fn plant(&mut self, path: PathBuf, plant: Plant) {
        if self.taken(&path) || std::fs::symlink_metadata(&path).is_ok() {
            return;
        }
        let n = self.planted.len();
        std::fs::create_dir_all(path.parent().expect("a log has a parent"))
            .expect("the planted entry's dir is created");
        let planted = match plant {
            Plant::File => {
                let bytes = format!("planted {n}").into_bytes();
                std::fs::write(&path, &bytes).expect("a planted file is written");
                Planted::File(path, bytes)
            }
            Plant::Dir => {
                std::fs::create_dir(&path).expect("a planted dir is created");
                Planted::Dir(path)
            }
            Plant::DanglingLink => {
                std::os::unix::fs::symlink(
                    self.sandbox.root.join(format!("link-target-{n}")),
                    &path,
                )
                .expect("a planted link is created");
                Planted::Link(path)
            }
        };
        self.planted.push(planted);
    }

    async fn apply(&mut self, op: &Op) {
        match op {
            Op::Run {
                runner,
                attempt,
                names,
                pattern,
                code,
            } => {
                let (runner, names, pattern) = (
                    self.at(runner),
                    names.index(self.names.len()),
                    pattern.index(self.patterns.len()),
                );
                self.run_now(runner, *attempt, names, pattern, *code).await;
            }
            Op::Start {
                runner,
                attempt,
                names,
                pattern,
                timeout,
            } => {
                let timeout = if *timeout { "200ms" } else { "30s" };
                let (runner, names, pattern) = (
                    runner.index(self.runners.len()),
                    names.index(self.names.len()),
                    pattern.index(self.patterns.len()),
                );
                let open = self
                    .start(
                        runner,
                        *attempt,
                        names,
                        pattern,
                        "exec /bin/sleep 3",
                        timeout,
                    )
                    .await;
                self.open.push(open);
            }
            Op::Finish { open } => {
                if !self.open.is_empty() {
                    let open = self.open.remove(open.index(self.open.len()));
                    self.end(open, true).await;
                }
            }
            Op::Abandon { runner, attempt } => {
                runner.get(&self.runners).abandon(AttemptId(*attempt));
            }
            Op::InFlight { runner } => {
                runner.get(&self.runners).in_flight();
            }
            Op::Restart => {
                self.runners.pop();
                let (runner, _) =
                    Runner::open(self.sandbox.pid_file()).expect("the pid file reopens");
                self.runners.push(Arc::new(runner));
            }
            Op::SecondRunner => {
                if self.runners.len() < 3 {
                    let (runner, _) =
                        Runner::open(self.sandbox.pid_file()).expect("the pid file opens again");
                    self.runners.push(Arc::new(runner));
                }
            }
            Op::PlantBesideLog { log, suffix, plant } => {
                if !self.logs.is_empty() {
                    let (path, _) = log.get(&self.logs);
                    let mut beside = path.clone().into_os_string();
                    beside.push(format!(".{suffix}"));
                    self.plant(PathBuf::from(beside), *plant);
                }
            }
            Op::PlantAhead {
                attempt,
                names,
                pattern,
                plant,
            } => {
                let names = names.get(&self.names).clone();
                let pattern = pattern.get(&self.patterns).clone();
                let request = self.sandbox.request(&names, *attempt);
                if let Expect::Logged {
                    exact: Some(exact), ..
                } = expect(&pattern, &request, &self.sandbox.work)
                {
                    self.plant(exact, *plant);
                }
            }
        }
    }

    fn at(&self, runner: &Index) -> usize {
        runner.index(self.runners.len())
    }

    async fn run_now(
        &mut self,
        runner: usize,
        attempt: u64,
        names: usize,
        pattern: usize,
        code: i32,
    ) {
        let open = self
            .start(
                runner,
                attempt,
                names,
                pattern,
                &format!("exit {code}"),
                "30s",
            )
            .await;
        self.end(open, false).await;
    }

    async fn close(&mut self) {
        while let Some(open) = self.open.pop() {
            self.end(open, true).await;
        }
    }
}

/// Names a reused run would share: drawn from the wide value space, plus the
/// pairs whose sequential rendering would coincide.
fn names_pool() -> impl Strategy<Value = Vec<Names>> {
    let colliding = prop_oneof![
        Just(Names {
            transition: "Run".to_owned(),
            subject: "{attempt}".to_owned(),
            occurrence: Occurrence::Caller("op".to_owned()),
        }),
        Just(Names {
            transition: "Run".to_owned(),
            subject: "1".to_owned(),
            occurrence: Occurrence::Caller("op".to_owned()),
        }),
        Just(Names {
            transition: "{subject}".to_owned(),
            subject: "a".to_owned(),
            occurrence: Occurrence::Caller("{transition}".to_owned()),
        }),
    ];
    prop::collection::vec(prop_oneof![2 => names(), 1 => colliding], 1..4)
}

/// Literal values with the case twins a folding host stores under one name,
/// and normalization twins, which the encoding keeps apart.
fn cased() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-zA-Z]{1,3}",
        1 => Just("\u{e9}".to_owned()),
        1 => Just("e\u{301}".to_owned()),
    ]
}

fn reused_patterns() -> impl Strategy<Value = Vec<Pattern>> {
    prop::collection::vec(
        prop_oneof![
            3 => pattern(),
            1 => Just(Pattern("{subject}-{attempt}.log".to_owned())),
            1 => Just(Pattern("{transition}-{attempt}.log".to_owned())),
        ],
        1..3,
    )
}

fn drive(names: Vec<Names>, patterns: Vec<Pattern>, ops: &[Op]) {
    runtime().block_on(async {
        let mut world = World::new(names, patterns);
        for op in ops {
            world.apply(op).await;
        }
        world.close().await;
    });
}

proptest! {
    #![proptest_config(Config { cases: 64, failure_persistence: None, ..Config::default() })]

    #[test]
    fn every_attempt_keeps_its_own_log_across_runs_restarts_and_runners(
        names in names_pool(),
        patterns in reused_patterns(),
        ops in prop::collection::vec(op(), 1..14),
    ) {
        drive(names, patterns, &ops);
    }

    /// The same sequences with values that render byte for byte and patterns
    /// the runner accepts, so a reused path comes only from a reused attempt.
    #[test]
    fn every_attempt_keeps_its_own_log_when_values_render_literally(
        names in prop::collection::vec(
            (cased(), cased(), cased()).prop_map(|(transition, subject, id)| Names {
                transition,
                subject,
                occurrence: Occurrence::Caller(id),
            }),
            1..3,
        ),
        patterns in prop::collection::vec(accepted_pattern(), 1..3),
        ops in prop::collection::vec(op(), 1..14),
    ) {
        drive(names, patterns, &ops);
    }
}

// ---- the known instances, one per defect ------------------------------------

fn subject_pattern(after: &str) -> Pattern {
    Pattern(format!("{{subject}}{after}{{attempt}}.log"))
}

fn subject(subject: &str) -> Names {
    Names {
        transition: "Run".to_owned(),
        subject: subject.to_owned(),
        occurrence: Occurrence::Caller("op-1".to_owned()),
    }
}

#[test]
fn a_subject_with_parent_components_keeps_the_log_inside_the_working_dir() {
    run_once(&subject_pattern("-"), &subject("../escaped"), 1);
    run_once(
        &subject_pattern("-"),
        &subject("../../../../x/../../escaped"),
        1,
    );
}

#[test]
fn an_absolute_subject_keeps_the_log_inside_the_working_dir() {
    run_once(
        &subject_pattern("-"),
        &subject(&format!("{ABS}/escaped")),
        1,
    );
}

#[test]
fn an_empty_subject_before_a_slash_keeps_the_log_inside_the_working_dir() {
    run_once(&subject_pattern("/"), &subject(""), 1);
}

#[test]
fn an_ordinary_subject_renders_byte_for_byte() {
    run_once(&subject_pattern("-"), &subject("escaped"), 1);
    run_once(&subject_pattern("-"), &subject("a b:c@d#e"), 7);
}

#[test]
fn a_subject_the_host_cannot_store_raw_still_gets_its_log() {
    run_once(&subject_pattern("-"), &subject("\u{3347a}"), 1);
    run_once(&subject_pattern("-"), &subject("\u{dc}bersicht"), 1);
    run_once(&subject_pattern("-"), &subject("a\tb\u{7f}"), 1);
}

#[test]
fn a_value_that_spells_a_placeholder_renders_as_its_own_text() {
    run_once(&subject_pattern("-"), &subject("{attempt}"), 1);
    run_once(
        &subject_pattern("-"),
        &subject("{transition}{occurrence}"),
        1,
    );
}

#[test]
fn subjects_attempt_and_1_get_different_logs() {
    let a = log_below_work(&subject_pattern("-"), &subject("{attempt}"), 1);
    let b = log_below_work(&subject_pattern("-"), &subject("1"), 1);
    assert_ne!(
        a, b,
        "expected subjects `{{attempt}}` and `1` to get different logs"
    );
}

#[test]
fn an_author_pattern_the_runner_cannot_keep_inside_the_working_dir_is_refused_before_it_runs() {
    let refused = [
        "../{attempt}.log",
        "logs/../../{attempt}.log",
        "/logs/{attempt}.log",
        "{attempt}/",
        "{attempt}/.",
        "{attempt}/..",
        "{attempt}-{foo}.log",
        "{attempt}-{.log",
        "{attempt}-}.log",
        "{subject}.log",
    ];
    for text in refused {
        let pattern = Pattern(text.to_owned());
        assert!(pattern.segments().is_none(), "{text} is a refused pattern");
        run_once(&pattern, &subject("s"), 1);
    }
    let accepted = [
        "./logs/{attempt}.log",
        "..x/{attempt}",
        "{attempt}",
        "a/./{attempt}.log",
        "x..{attempt}",
    ];
    for text in accepted {
        let pattern = Pattern(text.to_owned());
        assert!(
            pattern.segments().is_some(),
            "{text} is an accepted pattern"
        );
        run_once(&pattern, &subject("s"), 1);
    }
}

#[test]
fn a_restarted_runner_keeps_the_log_of_the_earlier_attempt() {
    let pattern = Pattern("{transition}-{attempt}.log".to_owned());
    runtime().block_on(async {
        let mut world = World::new(vec![subject("a")], vec![pattern]);
        world.run_now(0, 1, 0, 0, 0).await;
        world.apply(&Op::Restart).await;
        world.run_now(0, 1, 0, 0, 0).await;
        world.apply(&Op::SecondRunner).await;
        world.run_now(1, 1, 0, 0, 0).await;
        world.run_now(0, 1, 0, 0, 0).await;
        world.close().await;
    });
}

#[test]
fn a_run_whose_dir_another_patterns_log_holds_fails_and_keeps_that_log() {
    let patterns = vec![
        Pattern("{attempt}".to_owned()),
        Pattern("{attempt}/{transition}".to_owned()),
    ];
    runtime().block_on(async {
        let mut world = World::new(vec![subject("a")], patterns);
        world.run_now(0, 2, 0, 0, 0).await;
        world.run_now(0, 2, 0, 1, 0).await;
        world.run_now(0, 3, 0, 1, 0).await;
        world.run_now(0, 3, 0, 0, 0).await;
        world.close().await;
    });
}
