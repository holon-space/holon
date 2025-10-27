use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::mcp_integration::{AuthMode, McpIntegrationConfig, McpTransport};
use crate::mcp_sidecar::{EntityConfig, McpSidecar, ToolConfig};

/// Transport configuration as declared in the YAML file.
///
/// Exactly one of `child_process` or `http` must be set.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TransportConfig {
    pub child_process: Option<ChildProcessTransport>,
    pub http: Option<HttpTransport>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChildProcessTransport {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpTransport {
    pub uri: String,
}

/// Authentication configuration (only meaningful for HTTP transport).
///
/// Set `static_token` for bearer auth, or `oauth: true` for OAuth 2.1.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub static_token: Option<String>,
    #[serde(default)]
    pub oauth: bool,
}

/// Top-level structure of a provider YAML file in `~/.config/holon/integrations/`.
///
/// Combines transport config with the sidecar entity/tool declarations.
/// The provider name is derived from the filename (stem without `.yaml`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IntegrationFileConfig {
    pub transport: TransportConfig,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    /// Prefix prepended to all entity names for table names, ID schemes, etc.
    #[serde(default)]
    pub entity_prefix: Option<String>,
    #[serde(default)]
    pub entities: HashMap<String, EntityConfig>,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
}

impl IntegrationFileConfig {
    /// Convert into the `McpIntegrationConfig` expected by `build_mcp_integration()`.
    ///
    /// The `auth` field is mapped to `AuthMode`; OAuth requires a credential store
    /// which must be provided externally — this method returns `AuthMode::None` for
    /// OAuth declarations (the caller is responsible for upgrading to `AuthMode::OAuth`).
    pub fn into_mcp_config(self, provider_name: String) -> McpIntegrationConfig {
        let transport = if let Some(cp) = self.transport.child_process {
            McpTransport::ChildProcess {
                command: cp.command,
                args: cp.args,
                env: cp.env,
            }
        } else if let Some(http) = self.transport.http {
            McpTransport::Http { uri: http.uri }
        } else {
            panic!("TransportConfig must have either child_process or http set");
        };

        let auth_mode = match self.auth {
            Some(AuthConfig {
                static_token: Some(token),
                ..
            }) => AuthMode::StaticToken(token),
            // OAuth needs a credential store — caller must upgrade this.
            _ => AuthMode::None,
        };

        let sidecar = McpSidecar {
            entity_prefix: self.entity_prefix,
            entities: self.entities,
            tools: self.tools,
        };
        let sidecar_yaml =
            serde_yaml::to_string(&sidecar).expect("McpSidecar must be serializable");

        McpIntegrationConfig {
            provider_name,
            transport,
            sidecar_yaml,
            auth_mode,
        }
    }
}

/// Scan a directory for `*.yaml` provider config files and return `(name, config)` pairs.
///
/// The provider name is the file stem (e.g., `claude-history.yaml` -> `"claude-history"`).
/// Files that fail to parse are logged and skipped.
pub fn load_integration_configs(dir: &Path) -> Vec<(String, IntegrationFileConfig)> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::debug!(
                "[load_integration_configs] Cannot read '{}': {e}",
                dir.display()
            );
            return vec![];
        }
    };

    let mut configs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("yaml") && ext != Some("yml") {
            continue;
        }

        let name = match path.file_stem().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "[load_integration_configs] Failed to read '{}': {e}",
                    path.display()
                );
                continue;
            }
        };

        match serde_yaml::from_str::<IntegrationFileConfig>(&content) {
            Ok(config) => {
                tracing::info!(
                    "[load_integration_configs] Loaded provider '{}' from '{}'",
                    name,
                    path.display()
                );
                configs.push((name, config));
            }
            Err(e) => {
                tracing::warn!(
                    "[load_integration_configs] Failed to parse '{}': {e}",
                    path.display()
                );
            }
        }
    }

    configs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_child_process_config() {
        let yaml = r#"
transport:
  child_process:
    command: npx
    args: ["-y", "@anthropic/claude-code-history-mcp"]
    env:
      CLAUDE_DATA_DIR: "/Users/martin/.claude"

entities:
  session:
    sync:
      list_resource: "claude-history://projects/{project_id}/sessions"
      uri_params:
        project_id: "-Users-martin-Workspaces-pkm-holon"

tools: {}
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();

        let cp = config.transport.child_process.as_ref().unwrap();
        assert_eq!(cp.command, "npx");
        assert_eq!(cp.args, &["-y", "@anthropic/claude-code-history-mcp"]);
        assert_eq!(cp.env["CLAUDE_DATA_DIR"], "/Users/martin/.claude");
        assert!(config.transport.http.is_none());

        assert!(config.auth.is_none());
        assert_eq!(config.entities.len(), 1);
        assert!(config.entities.contains_key("session"));

        let sync = config.entities["session"].sync.as_ref().unwrap();
        assert_eq!(
            sync.list_resource.as_deref(),
            Some("claude-history://projects/{project_id}/sessions")
        );
    }

    #[test]
    fn parse_http_config_with_static_token() {
        let yaml = r#"
transport:
  http:
    uri: "https://api.example.com/mcp"

auth:
  static_token: "sk-test-key"

entities:
  task:
    short_name: task
    id_column: id
    sync:
      list_tool: get-tasks
      extract_path: tasks

tools:
  complete-task:
    entity: task
    affected_fields: [completed]
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();

        let http = config.transport.http.as_ref().unwrap();
        assert_eq!(http.uri, "https://api.example.com/mcp");
        assert!(config.transport.child_process.is_none());

        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.static_token.as_deref(), Some("sk-test-key"));
        assert!(!auth.oauth);

        assert!(config.tools.contains_key("complete-task"));
    }

    #[test]
    fn parse_http_config_with_oauth() {
        let yaml = r#"
transport:
  http:
    uri: "https://api.example.com/mcp"

auth:
  oauth: true
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();

        let auth = config.auth.as_ref().unwrap();
        assert!(auth.oauth);
        assert!(auth.static_token.is_none());
    }

    #[test]
    fn into_mcp_config_child_process() {
        let yaml = r#"
transport:
  child_process:
    command: node
    args: ["server.js"]
    env:
      PORT: "3000"

entities:
  item:
    short_name: item
    id_column: id
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        let mcp_config = config.into_mcp_config("test-provider".into());

        assert_eq!(mcp_config.provider_name, "test-provider");
        match &mcp_config.transport {
            McpTransport::ChildProcess { command, args, env } => {
                assert_eq!(command, "node");
                assert_eq!(args, &["server.js"]);
                assert_eq!(env["PORT"], "3000");
            }
            other => panic!("expected ChildProcess, got {other:?}"),
        }
        match &mcp_config.auth_mode {
            AuthMode::None => {}
            other => panic!("expected None, got {other:?}"),
        }
    }

    #[test]
    fn into_mcp_config_http_with_token() {
        let yaml = r#"
transport:
  http:
    uri: "https://example.com/mcp"
auth:
  static_token: "my-key"
entities: {}
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        let mcp_config = config.into_mcp_config("http-provider".into());

        match &mcp_config.transport {
            McpTransport::Http { uri } => assert_eq!(uri, "https://example.com/mcp"),
            other => panic!("expected Http, got {other:?}"),
        }
        match &mcp_config.auth_mode {
            AuthMode::StaticToken(token) => assert_eq!(token, "my-key"),
            other => panic!("expected StaticToken, got {other:?}"),
        }
    }

    #[test]
    fn load_configs_from_directory() {
        let dir = tempfile::tempdir().unwrap();

        // Valid config
        std::fs::write(
            dir.path().join("test-provider.yaml"),
            r#"
transport:
  child_process:
    command: echo
    args: ["hello"]
entities: {}
"#,
        )
        .unwrap();

        // Invalid config (should be skipped)
        std::fs::write(dir.path().join("bad.yaml"), "not: [valid: yaml: config").unwrap();

        // Non-yaml file (should be skipped)
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let configs = load_integration_configs(dir.path());
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "test-provider");
    }

    #[test]
    fn load_configs_missing_directory() {
        let configs = load_integration_configs(Path::new("/nonexistent/path"));
        assert!(configs.is_empty());
    }

    #[test]
    fn minimal_config_with_defaults() {
        let yaml = r#"
transport:
  child_process:
    command: my-server
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.auth.is_none());
        assert!(config.entities.is_empty());
        assert!(config.tools.is_empty());
    }
}
