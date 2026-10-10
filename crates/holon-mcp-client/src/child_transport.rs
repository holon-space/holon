//! The stdio leg of a connection, under the same budget as its HTTP legs.
//!
//! rmcp frames stdio messages with a line codec whose maximum length is
//! `usize::MAX`, and `TokioChildProcess` gives no way to set it, so one line a
//! sidecar never terminates is as much memory as it cares to write. Holon
//! therefore spawns the child itself and reads its stdout through
//! [`BoundedChildStdout`], which charges the unfinished line to the
//! connection's shared allowance — the same counter the HTTP legs' partial
//! events charge — and publishes the bound that ends the leg, so a dead leg is
//! disclosed rather than only logged.
//!
//! Spawning the child means owning its shutdown too: [`BoundedChildProcess`]
//! closes the sidecar's stdin and waits for it to leave on its own before
//! killing it, as `TokioChildProcess` does, because a sidecar killed mid-write
//! loses what it had not flushed.

use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;

use rmcp::RoleClient;
use rmcp::service::RxJsonRpcMessage;
use rmcp::service::TxJsonRpcMessage;
use rmcp::transport::Transport;
use rmcp::transport::async_rw::AsyncRwTransport;
use tokio::io::AsyncRead;
use tokio::io::ReadBuf;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tracing::info;
use tracing::warn;

use crate::peer_budget::PeerBudget;

/// How long a sidecar is given to leave on its own after its stdin closes,
/// before it is killed. rmcp's own child transport waits the same 3 s.
const GRACEFUL_EXIT: std::time::Duration = std::time::Duration::from_secs(3);

/// A sidecar's stdout, with the bytes of the line it has not finished yet
/// charged to its connection's budget.
pub(crate) struct BoundedChildStdout {
    stdout: ChildStdout,
    budget: Arc<PeerBudget>,
    /// Bytes of the unfinished line this reader has charged.
    held: usize,
}

/// Spawn `cmd` as an MCP sidecar and return the transport that talks to it.
///
/// stderr stays inherited, as rmcp's own child transport leaves it: piping it
/// without a reader would stop a chatty sidecar at the first full pipe.
pub(crate) fn spawn_bounded_child(
    cmd: &mut tokio::process::Command,
    budget: Arc<PeerBudget>,
) -> std::io::Result<BoundedChildProcess> {
    let (child, stdout, stdin) = spawn_parts(cmd, budget)?;
    Ok(BoundedChildProcess {
        child: Some(child),
        transport: AsyncRwTransport::new_client(stdout, stdin),
    })
}

/// The sidecar's process and the two ends of its stdio, split out so tests can
/// drive the reader on its own.
fn spawn_parts(
    cmd: &mut tokio::process::Command,
    budget: Arc<PeerBudget>,
) -> std::io::Result<(Child, BoundedChildStdout, ChildStdin)> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .expect("a child spawned with a piped stdout has one");
    let stdin = child
        .stdin
        .take()
        .expect("a child spawned with a piped stdin has one");
    Ok((
        child,
        BoundedChildStdout {
            stdout,
            budget,
            held: 0,
        },
        stdin,
    ))
}

/// A sidecar connection: the budgeted stdio transport plus the process it
/// speaks to, whose shutdown it owns.
pub(crate) struct BoundedChildProcess {
    /// Taken by the graceful shutdown. Spawned with `kill_on_drop`, so a drop
    /// that never reached [`Self::shut_down`] still ends the sidecar.
    child: Option<Child>,
    transport: AsyncRwTransport<RoleClient, BoundedChildStdout, ChildStdin>,
}

impl BoundedChildProcess {
    /// Close the sidecar's stdin, give it [`GRACEFUL_EXIT`] to leave on its
    /// own, then kill it.
    async fn shut_down(&mut self) -> std::io::Result<()> {
        // Dropping the writer is what the sidecar sees as EOF on its stdin,
        // which is the only signal it gets to stop.
        self.transport.close().await?;
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        match tokio::time::timeout(GRACEFUL_EXIT, child.wait()).await {
            Ok(Ok(status)) => info!("[child_transport] the sidecar exited on its own: {status}"),
            Ok(Err(e)) => {
                warn!("[child_transport] waiting for the sidecar to exit failed: {e}");
                return Err(e);
            }
            Err(_) => {
                warn!(
                    "[child_transport] the sidecar was still running {GRACEFUL_EXIT:?} after its \
                     stdin closed; killing it"
                );
                child.kill().await?;
            }
        }
        Ok(())
    }
}

impl Transport<RoleClient> for BoundedChildProcess {
    type Error = std::io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.transport.send(item)
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        self.transport.receive()
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.shut_down()
    }
}

impl AsyncRead for BoundedChildStdout {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let me = &mut *self;
        match Pin::new(&mut me.stdout).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {}
            other => return other,
        }
        let read = &buf.filled()[before..];
        // Everything up to the last newline is a message the codec can frame
        // and will release; only the tail after it is still held.
        let unfinished = match read.iter().rposition(|b| *b == b'\n') {
            Some(end) => read.len() - end - 1,
            None => me.held + read.len(),
        };
        if let Err(refused) = me.budget.held_event_bytes.recharge(me.held, unfinished) {
            // This ends the leg, and with it every operation the integration
            // serves, so the bound that did it has to reach the app and not
            // just the log.
            me.budget.transport.publish(format!(
                "the sidecar's stdio leg was ended by a bound: {refused}"
            ));
            return Poll::Ready(Err(refused));
        }
        me.held = unfinished;
        Poll::Ready(Ok(()))
    }
}

impl Drop for BoundedChildStdout {
    fn drop(&mut self) {
        self.budget.held_event_bytes.release(self.held);
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt as _;

    use super::*;

    fn sidecar(script: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("/usr/bin/python3");
        cmd.arg("-c").arg(script);
        cmd
    }

    /// 68 MiB without a newline: one message the codec would hold whole.
    #[tokio::test]
    async fn an_unfinished_line_past_the_allowance_names_the_bound() {
        let cmd = &mut sidecar("import sys\nsys.stdout.write('x' * (68 * 1024 * 1024))");
        let (_child, mut stdout, _stdin) =
            spawn_parts(cmd, PeerBudget::new()).expect("the sidecar spawns");
        let mut chunk = vec![0u8; 1 << 16];
        let err = loop {
            match stdout.read(&mut chunk).await {
                Ok(0) => panic!("the whole message was read without the allowance refusing it"),
                Ok(_) => continue,
                Err(e) => break e,
            }
        };
        assert!(
            err.to_string().contains("MAX_RESPONSE_BODY_BYTES"),
            "the refusal must name the bound it hit; got: {err}"
        );
    }

    /// 100 MiB in finished 1 MiB lines is past the allowance in total and
    /// inside it at every moment, so a reader that charges a line and never
    /// releases it would refuse a well-behaved sidecar.
    #[tokio::test]
    async fn finished_lines_release_what_they_held() {
        let cmd = &mut sidecar(
            "import sys\nfor _ in range(100): sys.stdout.write('x' * (1024 * 1024) + '\\n')",
        );
        let (_child, mut stdout, _stdin) =
            spawn_parts(cmd, PeerBudget::new()).expect("the sidecar spawns");
        let mut all = Vec::new();
        stdout
            .read_to_end(&mut all)
            .await
            .expect("100 finished lines are 100 messages, not one 100 MiB message");
        assert_eq!(all.len(), 100 * (1024 * 1024 + 1));
    }
}

#[cfg(test)]
mod shutdown_tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use crate::peer_budget::PeerBudget;

    /// A sidecar that answers `initialize`, and when its stdin ends takes 300
    /// ms to finish its work before writing `marker` — a sidecar flushing
    /// what it holds, which a kill mid-write loses.
    fn flushing_sidecar(marker: &std::path::Path) -> (String, Vec<String>) {
        let script = r#"
import sys, json, time
marker = sys.argv[1]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if msg.get("method") == "initialize":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "serverInfo": {"name": "flushing", "version": "0"}}}) + "\n")
        sys.stdout.flush()
time.sleep(0.3)
open(marker, "w").write("flushed")
"#;
        (
            "/usr/bin/python3".to_string(),
            vec![
                "-c".to_string(),
                script.to_string(),
                marker.display().to_string(),
            ],
        )
    }

    /// Dropping a connection must close the sidecar's stdin and give it time to
    /// exit on its own, the way rmcp's own child transport does; killing it at
    /// once loses whatever it had not written yet.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_connection_lets_its_sidecar_finish() {
        let marker = std::env::temp_dir().join(format!(
            "jaq-r8-flush-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock after 1970")
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&marker);
        let (command, args) = flushing_sidecar(&marker);
        let budget = PeerBudget::new();
        let (handler, _receiver) = budget.notifying_handler();
        let (peer, service) = crate::mcp_provider::connect_mcp_child_with_handler(
            &command,
            &args,
            &HashMap::new(),
            handler,
            budget,
        )
        .await
        .expect("the sidecar connects");
        drop(peer);
        drop(service);
        for _ in 0..40 {
            if marker.exists() {
                std::fs::remove_file(&marker).expect("the marker is removable");
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!(
            "the sidecar was killed before it could flush: {} never appeared",
            marker.display()
        );
    }
}
