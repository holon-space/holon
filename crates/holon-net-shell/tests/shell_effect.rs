//! The shell effect against real children: the exit-code map, the two kills
//! and what a runner that died without reaping leaves behind.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_net::exec::AttemptId;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Subject;
use holon_net::file::Outcome;
use holon_net::file::ShellEffect;
use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Kill;
use holon_net_shell::effect::Pgid;
use holon_net_shell::effect::Runner;
use holon_net_shell::effect::kill_group;

/// `3` is the one failing code, so every other non-zero code is unknown.
fn descriptor(argv: &[&str], timeout: &str, working_dir: &Path) -> ShellEffect {
    serde_json::from_value(serde_json::json!({
        "command": {
            "argv": argv,
            "working_dir": working_dir.to_str().expect("the temporary path is utf-8"),
            "env_allow": ["PATH"],
            "timeout": timeout,
        },
        "log_path": "{transition}-{attempt}.log",
        "exit_codes": { "done": [0], "failed": [3], "other": "outcome_unknown" },
    }))
    .expect("the descriptor is a valid shell effect")
}

fn request(attempt: u64) -> EffectRequest {
    EffectRequest {
        attempt: AttemptId(attempt),
        occurrence: OccurrenceId::caller("op-1"),
        transition: Arc::from("Run"),
        subject: Subject::new("a"),
        input: Content::new(),
        probe: false,
    }
}

fn runner(dir: &Path) -> Runner {
    let (runner, left) = Runner::open(dir.join("groups.pid")).expect("the pid file is writable");
    assert_eq!(left, Vec::new(), "a fresh pid file names no group");
    runner
}

fn alive(pgid: Pgid) -> bool {
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// Waits for `f`, which a child process makes true.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "the child never {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_zero_exit_is_done_and_the_log_holds_both_streams() {
    let dir = tempfile::tempdir().unwrap();
    let runner = runner(dir.path());
    let effect = descriptor(
        &["/bin/bash", "-c", "echo out; echo err >&2"],
        "10s",
        dir.path(),
    );

    let finished = runner.run(&effect, &request(1)).await.unwrap();

    assert_eq!(
        finished.ended,
        Ended::Exited {
            code: 0,
            outcome: Outcome::Done
        }
    );
    assert_eq!(finished.log_path, dir.path().join("Run-1.log"));
    let log = std::fs::read_to_string(&finished.log_path).unwrap();
    assert!(log.contains("out") && log.contains("err"), "log: {log}");
}

#[tokio::test]
async fn an_exit_code_settles_the_outcome_the_descriptor_maps_it_to() {
    let dir = tempfile::tempdir().unwrap();
    let runner = runner(dir.path());

    let failed = runner
        .run(
            &descriptor(&["/bin/bash", "-c", "exit 3"], "10s", dir.path()),
            &request(1),
        )
        .await
        .unwrap();
    let unmapped = runner
        .run(
            &descriptor(&["/bin/bash", "-c", "exit 7"], "10s", dir.path()),
            &request(2),
        )
        .await
        .unwrap();

    assert_eq!(
        failed.ended,
        Ended::Exited {
            code: 3,
            outcome: Outcome::Failed
        }
    );
    assert_eq!(
        unmapped.ended,
        Ended::Exited {
            code: 7,
            outcome: Outcome::OutcomeUnknown
        }
    );
}

#[tokio::test]
async fn the_timeout_kills_the_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let runner = runner(dir.path());
    let effect = descriptor(
        &[
            "/bin/bash",
            "-c",
            "/bin/sleep 60 & echo $! > grandchild.pid; wait",
        ],
        "200ms",
        dir.path(),
    );

    let finished = runner.run(&effect, &request(1)).await.unwrap();

    assert_eq!(finished.ended, Ended::Killed(Kill::Timeout));
    let grandchild: Pgid = std::fs::read_to_string(dir.path().join("grandchild.pid"))
        .expect("the child wrote its own child's pid")
        .trim()
        .parse()
        .unwrap();
    until("died with its process group", || unsafe {
        libc::kill(grandchild, 0) != 0
    })
    .await;
}

#[tokio::test]
async fn an_abandoned_attempt_is_killed_with_its_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Arc::new(runner(dir.path()));
    let effect = descriptor(&["/bin/sleep", "60"], "10s", dir.path());
    let run = {
        let runner = Arc::clone(&runner);
        tokio::spawn(async move { runner.run(&effect, &request(1)).await })
    };

    until("started", || !runner.in_flight().is_empty()).await;
    let pgid = runner.in_flight()[0].pgid;
    assert!(runner.abandon(AttemptId(1)));

    let finished = run.await.unwrap().unwrap();
    assert_eq!(finished.ended, Ended::Killed(Kill::Abandoned));
    assert!(!alive(pgid), "the abandoned group is gone");
    assert!(runner.in_flight().is_empty(), "the pid file is cleared");
}

#[tokio::test]
async fn a_restarted_runner_reports_the_group_a_dead_runner_left() {
    let dir = tempfile::tempdir().unwrap();
    let dead = Arc::new(runner(dir.path()));
    let effect = descriptor(&["/bin/sleep", "60"], "10s", dir.path());
    let run = {
        let dead = Arc::clone(&dead);
        tokio::spawn(async move { dead.run(&effect, &request(7)).await })
    };
    until("started", || !dead.in_flight().is_empty()).await;

    // The runner is gone without reaping: its child keeps running.
    run.abort();
    let _ = run.await;
    drop(dead);

    let (_restarted, left) =
        Runner::open(dir.path().join("groups.pid")).expect("the pid file is readable");

    assert_eq!(left.len(), 1, "the leftover group is reported: {left:?}");
    assert_eq!(left[0].attempt, AttemptId(7));
    assert_eq!(left[0].transition, "Run");
    assert_eq!(left[0].subject, "a");
    assert!(alive(left[0].pgid), "the leftover group is still running");

    kill_group(left[0].pgid).await;
    until("was killed", || !alive(left[0].pgid)).await;
}
