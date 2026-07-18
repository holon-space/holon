//! Mock MCP server binary. Reads the scenario from `MOCK_MCP_SCENARIO`, serves
//! MCP over stdio, and exhibits the selected challenging behaviour. Behaviour
//! and per-scenario latency live in [`holon_mcp_mock`].

use holon_mcp_mock::MockServer;
use holon_mcp_mock::Scenario;
use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // stdout is the MCP wire; nothing else may be written to it.
    let scenario = Scenario::from_env()?;
    let server = MockServer::new(scenario);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
