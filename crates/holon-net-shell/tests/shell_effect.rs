//! The shell effect against real children: the exit-code map and the two
//! kills.

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
async fn a_group_member_that_ignores_sigterm_is_killed_before_the_run_ends() {
    let dir = tempfile::tempdir().unwrap();
    let runner = runner(dir.path());
    let effect = descriptor(
        &[
            "/bin/bash",
            "-c",
            "/bin/bash -c 'trap \"\" TERM; echo $$ > stubborn.pid; exec /bin/sleep 60' & /bin/sleep 60",
        ],
        "200ms",
        dir.path(),
    );

    let finished = runner.run(&effect, &request(1)).await.unwrap();

    assert_eq!(finished.ended, Ended::Killed(Kill::Timeout));
    let stubborn: Pgid = std::fs::read_to_string(dir.path().join("stubborn.pid"))
        .expect("the member wrote its pid")
        .trim()
        .parse()
        .unwrap();
    until("died with its process group", || unsafe {
        libc::kill(stubborn, 0) != 0
    })
    .await;
}

#[test]
fn runs_on_many_threads_at_once_all_settle_by_their_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let runner = runner(dir.path());
    let effect = descriptor(&["/usr/bin/true"], "10s", dir.path());

    let ended: Vec<_> = std::thread::scope(|s| {
        let threads: Vec<_> = (0..16u64)
            .map(|t| {
                let (runner, effect) = (&runner, &effect);
                s.spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    (0..8u64)
                        .map(|i| rt.block_on(runner.run(effect, &request(t * 8 + i))))
                        .map(|r| r.map(|f| f.ended).map_err(|e| e.to_string()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect()
    });

    let done = Ok(Ended::Exited {
        code: 0,
        outcome: Outcome::Done,
    });
    let wrong: Vec<_> = ended.iter().filter(|e| **e != done).collect();
    assert_eq!(
        wrong,
        Vec::<&Result<Ended, String>>::new(),
        "of {} runs",
        ended.len()
    );
    let left = std::fs::read_to_string(dir.path().join("groups.pid")).unwrap();
    assert_eq!(left, "", "the pid file names no group once every run ended");
}
