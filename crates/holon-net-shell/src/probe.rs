//! One run of a probe's command (`docs/Architecture/NetFileFormat.md`): its
//! stdout, or why it gave none. The run is its own process group, so a
//! timeout ends what the command started as well.

use std::process::Stdio;

use holon_net::file::CommandSpec;
use holon_net::probe::ProbeOutcome;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::effect::Pgid;
use crate::effect::end_group;

/// Runs `command` once. Only exit 0 yields its stdout; every other end is a
/// [`ProbeOutcome::Failed`] that says what happened.
pub async fn run(command: &CommandSpec) -> ProbeOutcome {
    let argv = command.argv.join(" ");
    let mut spawn = Command::new(&command.argv[0]);
    spawn
        .args(&command.argv[1..])
        .current_dir(&command.working_dir)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for name in &command.env_allow {
        if let Some(value) = std::env::var_os(name) {
            spawn.env(name, value);
        }
    }
    let mut child = match spawn.spawn() {
        Ok(child) => child,
        Err(e) => return ProbeOutcome::Failed(format!("cannot start {argv}: {e}")),
    };
    let pgid = child
        .id()
        .expect("a child that has not been waited for has a pid") as Pgid;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let ended = tokio::select! {
        ended = async { tokio::join!(child.wait(), read(&mut stdout), read(&mut stderr)) } => Some(ended),
        () = tokio::time::sleep(command.timeout.duration()) => None,
    };
    let Some((status, out, err)) = ended else {
        let timed_out = format!("{argv} timed out after {}", command.timeout);
        return ProbeOutcome::Failed(match end_group(pgid, Some(&mut child)).await {
            Ok(()) => timed_out,
            Err(e) => format!("{timed_out}; ending its process group failed: {e}"),
        });
    };
    let (status, out, err) = match (status, out, err) {
        (Ok(status), Ok(out), Ok(err)) => (status, out, err),
        (status, out, err) => {
            let e = [status.err(), out.err(), err.err()]
                .into_iter()
                .flatten()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return ProbeOutcome::Failed(format!("cannot read the run of {argv}: {e}"));
        }
    };
    let stderr = String::from_utf8_lossy(&err);
    if !status.success() {
        return ProbeOutcome::Failed(format!("{argv} ended with {status}: {}", stderr.trim()));
    }
    match String::from_utf8(out) {
        Ok(stdout) => ProbeOutcome::Output(stdout),
        Err(e) => ProbeOutcome::Failed(format!("the stdout of {argv} is not UTF-8: {e}")),
    }
}

async fn read(stream: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    Ok(bytes)
}
