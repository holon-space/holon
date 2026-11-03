//! A host process that drives one [`Runner`] from commands on its stdin, so a
//! test can SIGKILL the host at any step and restart it on the same pid file,
//! and the children, generators and process checks the restart tests share.
//!
//! Every child carries a token in its arguments, so a test finds what it
//! started and kills only that.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::ffi::CString;
use std::io::BufRead;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use holon_net::exec::AttemptId;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Subject;
use holon_net::file::ShellEffect;
use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Group;
use holon_net_shell::effect::Kill;
use holon_net_shell::effect::Pgid;
use holon_net_shell::effect::Runner;
use holon_net_shell::effect::ShellError;
use proptest::prelude::*;
use serde_json::Value;
use serde_json::json;

pub const HOST_DIR: &str = "HOLON_RESTART_HOST_DIR";
pub const HOST_FLAVOR: &str = "HOLON_RESTART_HOST_FLAVOR";
const EVENT: &str = "@@ ";

/// The sleep argument every child and member carries: a duration no case
/// outlives, whose fraction is the token.
pub fn sleep_arg(token: &str) -> String {
    format!("86399.{token}")
}

// ---------------------------------------------------------------- processes

/// Every member of the group as `(pid, zombie)`, read at once from the
/// kernel's process table; a member that is exiting is listed as no zombie.
#[cfg(target_os = "macos")]
pub fn group_table(pgid: Pgid) -> Vec<(i32, bool)> {
    // The layout of `struct kinfo_proc` (sys/sysctl.h) on 64-bit macOS.
    const KINFO_PROC: usize = 648;
    const P_STAT: usize = 36;
    const P_PID: usize = 40;
    const SZOMB: u8 = 5;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PGRP, pgid];
    let mut sysctl = |buf: *mut libc::c_void, len: &mut usize| {
        // SAFETY: `buf` is null or holds `len` bytes.
        let read = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, buf, len, std::ptr::null_mut(), 0) };
        (read == 0)
            .then_some(())
            .ok_or_else(std::io::Error::last_os_error)
    };
    let table = loop {
        let mut len = 0;
        sysctl(std::ptr::null_mut(), &mut len)
            .unwrap_or_else(|e| panic!("sizing the members of group {pgid} failed: {e}"));
        len += 8 * KINFO_PROC;
        let mut table = vec![0u8; len];
        match sysctl(table.as_mut_ptr().cast(), &mut len) {
            Ok(()) => {
                table.truncate(len);
                break table;
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOMEM) => continue,
            Err(e) => panic!("listing the members of group {pgid} failed: {e}"),
        }
    };
    let (entries, rest) = table.as_chunks::<KINFO_PROC>();
    assert!(
        rest.is_empty(),
        "the group table of {pgid} holds whole entries"
    );
    entries
        .iter()
        .map(|entry| {
            let pid = i32::from_ne_bytes(entry[P_PID..P_PID + 4].try_into().expect("four bytes"));
            (pid, entry[P_STAT] == SZOMB)
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn group_table(pgid: Pgid) -> Vec<(i32, bool)> {
    let mut members = Vec::new();
    for entry in std::fs::read_dir("/proc").expect("/proc is readable") {
        let entry = entry.expect("a /proc entry is readable");
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let rest = &stat[stat.rfind(')').expect("a stat line names the command") + 2..];
        let fields: Vec<&str> = rest.split(' ').collect();
        if fields[2].parse::<i32>() == Ok(pgid) {
            members.push((pid, fields[0] == "Z"));
        }
    }
    members
}

/// The members of the group that are not zombies: a zombie runs no code, and
/// the dead host's children are reaped by init only later.
pub fn living_members(pgid: Pgid) -> Vec<i32> {
    group_table(pgid)
        .into_iter()
        .filter_map(|(pid, zombie)| (!zombie).then_some(pid))
        .collect()
}

/// Whether the group's leader exited but is not reaped yet.
pub fn leader_is_zombie(pgid: Pgid) -> bool {
    group_table(pgid).contains(&(pgid, true))
}

pub fn group_lives(pgid: Pgid) -> bool {
    !living_members(pgid).is_empty()
}

/// `(pid, pgid)` of every living process whose arguments carry `token`.
pub fn token_processes(token: &str) -> Vec<(i32, i32)> {
    let mark = sleep_arg(token);
    let out = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,pgid=,stat=,args="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.contains(&mark))
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let pgid = it.next()?.parse().ok()?;
            let stat = it.next()?;
            (!stat.starts_with('Z')).then_some((pid, pgid))
        })
        .collect()
}

/// SIGKILLs every group that carries `token` until none is left for a quiet
/// window after its last kill; `Err` when one survives `limit`. A fork in
/// flight escapes the killpg and shows only after it.
pub fn kill_token_groups(token: &str, limit: Duration) -> Result<(), String> {
    const QUIET: Duration = Duration::from_millis(500);
    let deadline = Instant::now() + limit;
    let mut quiet_since = Instant::now() - QUIET;
    loop {
        let left = token_processes(token);
        if left.is_empty() {
            if quiet_since.elapsed() >= QUIET {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        quiet_since = Instant::now();
        if Instant::now() >= deadline {
            return Err(format!(
                "processes of this case outlive their kill: {left:?}"
            ));
        }
        for (_, pgid) in left {
            assert!(pgid > 1, "a token process names the group {pgid}");
            // SAFETY: the group carries this case's token, so the test started it.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn mkfifo(path: &Path) {
    let c = CString::new(path.to_str().expect("a temporary path is utf-8")).expect("no NUL");
    // SAFETY: `c` is a NUL-terminated path.
    let made = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
    assert_eq!(
        made,
        0,
        "mkfifo {} failed: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

/// Opens the write end of the child's FIFO and closes it, so its `read`
/// ends. `false` when no reader appeared within `limit`.
pub fn release_fifo(path: &Path, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(f) => {
                drop(f);
                return true;
            }
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {}
            Err(e) => panic!("opening the FIFO {} failed: {e}", path.display()),
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The read end of a FIFO, so a writer's open returns and its short write
/// goes into the pipe.
pub fn open_fifo_reader(path: &Path) -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .expect("the FIFO opens for reading")
}

pub fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A process group that is no group of any runner: a sleep in its own group,
/// or a sleep whose leader started it and exited, as a daemon leaves its
/// group.
pub struct Foreign {
    /// `None` once the leader exited and the test reaped it.
    child: Option<Child>,
    pub pgid: Pgid,
}

impl Foreign {
    pub fn start(token: &str) -> Self {
        let child = std::process::Command::new("/bin/sleep")
            .arg(sleep_arg(token))
            .process_group(0)
            .stdin(Stdio::null())
            .spawn()
            .expect("sleep starts");
        let pgid = child.id() as Pgid;
        Self {
            child: Some(child),
            pgid,
        }
    }

    pub fn leaderless(token: &str) -> Self {
        let mut leader = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("/bin/sleep {} & exit 0", sleep_arg(token))])
            .process_group(0)
            .stdin(Stdio::null())
            .spawn()
            .expect("the leader starts");
        let pgid = leader.id() as Pgid;
        assert!(
            leader
                .wait()
                .expect("the leader can be waited for")
                .success(),
            "the leader of the foreign group {pgid} exits 0"
        );
        assert!(group_lives(pgid), "the member outlives its leader {pgid}");
        Self { child: None, pgid }
    }

    pub fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => child
                .try_wait()
                .expect("the foreign sleep can be waited for")
                .is_none(),
            None => group_lives(self.pgid),
        }
    }
}

impl Drop for Foreign {
    fn drop(&mut self) {
        match &mut self.child {
            Some(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            // Only while a member lives is the pgid still this group's.
            None if group_lives(self.pgid) => {
                // SAFETY: the group is one this test started.
                unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            }
            None => {}
        }
    }
}

// ------------------------------------------------------------------ children

/// What a member the leader starts does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Member {
    None,
    Sleeper,
    /// Ignores SIGTERM, so only SIGKILL ends it.
    TermIgnoring,
    /// The leader forks this many sleeping members one after the other, so a
    /// signal to the group can arrive while it forks.
    Forks(u16),
    /// A background member forks this many sleeping members one after the
    /// other, so it may still fork when the leader exits or a signal arrives.
    ForksInBackground(u16),
    /// A member that ignores SIGTERM forks a child that exits at once and
    /// never reaps it, so the group holds a zombie; with `leaves`, the parent
    /// then moves to a group of its own, and the group can end up holding
    /// zombies only.
    Unreaped {
        leaves: bool,
    },
}

/// What the leader does once its members run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leader {
    /// Blocks on its FIFO until the test releases it.
    Reads,
    /// Execs a setuid-root program that runs until killed: its effective user
    /// is not the runner's.
    Setuid,
}

/// One child: a leader that blocks on its FIFO until the test releases it and
/// then exits with `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildPlan {
    pub trap_term: bool,
    pub member: Member,
    pub leader: Leader,
    /// The descriptor's timeout; `None` is one no case reaches.
    pub timeout_ms: Option<u64>,
    pub code: i32,
    pub transition: String,
    pub subject: String,
}

/// Any characters, NUL and control characters included: names reach only the
/// pid file and the request, since the log pattern names the attempt alone.
pub fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => any::<String>(),
        1 => Just(String::new()),
        1 => "[a-z]{1,3}",
    ]
}

pub fn member() -> impl Strategy<Value = Member> {
    prop_oneof![
        4 => Just(Member::None),
        2 => Just(Member::Sleeper),
        1 => Just(Member::TermIgnoring),
        1 => (1u16..=40).prop_map(Member::Forks),
        1 => (1u16..=40).prop_map(Member::ForksInBackground),
        1 => any::<bool>().prop_map(|leaves| Member::Unreaped { leaves }),
    ]
}

pub fn leader() -> impl Strategy<Value = Leader> {
    prop_oneof![5 => Just(Leader::Reads), 1 => Just(Leader::Setuid)]
}

/// The `DurationSpec` of a descriptor takes whole milliseconds and refuses
/// zero.
pub fn timeout_ms() -> impl Strategy<Value = Option<u64>> {
    prop_oneof![5 => Just(None), 1 => (150u64..=1500).prop_map(Some)]
}

/// An exit status holds 8 bits: `0` is done, `3` failed, the rest unknown.
pub fn code() -> impl Strategy<Value = i32> {
    prop_oneof![Just(0), Just(3), 1..=255]
}

pub fn child_plan() -> impl Strategy<Value = ChildPlan> {
    (
        prop::bool::weighted(0.2),
        member(),
        leader(),
        timeout_ms(),
        code(),
        name(),
        name(),
    )
        .prop_map(
            |(trap_term, member, leader, timeout_ms, code, transition, subject)| ChildPlan {
                trap_term,
                member,
                leader,
                timeout_ms,
                code,
                transition,
                subject,
            },
        )
}

/// A child whose shell keeps forking members for a while.
pub fn forking_plan() -> impl Strategy<Value = ChildPlan> {
    (child_plan(), 200u16..=300).prop_map(|(plan, n)| ChildPlan {
        member: Member::Forks(n),
        ..plan
    })
}

/// The script of `plan`; `$0` is the child's control path: it writes its pgid
/// to `$0.pgid` and, with `Leader::Reads`, blocks on the FIFO `$0.fifo`.
pub fn script(plan: &ChildPlan, token: &str) -> String {
    let sleep = format!("/bin/sleep {}", sleep_arg(token));
    let trap = if plan.trap_term { "trap '' TERM; " } else { "" };
    let member = match plan.member {
        Member::None => String::new(),
        Member::Sleeper => format!("{sleep} & "),
        Member::TermIgnoring => format!("(trap '' TERM; exec {sleep}) & "),
        Member::Forks(n) => {
            format!("i=0; while [ $i -lt {n} ]; do {sleep} & i=$((i+1)); done; ")
        }
        Member::ForksInBackground(n) => {
            format!("(i=0; while [ $i -lt {n} ]; do {sleep} & i=$((i+1)); done; wait) & ")
        }
        Member::Unreaped { leaves } => format!(
            "/usr/bin/perl -e '$SIG{{TERM}}=\"IGNORE\"; fork or exit; {leave}sleep 86399' {mark} & ",
            leave = if leaves { "setpgrp; " } else { "" },
            mark = sleep_arg(token),
        ),
    };
    let end = match plan.leader {
        Leader::Reads => format!("read x < \"$0.fifo\"; exit {}", plan.code),
        // The sleeper carries the token once the leader no longer does.
        Leader::Setuid => format!("{sleep} & exec /usr/bin/top -l 0 -s 1 >/dev/null 2>&1"),
    };
    format!(
        "{trap}echo $$ > \"$0.pgid.w\" && mv \"$0.pgid.w\" \"$0.pgid\"; : {mark}; {member}{end}",
        mark = sleep_arg(token),
    )
}

pub fn descriptor(
    script: &str,
    ctl: &Path,
    working_dir: &Path,
    timeout_ms: Option<u64>,
) -> ShellEffect {
    let timeout = match timeout_ms {
        Some(ms) => format!("{ms}ms"),
        None => "3600s".to_owned(),
    };
    serde_json::from_value(json!({
        "command": {
            "argv": ["/bin/sh", "-c", script, ctl.to_str().expect("a temporary path is utf-8")],
            "working_dir": working_dir.to_str().expect("a temporary path is utf-8"),
            "env_allow": ["PATH"],
            "timeout": timeout,
        },
        "log_path": "logs/{attempt}.log",
        "exit_codes": { "done": [0], "failed": [3], "other": "outcome_unknown" },
    }))
    .expect("the descriptor is a valid shell effect")
}

pub fn request(attempt: u64, transition: &str, subject: &str) -> EffectRequest {
    EffectRequest {
        attempt: AttemptId(attempt),
        occurrence: OccurrenceId::caller("restart"),
        transition: Arc::from(transition),
        subject: Subject::new(subject),
        input: Content::new(),
        probe: false,
    }
}

// --------------------------------------------------------------- the host

pub fn group_json(g: &Group) -> Value {
    json!({
        "pgid": g.pgid,
        "attempt": g.attempt.0,
        "transition": g.transition,
        "subject": g.subject,
        "verified": g.started.is_some(),
    })
}

fn emit(event: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{EVENT}{event}").expect("the host writes its event");
    out.flush().expect("the host flushes its event");
}

fn ended_json(result: &Result<holon_net_shell::effect::Finished, ShellError>) -> Value {
    match result {
        Ok(f) => match &f.ended {
            Ended::Exited { code, .. } => json!({"kind": "exited", "code": code}),
            Ended::Killed(Kill::Abandoned) => json!({"kind": "abandoned"}),
            Ended::Killed(Kill::Timeout) => json!({"kind": "timeout"}),
            Ended::Signalled(s) => json!({"kind": "signalled", "signal": s}),
        },
        Err(ShellError::PidFile { .. }) => {
            json!({"kind": "pid_file", "error": result.as_ref().err().map(ToString::to_string)})
        }
        Err(ShellError::ShutDown { work }) => json!({"kind": "shut_down", "work": work}),
        Err(ShellError::Wait { .. }) => {
            json!({"kind": "wait", "error": result.as_ref().err().map(ToString::to_string)})
        }
        Err(ShellError::Spawn { .. }) => {
            json!({"kind": "spawn", "error": result.as_ref().err().map(ToString::to_string)})
        }
        Err(e) => json!({"kind": "error", "error": e.to_string()}),
    }
}

pub struct ByPgid;
pub struct ByGroup;

/// `kill_group`, whether it takes the bare pgid or the recorded group.
pub trait KillGroup<M> {
    fn kill(&self, group: &Group) -> impl Future<Output = Result<(), String>>;
}

impl<F: AsyncFn(Pgid)> KillGroup<ByPgid> for F {
    async fn kill(&self, group: &Group) -> Result<(), String> {
        self(group.pgid).await;
        Ok(())
    }
}

impl<F, E: std::fmt::Display> KillGroup<ByGroup> for F
where
    F: AsyncFn(&Group) -> Result<(), E>,
{
    async fn kill(&self, group: &Group) -> Result<(), String> {
        self(group).await.map_err(|e| e.to_string())
    }
}

pub async fn kill_by<M>(f: impl KillGroup<M>, group: &Group) -> Result<(), String> {
    f.kill(group).await
}

fn host_runtime(multi_thread: bool) -> tokio::runtime::Runtime {
    if multi_thread {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }
    .expect("the host runtime starts")
}

/// The host's command loop; runs until its stdin closes or it is killed.
/// `reboot` shuts the runtime down, which drops every run, and goes on with
/// a new runtime and no runner in the same process.
pub fn host_main(dir: PathBuf, multi_thread: bool) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let line = line.expect("the host reads its stdin");
            tx.send(serde_json::from_str(&line).expect("a command is JSON"))
                .expect("the host loop runs");
        }
    });
    let mut flavor = Some(multi_thread);
    while let Some(multi_thread) = flavor {
        let rt = host_runtime(multi_thread);
        flavor = rt.block_on(serve(&dir, &mut rx));
        rt.shutdown_timeout(Duration::from_millis(500));
        if flavor.is_some() {
            emit(json!({"ev": "rebooted"}));
        }
    }
}

/// Serves commands until stdin closes (`None`) or a `reboot` names the next
/// runtime's flavor.
async fn serve(dir: &Path, rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Option<bool> {
    let mut runner: Option<Arc<Runner>> = None;
    let mut left: Vec<Group> = Vec::new();
    let mut runs: BTreeMap<u64, tokio::task::JoinHandle<()>> = BTreeMap::new();
    while let Some(cmd) = rx.recv().await {
        let op = cmd["op"].as_str().expect("a command names its op");
        match op {
            "open" => match Runner::open(dir.join("pids/groups.pid")) {
                Ok((r, groups)) => {
                    runner = Some(Arc::new(r));
                    emit(
                        json!({"ev": "opened", "groups": groups.iter().map(group_json).collect::<Vec<_>>()}),
                    );
                    left = groups;
                }
                Err(e) => emit(
                    json!({"ev": "open_failed", "line": matches!(e, ShellError::PidFileLine { .. }), "error": e.to_string()}),
                ),
            },
            "run" => {
                let runner = Arc::clone(runner.as_ref().expect("a run follows the open"));
                let id = cmd["id"].as_u64().expect("a run has an id");
                let ctl = dir.join("ctl").join(id.to_string());
                let effect = descriptor(
                    cmd["script"].as_str().expect("a script"),
                    &ctl,
                    &dir.join("work"),
                    cmd["timeout_ms"].as_u64(),
                );
                let req = request(
                    cmd["attempt"].as_u64().expect("an attempt"),
                    cmd["transition"].as_str().expect("a transition"),
                    cmd["subject"].as_str().expect("a subject"),
                );
                runs.insert(id, tokio::spawn(async move {
                        let result = runner.run(&effect, &req).await;
                        // Read at once: a group that dies only after the return is a finding.
                        let living = std::fs::read_to_string(ctl.with_extension("pgid"))
                            .ok()
                            .and_then(|t| t.trim().parse::<Pgid>().ok())
                            .map(living_members)
                            .unwrap_or_default();
                        emit(json!({"ev": "ended", "id": id, "result": ended_json(&result), "living": living}));
                    }));
            }
            "abandon" => {
                let runner = runner.as_ref().expect("an abandon follows the open");
                let ok = runner.abandon(AttemptId(cmd["attempt"].as_u64().expect("an attempt")));
                emit(json!({"ev": "abandon", "ok": ok}));
            }
            "in_flight" => {
                let runner = runner.as_ref().expect("in_flight follows the open");
                emit(
                    json!({"ev": "in_flight", "groups": runner.in_flight().iter().map(group_json).collect::<Vec<_>>()}),
                );
            }
            "kill" => {
                let index = cmd["index"].as_u64().expect("a kill names a leftover") as usize;
                let pgid = left[index].pgid;
                let result = kill_by(holon_net_shell::effect::kill_group, &left[index]).await;
                // Read at once: kill_group returns only when the group is dead.
                let living = living_members(pgid);
                emit(
                    json!({"ev": "killed", "index": index, "pgid": pgid, "living": living, "error": result.err()}),
                );
            }
            "drop" => {
                let id = cmd["id"].as_u64().expect("a drop names a run");
                let task = runs.remove(&id).expect("a drop names a started run");
                task.abort();
                let _ = task.await;
                emit(json!({"ev": "dropped", "id": id}));
            }
            "reap" => {
                // Blocks this task in `waitpid`: on a current-thread runtime the
                // runner cannot reap the leader first.
                let id = cmd["id"].as_u64().expect("a reap names a run");
                let pid: Pgid = std::fs::read_to_string(dir.join("ctl").join(format!("{id}.pgid")))
                    .expect("a reaped run spawned its child")
                    .trim()
                    .parse()
                    .expect("the marker holds a pid");
                emit(json!({"ev": "reaping", "id": id}));
                let mut status = 0;
                // SAFETY: `pid` leads a group this host's runner spawned.
                let reaped = unsafe { libc::waitpid(pid, &mut status, 0) };
                emit(json!({"ev": "reaped", "id": id, "reaped": reaped == pid}));
            }
            "open_second" => match Runner::open(dir.join("pids/groups.pid")) {
                Ok((second, groups)) => {
                    drop(second);
                    emit(
                        json!({"ev": "second", "ok": true, "groups": groups.iter().map(group_json).collect::<Vec<_>>()}),
                    );
                }
                Err(e) => emit(json!({"ev": "second", "ok": false, "error": e.to_string()})),
            },
            "ping" => emit(json!({"ev": "pong"})),
            "reboot" => return Some(cmd["multi"].as_bool().expect("a reboot names the flavor")),
            other => panic!("unknown host command {other}"),
        }
    }
    None
}

/// One host process, as the test sees it.
pub struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::Receiver<Value>,
    backlog: VecDeque<Value>,
    pub pid: i32,
    stderr: PathBuf,
}

impl Host {
    /// Starts the test binary as a host on `dir` in its own process group;
    /// `test_name` is the test function that calls [`host_main`].
    pub fn start(test_name: &str, dir: &Path, multi_thread: bool, generation: u32) -> Self {
        let stderr = dir.join(format!("host-{generation}.stderr"));
        let mut child = std::process::Command::new(
            std::env::current_exe().expect("the test binary has a path"),
        )
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(HOST_DIR, dir)
        .env(HOST_FLAVOR, if multi_thread { "multi" } else { "current" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&stderr).expect("the host's stderr file"))
        .process_group(0)
        .spawn()
        .expect("the host starts");
        let pid = child.id() as i32;
        let stdout = child.stdout.take().expect("the host's stdout is piped");
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                // The harness prints the test's name on the line of the first event.
                if let Some((_, event)) = line.split_once(EVENT)
                    && tx
                        .send(serde_json::from_str(event).expect("an event is JSON"))
                        .is_err()
                {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            events,
            backlog: VecDeque::new(),
            pid,
            stderr,
        }
    }

    fn died(&mut self) -> String {
        let status = self.child.try_wait().ok().flatten();
        let text = std::fs::read_to_string(&self.stderr).unwrap_or_default();
        let tail: String = text
            .lines()
            .rev()
            .take(15)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "the host {} ended ({status:?}); its stderr ends with:\n{tail}",
            self.pid
        )
    }

    pub fn send(&mut self, cmd: Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().expect("the host's stdin is open");
        match writeln!(stdin, "{cmd}").and_then(|()| stdin.flush()) {
            Ok(()) => Ok(()),
            Err(_) => Err(self.died()),
        }
    }

    /// The first event `pick` accepts; the others stay queued.
    pub fn expect(
        &mut self,
        limit: Duration,
        what: &str,
        pick: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        if let Some(i) = self.backlog.iter().position(&pick) {
            return Ok(self.backlog.remove(i).expect("the index is in the backlog"));
        }
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) if pick(&event) => return Ok(event),
                Ok(event) => self.backlog.push_back(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "no {what} from the host within {limit:?}; queued: {:?}",
                        self.backlog
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // The reader may still hold a last event of a host that died.
                    std::thread::sleep(Duration::from_millis(50));
                    return Err(format!("no {what}: {}", self.died()));
                }
            }
        }
    }

    pub fn call(&mut self, cmd: Value, ev: &str, limit: Duration) -> Result<Value, String> {
        self.send(cmd)?;
        self.expect(limit, ev, |e| e["ev"] == ev)
    }

    /// The events that arrived and nobody took yet.
    pub fn drain(&mut self) -> Vec<Value> {
        while let Ok(event) = self.events.try_recv() {
            self.backlog.push_back(event);
        }
        self.backlog.drain(..).collect()
    }

    /// SIGKILL, so the runner cleans up nothing.
    pub fn kill(&mut self) {
        // SAFETY: the pid is this test's own child, not yet waited for.
        unsafe { libc::kill(self.pid, libc::SIGKILL) };
        let _ = self.child.wait();
    }

    pub fn alive(&mut self) -> bool {
        self.child
            .try_wait()
            .expect("the host can be waited for")
            .is_none()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The pgid a pid-file line names, for the line shapes a runner writes: a
/// JSON array that starts with it, or an object with a `pgid` field.
pub fn line_pgid(line: &str) -> Option<Pgid> {
    let v: Value = serde_json::from_str(line).ok()?;
    match &v {
        Value::Array(a) => a.first()?.as_i64().map(|p| p as Pgid),
        Value::Object(o) => o.get("pgid")?.as_i64().map(|p| p as Pgid),
        _ => None,
    }
}

/// Whether a pid-file line names its group's start.
pub fn line_names_start(line: &str) -> bool {
    let v: Value = serde_json::from_str(line).expect("a pid-file line is JSON");
    match &v {
        Value::Array(a) if a.len() == 5 => !a[1].is_null(),
        Value::Array(a) if a.len() == 4 => false,
        other => panic!("a pid-file line of an unknown shape: {other}"),
    }
}

/// The same line naming `pgid` instead: what the pid file of a dead runner
/// says once the system gave the pgid of its dead group to another group.
pub fn with_pgid(line: &str, pgid: Pgid) -> String {
    let mut v: Value = serde_json::from_str(line).expect("a pid-file line is JSON");
    match &mut v {
        Value::Array(a) => a[0] = json!(pgid),
        Value::Object(o) => {
            o.insert("pgid".into(), json!(pgid));
        }
        other => panic!("a pid-file line of an unknown shape: {other}"),
    }
    v.to_string()
}

/// A line in the shape `Runner` wrote it before groups carried an identity:
/// `[pgid, attempt, transition, subject]`.
pub fn bare_line(pgid: Pgid, attempt: u64, transition: &str, subject: &str) -> String {
    serde_json::to_string(&(pgid, attempt, transition, subject)).expect("a line is serializable")
}

/// Removes the directory even when a case left it read-only.
pub fn make_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

/// The pgid of a group whose leader belongs to another user: no process of
/// this user can hold it, and the test cannot signal it.
pub fn alien_leader() -> Option<Pgid> {
    let me = std::env::var("USER").expect("the test knows its user");
    let out = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,pgid=,user="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| {
        let mut f = l.split_whitespace();
        let pid: Pgid = f.next()?.parse().ok()?;
        let pgid: Pgid = f.next()?.parse().ok()?;
        let user = f.next()?;
        (pid == pgid && pid > 1 && user != me).then_some(pgid)
    })
}

/// Passes a pid-file write that a FIFO at `temp` holds, `after` the call: it
/// takes what the writer writes and, once the writer moved the FIFO onto
/// `target`, puts a regular file with those bytes there. A FIFO no writer
/// moved within two seconds is removed.
pub fn pass_held_write(
    temp: PathBuf,
    target: PathBuf,
    after: Duration,
) -> std::thread::JoinHandle<()> {
    use std::io::Read;
    use std::os::unix::fs::FileTypeExt;
    std::thread::spawn(move || {
        std::thread::sleep(after);
        let is_fifo =
            |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_fifo());
        let mut reader = open_fifo_reader(&temp);
        let mut bytes = Vec::new();
        let mut take = |reader: &mut std::fs::File| {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => bytes.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                    Err(e) => panic!("the test reads the held write: {e}"),
                }
            }
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !is_fifo(&target) && Instant::now() < deadline {
            take(&mut reader);
            std::thread::sleep(Duration::from_millis(5));
        }
        take(&mut reader);
        drop(reader);
        if is_fifo(&target) {
            std::fs::remove_file(&target).expect("the test removes the moved FIFO");
            std::fs::write(&target, &bytes).expect("the test writes the passed pid file");
        }
        let _ = std::fs::remove_file(&temp);
    })
}
