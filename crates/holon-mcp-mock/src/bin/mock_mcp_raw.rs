//! Raw-stdio mock that violates the MCP initialization handshake. It reads
//! newline-delimited JSON-RPC from stdin and replies to the first `initialize`
//! request with a JSON-RPC error, modelling a server that refuses the
//! handshake (version mismatch, crash, misconfig). The connector must surface
//! this as a hard error out of `build_mcp_integration`, never hang or silently
//! produce an empty cache.
//!
//! Built raw (not via rmcp's server framework) precisely because the framework
//! guarantees a well-formed handshake — we need full wire control to break it.

use std::io::Write;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let stdout = std::io::stdout();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: serde_json::Value = serde_json::from_str(line)?;
        // Only requests (those carrying an `id`) get a reply; notifications do
        // not. We refuse `initialize` with a JSON-RPC error.
        let Some(id) = msg.get("id") else { continue };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        if method == "initialize" {
            let err = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32002,
                    "message": "mock: server refuses initialize (simulated handshake failure)"
                }
            });
            let mut out = stdout.lock();
            writeln!(out, "{err}")?;
            out.flush()?;
            // A real broken server would close; stop after refusing.
            break;
        }
    }
    Ok(())
}
