use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;
use serde::Serialize;

use crate::bundled_sidecars::BUNDLED_SIDECARS;
use crate::bundled_sidecars::BundledSidecar;
use crate::bundled_sidecars::SIDECAR_SCHEMA_VERSION;
use crate::bundled_sidecars::bundled_sidecar;
use crate::integration_state::ENABLE_COMMAND;
use crate::integration_state::IntegrationConfigStore;
use crate::integration_state::enabling_state_file;
use crate::mcp_integration::AuthMode;
use crate::mcp_integration::McpIntegrationConfig;
use crate::mcp_integration::McpTransport;
use crate::mcp_sidecar::EntityConfig;
use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sidecar::ToolConfig;
use crate::redaction::Redactor;
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
    fn resolve(self, lookup: &VarLookup<'_>, redactor: &Redactor) -> anyhow::Result<RestAuth> {
        match (self.header, self.value, self.oauth2) {
            (Some(header), Some(value), None) => Ok(RestAuth::Static {
                header,
                value: expand_vars(&value, lookup, redactor)?,
            }),
            (None, None, Some(oauth2)) => {
                let provider = crate::rest_oauth2::build_provider(&oauth2, lookup, redactor)?;
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
    /// What the Integrations sidebar calls this provider. Absent means derive
    /// it from the provider name (`IntegrationStateProjector`), so a sidecar
    /// only carries the key when the derivation would read badly.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The glyph the sidebar row shows. Parsed against the renderer's table at
    /// load, because nobody watches an integration row render and a bad name
    /// would otherwise become a silent bullet.
    #[serde(default)]
    pub icon: Option<holon_api::icon_name::IconName>,
    /// The page `integration.open_default_view` focuses in the main panel — the
    /// BARE id of a block (org convention, no `block:` prefix). Absent means
    /// this integration has no view yet and the operation refuses.
    #[serde(default)]
    pub default_view: Option<String>,
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
///
/// `Send + Sync` because the consent flow borrows one across an await and is
/// spawned as a background task.
pub type VarLookup<'a> = dyn Fn(&str) -> Option<String> + Send + Sync + 'a;

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
        // Every `${VAR}` value is a secret by construction, so expansion and
        // registration are the same pass. The transport and the OAuth2 provider
        // share this one `Redactor`, so a token minted mid-request joins the set
        // that guards every message either of them emits.
        let redactor = Redactor::new();

        let auth_mode = match self.auth {
            Some(AuthConfig {
                static_token: Some(token),
                ..
            }) => AuthMode::StaticToken(expand_vars(&token, lookup, &redactor)?),
            // OAuth needs a credential store — caller must upgrade this.
            _ => AuthMode::None,
        };

        let transport = if let Some(cp) = self.transport.child_process {
            McpTransport::ChildProcess {
                command: expand_vars(&cp.command, lookup, &redactor)?,
                args: cp
                    .args
                    .iter()
                    .map(|a| expand_vars(a, lookup, &redactor))
                    .collect::<anyhow::Result<Vec<_>>>()?,
                env: cp
                    .env
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), expand_vars(v, lookup, &redactor)?)))
                    .collect::<anyhow::Result<HashMap<_, _>>>()?,
            }
        } else if let Some(http) = self.transport.http {
            McpTransport::Http {
                uri: expand_vars(&http.uri, lookup, &redactor)?,
            }
        } else if let Some(rest) = self.transport.rest {
            let auth = match rest.auth {
                Some(a) => a.resolve(lookup, &redactor)?,
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
            let base_url = expand_vars(&rest.base_url, lookup, &redactor)?;
            McpTransport::Rest {
                manual: crate::rest_transport::RestManual {
                    base_url,
                    auth,
                    calls,
                    redactor,
                },
                poll_interval: rest.poll_interval,
            }
        } else {
            anyhow::bail!("TransportConfig must set exactly one of child_process, http, or rest");
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

/// Expand `${VAR}` references in a config string using `lookup`, registering
/// each resolved value with `redactor`.
///
/// Only the `${VAR}` form is recognized; a bare `$` (or `$VAR` without braces)
/// is left untouched. Fails loudly if a referenced variable is unresolved or
/// the `${` is unterminated — never silently substitutes a default.
fn expand_vars(input: &str, lookup: &VarLookup<'_>, redactor: &Redactor) -> anyhow::Result<String> {
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
        redactor.register(&value);
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

/// An installed `*.yaml` that enabled nothing, and why. Carried out of the
/// loader as data for the same reason as [`SupersededSidecar`]: a file the user
/// put there deliberately, silently doing nothing, is exactly the failure this
/// crate refuses to ship.
#[derive(Debug, Clone)]
pub struct IgnoredSidecar {
    /// File stem — the provider the file was meant to be.
    pub provider: String,
    pub installed_path: PathBuf,
    pub reason: IgnoredReason,
}

/// Why an installed sidecar produced no provider. Both arms are user-visible;
/// adding a third one will not compile until every disclosure site names it.
#[derive(Debug, Clone)]
pub enum IgnoredReason {
    /// The build ships this provider, but the store does not have it switched
    /// on. Carries the state file to write and the content that switches it on.
    NotEnabled {
        state_path: PathBuf,
        /// The command that writes that file. Composed here so every disclosure
        /// site quotes one instruction that is known to work, rather than each
        /// inventing its own fragment of TOML.
        remedy: String,
        /// The file's content, for a disclosure with room for five lines.
        enabling_state_file: String,
    },
    /// The build ships no sidecar for this name, and presence is settled at
    /// compile time — so nothing on disk can introduce a provider.
    NotBundled,
}

/// What a scan of the integrations directory yielded.
#[derive(Debug)]
pub struct LoadedIntegrations {
    pub configs: Vec<(String, IntegrationFileConfig)>,
    pub superseded: Vec<SupersededSidecar>,
    pub ignored: Vec<IgnoredSidecar>,
}

/// Resolve which integrations run, and with what content.
///
/// The three axes stay separate. PRESENCE is settled at compile time by
/// [`crate::bundled_sidecars`], so this iterates the bundled list — a file on
/// disk cannot introduce a provider. ENABLEMENT comes from `store` and nowhere
/// else: an integration runs iff it is bundled AND its state says `enabled`.
/// CONTENT is the bundled sidecar, unless an installed file for that provider
/// declares this build's [`SIDECAR_SCHEMA_VERSION`], which is the deliberate
/// override; an installed file that does not is reported in
/// [`LoadedIntegrations::superseded`] and the bundled copy runs.
///
/// Every installed `*.yaml` that ends up producing no provider — for a disabled
/// provider, or for a name this build does not ship — is reported in
/// [`LoadedIntegrations::ignored`] so the caller discloses it. A missing
/// integrations directory just means no installed files; the store still
/// decides.
/// The CONTENT rule for one provider, in one place: the installed file when it
/// declares this build's [`SIDECAR_SCHEMA_VERSION`], the bundled copy
/// otherwise.
///
/// Returns the config plus, when an installed file was passed over, the reason
/// — which the caller turns into a [`SupersededSidecar`] disclosure. Enablement
/// is a separate axis and is deliberately not consulted here, so a surface that
/// must read a switched-OFF provider's sidecar (the consent flow) gets the same
/// answer the loader would give once it is switched on.
fn choose_content(
    bundled: &'static BundledSidecar,
    file: Option<&(PathBuf, String)>,
) -> anyhow::Result<(IntegrationFileConfig, Option<String>)> {
    let provider = bundled.provider;
    let Some((path, content)) = file else {
        return Ok((parse_bundled(bundled)?, None));
    };

    // Byte-identical to what we ship: the same file, not an override. Nothing
    // drifted, so nothing to log and nothing to disclose.
    if content == bundled.yaml {
        return Ok((parse_bundled(bundled)?, None));
    }

    let incompatibility = match serde_yaml::from_str::<IntegrationFileConfig>(content) {
        Ok(config) if config.schema_version == Some(SIDECAR_SCHEMA_VERSION) => {
            tracing::info!(
                "[load_integration_configs] Provider '{provider}' is OVERRIDDEN by '{}' \
                 (schema_version {SIDECAR_SCHEMA_VERSION}); the sidecar bundled at '{}' is not used",
                path.display(),
                bundled.source_path
            );
            return Ok((config, None));
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
            "it does not parse against this build's sidecar format, so no schema_version could be \
             established: {e}"
        ),
    };
    Ok((parse_bundled(bundled)?, Some(incompatibility)))
}

/// One provider's governing content, plus whatever the caller must disclose
/// about how it was chosen.
#[derive(Debug)]
pub struct ProviderContent {
    pub config: IntegrationFileConfig,
    /// Set when an installed sidecar was passed over for the bundled copy, and
    /// why. Dropping this on the consent path would let a user edit an
    /// installed sidecar, watch the flow ignore it, and get no hint that it
    /// did — the startup loader discloses the same fact, but a user
    /// configuring an integration is not reading startup logs.
    pub superseded: Option<String>,
}

/// The sidecar content that governs one provider, whether or not it is switched
/// on — what the in-app consent flow reads to learn the provider's OAuth
/// endpoints and where its credentials belong.
pub fn provider_content(dir: &Path, provider: &str) -> anyhow::Result<ProviderContent> {
    let bundled = bundled_sidecar(provider)
        .ok_or_else(|| anyhow::anyhow!("this build ships no integration named '{provider}'"))?;
    let installed = scan_installed_sidecars(dir)?;
    let files = installed.get(provider).map(Vec::as_slice).unwrap_or(&[]);
    anyhow::ensure!(
        files.len() <= 1,
        "Integration '{provider}' has {} installed sidecars — delete all but one, there is no rule \
         that picks between them",
        files.len()
    );
    let (config, superseded) = choose_content(bundled, files.first())?;
    Ok(ProviderContent { config, superseded })
}

pub fn load_integration_configs(
    dir: &Path,
    store: &IntegrationConfigStore,
) -> anyhow::Result<LoadedIntegrations> {
    let installed = scan_installed_sidecars(dir)?;
    let mut configs = Vec::new();
    let mut superseded = Vec::new();
    let mut ignored = Vec::new();

    for bundled in BUNDLED_SIDECARS {
        let provider = bundled.provider;
        let files = installed.get(provider).map(Vec::as_slice).unwrap_or(&[]);
        // Two files for a provider that RUNS disagree about what is running, and
        // there is no rule that picks between them. Two for a provider that
        // cannot run are handled below, where they are merely ignored.
        if files.len() > 1 {
            anyhow::bail!(
                "Integration '{provider}' has {} installed sidecars ({}) — delete all but one, \
                 there is no rule that picks between them",
                files.len(),
                files
                    .iter()
                    .map(|(p, _)| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let file = files.first();

        if !store.get(provider)?.enabled {
            if let Some((path, _)) = file {
                ignored.push(IgnoredSidecar {
                    provider: provider.to_string(),
                    installed_path: path.clone(),
                    reason: IgnoredReason::NotEnabled {
                        state_path: store.state_path(provider)?,
                        remedy: enable_remedy(dir, provider),
                        enabling_state_file: enabling_state_file(),
                    },
                });
            }
            continue;
        }

        let (config, incompatibility) = choose_content(bundled, file)?;
        if let Some(incompatibility) = incompatibility {
            let (path, _) = file.expect("only an installed file can be incompatible");
            superseded.push(SupersededSidecar {
                provider: provider.to_string(),
                installed_path: path.clone(),
                bundled_source: bundled.source_path,
                incompatibility,
            });
        }
        configs.push((provider.to_string(), config));
    }

    for (provider, files) in installed.iter() {
        if bundled_sidecar(provider).is_none() {
            // One entry per FILE: an unbundled name can enable nothing, so two
            // of them are two useless files, not an ambiguity to refuse over.
            for (path, _) in files {
                ignored.push(IgnoredSidecar {
                    provider: provider.clone(),
                    installed_path: path.clone(),
                    reason: IgnoredReason::NotBundled,
                });
            }
        }
    }

    // A state file for a name this build does not ship is read by nothing. It
    // is the shape a typo takes, so it is disclosed rather than left inert.
    for path in orphan_state_files(dir)? {
        let provider = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".state.toml"))
            .expect("orphan_state_files only yields '<provider>.state.toml' paths")
            .to_string();
        ignored.push(IgnoredSidecar {
            provider,
            installed_path: path,
            reason: IgnoredReason::NotBundled,
        });
    }
    // The bundled arm above walks a fixed list, but the unbundled one walks a
    // map, so sort to keep a boot's disclosures in a stable order.
    ignored.sort_by(|a, b| a.installed_path.cmp(&b.installed_path));

    Ok(LoadedIntegrations {
        configs,
        superseded,
        ignored,
    })
}

/// The exact command that writes `provider`'s state file INTO `dir`.
///
/// The directory is always named, even when it is the default one: this crate
/// cannot tell a default from an override (the default is composed from the
/// config dir, which lives a layer up), and a remedy that quietly writes
/// somewhere other than the path the same disclosure names is worse than no
/// remedy — it looks like it worked.
fn enable_remedy(dir: &Path, provider: &str) -> String {
    format!(
        "HOLON_MCP_INTEGRATIONS_DIR='{}' {ENABLE_COMMAND} {provider}",
        dir.display()
    )
}

/// The installed `*.yaml`/`*.yml` files in `dir`, grouped by file stem, with
/// their content. A missing directory yields nothing. Grouping rather than
/// overwriting keeps the "two files, one provider" case visible to the caller,
/// which is the only place that knows whether it matters.
fn scan_installed_sidecars(dir: &Path) -> anyhow::Result<HashMap<String, Vec<(PathBuf, String)>>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                "[load_integration_configs] Integrations directory '{}' does not exist — no \
                 installed sidecars",
                dir.display()
            );
            return Ok(HashMap::new());
        }
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!(
                "Failed to read integrations directory '{}'",
                dir.display()
            )));
        }
    };

    let mut installed: HashMap<String, Vec<(PathBuf, String)>> = HashMap::new();
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

        installed.entry(name).or_default().push((path, content));
    }
    for files in installed.values_mut() {
        files.sort_by(|a, b| a.0.cmp(&b.0));
    }
    Ok(installed)
}

/// The `*.state.toml` files in `dir` whose provider this build does not ship.
/// The store only ever looks up the providers it knows, so these are invisible
/// to it — this scan is what makes them findable.
fn orphan_state_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!(
                "Failed to read integrations directory '{}'",
                dir.display()
            )));
        }
    };

    let mut orphans = Vec::new();
    for entry in entries {
        let entry = entry
            .with_context(|| format!("Failed to read directory entry in '{}'", dir.display()))?;
        let path = entry.path();
        let Some(provider) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".state.toml"))
        else {
            continue;
        };
        if bundled_sidecar(provider).is_none() {
            orphans.push(path);
        }
    }
    orphans.sort();
    Ok(orphans)
}

fn parse_bundled(bundled: &'static BundledSidecar) -> anyhow::Result<IntegrationFileConfig> {
    let config = serde_yaml::from_str::<IntegrationFileConfig>(bundled.yaml)
        .with_context(|| format!("Bundled sidecar '{}' does not parse", bundled.source_path))?;
    refuse_machine_specific_command(&config, bundled.source_path)?;
    Ok(config)
}

/// Is `command` an absolute filesystem path — a location that exists on the
/// machine that wrote it and nowhere else?
///
/// Judged from the STRING, not from `Path::is_absolute`, which answers for the
/// host running the check: a `/Users/…` command compiled into a build shipped
/// to Windows would read as relative there, and the refusal must not depend on
/// who is compiling.
fn is_absolute_path(command: &str) -> bool {
    if command.starts_with('/') {
        return true;
    }
    let bytes = command.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Does `value` name somebody's home directory?
///
/// The narrow true subset of "absolute": `/usr/share/dict/words` is the same
/// file on every machine, but anything under a home root is one person's by
/// construction. Only the latter is refused, so the gate stays the width of the
/// defect.
fn is_home_directory_literal(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    if lowered.starts_with("/users/") || lowered.starts_with("/home/") {
        return true;
    }
    // `C:\Users\…` — the drive letter varies, the shape does not.
    let bytes = lowered.as_bytes();
    bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
        && lowered[2..]
            .trim_start_matches(['\\', '/'])
            .starts_with("users")
}

/// A BUNDLED sidecar may not name one machine — not as its child-process
/// command, and not as a home directory in the env it hands that process.
///
/// A bundled sidecar is compiled into the binary and ships to every user, so a
/// path that resolves on the author's machine makes the integration inert
/// everywhere else — it spawns ENOENT, the provider registers as unavailable,
/// and the pages that read it render errors that name none of this (bugfunnel
/// `2026-08-31-bundled-sidecar-hardcodes-developer-local-binary-path`).
///
/// Three portable command forms remain: a bare program name resolved through
/// `PATH`, a path relative to the working directory, and a `${VAR}` the
/// environment supplies. For env VALUES only home directories are refused —
/// `/usr/share/…` is the same file everywhere, so a blanket absolute-path
/// refusal there would be wider than the defect.
///
/// An INSTALLED sidecar under `~/.config/holon/integrations/` is deliberately
/// NOT checked — it describes one machine on purpose, and its author is the
/// person whose machine it is.
fn refuse_machine_specific_command(
    config: &IntegrationFileConfig,
    source_path: &str,
) -> anyhow::Result<()> {
    let Some(child_process) = config.transport.child_process.as_ref() else {
        return Ok(());
    };
    // `${VAR}` is expanded from the environment later, so a reference that
    // happens to start with a slash-prefixed variable is still the
    // environment's answer, not the repository's. The same holds for env
    // values, which `into_mcp_config_with` expands the same way.
    // Sorted, because `env` is a HashMap and a sidecar with two bad values
    // would otherwise name a different one on each run.
    let mut env: Vec<_> = child_process.env.iter().collect();
    env.sort();
    for (key, value) in env {
        if !value.contains("${") && is_home_directory_literal(value) {
            anyhow::bail!(
                "Bundled sidecar '{source_path}' sets env '{key}' to the home-directory literal \
                 '{value}'. That directory belongs to one person, so the value is wrong on every \
                 other machine the build reaches. Use ${{HOME}} (or another ${{VAR}} the \
                 environment supplies) instead."
            )
        }
    }
    if child_process.command.contains("${") || !is_absolute_path(&child_process.command) {
        return Ok(());
    }
    anyhow::bail!(
        "Bundled sidecar '{source_path}' names an absolute command path \
         '{}'. A bundled sidecar ships to every user, so a path that resolves on one machine \
         leaves the integration inert on all the others. Use a bare program name (resolved \
         through PATH), a relative path, or a ${{VAR}} the environment supplies.",
        child_process.command
    )
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
    fn load_configs_ignores_an_unbundled_yaml_and_skips_non_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("test-provider.yaml");
        std::fs::write(
            &installed,
            r#"
transport:
  child_process:
    command: echo
    args: ["hello"]
entities: {}
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let store = IntegrationConfigStore::load(dir.path()).unwrap();
        let loaded = load_integration_configs(dir.path(), &store).unwrap();
        assert!(loaded.configs.is_empty());
        assert_eq!(loaded.ignored.len(), 1, "the .txt is not a sidecar at all");
        assert_eq!(loaded.ignored[0].installed_path, installed);
        assert!(matches!(
            loaded.ignored[0].reason,
            IgnoredReason::NotBundled
        ));
    }

    #[test]
    fn load_configs_malformed_unbundled_yaml_is_ignored_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let bad_path = dir.path().join("bad.yaml");
        std::fs::write(&bad_path, "not: [valid: yaml: config").unwrap();

        let store = IntegrationConfigStore::load(dir.path()).unwrap();
        let loaded = load_integration_configs(dir.path(), &store)
            .expect("a file that can enable nothing cannot break the boot either");
        assert!(loaded.configs.is_empty());
        assert_eq!(loaded.ignored[0].installed_path, bad_path);
    }

    #[test]
    fn load_configs_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let store = IntegrationConfigStore::load(dir.path()).unwrap();
        let loaded = load_integration_configs(Path::new("/nonexistent/path"), &store).unwrap();
        assert!(loaded.configs.is_empty());
        assert!(loaded.ignored.is_empty());
    }

    #[test]
    fn two_installed_files_for_one_provider_fail_loud() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gcal.yaml"), "entities: {}\n").unwrap();
        std::fs::write(dir.path().join("gcal.yml"), "entities: {}\n").unwrap();

        let store = IntegrationConfigStore::load(dir.path()).unwrap();
        let err = load_integration_configs(dir.path(), &store)
            .expect_err("two files claiming one provider has no winner");
        assert!(format!("{err:#}").contains("installed sidecars"));
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
        let redactor = Redactor::new();
        assert_eq!(
            expand_vars("plain-token", &env_var_lookup, &redactor).unwrap(),
            "plain-token"
        );
        // bare $ (no braces) is left untouched
        assert_eq!(
            expand_vars("$HOME/x", &env_var_lookup, &redactor).unwrap(),
            "$HOME/x"
        );
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

#[cfg(test)]
mod presentation_fields {
    use super::*;

    const MINIMAL_TRANSPORT: &str = "\ntransport:\n  http:\n    uri: https://example.invalid/mcp\n";

    fn parse(extra: &str) -> Result<IntegrationFileConfig, serde_yaml::Error> {
        serde_yaml::from_str(&format!("{extra}{MINIMAL_TRANSPORT}"))
    }

    #[test]
    fn all_three_presentation_fields_are_optional() {
        let config = parse("").expect("a sidecar may state none of them");
        assert_eq!(config.display_name, None);
        assert_eq!(config.icon, None);
        assert_eq!(config.default_view, None);
    }

    #[test]
    fn a_sidecar_states_its_own_name_glyph_and_view() {
        let config = parse(
            "display_name: \"Claude History\"\nicon: robot\ndefault_view: claude-history-view\n",
        )
        .expect("all three parse");
        assert_eq!(config.display_name.as_deref(), Some("Claude History"));
        assert_eq!(config.icon.as_ref().map(|i| i.as_str()), Some("robot"));
        assert_eq!(config.default_view.as_deref(), Some("claude-history-view"));
    }

    /// A glyph name nobody draws would render as a bullet with no reader to
    /// notice, so it is refused where it enters instead.
    #[test]
    fn an_icon_the_renderer_cannot_draw_is_refused_at_parse() {
        let err = parse("icon: plug\n").expect_err("an unknown glyph name must not parse");
        let msg = err.to_string();
        assert!(
            msg.contains("plug"),
            "the refusal must name the bad glyph: {msg}"
        );
        assert!(
            msg.contains("ICON_NAMES"),
            "the refusal must say where the valid names are: {msg}"
        );
    }

    /// The bundled acceptance case, read the way the app reads it.
    #[test]
    fn the_bundled_claude_history_sidecar_carries_the_presentation_triple() {
        let bundled = crate::bundled_sidecars::bundled_sidecar("claude-history")
            .expect("claude-history is bundled");
        let config: IntegrationFileConfig =
            serde_yaml::from_str(bundled.yaml).expect("the bundled sidecar parses");
        assert_eq!(config.display_name.as_deref(), Some("Claude History"));
        assert_eq!(config.icon.as_ref().map(|i| i.as_str()), Some("robot"));
        assert_eq!(config.default_view.as_deref(), Some("claude-history-view"));
    }

    /// D53.c: every bundled sidecar states all three, so every Integrations
    /// row wears a glyph, reads as a name, and opens a page. The fields stay
    /// optional for a user's own sidecar —
    /// `all_three_presentation_fields_are_optional` covers that path.
    #[test]
    fn every_bundled_sidecar_carries_the_presentation_triple() {
        for bundled in crate::bundled_sidecars::BUNDLED_SIDECARS {
            let config: IntegrationFileConfig = serde_yaml::from_str(bundled.yaml)
                .unwrap_or_else(|e| panic!("{} parses: {e}", bundled.source_path));
            assert!(
                config.display_name.is_some()
                    && config.icon.is_some()
                    && config.default_view.is_some(),
                "{} must state display_name, icon and default_view — got {:?}/{:?}/{:?}",
                bundled.source_path,
                config.display_name,
                config.icon,
                config.default_view,
            );
        }
    }
}

#[cfg(test)]
mod bundled_command_portability {
    use super::*;

    /// The gate the bugfunnel entry asks for: every sidecar this build ships
    /// must be spawnable on a machine that is not the author's.
    #[test]
    fn every_bundled_sidecar_names_a_portable_command() {
        for bundled in BUNDLED_SIDECARS {
            parse_bundled(bundled)
                .unwrap_or_else(|e| panic!("bundled sidecar '{}': {e:#}", bundled.provider));
        }
    }

    /// The shape that shipped: an absolute path into one developer's `target/`
    /// directory. Refused with a message naming the path and the three forms
    /// that would have worked.
    #[test]
    fn an_absolute_command_in_a_bundled_sidecar_is_refused_by_name() {
        let config: IntegrationFileConfig = serde_yaml::from_str(
            "transport:\n  child_process:\n    command: \
             /Users/someone/Workspaces/ai/claude-code-history-mcp/target/debug/claude-code-history-mcp\n",
        )
        .expect("the yaml itself is well-formed");

        let err = refuse_machine_specific_command(&config, "assets/integrations/example.yaml")
            .expect_err("an absolute command must not survive the bundled boundary")
            .to_string();

        assert!(
            err.contains("target/debug"),
            "the refusal must quote the path: {err}"
        );
        assert!(
            err.contains("PATH"),
            "the refusal must name the remedy: {err}"
        );
    }

    #[test]
    fn the_three_portable_forms_are_accepted() {
        for command in [
            "claude-code-history-mcp",
            "npx",
            "./bin/connector",
            "${CLAUDE_HISTORY_MCP}",
            "${MCP_BIN_DIR}/connector",
        ] {
            let config: IntegrationFileConfig = serde_yaml::from_str(&format!(
                "transport:\n  child_process:\n    command: \"{command}\"\n"
            ))
            .expect("well-formed yaml");
            refuse_machine_specific_command(&config, "assets/integrations/example.yaml")
                .unwrap_or_else(|e| panic!("{command:?} is portable and must be accepted: {e:#}"));
        }
    }

    /// A Windows drive-letter path is machine-specific too, and the refusal
    /// must not depend on which platform compiled the check.
    #[test]
    fn a_windows_drive_path_is_machine_specific_on_every_host() {
        assert!(is_absolute_path("C:\\Users\\someone\\connector.exe"));
        assert!(is_absolute_path("/opt/connector"));
        assert!(!is_absolute_path("connector"));
        assert!(!is_absolute_path("./connector"));
    }

    /// The line `claude-history.yaml` shipped beside the bad command:
    /// `CLAUDE_DATA_DIR: /Users/martin/.claude`. Reconstructed here rather than
    /// left in the yaml, so the gate keeps describing the defect after the
    /// asset is fixed.
    #[test]
    fn a_home_directory_env_value_in_a_bundled_sidecar_is_refused_by_name() {
        let config: IntegrationFileConfig = serde_yaml::from_str(
            "transport:\n  child_process:\n    command: claude-code-history-mcp\n    env:\n      \
             CLAUDE_DATA_DIR: \"/Users/martin/.claude\"\n",
        )
        .expect("the yaml itself is well-formed");

        let err = refuse_machine_specific_command(&config, "assets/integrations/example.yaml")
            .expect_err("a home-directory env value must not survive the bundled boundary")
            .to_string();

        assert!(
            err.contains("CLAUDE_DATA_DIR"),
            "the refusal must name the variable: {err}"
        );
        assert!(
            err.contains("/Users/martin/.claude"),
            "the refusal must quote the value: {err}"
        );
        assert!(
            err.contains("${HOME}"),
            "the refusal must name the required form: {err}"
        );
    }

    /// An absolute env value that is NOT under a home directory stays legal —
    /// a system path is the same on every machine, and refusing it would be a
    /// gate wider than the defect.
    #[test]
    fn a_system_absolute_env_value_is_left_alone() {
        for value in ["/usr/share/dict/words", "/etc/ssl/certs", "${HOME}/.claude"] {
            let config: IntegrationFileConfig = serde_yaml::from_str(&format!(
                "transport:\n  child_process:\n    command: connector\n    env:\n      DATA: \"{value}\"\n"
            ))
            .expect("well-formed yaml");
            refuse_machine_specific_command(&config, "assets/integrations/example.yaml")
                .unwrap_or_else(|e| panic!("{value:?} is not machine-specific: {e:#}"));
        }
    }

    /// A transport with no child process has no command to judge.
    #[test]
    fn an_http_only_sidecar_passes_without_a_command() {
        let config: IntegrationFileConfig =
            serde_yaml::from_str("transport:\n  http:\n    uri: https://example.invalid/mcp\n")
                .expect("well-formed yaml");
        refuse_machine_specific_command(&config, "assets/integrations/example.yaml")
            .expect("no child_process means nothing to refuse");
    }
}
