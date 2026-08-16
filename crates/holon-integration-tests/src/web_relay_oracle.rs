//! `WebRelayOracle` — the web arm's AUTHORITATIVE observation channel (design
//! ruling D4.a, `docs/Testing/WebArmPBT.md`).
//!
//! The test process joins the `serve.mjs` hub as `role=native`, the browser
//! page is already on it as `role=browser`, and MCP tool calls cross to the
//! live wasm worker. That gives the harness two things the DOM cannot give it:
//!
//! * **Quiescence.** `await_quiescence` polls the engine's CDC watermark inside
//!   the worker and returns only once it has held still. Increment 1 had to
//!   infer settling from the DOM, which is unsound here — the worker advances
//!   on a 16ms tick pump, so the DOM sits unchanged mid-flight and a stability
//!   rule declares victory one gesture early (§4a of the design).
//! * **Engine truth.** `debug_pbt_snapshot` and `execute_raw_sql` read the
//!   projection the browser is rendering FROM, so a DOM assertion can be
//!   cross-checked against what the engine actually holds.
//!
//! # Transport
//!
//! `holon_mcp::browser_relay::BrowserRelay` is the wire client (already a
//! dependency; it owns the id-multiplexing and the reconnect loop). This type
//! is the thin layer above it that the harness actually wants: it turns a tool
//! result into parsed JSON and — importantly — turns an `is_error` result into
//! an `Err`. `BrowserRelay::forward` reports a failed tool as
//! `Ok(CallToolResult::error(..))`, which a caller that only matched on `Err`
//! would read as success.
//!
//! # One browser at a time
//!
//! The hub keeps a single socket per role, so cases must be serial. That is
//! already the arm's shape (one server, one browser context per case).

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_mcp::browser_relay::BrowserRelay;
use holon_mcp_client::mcp_call_surface::extract_tool_response;
use rmcp::model::CallToolRequestParam;

/// Hub endpoint the oracle dials. Override with `HOLON_WEB_HUB_URL`.
pub fn hub_url() -> String {
    std::env::var("HOLON_WEB_HUB_URL").unwrap_or_else(|_| "ws://127.0.0.1:8791/mcp-hub".to_string())
}

/// What the engine reports about itself — the authoritative half of the dual
/// oracle.
#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    /// Every block the engine holds, by scheme-qualified uri.
    pub block_ids: Vec<String>,
    /// `content` per block uri, for text cross-checks against the DOM.
    pub block_content: std::collections::BTreeMap<String, String>,
    /// The block the frontend engine considers focused, if any.
    pub focused_block: Option<String>,
}

/// One `await_quiescence` result — kept so a run can report how much of its
/// wall time was engine convergence rather than harness padding.
#[derive(Debug, Clone, Copy)]
pub struct QuiescenceReport {
    pub waited: Duration,
}

pub struct WebRelayOracle {
    relay: Arc<BrowserRelay>,
    hub_url: String,
}

impl WebRelayOracle {
    /// Join the hub. Returns immediately — the browser page usually is not on
    /// the hub yet at this point, which is why liveness is a separate step.
    ///
    /// # Exactly one per tokio runtime, reused across cases
    ///
    /// TOO MANY: the hub keeps a single `role=native` socket and replaces it on
    /// each new connection, while `BrowserRelay` reconnects on a 1s loop — so a
    /// second live oracle does not duplicate the first, it fights it for the
    /// slot and both start losing tool calls. A per-CASE oracle produced
    /// exactly that: case 1 green, case 2 unable to reach the engine at boot.
    ///
    /// TOO FEW: `start` spawns its connection loop on the CURRENT runtime, and
    /// `#[tokio::test]` gives every test its own. An oracle cached across tests
    /// is a dead loop from the second test on — "relay not connected", forever.
    ///
    /// So: one per test, threaded through every case that test runs.
    pub fn start(hub_url: &str) -> Self {
        Self {
            relay: BrowserRelay::start(hub_url.to_string()),
            hub_url: hub_url.to_string(),
        }
    }

    /// Block until the browser answers a tool call, so a caller never mistakes
    /// "hub not up yet" for "engine says no".
    pub async fn await_ready(&self, timeout: Duration) -> Result<()> {
        let hub_url = &self.hub_url;
        let deadline = Instant::now() + timeout;
        let mut last_err = "never attempted".to_string();
        loop {
            match self
                .call("await_quiescence", serde_json::json!({"budget_ms": 5000}))
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) => last_err = format!("{e:#}"),
            }
            if Instant::now() >= deadline {
                bail!(
                    "MCP relay oracle never reached the browser engine over {hub_url} within \
                     {timeout:?}. Is `serve.mjs` running and is the page connected as \
                     role=browser? Last error: {last_err}"
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub fn hub(&self) -> &str {
        &self.hub_url
    }

    /// Call one tool on the live browser engine and parse its JSON result.
    /// A tool-reported error is an `Err` here, never a value.
    pub async fn call(&self, tool: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        let serde_json::Value::Object(arguments) = args else {
            bail!("MCP tool arguments must be a JSON object, got: {args}");
        };
        let result = self
            .relay
            .forward(CallToolRequestParam {
                name: std::borrow::Cow::Owned(tool.to_string()),
                arguments: Some(arguments),
            })
            .await
            .map_err(|e| anyhow::anyhow!("relay transport failed for tool {tool:?}: {e}"))?;
        if result.is_error == Some(true) {
            let text: String = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            bail!("browser engine returned an error for tool {tool:?}: {text}");
        }
        extract_tool_response(&result)
            .with_context(|| format!("tool {tool:?} result did not parse as JSON"))
    }

    /// Block until the engine's CDC watermark has held still — the PRIMARY
    /// settled signal for every web-arm gesture.
    pub async fn await_quiescence(&self, budget: Duration) -> Result<QuiescenceReport> {
        let value = self
            .call(
                "await_quiescence",
                serde_json::json!({"budget_ms": budget.as_millis() as u64}),
            )
            .await?;
        let converged = value.get("converged").and_then(|v| v.as_bool());
        if converged != Some(true) {
            bail!("await_quiescence did not converge: {value}");
        }
        let waited_ms = value
            .get("waited_ms")
            .and_then(serde_json::Value::as_u64)
            .context("await_quiescence result has no numeric `waited_ms`")?;
        Ok(QuiescenceReport {
            waited: Duration::from_millis(waited_ms),
        })
    }

    /// Read the engine's own view of its blocks and focus.
    pub async fn engine_snapshot(&self) -> Result<EngineSnapshot> {
        let value = self
            .call("debug_pbt_snapshot", serde_json::json!({}))
            .await?;
        let live = value
            .get("live_blocks")
            .and_then(serde_json::Value::as_array)
            .context("debug_pbt_snapshot has no `live_blocks` array")?;
        let mut block_ids = Vec::with_capacity(live.len());
        let mut block_content = std::collections::BTreeMap::new();
        for block in live {
            let id = block
                .get("id")
                .and_then(serde_json::Value::as_str)
                .with_context(|| format!("live block without a string `id`: {block}"))?;
            block_ids.push(id.to_string());
            if let Some(content) = block.get("content").and_then(serde_json::Value::as_str) {
                block_content.insert(id.to_string(), content.to_string());
            }
        }
        block_ids.sort();
        Ok(EngineSnapshot {
            block_ids,
            block_content,
            focused_block: value
                .get("focused_block")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        })
    }

    /// Rows of an arbitrary SQL read against the live browser DB — the third
    /// oracle point, one layer below `debug_pbt_snapshot`'s block-query seam.
    pub async fn raw_sql(&self, sql: &str) -> Result<Vec<serde_json::Value>> {
        let value = self
            .call("execute_raw_sql", serde_json::json!({"sql": sql}))
            .await?;
        Ok(value
            .get("rows")
            .and_then(serde_json::Value::as_array)
            .with_context(|| format!("execute_raw_sql({sql:?}) returned no `rows` array"))?
            .clone())
    }
}
