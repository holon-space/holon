//! netd start against pid files the test controls: a held pid-file write never
//! stalls the runtime that awaits the start, and every pid file found at start
//! is read or refused by name.

#[path = "../../holon-net-shell/tests/pid_file_support/mod.rs"]
mod pid_file_support;

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use holon_net_shell::effect::ShellError;
use holon_netd::Config;
use holon_netd::Netd;
use holon_netd::NetdError;
use pid_file_support::BOUND;
use pid_file_support::Drained;
use pid_file_support::Flavor;
use pid_file_support::Hold;
use pid_file_support::Release;
use pid_file_support::SETTLE;
use pid_file_support::eventually;
use pid_file_support::flavor;
use proptest::prelude::*;

/// How long the test holds the pid-file write: twice the bound, so a worker
/// that is blocked from the write until the release misses its beats for
/// longer than the bound.
const WINDOW: Duration = Duration::from_millis(2 * BOUND.as_millis() as u64);

/// No group with an id this large exists: Linux caps process ids at 2^22,
/// macOS far below.
const NO_GROUP: std::ops::RangeInclusive<i32> = (1 << 22) + 1..=i32::MAX;

#[derive(Debug, Clone)]
enum Live {
    Dead(i32),
    /// A group the test starts and ends.
    Alive,
}

/// A group record as the runner writes it: pgid, attempt, transition, subject.
type Record = (Live, u64, String, String);

/// A line no runner writes.
#[derive(Debug, Clone)]
enum Bad {
    NotJson(String),
    Arity(usize),
    PgidText,
    PgidOutOfRange(i64),
    AttemptNegative(i64),
    Blank,
    /// A pgid that names no single group: netd asserts on it at start.
    NoSingleGroup(i32),
}

impl Bad {
    fn line(&self) -> String {
        match self {
            Bad::NotJson(text) => format!("#{text}"),
            Bad::Arity(n) => serde_json::to_string(&vec![5_000_000; *n]).unwrap(),
            Bad::PgidText => r#"["5000000",0,"t","s"]"#.to_owned(),
            Bad::PgidOutOfRange(pgid) => format!(r#"[{pgid},0,"t","s"]"#),
            Bad::AttemptNegative(attempt) => format!(r#"[5000000,{attempt},"t","s"]"#),
            Bad::Blank => String::new(),
            Bad::NoSingleGroup(pgid) => format!(r#"[{pgid},0,"t","s"]"#),
        }
    }
}

/// What a net's state directory holds for it before netd starts.
#[derive(Debug, Clone)]
enum Prior {
    Absent,
    /// What a runner wrote, and maybe the temporary file of a crash between
    /// its write and its rename.
    Written {
        records: Vec<Record>,
        trailing_newline: bool,
        stale_temp: Option<Vec<u8>>,
    },
    BadLine {
        before: Vec<Record>,
        bad: Bad,
    },
    NotUtf8 {
        before: Vec<Record>,
        bytes: Vec<u8>,
    },
    Directory {
        empty: bool,
    },
    /// Not drawn: the file system of the state directory refuses the net's
    /// name (APFS refuses unassigned code points), so no pid file can exist.
    Unwritable,
}

#[derive(Debug, Clone)]
struct NetPlan {
    /// The name is the net's index and this suffix: names that a file system
    /// folds together (case, Unicode normalization) would share one pid file.
    suffix: String,
    prior: Prior,
    /// The net file names the same net as an earlier one.
    same_name_as: Option<usize>,
}

/// Which pid-file access of the held net stalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    /// The read of the pid file: a FIFO at its path that sends the prior's
    /// bytes once released.
    Read,
    /// The write of the temporary file.
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// The held access goes through; a write's rename puts it in place.
    Write,
    /// The rename fails: the pid file path became a non-empty directory.
    FailRename,
    /// The start's future is dropped while the write is held, which then goes
    /// through.
    DropStart,
    /// As `DropStart`, and a new start begins before the held access goes
    /// through.
    RestartWhileHeld,
}

impl End {
    /// A new start follows the dropped one.
    fn restarts(self) -> bool {
        matches!(self, End::DropStart | End::RestartWhileHeld)
    }
}

/// Where the task that must keep running comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Beat {
    Outside,
    /// Spawned by the task that awaits the start, just before it does.
    SameTask,
}

#[derive(Debug, Clone)]
struct HoldPlan {
    net: usize,
    point: Point,
    end: End,
    beat: Beat,
    /// A second start on the same state directory while the write is held.
    second: bool,
}

#[derive(Debug, Clone)]
struct Case {
    flavor: Flavor,
    nets: Vec<NetPlan>,
    hold: Option<HoldPlan>,
}

/// Any characters but `/` and NUL: one plain file name, which is all a net
/// name may be; a name with a path in it is out of this test. Half the
/// characters are printable ASCII, so that most names are ones the file system
/// takes and the nets behind them are reached.
fn suffix() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![any::<char>(), prop::char::range(' ', '~')]
            .prop_filter("one file name", |c| *c != '/' && *c != '\0'),
        0..12,
    )
    .prop_map(String::from_iter)
}

fn name() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..12).prop_map(String::from_iter)
}

fn record() -> impl Strategy<Value = Record> {
    (
        prop_oneof![NO_GROUP.prop_map(Live::Dead), Just(Live::Alive)],
        any::<u64>(),
        name(),
        name(),
    )
}

fn records() -> impl Strategy<Value = Vec<Record>> {
    prop::collection::vec(record(), 0..4)
}

fn bad() -> impl Strategy<Value = Bad> {
    prop_oneof![
        // A line ending ends the line.
        prop::collection::vec(
            any::<char>().prop_filter("one line", |c| *c != '\n' && *c != '\r'),
            0..12
        )
        .prop_map(|c| Bad::NotJson(String::from_iter(c))),
        (0usize..=8)
            .prop_filter("not four fields", |n| *n != 4)
            .prop_map(Bad::Arity),
        Just(Bad::PgidText),
        prop_oneof![
            (i64::from(i32::MAX) + 1..=i64::MAX),
            (i64::MIN..i64::from(i32::MIN))
        ]
        .prop_map(Bad::PgidOutOfRange),
        (i64::MIN..0).prop_map(Bad::AttemptNegative),
        Just(Bad::Blank),
        (i32::MIN..=1).prop_map(Bad::NoSingleGroup),
    ]
}

/// Weighted towards the priors a start reads through, so that later nets and a
/// held write behind them are reached.
fn prior() -> impl Strategy<Value = Prior> {
    prop_oneof![
        2 => Just(Prior::Absent),
        4 => (
            records(),
            any::<bool>(),
            prop::option::of(prop::collection::vec(any::<u8>(), 0..32))
        )
            .prop_map(|(records, trailing_newline, stale_temp)| Prior::Written {
                records,
                trailing_newline,
                stale_temp,
            }),
        1 => (records(), bad()).prop_map(|(before, bad)| Prior::BadLine { before, bad }),
        1 => (records(), prop::collection::vec(any::<u8>(), 0..8)).prop_map(|(before, rest)| {
            // 0xFF starts no UTF-8 sequence.
            let mut bytes = vec![0xFF];
            bytes.extend(rest);
            Prior::NotUtf8 { before, bytes }
        }),
        1 => any::<bool>().prop_map(|empty| Prior::Directory { empty }),
    ]
}

fn nets(same_names: bool) -> impl Strategy<Value = Vec<NetPlan>> {
    prop::collection::vec((suffix(), prior(), any::<prop::sample::Index>()), 1..=3).prop_flat_map(
        move |drawn| {
            let n = drawn.len();
            prop::collection::vec(any::<bool>(), n).prop_map(move |same| {
                drawn
                    .iter()
                    .zip(same)
                    .enumerate()
                    .map(|(i, ((suffix, prior, earlier), same))| NetPlan {
                        suffix: suffix.clone(),
                        prior: prior.clone(),
                        same_name_as: (same_names && same && i > 0).then(|| earlier.index(i)),
                    })
                    .collect()
            })
        },
    )
}

fn end() -> impl Strategy<Value = End> {
    prop_oneof![
        Just(End::Write),
        Just(End::FailRename),
        Just(End::DropStart),
        Just(End::RestartWhileHeld),
    ]
}

fn beat() -> impl Strategy<Value = Beat> {
    prop_oneof![Just(Beat::Outside), Just(Beat::SameTask)]
}

fn held_case() -> impl Strategy<Value = Case> {
    (
        flavor(),
        nets(false),
        any::<prop::sample::Index>(),
        prop_oneof![
            // A read that fails is a prior the unheld case draws.
            prop_oneof![
                Just(End::Write),
                Just(End::DropStart),
                Just(End::RestartWhileHeld)
            ]
            .prop_map(|end| (Point::Read, end)),
            end().prop_map(|end| (Point::Write, end)),
        ],
        beat(),
        any::<bool>(),
    )
        // netd asserts on such a pgid; a start that panics
        // holds no write, and the unheld case draws it.
        .prop_filter("no pgid that names no single group", |(_, nets, ..)| {
            !nets.iter().any(|n| {
                matches!(
                    n.prior,
                    Prior::BadLine {
                        bad: Bad::NoSingleGroup(_),
                        ..
                    }
                )
            })
        })
        .prop_map(|(flavor, mut nets, held, (point, end), beat, second)| {
            let net = held.index(nets.len());
            // The held write's temporary path is the test's FIFO.
            if let (Point::Write, Prior::Written { stale_temp, .. }) = (point, &mut nets[net].prior)
            {
                *stale_temp = None;
            }
            Case {
                flavor,
                nets,
                hold: Some(HoldPlan {
                    net,
                    point,
                    end,
                    beat,
                    second,
                }),
            }
        })
}

fn unheld_case() -> impl Strategy<Value = Case> {
    (flavor(), nets(true)).prop_map(|(flavor, nets)| Case {
        flavor,
        nets,
        hold: None,
    })
}

/// What a start of the case answers.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Want {
    Started,
    Line {
        net: usize,
        line: String,
    },
    Unreadable {
        net: usize,
    },
    /// The read finds no file, and the write of the temporary file fails.
    Unwritable {
        net: usize,
    },
    Duplicate {
        net: usize,
    },
    /// netd asserts on a pgid that names no single group.
    Panic {
        net: usize,
    },
    /// A live start of this process holds the net's pid file.
    InUse {
        net: usize,
    },
}

impl Want {
    fn refused_at(&self) -> Option<usize> {
        match self {
            Want::Line { net, .. }
            | Want::Unreadable { net }
            | Want::Unwritable { net }
            | Want::Duplicate { net }
            | Want::Panic { net }
            | Want::InUse { net } => Some(*net),
            Want::Started => None,
        }
    }
}

/// From which net on the files are as before the start: the nets it never
/// loaded, and the one whose pid file it refused. A pid file the start read
/// and wrote before it ended (the pgid assert, a duplicate name, a failed
/// rename) is not kept.
fn kept_from(case: &Case, want: &Want) -> Option<usize> {
    let refused = want.refused_at()?;
    let read_as_is = matches!(
        want,
        Want::Line { .. } | Want::Unreadable { .. } | Want::Unwritable { .. }
    ) && *want == want_of(case, false);
    Some(if read_as_is { refused } else { refused + 1 })
}

/// The nets load in order and the first that cannot load ends the start.
fn want_of(case: &Case, with_hold: bool) -> Want {
    for (i, net) in case.nets.iter().enumerate() {
        if net.same_name_as.is_some() {
            return Want::Duplicate { net: i };
        }
        match &net.prior {
            Prior::BadLine {
                bad: Bad::NoSingleGroup(_),
                ..
            } => return Want::Panic { net: i },
            Prior::BadLine { bad, .. } => {
                return Want::Line {
                    net: i,
                    line: bad.line(),
                };
            }
            Prior::NotUtf8 { .. } | Prior::Directory { .. } => {
                return Want::Unreadable { net: i };
            }
            Prior::Unwritable => return Want::Unwritable { net: i },
            Prior::Absent | Prior::Written { .. } => {}
        }
        if with_hold
            && case
                .hold
                .as_ref()
                .is_some_and(|h| h.net == i && h.end == End::FailRename)
        {
            return Want::Unreadable { net: i };
        }
    }
    Want::Started
}

/// One net file: a place and a manual transition. A net's file reaches the pid
/// file only through its name.
fn net_file(name: &str) -> String {
    // Every character a YAML double-quoted scalar might fold or refuse raw is
    // escaped, so the name reads back as drawn.
    let quoted: String = name
        .chars()
        .map(|c| match c {
            ' '..='~' if c != '"' && c != '\\' => c.to_string(),
            _ => format!("\\U{:08X}", c as u32),
        })
        .collect();
    format!(
        "name: \"{quoted}\"\nplaces:\n  P:\n    capacity: single\ntransitions:\n  T:\n    trigger: manual\n    priority: 1\n    consumes:\n    - P\n"
    )
}

fn pid_path(state: &Path, name: &str) -> PathBuf {
    state.join(format!("{name}.pid"))
}

fn temp_path(state: &Path, name: &str) -> PathBuf {
    state.join(format!("{name}.pid.writing"))
}

/// A process group the test starts, and ends when dropped.
struct Group(std::process::Child);

impl Group {
    fn start() -> Self {
        Self(
            std::process::Command::new("/bin/sleep")
                .arg("600")
                .process_group(0)
                .spawn()
                .expect("sleep starts"),
        )
    }

    fn pgid(&self) -> i32 {
        self.0.id() as i32
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // SAFETY: the group is the one this test started.
        unsafe { libc::kill(-self.pgid(), libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

/// The groups the priors name.
#[derive(Default)]
struct Leftovers {
    groups: Vec<Group>,
    dead: Vec<i32>,
    /// Per net name, the line of each live group its prior records, in the
    /// form a runner keeps a leftover: a line without the group's identity
    /// keeps `null` for it.
    kept: HashMap<String, Vec<serde_json::Value>>,
}

impl Leftovers {
    fn lines(&mut self, name: &str, records: &[Record]) -> Vec<String> {
        records
            .iter()
            .map(|(live, attempt, transition, subject)| {
                let pgid = match live {
                    Live::Dead(pgid) => {
                        self.dead.push(*pgid);
                        *pgid
                    }
                    Live::Alive => {
                        let group = Group::start();
                        let pgid = group.pgid();
                        self.groups.push(group);
                        self.kept
                            .entry(name.to_owned())
                            .or_default()
                            .push(serde_json::json!([
                                pgid, null, attempt, transition, subject
                            ]));
                        pgid
                    }
                };
                serde_json::to_string(&(pgid, attempt, transition, subject)).unwrap()
            })
            .collect()
    }
}

fn stage(state: &Path, name: &str, prior: &Prior, leftovers: &mut Leftovers) {
    let pid = pid_path(state, name);
    match prior {
        Prior::Absent | Prior::Unwritable => {}
        Prior::Written {
            records,
            trailing_newline,
            stale_temp,
        } => {
            let mut text = leftovers.lines(name, records).join("\n");
            if *trailing_newline && !records.is_empty() {
                text.push('\n');
            }
            fs::write(&pid, text).unwrap();
            if let Some(bytes) = stale_temp {
                fs::write(temp_path(state, name), bytes).unwrap();
            }
        }
        Prior::BadLine { before, bad } => {
            let mut lines = leftovers.lines(name, before);
            lines.push(bad.line());
            fs::write(&pid, lines.join("\n") + "\n").unwrap();
        }
        Prior::NotUtf8 { before, bytes } => {
            let mut content = leftovers.lines(name, before).join("\n").into_bytes();
            if !content.is_empty() {
                content.push(b'\n');
            }
            content.extend(bytes);
            fs::write(&pid, content).unwrap();
        }
        Prior::Directory { empty } => {
            fs::create_dir(&pid).unwrap();
            if !empty {
                fs::write(pid.join("keep"), "").unwrap();
            }
        }
    }
}

/// What a path holds, compared before and after a start.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disk {
    Absent,
    File(Vec<u8>),
    Dir,
    Fifo,
}

fn disk(path: &Path) -> Disk {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Disk::Absent,
        Err(e) => panic!("{path:?} cannot be inspected: {e}"),
        Ok(meta) if meta.is_dir() => Disk::Dir,
        Ok(meta) if meta.file_type().is_fifo() => Disk::Fifo,
        Ok(_) => Disk::File(fs::read(path).unwrap()),
    }
}

static WARNINGS: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct Capture;

impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        WARNINGS.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// netd reports a leftover group by a warning only; every thread's warnings
/// land in one buffer.
fn capture_warnings() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(|| Capture)
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("one global subscriber");
    });
}

fn warned(pgid: i32) -> bool {
    String::from_utf8_lossy(&WARNINGS.lock().unwrap()).contains(&format!("process group {pgid} ("))
}

/// A runtime on a thread of its own.
struct Rt {
    handle: tokio::runtime::Handle,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    driver: Option<std::thread::JoinHandle<()>>,
}

impl Rt {
    fn new(flavor: Flavor) -> Self {
        let runtime = match flavor {
            Flavor::CurrentThread => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
            Flavor::MultiThread(workers) => tokio::runtime::Builder::new_multi_thread()
                .worker_threads(workers)
                .enable_all()
                .build(),
        }
        .expect("a tokio runtime");
        let handle = runtime.handle().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let driver = std::thread::spawn(move || {
            let _ = runtime.block_on(stopped);
            runtime.shutdown_timeout(SETTLE);
        });
        Self {
            handle,
            stop: Some(stop),
            driver: Some(driver),
        }
    }

    fn start(&self, config: Config, beats: Option<Beats>) -> Started {
        let (tx, answer) = mpsc::channel();
        let task = self.handle.spawn(async move {
            if let Some(beats) = beats {
                tokio::spawn(heartbeat(beats));
            }
            let _ = tx.send(holon_netd::start(config).await);
        });
        Started { answer, task }
    }
}

impl Drop for Rt {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(driver) = self.driver.take() {
            driver.join().expect("the runtime driver does not panic");
        }
    }
}

/// A start in flight.
struct Started {
    answer: mpsc::Receiver<Result<Netd, NetdError>>,
    task: tokio::task::JoinHandle<()>,
}

/// How a start ended.
enum Outcome {
    Answered(Result<Netd, NetdError>),
    Panicked,
    Hung,
}

impl Started {
    fn outcome(&self) -> Outcome {
        match self.answer.recv_timeout(SETTLE) {
            Ok(answer) => Outcome::Answered(answer),
            Err(mpsc::RecvTimeoutError::Disconnected) => Outcome::Panicked,
            Err(mpsc::RecvTimeoutError::Timeout) => Outcome::Hung,
        }
    }
}

type Beats = Arc<Mutex<Vec<Instant>>>;

async fn heartbeat(beats: Beats) {
    loop {
        beats.lock().unwrap().push(Instant::now());
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// The longest stretch of `from..to` without a beat.
fn largest_gap(from: Instant, to: Instant, beats: &[Instant]) -> Duration {
    let mut last = from;
    let mut gap = Duration::ZERO;
    for &beat in beats.iter().filter(|b| **b > from && **b <= to) {
        gap = gap.max(beat - last);
        last = beat;
    }
    gap.max(to - last)
}

/// Lets a read of the pid file see the empty file the held write sent, once
/// its rename has put the FIFO at the pid file path.
struct Feeder {
    stop: Arc<AtomicBool>,
    /// A start opened the staged FIFO at the pid-file path and read EOF.
    fed: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Feeder {
    fn new(pid: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let fed = Arc::new(AtomicBool::new(false));
        let feeding = Arc::clone(&fed);
        let path = CString::new(pid.to_str().expect("the temporary path is utf-8")).unwrap();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                if fs::symlink_metadata(&pid).is_ok_and(|m| m.file_type().is_fifo()) {
                    // SAFETY: a valid C path; the descriptor is closed at once.
                    unsafe {
                        let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
                        if fd >= 0 {
                            // A rename after the FIFO check may have put a regular file there.
                            let mut stat = std::mem::zeroed::<libc::stat>();
                            assert_eq!(libc::fstat(fd, &mut stat), 0, "fstat the pid path");
                            if stat.st_mode & libc::S_IFMT == libc::S_IFIFO {
                                feeding.store(true, Ordering::SeqCst);
                            }
                            libc::close(fd);
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            fed,
            thread: Some(thread),
        }
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the feeder does not panic");
        }
    }
}

enum Held {
    Read(ReadHold),
    Write(Hold),
}

/// A FIFO in place of a staged pid file: a read of the pid file stalls until
/// the release, then reads the staged bytes.
struct ReadHold {
    fifo: PathBuf,
    bytes: Vec<u8>,
}

impl ReadHold {
    fn new(pid: &Path) -> Self {
        let bytes = match disk(pid) {
            Disk::Absent => Vec::new(),
            Disk::File(bytes) => {
                fs::remove_file(pid).unwrap();
                bytes
            }
            other => panic!("a held read needs a pid file, found {other:?}"),
        };
        let path = CString::new(pid.to_str().expect("the pid path is utf-8")).unwrap();
        // SAFETY: a valid C path and a mode.
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "mkfifo {pid:?}: {}", io::Error::last_os_error());
        Self {
            fifo: pid.to_owned(),
            bytes,
        }
    }

    /// Sends the bytes to every read of the FIFO until the returned server is
    /// dropped; fails when no read opened it within [`SETTLE`].
    fn release(self) -> Result<Server, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let (served, first) = mpsc::channel();
        let fifo = self.fifo.clone();
        let thread = std::thread::spawn(move || {
            let path = CString::new(fifo.to_str().unwrap()).unwrap();
            while !stopping.load(Ordering::SeqCst)
                && fs::symlink_metadata(&fifo).is_ok_and(|m| m.file_type().is_fifo())
            {
                // SAFETY: a valid C path; the descriptor is closed below.
                let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
                if fd < 0 {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                // A rename after the FIFO check puts the start's own file at
                // the path; the staged bytes must never land in it.
                // SAFETY: the descriptor is open and `stat` is a valid buffer.
                let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
                assert_eq!(
                    unsafe { libc::fstat(fd, &mut stat) },
                    0,
                    "fstat the held pid path"
                );
                if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
                    // SAFETY: the descriptor is open.
                    unsafe { libc::close(fd) };
                    break;
                }
                // SAFETY: the descriptor is open and the buffer is valid.
                let sent = unsafe { libc::write(fd, self.bytes.as_ptr().cast(), self.bytes.len()) };
                let error = io::Error::last_os_error();
                // SAFETY: the descriptor is open.
                unsafe { libc::close(fd) };
                match sent {
                    // The reader closed between the open and the write.
                    -1 if error.raw_os_error() == Some(libc::EPIPE) => {}
                    n => {
                        assert_eq!(
                            n,
                            self.bytes.len() as isize,
                            "a FIFO takes a pid file's bytes at once: {error}"
                        );
                        let _ = served.send(());
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let server = Server {
            stop,
            thread: Some(thread),
        };
        match first.recv_timeout(SETTLE) {
            Ok(()) => Ok(server),
            Err(_) => Err(format!(
                "no pid-file read opened the held pid path {:?} within {SETTLE:?}",
                self.fifo
            )),
        }
    }
}

struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the pid-file server does not panic");
        }
    }
}

struct Lab {
    dir: tempfile::TempDir,
    names: Vec<String>,
    net_files: Vec<PathBuf>,
}

impl Lab {
    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn config(&self) -> Config {
        Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            state_dir: self.state(),
            nets: self.net_files.clone(),
        }
    }

    fn pid(&self, net: usize) -> PathBuf {
        pid_path(&self.state(), &self.names[net])
    }

    fn temp(&self, net: usize) -> PathBuf {
        temp_path(&self.state(), &self.names[net])
    }
}

fn lab(case: &Case) -> Lab {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("state")).unwrap();
    fs::create_dir(dir.path().join("nets")).unwrap();
    let mut names: Vec<String> = Vec::new();
    for (i, net) in case.nets.iter().enumerate() {
        let name = match net.same_name_as {
            Some(earlier) => names[earlier].clone(),
            None => format!("{i}{}", net.suffix),
        };
        names.push(name);
    }
    let net_files = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let path = dir.path().join("nets").join(format!("{i}.yaml"));
            fs::write(&path, net_file(name)).unwrap();
            path
        })
        .collect();
    Lab {
        dir,
        names,
        net_files,
    }
}

/// Whether `outcome` is the one `want` names; `None` when it is.
fn mismatch(lab: &Lab, want: &Want, outcome: &Outcome, which: &str) -> Option<String> {
    let answer = match outcome {
        Outcome::Hung => return Some(format!("{which}: start did not end within {SETTLE:?}")),
        Outcome::Panicked => {
            return (!matches!(want, Want::Panic { .. }))
                .then(|| format!("{which}: start panicked, wanted {want:?}"));
        }
        Outcome::Answered(answer) => answer,
    };
    let matched = match (want, answer) {
        (Want::Started, Ok(_)) => true,
        (
            Want::Line { net, line },
            Err(NetdError::Shell {
                net: named,
                source:
                    ShellError::PidFileLine {
                        path,
                        line: refused,
                    },
            }),
        ) => *named == lab.names[*net] && *path == lab.pid(*net) && refused == line,
        (
            Want::Unreadable { net },
            Err(NetdError::Shell {
                net: named,
                source: ShellError::PidFile { path, .. },
            }),
        ) => *named == lab.names[*net] && *path == lab.pid(*net),
        (
            Want::Unwritable { net },
            Err(NetdError::Shell {
                net: named,
                source: ShellError::PidFile { path, .. },
            }),
        ) => {
            let mut lock = lab.pid(*net).into_os_string();
            lock.push(".lock");
            *named == lab.names[*net] && path.as_os_str() == lock
        }
        (Want::Duplicate { net }, Err(NetdError::DuplicateNet { name, path, .. })) => {
            *name == lab.names[*net] && *path == lab.net_files[*net].display().to_string()
        }
        (Want::InUse { net }, Err(NetdError::Shell { net: named, source })) => {
            let text = source.to_string();
            *named == lab.names[*net]
                && text.contains(&lab.pid(*net).display().to_string())
                && text.contains(&std::process::id().to_string())
        }
        _ => false,
    };
    (!matched).then(|| match answer {
        Ok(_) => format!("{which}: start succeeded, wanted {want:?}"),
        Err(e) => format!("{which}: start answered `{e}` ({e:?}), wanted {want:?}"),
    })
}

/// What the test saw of a held pid-file write: the bytes it drained, and
/// whether a start read the staged FIFO at the pid-file path.
struct WriteSeen<'a> {
    drained: Option<&'a [u8]>,
    fifo_read: bool,
}

/// The pid files after a start that `want` names.
fn check_disk(
    lab: &Lab,
    case: &Case,
    want: &Want,
    before: &[(Disk, Disk)],
    WriteSeen { drained, fifo_read }: WriteSeen<'_>,
    leftovers: &Leftovers,
    failures: &mut Vec<String>,
) {
    // A net that loaded leaves the pid file of a runner with no runs of its
    // own: it keeps the line of every live group the prior recorded.
    let loaded = |net: usize, failures: &mut Vec<String>| {
        let written = match (disk(&lab.pid(net)), drained) {
            (Disk::File(bytes), _) => Some(bytes),
            (Disk::Fifo, Some(bytes)) => Some(bytes.to_vec()),
            _ => None,
        };
        let mut want_lines = leftovers
            .kept
            .get(&lab.names[net])
            .cloned()
            .unwrap_or_default();
        want_lines.sort_by_key(ToString::to_string);
        let parse = |bytes: &[u8]| -> Option<Vec<serde_json::Value>> {
            let mut lines = std::str::from_utf8(bytes)
                .ok()?
                .lines()
                .map(|line| serde_json::from_str(line).ok())
                .collect::<Option<Vec<serde_json::Value>>>()?;
            lines.sort_by_key(ToString::to_string);
            Some(lines)
        };
        // A start that read the staged FIFO at the pid-file path read EOF, as
        // the test drained the held write's bytes: production never stages a
        // FIFO. The held write itself carries every kept line.
        let reread = fifo_read && case.hold.as_ref().is_some_and(|h| h.net == net);
        // The FIFO of a held read sends its bytes again on each open of its
        // write end, so a slow read can take the staged lines several times.
        let read_held = case
            .hold
            .as_ref()
            .is_some_and(|h| h.net == net && h.point == Point::Read);
        let got_lines = written.as_deref().and_then(parse).map(|mut lines| {
            if read_held {
                lines.dedup();
            }
            lines
        });
        let empty_iff_no_line = written
            .as_ref()
            .is_some_and(|bytes| bytes.is_empty() == want_lines.is_empty());
        let as_wanted = match reread {
            false => got_lines.as_ref() == Some(&want_lines) && empty_iff_no_line,
            true => {
                drained.and_then(parse).as_ref() == Some(&want_lines)
                    && got_lines.is_some_and(|got| got.iter().all(|l| want_lines.contains(l)))
            }
        };
        if !as_wanted {
            failures.push(format!(
                "net {net} ({want:?}): the pid file of a runner with no runs of its own holds {:?}, wanted the lines {want_lines:?} (held write sent {drained:?})",
                written.as_ref().map(|b| String::from_utf8_lossy(b).into_owned())
            ));
        }
        let temp = disk(&lab.temp(net));
        if temp != Disk::Absent {
            failures.push(format!(
                "net {net} ({want:?}): the temporary pid file is left as {temp:?}"
            ));
        }
    };
    match want {
        Want::Started => {
            for net in 0..case.nets.len() {
                loaded(net, failures);
            }
        }
        _ => {
            let kept = kept_from(case, want).expect("a refusal names its net");
            for net in 0..want.refused_at().expect("a refusal names its net") {
                loaded(net, failures);
            }
            for (net, (pid, temp)) in before.iter().enumerate().skip(kept) {
                // A repeated name's files are the earlier net's.
                if case.nets[net].same_name_as.is_some() {
                    continue;
                }
                let now = (disk(&lab.pid(net)), disk(&lab.temp(net)));
                if now != (pid.clone(), temp.clone()) {
                    failures.push(format!(
                        "net {net} ({want:?}): its files changed from {:?} to {now:?}",
                        (pid, temp)
                    ));
                }
            }
        }
    }
}

fn check_warnings(want: &Want, leftovers: &Leftovers, failures: &mut Vec<String>) {
    if *want != Want::Started {
        return;
    }
    for group in &leftovers.groups {
        if !warned(group.pgid()) {
            failures.push(format!(
                "no warning names the leftover group {} that still runs",
                group.pgid()
            ));
        }
    }
    for pgid in &leftovers.dead {
        if warned(*pgid) {
            failures.push(format!("a warning names the ended group {pgid}"));
        }
    }
}

/// The case with each net whose name the file system refuses marked so.
fn on_this_file_system(case: &Case, lab: &Lab) -> Case {
    let probe = lab.dir.path().join("probe");
    fs::create_dir(&probe).unwrap();
    let mut case = case.clone();
    for (net, plan) in case.nets.iter_mut().enumerate() {
        if fs::write(pid_path(&probe, &lab.names[net]), "").is_err() {
            plan.prior = Prior::Unwritable;
        }
    }
    case
}

/// Whether the start reaches the held access: every net before it loads, and
/// the file system takes the held net's name. A held read is also reached when
/// the bytes it sends make the start refuse that net.
fn reaches_the_held_access(case: &Case) -> bool {
    let lab = lab(case);
    let case = on_this_file_system(case, &lab);
    let hold = case.hold.as_ref().expect("a held case");
    let read_refuses = hold.point == Point::Read
        && matches!(
            case.nets[hold.net].prior,
            Prior::BadLine { .. } | Prior::NotUtf8 { .. }
        );
    want_of(&case, false)
        .refused_at()
        .is_none_or(|refused| refused > hold.net || (refused == hold.net && read_refuses))
}

fn play(case: &Case) -> Vec<String> {
    capture_warnings();
    let lab = lab(case);
    let case = &on_this_file_system(case, &lab);
    let mut leftovers = Leftovers::default();
    for (net, plan) in case.nets.iter().enumerate() {
        if plan.same_name_as.is_none() {
            stage(&lab.state(), &lab.names[net], &plan.prior, &mut leftovers);
        }
    }
    let hold = case.hold.as_ref().map(|h| {
        let pid = lab.pid(h.net);
        let held = match h.point {
            Point::Read => Held::Read(ReadHold::new(&pid)),
            Point::Write => Held::Write(Hold::new(&pid)),
        };
        (h, held)
    });
    let before: Vec<(Disk, Disk)> = (0..case.nets.len())
        .map(|net| (disk(&lab.pid(net)), disk(&lab.temp(net))))
        .collect();
    let rt = Rt::new(case.flavor);
    let want = want_of(case, true);
    let mut failures = Vec::new();
    let mut outcomes = Vec::new();
    let mut drained_bytes = None;
    let mut fifo_read = false;
    match hold {
        None => {
            let outcome = rt.start(lab.config(), None).outcome();
            if let Some(m) = mismatch(&lab, &want, &outcome, "start") {
                failures.push(m);
            }
            outcomes.push(outcome);
        }
        Some((plan, hold)) => {
            let beats: Beats = Arc::default();
            if plan.beat == Beat::Outside {
                rt.handle.spawn(heartbeat(Arc::clone(&beats)));
                eventually(|| match beats.lock().unwrap().is_empty() {
                    true => Err("the heartbeat has not run".to_owned()),
                    false => Ok(()),
                })
                .expect("an idle runtime runs a task");
            }
            let from = Instant::now();
            let first = rt.start(
                lab.config(),
                (plan.beat == Beat::SameTask).then(|| Arc::clone(&beats)),
            );
            // The second start comes once the first holds the access, so the
            // first holds every pid file up to the held one.
            std::thread::sleep(WINDOW / 2);
            let second = plan.second.then(|| rt.start(lab.config(), None));
            std::thread::sleep(WINDOW / 2);
            let to = Instant::now();
            let gap = largest_gap(from, to, &beats.lock().unwrap());
            if gap > BOUND {
                failures.push(format!(
                    "a task on the runtime waited {gap:?} (bound {BOUND:?}) while netd start held a pid-file {:?}",
                    plan.point
                ));
            }
            if first.task.is_finished() {
                failures.push(format!(
                    "start ended while its pid-file {:?} was held",
                    plan.point
                ));
            }
            if plan.end.restarts() {
                first.task.abort();
            }
            let early = (plan.end == End::RestartWhileHeld).then(|| rt.start(lab.config(), None));
            if plan.end == End::FailRename {
                let pid = lab.pid(plan.net);
                if pid.exists() {
                    fs::remove_file(&pid).unwrap();
                }
                fs::create_dir(&pid).unwrap();
                fs::write(pid.join("keep"), "").unwrap();
            }
            let (drained, _server, _feeder): (Option<Drained>, Option<Server>, Option<Feeder>) =
                match hold {
                    Held::Read(hold) => (
                        None,
                        hold.release()
                            .inspect_err(|e| failures.push(e.clone()))
                            .ok(),
                        None,
                    ),
                    Held::Write(hold) => {
                        let how = match plan.end {
                            End::FailRename => Release::FailRename,
                            End::Write | End::DropStart | End::RestartWhileHeld => Release::Write,
                        };
                        (
                            hold.release(how)
                                .inspect_err(|e| failures.push(e.clone()))
                                .ok(),
                            None,
                            Some(Feeder::new(lab.pid(plan.net))),
                        )
                    }
                };
            let main = if plan.end.restarts() {
                eventually(|| match first.task.is_finished() {
                    true => Ok(()),
                    false => Err("the dropped start's task has not ended".to_owned()),
                })
                .expect("an aborted task ends once its write is released");
                let restart = early.unwrap_or_else(|| rt.start(lab.config(), None));
                ("the start after a dropped one", restart.outcome())
            } else {
                ("start", first.outcome())
            };
            let second = second.map(|s| ("the second start", s.outcome()));
            // A start that succeeds holds its pid files: a second start that
            // waits for them while it lives is refused at the first net.
            let in_use = Want::InUse { net: 0 };
            match (&want, second) {
                (Want::Started, Some(second)) if plan.end.restarts() => {
                    // The restart and the second start both wait for the pid
                    // files the dropped start lets go: one of them gets them.
                    let wins = |o: &(&str, Outcome)| mismatch(&lab, &want, &o.1, o.0).is_none();
                    let loses = |o: &(&str, Outcome)| mismatch(&lab, &in_use, &o.1, o.0).is_none();
                    if !(wins(&main) && loses(&second) || wins(&second) && loses(&main)) {
                        failures.push(format!(
                            "of {} and {}, exactly one must start and the other be refused as {in_use:?}: {:?} and {:?}",
                            main.0,
                            second.0,
                            mismatch(&lab, &want, &main.1, main.0),
                            mismatch(&lab, &want, &second.1, second.0),
                        ));
                    }
                    outcomes.extend([main.1, second.1]);
                }
                (_, second) => {
                    let second_want = match want {
                        Want::Started => in_use.clone(),
                        _ => want.clone(),
                    };
                    failures.extend(mismatch(&lab, &want, &main.1, main.0));
                    outcomes.push(main.1);
                    if let Some((which, outcome)) = second {
                        failures.extend(mismatch(&lab, &second_want, &outcome, which));
                        outcomes.push(outcome);
                    }
                }
            }
            drained_bytes = drained.as_ref().map(|d| d.first.clone());
            fifo_read = _feeder
                .as_ref()
                .is_some_and(|f| f.fed.load(Ordering::SeqCst));
            if let Some(drained) = drained {
                drained.stop();
            }
        }
    }
    {
        let _context = rt.handle.enter();
        drop(outcomes);
    }
    // An aborted start's open runs on in the blocking pool and may write the
    // pid file after every later start returned; the runtime's end waits for it.
    drop(rt);
    check_disk(
        &lab,
        case,
        &want,
        &before,
        WriteSeen {
            drained: drained_bytes.as_deref(),
            fifo_read,
        },
        &leftovers,
        &mut failures,
    );
    check_warnings(&want, &leftovers, &mut failures);
    for group in &mut leftovers.groups {
        if group
            .0
            .try_wait()
            .expect("the test waits on its own group")
            .is_some()
        {
            failures.push(format!(
                "the leftover group {} ended, but no start may signal it ({want:?})",
                group.pgid()
            ));
        }
    }
    failures
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn a_task_on_the_runtime_runs_within_the_bound_while_netd_start_waits_for_a_held_pid_file_write(
        case in held_case()
    ) {
        // A start that refuses before the held net holds nothing; the unheld
        // property draws those starts.
        prop_assume!(reaches_the_held_access(&case), "the start reaches the held access");
        let failures = play(&case);
        prop_assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn netd_start_reads_every_pid_file_a_runner_writes_and_refuses_every_other_by_name(
        case in unheld_case()
    ) {
        let failures = play(&case);
        prop_assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
