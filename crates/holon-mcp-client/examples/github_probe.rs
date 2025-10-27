//! One-shot probe of the GitHub hosted MCP server.
//!
//! Usage:
//!   GITHUB_PAT=ghp_xxx GITHUB_USER=martinmauch \
//!     cargo run -p holon-mcp-client --example github_probe
//!
//! Prints, for each of the three tools we plan to wire up:
//! - top-level JSON keys (the `extract_path` candidates for the YAML)
//! - the first record's field names (the column-mapping starting point)
//! - any pagination metadata (`pageInfo`, `totalCount`, etc.)
//!
//! No writes, no persistence.

use std::env;

use anyhow::Context;
use anyhow::Result;
use holon_mcp_client::connect_mcp;
use rmcp::model::CallToolRequestParam;
use serde_json::Value;
use serde_json::json;

const ENDPOINT: &str = "https://api.githubcopilot.com/mcp/";

#[tokio::main]
async fn main() -> Result<()> {
    let pat = env::var("GITHUB_PAT").context("set GITHUB_PAT env var to a GitHub PAT")?;
    let user = env::var("GITHUB_USER").ok();

    let (peer, _service) = connect_mcp(ENDPOINT, Some(&pat))
        .await
        .context("connect to hosted GitHub MCP")?;

    println!("# Probe 1: search_repositories");
    let probe_query = match &user {
        Some(u) => format!("user:{u}"),
        None => "user:@me".to_string(),
    };
    println!("  query: {:?}", probe_query);
    call_and_summarise(
        &peer,
        "search_repositories",
        json!({"query": probe_query, "perPage": 5}),
    )
    .await?;

    // Pick the first repo we got back so the issue/PR probes hit a real target.
    let (owner, repo) = pick_first_repo(&peer, &probe_query)
        .await
        .unwrap_or_else(|| ("octocat".to_string(), "Hello-World".to_string()));
    println!("\n  using {owner}/{repo} for issue/PR probes\n");

    println!("# Probe 2: list_issues");
    call_and_summarise(
        &peer,
        "list_issues",
        json!({"owner": owner, "repo": repo, "perPage": 5}),
    )
    .await?;

    println!("\n# Probe 3: list_pull_requests");
    call_and_summarise(
        &peer,
        "list_pull_requests",
        json!({"owner": owner, "repo": repo, "perPage": 5, "state": "all"}),
    )
    .await?;

    Ok(())
}

async fn call_and_summarise(
    peer: &rmcp::service::Peer<rmcp::RoleClient>,
    tool: &str,
    args: Value,
) -> Result<()> {
    let result = peer
        .call_tool(CallToolRequestParam {
            name: tool.to_string().into(),
            arguments: args.as_object().cloned(),
        })
        .await
        .with_context(|| format!("call_tool {tool}"))?;

    if result.is_error == Some(true) {
        let err_text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        println!("  ERROR: {err_text}");
        return Ok(());
    }

    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("");

    let body: Value =
        serde_json::from_str(&text).with_context(|| format!("parse {tool} response as JSON"))?;

    match &body {
        Value::Object(map) => {
            println!("  top-level keys: {:?}", map.keys().collect::<Vec<_>>());
            for (k, v) in map.iter() {
                match v {
                    Value::Array(items) => {
                        println!("  '{k}' is an array of {} items", items.len());
                        if let Some(Value::Object(first)) = items.first() {
                            let mut fields: Vec<&String> = first.keys().collect();
                            fields.sort();
                            println!("    first item fields: {:?}", fields);
                            // Surface a few field types so we know what's nested vs scalar.
                            for f in fields.iter().take(20) {
                                let kind = type_label(&first[*f]);
                                println!("      {f}: {kind}");
                            }
                        }
                    }
                    Value::Object(inner) => {
                        let keys: Vec<&String> = inner.keys().collect();
                        println!("  '{k}' is an object: {:?}", keys);
                    }
                    other => {
                        println!("  '{k}' = {}", short_repr(other));
                    }
                }
            }
        }
        Value::Array(items) => {
            println!("  top-level is a bare array of {} items", items.len());
            if let Some(Value::Object(first)) = items.first() {
                let fields: Vec<&String> = first.keys().collect();
                println!("    first item fields: {:?}", fields);
            }
        }
        other => {
            println!("  top-level is a {} value", type_label(other));
        }
    }

    Ok(())
}

async fn pick_first_repo(
    peer: &rmcp::service::Peer<rmcp::RoleClient>,
    query: &str,
) -> Option<(String, String)> {
    let result = peer
        .call_tool(CallToolRequestParam {
            name: "search_repositories".to_string().into(),
            arguments: json!({"query": query, "perPage": 1}).as_object().cloned(),
        })
        .await
        .ok()?;
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("");
    let body: Value = serde_json::from_str(&text).ok()?;

    let arr = body
        .as_object()
        .and_then(|m| m.values().find_map(|v| v.as_array()))?;
    let first = arr.first()?.as_object()?;

    // Try a few likely shapes:
    let owner = first
        .get("owner")
        .and_then(|o| o.as_object())
        .and_then(|m| m.get("login"))
        .and_then(|v| v.as_str())
        .or_else(|| first.get("owner_login").and_then(|v| v.as_str()))
        .or_else(|| {
            first
                .get("full_name")
                .and_then(|v| v.as_str())
                .and_then(|s| s.split('/').next())
        })?
        .to_string();
    let name = first
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| {
            first
                .get("full_name")
                .and_then(|v| v.as_str())
                .and_then(|s| s.split('/').nth(1))
        })?
        .to_string();
    Some((owner, name))
}

fn type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn short_repr(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 80 {
        format!("{}…", &s[..80])
    } else {
        s
    }
}
