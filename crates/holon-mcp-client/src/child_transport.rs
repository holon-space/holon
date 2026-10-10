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
//! Its stderr is piped and read here too, under
//! [`crate::peer_budget::StderrAllowance`]: inherited it goes wherever Holon's
//! own stderr goes, which with a file log sink is an unrotated file a peer can
//! grow.
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
use tokio::io::AsyncBufReadExt as _;
use tokio::io::AsyncRead;
use tokio::io::ReadBuf;
use tokio::process::Child;
use tokio::process::ChildStderr;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::peer_budget::PeerBudget;
use crate::peer_budget::StderrVerdict;
use crate::secure_client::MAX_DISCLOSED_PEER_TEXT_BYTES;

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
pub(crate) fn spawn_bounded_child(
    cmd: &mut tokio::process::Command,
    budget: Arc<PeerBudget>,
) -> std::io::Result<BoundedChildProcess> {
    let who = cmd.as_std().get_program().to_string_lossy().into_owned();
    let (child, stdout, stdin, stderr) = spawn_parts(cmd, budget.clone())?;
    let reader_who = who.clone();
    tokio::spawn(forward_stderr(
        stderr,
        who.clone(),
        budget.clone(),
        move |verdict| match verdict {
            StderrVerdict::Forward(line) => info!("[sidecar {reader_who}] {line}"),
            StderrVerdict::Say(bound) => warn!("[sidecar {reader_who}] {bound}"),
            StderrVerdict::Nothing => {}
        },
    ));
    Ok(BoundedChildProcess {
        child: Some(child),
        transport: AsyncRwTransport::new_client(stdout, stdin),
        budget,
        who,
    })
}

/// The sidecar's process and the three ends of its stdio, split out so tests
/// can drive the readers on their own.
fn spawn_parts(
    cmd: &mut tokio::process::Command,
    budget: Arc<PeerBudget>,
) -> std::io::Result<(Child, BoundedChildStdout, ChildStdin, ChildStderr)> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    let stderr = child
        .stderr
        .take()
        .expect("a child spawned with a piped stderr has one");
    Ok((
        child,
        BoundedChildStdout {
            stdout,
            budget,
            held: 0,
        },
        stdin,
        stderr,
    ))
}

/// Read `stderr` to its end, handing every line to `say` under the
/// connection's stderr allowance.
///
/// Reading never stops, so the pipe cannot fill and stall the sidecar: a line
/// the bound refuses is read and dropped, not left unread.
async fn forward_stderr<R: AsyncRead + Unpin>(
    stderr: R,
    who: String,
    budget: Arc<PeerBudget>,
    mut say: impl FnMut(StderrVerdict),
) {
    let mut reader = tokio::io::BufReader::new(stderr);
    loop {
        match next_line(&mut reader).await {
            Ok(Some(line)) => say(budget.stderr.admit(&line)),
            Ok(None) => break,
            Err(e) => {
                say(StderrVerdict::Say(format!(
                    "reading this sidecar's stderr failed, so nothing it writes there is logged \
                     any more: {e}"
                )));
                break;
            }
        }
    }
    if let Some(report) = budget.stderr.dropped_report() {
        say(StderrVerdict::Say(format!("{report}; stderr ended")));
    }
    debug!("[child_transport] the stderr of sidecar {who} ended");
}

/// The next stderr line, at most [`MAX_DISCLOSED_PEER_TEXT_BYTES`] of it with
/// the rest of an over-long line counted instead of buffered.
///
/// `read_line` would buffer a line a sidecar never terminates whole, which is
/// the same hole the stdout reader closes.
async fn next_line<R: AsyncRead + Unpin>(
    reader: &mut tokio::io::BufReader<R>,
) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut cut = 0usize;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            if line.is_empty() && cut == 0 {
                return Ok(None);
            }
            break;
        }
        let (text, consumed, complete) = match chunk.iter().position(|b| *b == b'\n') {
            Some(newline) => (&chunk[..newline], newline + 1, true),
            None => (chunk, chunk.len(), false),
        };
        let fits = text
            .len()
            .min(MAX_DISCLOSED_PEER_TEXT_BYTES.saturating_sub(line.len()));
        line.extend_from_slice(&text[..fits]);
        cut += text.len() - fits;
        reader.consume(consumed);
        if complete {
            break;
        }
    }
    let mut text = String::from_utf8_lossy(&line).into_owned();
    if cut > 0 {
        text.push_str(&format!("… ({cut} bytes cut)"));
    }
    Ok(Some(text))
}

/// A sidecar connection: the budgeted stdio transport plus the process it
/// speaks to, whose shutdown it owns.
pub(crate) struct BoundedChildProcess {
    /// Taken by the graceful shutdown. Spawned with `kill_on_drop`, so a drop
    /// that never reached [`Self::shut_down`] still ends the sidecar.
    child: Option<Child>,
    transport: AsyncRwTransport<RoleClient, BoundedChildStdout, ChildStdin>,
    budget: Arc<PeerBudget>,
    /// The sidecar's program name, which is how its lines are keyed in the log.
    who: String,
}

impl BoundedChildProcess {
    /// Close the sidecar's stdin, give it [`GRACEFUL_EXIT`] to leave on its
    /// own, then kill it.
    async fn shut_down(&mut self) -> std::io::Result<()> {
        // The stderr reader says this at its own EOF, which a process on its
        // way down does not reach: the reader is a task and is dropped
        // wherever it was parked.
        if let Some(report) = self.budget.stderr.dropped_report() {
            warn!("[sidecar {}] {report}; the connection closed", self.who);
        }
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
    use crate::secure_client::MAX_STDERR_BYTES;

    fn sidecar(script: &str) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("/usr/bin/python3");
        cmd.arg("-c").arg(script);
        cmd
    }

    /// 68 MiB without a newline: one message the codec would hold whole.
    #[tokio::test]
    async fn an_unfinished_line_past_the_allowance_names_the_bound() {
        let cmd = &mut sidecar("import sys\nsys.stdout.write('x' * (68 * 1024 * 1024))");
        let (_child, mut stdout, _stdin, _stderr) =
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
        let (_child, mut stdout, _stdin, _stderr) =
            spawn_parts(cmd, PeerBudget::new()).expect("the sidecar spawns");
        let mut all = Vec::new();
        stdout
            .read_to_end(&mut all)
            .await
            .expect("100 finished lines are 100 messages, not one 100 MiB message");
        assert_eq!(all.len(), 100 * (1024 * 1024 + 1));
    }

    /// A sidecar whose stderr carries escape sequences, one line far past the
    /// per-line cap, and then 4 MiB of flood — four times what the log may
    /// take from it.
    #[tokio::test]
    async fn a_flooding_sidecars_stderr_is_bounded_and_what_it_cost_is_disclosed() {
        let cmd = &mut sidecar(
            "import sys\nsys.stderr.write('\\x1b[2Jbanner\\x07\\n')\nsys.stderr.write('L' * (8 \
             * 1024 * 1024) + '\\n')\nfor _ in range(4096): sys.stderr.write('e' * 1024 + '\\n')",
        );
        let budget = PeerBudget::new();
        let (_child, _stdout, _stdin, stderr) =
            spawn_parts(cmd, budget.clone()).expect("the sidecar spawns");

        let mut forwarded: Vec<String> = Vec::new();
        let mut said: Vec<String> = Vec::new();
        forward_stderr(
            stderr,
            "flooder".to_string(),
            budget.clone(),
            |verdict| match verdict {
                StderrVerdict::Forward(line) => forwarded.push(line),
                StderrVerdict::Say(bound) => said.push(bound),
                StderrVerdict::Nothing => {}
            },
        )
        .await;

        let logged: usize = forwarded.iter().map(String::len).sum();
        assert!(
            logged <= MAX_STDERR_BYTES,
            "the log may take at most MAX_STDERR_BYTES ({MAX_STDERR_BYTES}) from one sidecar's \
             stderr; it took {logged}"
        );
        assert!(
            !forwarded[0].chars().any(char::is_control)
                && forwarded[0].contains("\\u{1b}")
                && forwarded[0].contains("\\u{7}"),
            "a stderr line is peer text in a terminal: {:?}",
            forwarded[0]
        );
        assert!(
            forwarded[1].len() <= MAX_DISCLOSED_PEER_TEXT_BYTES + 64
                && forwarded[1].contains("bytes cut"),
            "an 8 MiB line must be cut to MAX_DISCLOSED_PEER_TEXT_BYTES and say so; got {} bytes \
             ending {:?}",
            forwarded[1].len(),
            &forwarded[1][forwarded[1].len().saturating_sub(40)..]
        );
        assert!(
            said.first()
                .is_some_and(|first| first.contains("MAX_STDERR_BYTES")),
            "the first dropped line must name the bound that dropped it; said: {said:?}"
        );
        assert!(
            said.last()
                .is_some_and(|last| last.contains("stderr ended")
                    && last.contains("stderr lines were dropped")),
            "how many lines the bound dropped must be disclosed: {said:?}"
        );
    }

    #[derive(Clone)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("the capture buffer")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl tracing_subscriber::fmt::MakeWriter<'_> for Captured {
        type Writer = Self;
        fn make_writer(&self) -> Self {
            self.clone()
        }
    }

    /// The stderr reader is a task of its own, and a process shutting down
    /// drops it wherever it was parked. The close path therefore says the
    /// total too — what a bound dropped is only disclosed if the disclosure
    /// outlives the flood.
    #[tokio::test]
    async fn closing_a_connection_says_what_its_stderr_bound_dropped() {
        let budget = PeerBudget::new();
        let line = "x".repeat(4096);
        for _ in 0..(MAX_STDERR_BYTES / line.len() + 2) {
            budget.stderr.admit(&line);
        }
        let mut transport =
            spawn_bounded_child(&mut sidecar("import time\ntime.sleep(10)"), budget.clone())
                .expect("the sidecar spawns");
        // Held here, so the sidecar's stderr stays open across the close and
        // its reader cannot reach the EOF where it would say the total itself.
        let _sidecar_alive = transport.child.take();

        let log = Captured(Arc::new(std::sync::Mutex::new(Vec::new())));
        let captured = log.clone();
        let guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_writer(log)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish(),
        );
        transport.shut_down().await.expect("the connection closes");
        drop(guard);

        let said =
            String::from_utf8_lossy(&captured.0.lock().expect("the capture buffer")).into_owned();
        assert!(
            said.contains("stderr lines were dropped by MAX_STDERR_BYTES"),
            "closing the connection must disclose the total its stderr bound dropped; the log \
             said: {said}"
        );
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
