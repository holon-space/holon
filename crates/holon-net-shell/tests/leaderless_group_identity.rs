//! A group's identity across a restart when the group's leader is gone.

use std::os::unix::process::CommandExt;

use holon_net_shell::effect::Pgid;
use holon_net_shell::effect::Runner;
use holon_net_shell::effect::ShellError;
use holon_net_shell::effect::kill_group;

fn alive(pgid: Pgid) -> bool {
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// Kills a group this test started, also when an assertion fails.
struct Own(Pgid);

impl Drop for Own {
    fn drop(&mut self) {
        if alive(self.0) {
            unsafe { libc::kill(-self.0, libc::SIGKILL) };
        }
    }
}

/// A group in which only a member runs: its leader started the member and
/// exited, as a daemon that forks and lets its parent exit leaves it.
fn leaderless_group() -> Own {
    let mut leader = std::process::Command::new("/bin/sh")
        .args(["-c", "/bin/sleep 59.317 & exit 0"])
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = leader.id() as Pgid;
    let own = Own(pgid);
    assert!(leader.wait().unwrap().success());
    assert!(alive(pgid), "the member outlives its leader");
    own
}

#[tokio::test]
async fn kill_group_spares_a_foreign_group_whose_leader_exited() {
    let dir = tempfile::tempdir().unwrap();
    let foreign = leaderless_group();
    let pid_file = dir.path().join("groups.pid");
    // A dead group of this runner whose pgid the foreign group took: its
    // leader's start can never equal the start of a process that runs now.
    std::fs::write(
        &pid_file,
        format!("[{}, 1, 1, \"Run\", \"a\"]\n", foreign.0),
    )
    .unwrap();

    let (_runner, left) = Runner::open(pid_file).unwrap();
    for group in &left {
        match (group.pgid == foreign.0, kill_group(group).await) {
            (true, Err(ShellError::Unverified { pgid })) if pgid == foreign.0 => {}
            (false, Ok(())) => {}
            (_, outcome) => panic!("kill_group of {group:?} gave {outcome:?}"),
        }
    }

    assert!(
        alive(foreign.0),
        "the foreign group {} is alive after the restart; open reported {left:?}",
        foreign.0
    );
}
