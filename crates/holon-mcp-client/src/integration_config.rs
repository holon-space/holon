use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;
use serde::Serialize;

use crate::bundled_sidecars::BundledSidecar;
use crate::bundled_sidecars::SIDECAR_SCHEMA_VERSION;
use crate::bundled_sidecars::bundled_sidecar;
use crate::mcp_integration::AuthMode;
use crate::mcp_integration::McpIntegrationConfig;
use crate::mcp_integration::McpTransport;
use crate::mcp_sidecar::EntityConfig;
use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sidecar::ToolConfig;
use crate::rest_oauth2::RestOAuth2Config;
use crate::rest_transport::RestAuth;

/// Transport configuration as declared in the YAML file.
///
/// Exactly one of the variants must be set. Two of them reach a server that
/// *speaks MCP* (`child_process` = stdio, `http` = MCP-over-Streamable-HTTP);
/// the third, `rest`, reaches a plain HTTP/JSON API *directly* via a
/// UTCP-manual-style description (see [`RestTransport`]). All three plug into
/// the same connector engine behind the
/// [`crate::mcp_call_surface::McpCallSurface`] seam — one engine, plural
/// transports.
///
/// Note on naming: `http` here is historical and means *MCP over HTTP*, not a
/// generic REST call. The direct-API transport is `rest`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransportConfig {
    pub child_process: Option<ChildProcessTransport>,
    pub http: Option<HttpTransport>,
    pub rest: Option<RestTransport>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildProcessTransport {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpTransport {
    pub uri: String,
}

/// Direct HTTP-API transport (UTCP-manual style). Describes a plain JSON API by
/// its base URL and a set of named `calls`, each a GET endpoint. A
/// [`crate::rest_transport::RestCallSurface`] serves these calls behind the
/// same [`McpCallSurface`](crate::mcp_call_surface::McpCallSurface) seam the
/// MCP transports use, so the rest of the connector engine is unchanged.
///
/// Read-only for now: only `GET` methods are accepted (write/mutation and lease
/// semantics are out of scope).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestTransport {
    /// Base URL; may be `${VAR}`-expanded. No trailing slash required.
    pub base_url: String,
    /// Optional auth header sent on every request. NEVER inline a secret —
    /// reference an env/keychain name via `${VAR}` in `value`.
    #[serde(default)]
    pub auth: Option<RestAuthConfig>,
    /// Named endpoints, keyed by the tool name referenced from
    /// `sync.list_tool`.
    pub calls: HashMap<String, RestCallConfig>,
    /// Default poll cadence for sync entities that declare no per-entity
    /// `sync.interval`. REST has no subscription freshness, so every sync
    /// entity polls; this sets the transport-wide default (per-entity
    /// `sync.interval` still overrides it, and an unset value falls to the
    /// 300s built-in). Accepts an integer (seconds) or a humantime-style
    /// string (`"5m"`).
    #[serde(default)]
    pub poll_interval: Option<crate::mcp_sidecar::SyncInterval>,
}

/// Auth for the `rest` transport. Exactly one arm must be set:
///
/// - a **static header** — `{ header: Authorization, value: "Bearer ${TOKEN}"
///   }` (back-compatible), or
/// - **OAuth2** — `{ oauth2: { token_url, client_id_env, …, refresh_token_file
///   } }` (refresh-token grant; see [`RestOAuth2Config`]).
///
/// Modeled as optional fields (rather than a serde-tagged enum) so both shapes
/// parse cleanly under `deny_unknown_fields` and a mistake yields a precise
/// error at [`RestAuthConfig::resolve`] rather than a "did not match any
/// variant".
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestAuthConfig {
    /// Static-header arm: the header name (e.g. `Authorization`).
    #[serde(default)]
    pub header: Option<String>,
    /// Static-header arm: the header value; `${VAR}`-expanded at startup. Keep
    /// the secret out of YAML.
    #[serde(default)]
    pub value: Option<String>,
    /// OAuth2 arm: refresh-token-grant configuration.
    #[serde(default)]
    pub oauth2: Option<RestOAuth2Config>,
}

impl RestAuthConfig {
    /// Resolve into the runtime [`RestAuth`], expanding `${VAR}` in a static
    /// value and building the OAuth2 provider (reading credential files,
    /// running the 0600 checks) for the OAuth2 arm. Fails loud on an
    /// ambiguous or empty arm; surfaces [`UnresolvedVar`] when an OAuth2
    /// integration is simply not configured yet (disclosed skip).
    fn resolve(self, lookup: &VarLookup<'_>) -> anyhow::Result<RestAuth> {
        match (self.header, self.value, self.oauth2) {
            (Some(header), Some(value), None) => Ok(RestAuth::Static {
                header,
                value: expand_vars(&value, lookup)?,
            }),
            (None, None, Some(oauth2)) => {
                let provider = crate::rest_oauth2::build_provider(&oauth2, lookup)?;
                Ok(RestAuth::OAuth2(provider))
            }
            (None, None, None) => anyhow::bail!(
                "transport.rest.auth is empty — set either a static `{{ header, value }}` or an \
                 `oauth2` block"
            ),
            _ => anyhow::bail!(
                "transport.rest.auth must set EXACTLY ONE of a static `{{ header, value }}` pair \
                 or an `oauth2` block (not a mix)"
            ),
        }
    }
}

/// A single GET endpoint. `path` and `query` values may contain `{arg}`
/// placeholders filled from the tool-call arguments at request time.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestCallConfig {
    /// HTTP method. Only `GET` is supported today (fails loud otherwise).
    pub method: String,
    /// Path appended to `base_url`, e.g. `/posts` or `/users/{id}/posts`.
    pub path: String,
    /// Query parameters; values may be literals or `{arg}` placeholders.
    #[serde(default)]
    pub query: HashMap<String, String>,
    /// Response body codec: `json` (default), `atom`, or `rss`. `atom`/`rss`
    /// decode a syndication feed into the same record shape as JSON.
    #[serde(default)]
    pub format: crate::rest_transport::ResponseFormat,
    /// If set, a non-object JSON body is wrapped as `{ result_key: <body> }` so
    /// a `sync.extract_path` can select it (bare-array responses → object). For
    /// `atom`/`rss` the decoded entry array is wrapped under this key (default
    /// `entries`).
    #[serde(default)]
    pub result_key: Option<String>,
    /// Optional response-token pagination (`json` only): follow a continuation
    /// token (e.g. `nextPageToken`) across pages, bounded fail-loud by
    /// `max_pages`.
    #[serde(default)]
    pub pagination: Option<crate::rest_transport::Pagination>,
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

/// Top-level structure of a provider YAML file in
/// `~/.config/holon/integrations/`.
///
/// Combines transport config with the sidecar entity/tool declarations.
/// The provider name is derived from the filename (stem without `.yaml`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IntegrationFileConfig {
    /// Sidecar-format generation this file was authored against. Absent in
    /// every file written before the format was versioned, which is precisely
    /// the population that cannot be trusted to still match the engine — see
    /// [`crate::bundled_sidecars`].
    #[serde(default)]
    pub schema_version: Option<u32>,
    pub transport: TransportConfig,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    /// Prefix prepended to all entity names for table names, ID schemes, etc.
    #[serde(default)]
    pub entity_prefix: Option<String>,
    #[serde(default)]
    pub entities: HashMap<String, EntityConfig>,
    /// Master write switch (leases/read-write ruling). Absent = disabled. Flows
    /// through into the [`McpSidecar`] so the dispatch chokepoint can enforce
    /// it.
    #[serde(default)]
    pub writes: crate::mcp_sidecar::WritesPolicy,
    /// Writer designation for `once_only` effects (leases/read-write ruling,
    /// increment 4). Absent = confirm_manually. Flows through into the
    /// [`McpSidecar`] so the dispatch chokepoint can select the policy.
    #[serde(default)]
    pub once_only: crate::mcp_sidecar::OnceOnlyAuthorization,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    /// Sidecar-declared derived views (see
    /// [`crate::mcp_sidecar::McpSidecar::views`]).
    #[serde(default)]
    pub views: Vec<crate::mcp_sidecar::ViewConfig>,
}

/// Resolves `${VAR}` references in integration config strings.
///
/// Returns the value for a variable name, or `None` if it is not set in this
/// source. Implementors layer sources (env vars, app settings, …); see
/// [`env_var_lookup`] for the default environment-only resolver.
pub type VarLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// The default resolver: process environment variables only. An env var set to
/// the empty string is treated as unset, so it falls through to other layers.
pub fn env_var_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Typed error for a `${VAR}` reference that no resolver layer could supply.
///
/// Callers can `downcast_ref::<UnresolvedVar>()` on the `anyhow::Error` to
/// distinguish "integration not configured yet" (a missing secret — may be a
/// disclosed skip) from a structurally invalid config (always a hard error).
#[derive(Debug, Clone)]
pub struct UnresolvedVar {
    pub var: String,
}

impl std::fmt::Display for UnresolvedVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Variable '{}' referenced in config is not set (checked environment and settings)",
            self.var
        )
    }
}

impl std::error::Error for UnresolvedVar {}

impl IntegrationFileConfig {
    /// Convert into the runtime config, expanding `${VAR}` references in
    /// transport and auth string fields from the process environment.
    ///
    /// The `auth` field is mapped to `AuthMode`; OAuth returns `AuthMode::None`
    /// (the caller upgrades it with a credential store).
    ///
    /// Fails loudly if a referenced variable is unset — secrets (e.g. a
    /// `static_token: "${TODOIST_API_KEY}"`) are kept out of the YAML and
    /// resolved here at startup, so a missing var is surfaced rather than
    /// silently producing an unauthenticated connection.
    pub fn into_mcp_config(self, provider_name: String) -> anyhow::Result<McpIntegrationConfig> {
        self.into_mcp_config_with(provider_name, &env_var_lookup)
    }

    /// Like [`into_mcp_config`](Self::into_mcp_config) but with a
    /// caller-supplied variable resolver, so a frontend can layer app
    /// settings on top of the environment (e.g. resolve
    /// `${TODOIST_API_KEY}` from a `todoist.api_key`
    /// setting). `holon-mcp-client` stays agnostic of where values come from.
    pub fn into_mcp_config_with(
        self,
        provider_name: String,
        lookup: &VarLookup<'_>,
    ) -> anyhow::Result<McpIntegrationConfig> {
        let transport = if let Some(cp) = self.transport.child_process {
            McpTransport::ChildProcess {
                command: expand_vars(&cp.command, lookup)?,
                args: cp
                    .args
                    .iter()
                    .map(|a| expand_vars(a, lookup))
                    .collect::<anyhow::Result<Vec<_>>>()?,
                env: cp
                    .env
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), expand_vars(v, lookup)?)))
                    .collect::<anyhow::Result<HashMap<_, _>>>()?,
            }
        } else if let Some(http) = self.transport.http {
            McpTransport::Http {
                uri: expand_vars(&http.uri, lookup)?,
            }
        } else if let Some(rest) = self.transport.rest {
            let auth = match rest.auth {
                Some(a) => a.resolve(lookup)?,
                None => RestAuth::None,
            };
            let mut calls = HashMap::with_capacity(rest.calls.len());
            for (name, c) in rest.calls {
                calls.insert(
                    name,
                    crate::rest_transport::RestCall {
                        method: c.method,
                        path: c.path,
                        query: c.query,
                        format: c.format,
                        result_key: c.result_key,
                        pagination: c.pagination,
                    },
                );
            }
            McpTransport::Rest {
                manual: crate::rest_transport::RestManual {
                    base_url: expand_vars(&rest.base_url, lookup)?,
                    auth,
                    calls,
                },
                poll_interval: rest.poll_interval,
            }
        } else {
            anyhow::bail!("TransportConfig must set exactly one of child_process, http, or rest");
        };

        let auth_mode = match self.auth {
            Some(AuthConfig {
                static_token: Some(token),
                ..
            }) => AuthMode::StaticToken(expand_vars(&token, lookup)?),
            // OAuth needs a credential store — caller must upgrade this.
            _ => AuthMode::None,
        };

        let sidecar = McpSidecar {
            entity_prefix: self.entity_prefix,
            entities: self.entities,
            writes: self.writes,
            once_only: self.once_only,
            tools: self.tools,
            views: self.views,
        };
        let sidecar_yaml =
            serde_yaml::to_string(&sidecar).expect("McpSidecar must be serializable");

        Ok(McpIntegrationConfig {
            provider_name,
            transport,
            sidecar_yaml,
            auth_mode,
        })
    }
}

/// Expand `${VAR}` references in a config string using `lookup`.
///
/// Only the `${VAR}` form is recognized; a bare `$` (or `$VAR` without braces)
/// is left untouched. Fails loudly if a referenced variable is unresolved or
/// the `${` is unterminated — never silently substitutes a default.
fn expand_vars(input: &str, lookup: &VarLookup<'_>) -> anyhow::Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| anyhow::anyhow!("Unterminated '${{' in config value '{input}'"))?;
        let var = &after[..end];
        let value = lookup(var).ok_or_else(|| {
            anyhow::Error::new(UnresolvedVar {
                var: var.to_string(),
            })
        })?;
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// An installed sidecar that was NOT used, and why. Carried out of the loader
/// as data so the caller cannot forget to disclose it — a supersede that only
/// logged would be the same silent-staleness defect in a new place.
#[derive(Debug, Clone)]
pub struct SupersededSidecar {
    pub provider: String,
    pub installed_path: PathBuf,
    /// Repo-relative path of the sidecar that was used instead.
    pub bundled_source: &'static str,
    /// Why the installed file could not be honored, in the reader's terms.
    pub incompatibility: String,
}

/// What a scan of the integrations directory yielded.
#[derive(Debug)]
pub struct LoadedIntegrations {
    pub configs: Vec<(String, IntegrationFileConfig)>,
    pub superseded: Vec<SupersededSidecar>,
}

/// Scan a directory for `*.yaml` provider config files.
///
/// The provider name is the file stem (e.g., `claude-history.yaml` ->
/// `"claude-history"`), and the file's PRESENCE is what enables that provider.
///
/// For a provider this build ships (see [`crate::bundled_sidecars`]) the
/// installed file supplies content only when it declares this build's
/// [`SIDECAR_SCHEMA_VERSION`]; otherwise the bundled sidecar is used and the
/// installed one is reported in [`LoadedIntegrations::superseded`]. That is
/// what keeps a copy taken before a format requirement landed from silently
/// outranking the sidecar the engine was built against.
///
/// A missing integrations directory means "no integrations configured".
/// Files without a `.yaml`/`.yml` extension are skipped. A YAML file for a
/// provider this build does NOT ship is a hard error when it cannot be read or
/// parsed — there is nothing to fall back to, so it must never be skipped.
pub fn load_integration_configs(dir: &Path) -> anyhow::Result<LoadedIntegrations> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                "[load_integration_configs] Integrations directory '{}' does not exist — no \
                 integrations configured",
                dir.display()
            );
            return Ok(LoadedIntegrations {
                configs: Vec::new(),
                superseded: Vec::new(),
            });
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!(
                "Failed to read integrations directory '{}'",
                dir.display()
            )));
        }
    };

    let mut configs = Vec::new();
    let mut superseded = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("Failed to read directory entry in '{}'", dir.display()))?;
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("yaml") && ext != Some("yml") {
            continue;
        }

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .with_context(|| {
                format!(
                    "Integration config '{}' has a non-UTF-8 file name",
                    path.display()
                )
            })?
            .to_string();

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read integration config '{}'", path.display()))?;

        let Some(bundled) = bundled_sidecar(&name) else {
            let config =
                serde_yaml::from_str::<IntegrationFileConfig>(&content).with_context(|| {
                    format!("Failed to parse integration config '{}'", path.display())
                })?;
            tracing::info!(
                "[load_integration_configs] Loaded provider '{}' from '{}'",
                name,
                path.display()
            );
            configs.push((name, config));
            continue;
        };

        // Byte-identical to what we ship: the same file, not an override.
        // Nothing drifted, so nothing to log and nothing to disclose.
        if content == bundled.yaml {
            configs.push((name, parse_bundled(bundled)?));
            continue;
        }

        let incompatibility = match serde_yaml::from_str::<IntegrationFileConfig>(&content) {
            Ok(config) if config.schema_version == Some(SIDECAR_SCHEMA_VERSION) => {
                tracing::info!(
                    "[load_integration_configs] Provider '{}' is OVERRIDDEN by '{}' \
                     (schema_version {SIDECAR_SCHEMA_VERSION}); the sidecar bundled at '{}' is \
                     not used",
                    name,
                    path.display(),
                    bundled.source_path
                );
                configs.push((name, config));
                continue;
            }
            Ok(config) => format!(
                "it declares schema_version {} but this build's sidecar format is schema_version \
                 {SIDECAR_SCHEMA_VERSION}",
                match config.schema_version {
                    Some(v) => v.to_string(),
                    None => "none".to_string(),
                }
            ),
            Err(e) => format!(
                "it does not parse against this build's sidecar format, so no schema_version \
                 could be established: {e}"
            ),
        };

        superseded.push(SupersededSidecar {
            provider: name.clone(),
            installed_path: path,
            bundled_source: bundled.source_path,
            incompatibility,
        });
        configs.push((name, parse_bundled(bundled)?));
    }

    Ok(LoadedIntegrations {
        configs,
        superseded,
    })
}

fn parse_bundled(bundled: &'static BundledSidecar) -> anyhow::Result<IntegrationFileConfig> {
    serde_yaml::from_str::<IntegrationFileConfig>(bundled.yaml)
        .with_context(|| format!("Bundled sidecar '{}' does not parse", bundled.source_path))
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
        let mcp_config = config.into_mcp_config("test-provider".into()).unwrap();

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
        let mcp_config = config.into_mcp_config("http-provider".into()).unwrap();

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

        // Non-yaml file (should be skipped)
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let loaded = load_integration_configs(dir.path()).unwrap();
        assert_eq!(loaded.configs.len(), 1);
        assert_eq!(loaded.configs[0].0, "test-provider");
    }

    #[test]
    fn load_configs_malformed_yaml_fails_loud() {
        let dir = tempfile::tempdir().unwrap();
        let bad_path = dir.path().join("bad.yaml");
        std::fs::write(&bad_path, "not: [valid: yaml: config").unwrap();

        let err = load_integration_configs(dir.path())
            .expect_err("malformed YAML must be a hard error, not a silent skip");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(bad_path.display().to_string().as_str()),
            "error must name the offending file: {msg}"
        );
    }

    #[test]
    fn load_configs_missing_directory() {
        let loaded = load_integration_configs(Path::new("/nonexistent/path")).unwrap();
        assert!(loaded.configs.is_empty());
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

    #[test]
    fn static_token_env_var_is_expanded() {
        // SAFETY: unique var name avoids clashing with other tests/process state.
        unsafe { std::env::set_var("HOLON_TEST_TODOIST_TOKEN", "secret-123") };
        let yaml = r#"
transport:
  http:
    uri: "https://${HOLON_TEST_TODOIST_HOST}/mcp"
auth:
  static_token: "${HOLON_TEST_TODOIST_TOKEN}"
entities: {}
"#;
        unsafe { std::env::set_var("HOLON_TEST_TODOIST_HOST", "ai.todoist.net") };
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        let mcp_config = config.into_mcp_config("todoist".into()).unwrap();

        match &mcp_config.transport {
            McpTransport::Http { uri } => assert_eq!(uri, "https://ai.todoist.net/mcp"),
            other => panic!("expected Http, got {other:?}"),
        }
        match &mcp_config.auth_mode {
            AuthMode::StaticToken(token) => assert_eq!(token, "secret-123"),
            other => panic!("expected StaticToken, got {other:?}"),
        }
        unsafe { std::env::remove_var("HOLON_TEST_TODOIST_TOKEN") };
        unsafe { std::env::remove_var("HOLON_TEST_TODOIST_HOST") };
    }

    #[test]
    fn missing_env_var_fails_loud() {
        let yaml = r#"
transport:
  http:
    uri: "https://example.com/mcp"
auth:
  static_token: "${HOLON_TEST_DEFINITELY_UNSET_VAR}"
entities: {}
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        let err = match config.into_mcp_config("p".into()) {
            Ok(_) => panic!("expected error for unset env var"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("HOLON_TEST_DEFINITELY_UNSET_VAR"),
            "error should name the missing var: {err}"
        );
        let unresolved = err
            .downcast_ref::<UnresolvedVar>()
            .expect("unset var must surface as the typed UnresolvedVar error");
        assert_eq!(unresolved.var, "HOLON_TEST_DEFINITELY_UNSET_VAR");
    }

    #[test]
    fn invalid_config_is_not_unresolved_var() {
        let yaml = r#"
transport: {}
entities: {}
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        let err = config
            .into_mcp_config("p".into())
            .expect_err("missing transport must be a hard error");
        assert!(
            err.downcast_ref::<UnresolvedVar>().is_none(),
            "structural config errors must not masquerade as missing secrets: {err}"
        );
    }

    #[test]
    fn no_env_refs_pass_through_unchanged() {
        assert_eq!(
            expand_vars("plain-token", &env_var_lookup).unwrap(),
            "plain-token"
        );
        // bare $ (no braces) is left untouched
        assert_eq!(expand_vars("$HOME/x", &env_var_lookup).unwrap(), "$HOME/x");
    }

    #[test]
    fn layered_lookup_resolves_from_settings() {
        // Env-less lookup that mimics the frontend's settings layer.
        let lookup = |name: &str| -> Option<String> {
            (name == "TODOIST_API_KEY").then(|| "from-settings".to_string())
        };
        let yaml = r#"
transport:
  http:
    uri: "https://ai.todoist.net/mcp"
auth:
  static_token: "${TODOIST_API_KEY}"
entities: {}
"#;
        let config: IntegrationFileConfig = serde_yaml::from_str(yaml).unwrap();
        let mcp_config = config
            .into_mcp_config_with("todoist".into(), &lookup)
            .unwrap();
        match &mcp_config.auth_mode {
            AuthMode::StaticToken(t) => assert_eq!(t, "from-settings"),
            other => panic!("expected StaticToken, got {other:?}"),
        }
    }
}
