//! A client of netd over its MCP endpoint, as `mcpc` is one, and a netd
//! process to talk to.

#![allow(dead_code)]

use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::ClientInfo;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::Value;

pub struct Cli {
    client: RunningService<RoleClient, ClientInfo>,
}

impl Cli {
    pub async fn connect(url: &str) -> Self {
        let transport = StreamableHttpClientTransport::from_uri(url.to_owned());
        let client = ClientInfo::default()
            .serve(transport)
            .await
            .unwrap_or_else(|e| panic!("cannot connect to netd at {url}: {e}"));
        Self { client }
    }

    /// The tool's result, or the refusal's message.
    pub async fn call(&self, tool: &str, args: Value) -> Result<CallToolResult, String> {
        let arguments = args
            .as_object()
            .unwrap_or_else(|| panic!("the arguments of {tool} are an object"))
            .clone();
        let result = self
            .client
            .call_tool(CallToolRequestParam {
                name: tool.to_owned().into(),
                arguments: Some(arguments),
            })
            .await
            .map_err(|e| e.to_string())?;
        if result.is_error == Some(true) {
            return Err(text(&result));
        }
        Ok(result)
    }

    /// The JSON output of a call that must succeed.
    pub async fn json(&self, tool: &str, args: Value) -> Value {
        match self.call(tool, args.clone()).await {
            Ok(result) => result
                .structured_content
                .unwrap_or_else(|| panic!("{tool} {args} answers without JSON")),
            Err(e) => panic!("{tool} {args} is refused: {e}"),
        }
    }

    /// The plain-text output of a call that must succeed.
    pub async fn text(&self, tool: &str, args: Value) -> String {
        match self.call(tool, args.clone()).await {
            Ok(result) => text(&result),
            Err(e) => panic!("{tool} {args} is refused: {e}"),
        }
    }

    /// The message of a call that must be refused.
    pub async fn refusal(&self, tool: &str, args: Value) -> String {
        match self.call(tool, args.clone()).await {
            Ok(result) => panic!("{tool} {args} must be refused, got {result:?}"),
            Err(e) => e,
        }
    }
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A netd process on a free loopback port.
pub struct Process {
    pub child: Child,
    pub url: String,
}

impl Process {
    /// Starts netd and waits for the line that says where it listens.
    pub fn start(state_dir: &Path, nets: &[&Path]) -> Self {
        Self::start_on("127.0.0.1:0", state_dir, nets).unwrap_or_else(|stderr| {
            panic!("netd does not start: {stderr}");
        })
    }

    /// `Err` holds the stderr of a netd that exited instead of listening.
    pub fn start_on(listen: &str, state_dir: &Path, nets: &[&Path]) -> Result<Self, String> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_netd"))
            .arg("--listen")
            .arg(listen)
            .arg("--state-dir")
            .arg(state_dir)
            .args(nets)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the netd binary starts");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout is piped"))
            .read_line(&mut line)
            .expect("netd's stdout is readable");
        match line.trim().strip_prefix("netd listening on ") {
            Some(url) => Ok(Self {
                url: url.to_owned(),
                child,
            }),
            None => {
                let output = child.wait_with_output().expect("netd ends");
                Err(format!(
                    "{}; stdout `{line}`; stderr: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        }
    }

    /// SIGKILL: no shutdown path runs.
    pub fn kill(mut self) {
        self.child.kill().expect("netd can be killed");
        self.child.wait().expect("netd is reaped");
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
