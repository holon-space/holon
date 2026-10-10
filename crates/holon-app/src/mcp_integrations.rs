use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context as _;
use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::Provider;
use fluxdi::Shared;
use holon::api::BackendEngine;
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKey;
use holon_api::ConditionKind;
use holon_api::EntityName;
use holon_api::lifecycle::SessionShutdown;
use holon_core::OperationProvider;
use holon_core::SyncGate;
use holon_core::SyncTokenStore;
use holon_core::integration_attribution::IntegrationAttribution;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IgnoredReason;
use holon_mcp_client::IgnoredSidecar;
use holon_mcp_client::InertIntegration;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::LoadedIntegrations;
use holon_mcp_client::McpIntegration;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::PendingWriteStore;
use holon_mcp_client::SupersededSidecar;
use holon_mcp_client::build_mcp_integration;
use holon_mcp_client::integration_config::UnresolvedVar;
use holon_mcp_client::load_integration_configs;
use holon_mcp_client::oauth_bootstrap::BrowserOpener;
use holon_profiles::TypeRegistry;
use tracing::error;
use tracing::info;
use tracing::warn;

use crate::integration_projection::IntegrationStatus;
use crate::integrations_settings::IntegrationsSettingsVm;

/// Record `name`'s boot outcome in the `integration_state` mirror, so the
/// left-sidebar Integrations section shows an enabled-but-broken integration as
/// broken rather than as healthy.
///
/// A failure here is logged, not fatal: the status column going stale must
/// never take down a boot that otherwise succeeded. The row itself is written
/// by `IntegrationStateProjector`, so a miss leaves `Pending` — visibly
/// unresolved rather than a wrong claim.
/// The same call also stamps the verdict onto `name`'s declared tables, so the
/// sidebar row and the render path can never disagree about whether an
/// integration is running.
async fn record_status(
    db: &holon::storage::DbHandle,
    attribution: &holon_core::integration_attribution::IntegrationAttribution,
    name: &str,
    status: crate::integration_projection::IntegrationStatus,
    cause: &str,
) {
    // A cause can be a peer's or a sidecar's own text, and it lands in the
    // Integrations row.
    let cause = &holon_mcp_client::bounded_peer_text(cause);
    attribution.set_status(name, status, cause);
    if let Err(e) = crate::integration_projection::set_integration_status(db, name, status).await {
        warn!(
            "[McpIntegrationsModule] Could not record boot status for '{name}' — the \
             Integrations section will show it as Pending: {e:#}"
        );
    }
}

/// The status word a sync health reading justifies for a CONNECTED provider.
///
/// `Untried` keeps the row at whatever the projector born it as (`Pending`) —
/// the loop has not spoken, and claiming either verdict would be inventing one.
pub(crate) fn status_for_sync_health(
    health: holon_mcp_client::SyncHealth,
) -> Option<crate::integration_projection::IntegrationStatus> {
    use crate::integration_projection::IntegrationStatus;
    match health {
        holon_mcp_client::SyncHealth::Untried => None,
        holon_mcp_client::SyncHealth::Healthy => Some(IntegrationStatus::Connected),
        holon_mcp_client::SyncHealth::Failing => Some(IntegrationStatus::SyncFailing),
    }
}

/// Follow one connected integration's sync health for the life of the SESSION,
/// writing the status its batches actually justify.
///
/// A task rather than a one-shot write because the verdict is not available at
/// connect: the initial sync is only ENQUEUED there, so the first batch has not
/// run and every status written at that point is a guess.
///
/// Registered on `shutdown` because it holds a `DbHandle` and writes through
/// it — an unregistered task would outlive the store it writes to, the same
/// contract every other integration watcher keeps
/// (`IntegrationStateProjector::reproject_on`).
fn spawn_status_from_sync_health(
    db: holon::storage::DbHandle,
    attribution: holon_core::integration_attribution::IntegrationAttribution,
    name: String,
    mut health: tokio::sync::watch::Receiver<holon_mcp_client::SyncHealth>,
    shutdown: &holon_api::lifecycle::SessionShutdown,
) {
    let cancelled = shutdown.cancelled();
    let pump = async move {
        while health.changed().await.is_ok() {
            let Some(status) = status_for_sync_health(*health.borrow_and_update()) else {
                continue;
            };
            let cause = match status {
                crate::integration_projection::IntegrationStatus::SyncFailing => {
                    "connected, but no sync has succeeded — see the log for the batch error"
                }
                _ => "",
            };
            record_status(&db, &attribution, &name, status, cause).await;
        }
    };
    shutdown.spawn("integration-sync-status", async move {
        tokio::select! {
            biased;
            () = cancelled => {}
            () = pump => {}
        }
    });
}

/// Follow one connected integration's inbound signal bound for the life of the
/// SESSION, disclosing each collapse.
///
/// A task rather than a one-shot read at connect: when a peer floods is the
/// peer's choice, and it is always after the connect.
fn spawn_disclose_signal_loss(
    bus: Arc<ConditionBus>,
    name: String,
    mut signals: tokio::sync::watch::Receiver<Option<String>>,
    shutdown: &holon_api::lifecycle::SessionShutdown,
) {
    let cancelled = shutdown.cancelled();
    let pump = async move {
        // The current value first: a peer can flood during the connect, which
        // is before this task exists, and waiting for the next change would
        // leave that one disclosed in the log only.
        loop {
            let collapsed = signals.borrow_and_update().clone();
            if let Some(reason) = collapsed {
                disclose_signals_collapsed(&name, &reason, &bus);
            }
            if signals.changed().await.is_err() {
                break;
            }
        }
    };
    shutdown.spawn("integration-signal-loss", async move {
        tokio::select! {
            biased;
            () = cancelled => {}
            () = pump => {}
        }
    });
}

/// Declare the tables `provider` owns BEFORE the connect attempt.
///
/// A sidecar names its entities whether or not the remote ever answers, so
/// declaring here is what makes a FAILED connect attributable: without it the
/// matview over `cc_session` fails hours later owned by nobody.
fn declare_entity_tables(
    attribution: &holon_core::integration_attribution::IntegrationAttribution,
    provider: &str,
    display_name: &str,
    config: &holon_mcp_client::IntegrationFileConfig,
) {
    for key in config.entities.keys() {
        let table = holon_mcp_client::mcp_sidecar::canonical_entity_name(
            config.entity_prefix.as_deref(),
            key,
        )
        .table_name();
        attribution.declare(
            table,
            holon_core::integration_attribution::TableOwner {
                integration: provider.to_string(),
                display_name: display_name.to_string(),
                status: crate::integration_projection::IntegrationStatus::Pending,
                cause: String::new(),
            },
        );
    }
}

/// The boot cause worth showing a user.
///
/// A sidecar whose `command` could not be resolved carries the classification
/// (not installed vs. not on this process's PATH) — the two need different
/// remedies, and the raw `ENOENT` tells them apart for nobody.
fn boot_cause(error: &anyhow::Error) -> String {
    match error.downcast_ref::<holon_mcp_client::command_resolution::SidecarCommandUnavailable>() {
        Some(unavailable) => unavailable.to_string(),
        None => format!("{error:#}"),
    }
}

/// Disclose that `name` could not be connected at boot. Without this the
/// failure is log-only and every page backed by the integration's `cc_*`
/// tables renders blank as if the remote had no data.
fn disclose_connect_failure(name: &str, error: &anyhow::Error, bus: &ConditionBus) {
    bus.emit(Condition {
        subject: name.to_string(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: name.to_string(),
            error: holon_mcp_client::bounded_peer_text(&format!("{error:#}")),
        },
    });
}

/// Disclose that `name`'s config was refused, so the connection does not run.
///
/// Same condition as a file the directory scan refuses
/// ([`disclose_ignored_sidecar`]'s `Unusable` arm) and deliberately the same
/// banner: from the user's side both are "this file names a connection that
/// cannot be used, edit it". The seam differs only in WHEN the rule fires —
/// the scan reads the text, this one resolves the variables.
///
/// `origin` is `None` for a connection this build ships, where the file to fix
/// is not the user's.
fn disclose_unusable_config(
    name: &str,
    origin: Option<&str>,
    error: &anyhow::Error,
    bus: &ConditionBus,
) {
    bus.emit(Condition {
        subject: name.to_string(),
        reason: ConditionKind::IntegrationSidecarUnusable {
            provider: name.to_string(),
            installed_path: origin.unwrap_or("(bundled with this build)").to_string(),
            why: holon_mcp_client::bounded_peer_text(&format!("{error:#}")),
        },
    });
}

/// Disclose that `name` connected but came up without part of what its peer
/// publishes.
///
/// Not a connect failure: the tool list is the dispatch surface and it came
/// through, so the integration serves its operations and taking those away
/// would be the worse outcome. What is degraded is the auto-discovered
/// entities, whose absence otherwise reads as "this peer has none".
fn disclose_discovery_incomplete(name: &str, error: &str, bus: &ConditionBus) {
    warn!(
        "[IntegrationSupervisor] Provider '{name}' connected with INCOMPLETE discovery: {error}.          Entities its resource templates would have declared are not registered and do not sync."
    );
    bus.emit(Condition {
        subject: name.to_string(),
        reason: ConditionKind::IntegrationDiscoveryIncomplete {
            integration: name.to_string(),
            error: holon_mcp_client::bounded_peer_text(error),
        },
    });
}

/// Disclose that `name`'s peer pushed more resource-change signals than one
/// connection may hold.
///
/// Not a sync failure and not a lost update: every signal that did not fit was
/// widened into a full re-sync, so the rows are current. What the user is told
/// is that this peer's freshness is coarse, because a peer doing this
/// repeatedly turns every notice into a full re-sync of everything it syncs.
fn disclose_signals_collapsed(name: &str, reason: &str, bus: &ConditionBus) {
    warn!(
        "[IntegrationSupervisor] Provider '{name}' pushed more resource-change signals than one \
         connection may hold: {reason}. Every entity it syncs is re-synced once instead of the \
         resources it named."
    );
    bus.emit(Condition {
        subject: name.to_string(),
        reason: ConditionKind::IntegrationChangeSignalsCollapsed {
            integration: name.to_string(),
            reason: holon_mcp_client::bounded_peer_text(reason),
        },
    });
}

/// Disclose that `name` is connectable but waiting on an OAuth grant — same
/// blank-page consequence as a failed connect, different remedy.
fn disclose_needs_auth(name: &str, auth_url: &str, bus: &ConditionBus) {
    bus.emit(Condition {
        subject: name.to_string(),
        reason: ConditionKind::IntegrationNeedsAuth {
            integration: name.to_string(),
            auth_url: auth_url.to_string(),
        },
    });
}

/// Disclose that an installed sidecar was ignored in favour of the bundled
/// one. The integration works, so nothing else in the boot path would ever say
/// that the file on disk is not what is running.
fn disclose_superseded_sidecar(s: &SupersededSidecar, bus: &ConditionBus) {
    bus.emit(Condition {
        subject: s.provider.clone(),
        reason: ConditionKind::IntegrationSidecarSuperseded {
            integration: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
            bundled_source: s.bundled_source.to_string(),
            incompatibility: s.incompatibility.clone(),
        },
    });
}

/// Disclose that an installed sidecar produced no provider at all. Its pages
/// render blank exactly like a failed connect, but nothing else in the boot
/// path would say why — the file is present, so from the user's side it looks
/// like the integration should be running.
fn disclose_ignored_sidecar(s: &IgnoredSidecar, bus: &ConditionBus) {
    let reason = match &s.reason {
        IgnoredReason::NotEnabled {
            state_path, remedy, ..
        } => ConditionKind::IntegrationNotEnabled {
            integration: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
            state_path: state_path.display().to_string(),
            remedy: remedy.clone(),
        },
        IgnoredReason::NotBundled => ConditionKind::IntegrationSidecarNotBundled {
            provider: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
        },
        IgnoredReason::Unusable { why } => ConditionKind::IntegrationSidecarUnusable {
            provider: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
            why: why.clone(),
        },
    };
    bus.emit(Condition {
        subject: s.provider.clone(),
        reason,
    });
}

/// The boot log line for an installed sidecar that enabled nothing. The bus
/// carries the paths; the remedy is spelled out here, where a multi-line state
/// file fits.
fn log_ignored_sidecar(s: &IgnoredSidecar) {
    match &s.reason {
        IgnoredReason::NotEnabled {
            state_path,
            remedy,
            enabling_state_file,
        } => warn!(
            "[McpIntegrationsModule] Provider '{}' is NOT enabled, so '{}' does nothing — a \
             sidecar file is no longer the switch. To switch it on, run `{remedy}`, or write \
             '{}' yourself — ALL of it, a partial file is rejected:\n{}",
            s.provider,
            s.installed_path.display(),
            state_path.display(),
            enabling_state_file
        ),
        IgnoredReason::NotBundled => warn!(
            "[McpIntegrationsModule] '{}' refers to connection '{}', which nothing provides — \
             this build ships no sidecar for it and no usable file introduces one. Delete the \
             leftover file, or add a '{}.yaml' that declares this build's schema_version.",
            s.installed_path.display(),
            s.provider,
            s.provider
        ),
        IgnoredReason::Unusable { why } => warn!(
            "[McpIntegrationsModule] '{}' names connection '{}' but cannot be used, so that \
             connection does not exist: {why}",
            s.installed_path.display(),
            s.provider
        ),
    }
}

/// Log and disclose a provider that is switched on but has no credentials in
/// this profile.
///
/// Its pages render blank exactly like a failed connect, and from the user's
/// side the switch is ON — so saying nothing here is the silent-degradation
/// this module exists to refuse. It reads as `Unavailable` rather than
/// `Connected`, because nothing was connected.
fn log_inert_integration(i: &InertIntegration) {
    warn!(
        "[McpIntegrationsModule] {} It stays inert until it is configured: {}. Its state file is \
         '{}'.",
        i.reason,
        i.remedy,
        i.state_path.display()
    );
}

fn disclose_inert_integration(i: &InertIntegration, bus: &ConditionBus) {
    bus.emit(Condition {
        subject: i.provider.clone(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: i.provider.clone(),
            error: format!("{} Remedy: {}", i.reason, i.remedy),
        },
    });
}

/// A `${VAR}` resolution still running after this long is waiting on the
/// keychain: the environment and the preferences answer at once.
const KEYCHAIN_SLOW: Duration = Duration::from_secs(2);

/// A connect still running after this long is disclosed, not cancelled: a
/// first-run `npx` sidecar can legitimately take that long.
const CONNECT_SLOW: Duration = Duration::from_secs(30);

/// Connects the session's integrations, each in its own session-scoped task,
/// and registers each one into the running engine when IT connects.
///
/// The session factory starts it and does not wait for any connect, so no
/// remote system is on the boot path. On connect an integration's types,
/// FDW tables, clock grains and matview hook are installed BEFORE its
/// operations reach the dispatcher: the catalog change is what re-renders
/// pages over its tables (`UiWatcher` follows the profile signal).
pub struct IntegrationSupervisor {
    configs: Arc<Vec<(String, IntegrationFileConfig)>>,
    superseded: Arc<Vec<SupersededSidecar>>,
    ignored: Arc<Vec<IgnoredSidecar>>,
    inert: Arc<Vec<InertIntegration>>,
    settings_vm: Arc<IntegrationsSettingsVm>,
    credential_root: CredentialRoot,
    pending_flows: Arc<PendingOAuthFlows>,
    pending_writes: Arc<PendingWriteStore>,
    db: holon::storage::DbHandle,
    attribution: IntegrationAttribution,
    bus: Arc<ConditionBus>,
    shutdown: Arc<SessionShutdown>,
    cache_factory: Arc<dyn holon_core::CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    type_registry: Arc<TypeRegistry>,
    sync_gate: SyncGate,
    keychain: Arc<dyn holon_secrets::KeychainStore>,
    /// Its `preferences` are the last place a `${VAR}` is looked up.
    holon_config: Arc<holon_frontend::config::HolonConfig>,
    started: AtomicBool,
    /// Held so each connected integration's services stay alive.
    connected: Mutex<Vec<Arc<McpIntegration>>>,
}

impl IntegrationSupervisor {
    /// Disclose what the integrations directory enabled, record every
    /// integration's first status, and spawn one connect task per integration.
    ///
    /// Awaits only the local store; never a connect.
    pub async fn start(self: &Arc<Self>, engine: Arc<BackendEngine>) {
        assert!(
            !self.started.swap(true, Ordering::SeqCst),
            "[IntegrationSupervisor] started twice — every integration would connect twice and \
             its operations would be refused as duplicates"
        );
        for s in self.superseded.iter() {
            disclose_superseded_sidecar(s, &self.bus);
        }
        for s in self.ignored.iter() {
            disclose_ignored_sidecar(s, &self.bus);
        }
        for i in self.inert.iter() {
            disclose_inert_integration(i, &self.bus);
        }

        // The status writes below are refused for a provider with no enabled
        // row, and the session factory's projector runs after this.
        crate::integration_projection::IntegrationStateProjector::new(
            self.db.clone(),
            self.settings_vm.clone(),
        )
        .project()
        .await
        .unwrap_or_else(|e| {
            panic!(
                "[IntegrationSupervisor] Could not populate the integration_state mirror before \
                 connecting ({e:#}) — every provider's status would be refused and the \
                 Integrations section would read Pending forever."
            )
        });

        for i in self.inert.iter() {
            self.record_status(&i.provider, IntegrationStatus::Unavailable, &i.reason)
                .await;
        }

        let presentation: HashMap<String, (String, Option<String>)> = self
            .settings_vm
            .rows()
            .into_iter()
            .map(|row| (row.provider.to_string(), (row.display_name, row.origin)))
            .collect();

        for (name, config) in self.configs.iter() {
            let (display_name, origin) = presentation.get(name).cloned().unwrap_or_else(|| {
                warn!(
                    "[IntegrationSupervisor] provider '{name}' has no row in the enablement \
                     store — its banners use the raw provider name"
                );
                (name.clone(), None)
            });
            declare_entity_tables(&self.attribution, name, &display_name, config);
            self.record_status(name, IntegrationStatus::Connecting, "")
                .await;

            let supervisor = self.clone();
            let engine = engine.clone();
            let name = name.clone();
            let config = config.clone();
            let cancelled = self.shutdown.cancelled();
            let bus = self.bus.clone();
            let panicked = name.clone();
            self.shutdown.spawn_disclosing_panic(
                format!("integration-connect:{name}"),
                async move {
                    tokio::select! {
                        biased;
                        () = cancelled => {}
                        () = supervisor.connect(engine, name, config, origin) => {}
                    }
                },
                move |panic| {
                    bus.emit(Condition {
                        subject: panicked.clone(),
                        reason: ConditionKind::IntegrationConnectFailed {
                            integration: panicked,
                            error: format!("its connect task panicked: {panic}"),
                        },
                    })
                },
            );
        }
    }

    async fn connect(
        self: Arc<Self>,
        engine: Arc<BackendEngine>,
        name: String,
        config: IntegrationFileConfig,
        origin: Option<String>,
    ) {
        let mcp_config = match self.resolve_vars(&name, config).await {
            Ok(c) => c,
            Err(e) if e.downcast_ref::<UnresolvedVar>().is_some() => {
                warn!(
                    "[IntegrationSupervisor] Provider '{name}' is not configured — skipping: {e}"
                );
                disclose_connect_failure(&name, &e, &self.bus);
                self.record_status(&name, IntegrationStatus::Unavailable, &format!("{e}"))
                    .await;
                return;
            }
            // The file names a connection this build will not run. Installed
            // files are user-supplied, so the session stays up (D94.a) and the
            // file and the reason are disclosed.
            Err(e) => {
                warn!(
                    "[IntegrationSupervisor] Provider '{name}' is NOT running: {e:#}. Fix the \
                     file and restart; the rest of the app is unaffected."
                );
                disclose_unusable_config(&name, origin.as_deref(), &e, &self.bus);
                self.record_status(&name, IntegrationStatus::Unavailable, &format!("{e}"))
                    .await;
                return;
            }
        };

        let connect = build_mcp_integration(
            mcp_config,
            self.db.clone(),
            self.cache_factory.clone(),
            self.token_store.clone(),
            &self.pending_flows,
            self.sync_gate.clone(),
        );
        let result = self.disclosing_slow(&name, connect).await;

        match result {
            Ok(holon_mcp_client::McpConnectionResult::Connected(mut integration)) => {
                info!(
                    "[IntegrationSupervisor] Provider '{name}' connected ({} operations)",
                    integration.operation_provider.operations().len()
                );
                if let Some(why) = integration.discovery_incomplete.clone() {
                    disclose_discovery_incomplete(&name, &why, &self.bus);
                }
                integration.set_pending_store(self.pending_writes.clone());
                if let Err(e) = self
                    .go_live(&engine, &name, origin.as_deref(), Arc::new(integration))
                    .await
                {
                    error!(
                        "[IntegrationSupervisor] Provider '{name}' connected but could not go live: {e:#}"
                    );
                    disclose_connect_failure(&name, &e, &self.bus);
                    self.record_status(&name, IntegrationStatus::Unavailable, &format!("{e:#}"))
                        .await;
                }
            }
            Ok(holon_mcp_client::McpConnectionResult::NeedsAuth {
                auth_url,
                provider_name,
            }) => {
                warn!(
                    "[IntegrationSupervisor] Provider '{provider_name}' needs OAuth — auth_url: {auth_url}"
                );
                assert_eq!(
                    provider_name, name,
                    "the connect result named provider '{provider_name}' for the config keyed \
                     '{name}' — the attribution and status keys have diverged and this \
                     integration's tables would never be marked"
                );
                disclose_needs_auth(&name, &auth_url, &self.bus);
                self.record_status(
                    &name,
                    IntegrationStatus::NeedsAuth,
                    &format!("sign in at {auth_url}"),
                )
                .await;
            }
            Err(e) => {
                warn!("[IntegrationSupervisor] Failed to connect provider '{name}': {e}");
                disclose_connect_failure(&name, &e, &self.bus);
                self.record_status(&name, IntegrationStatus::Unavailable, &boot_cause(&e))
                    .await;
            }
        }
    }

    /// Expand `config`'s `${VAR}`s on a blocking thread: the keychain read is a
    /// synchronous OS call that can wait on the user's answer to an access
    /// prompt for any length of time.
    async fn resolve_vars(
        &self,
        name: &str,
        config: IntegrationFileConfig,
    ) -> anyhow::Result<holon_mcp_client::McpIntegrationConfig> {
        let keychain = self.keychain.clone();
        let holon_config = self.holon_config.clone();
        let root = self.credential_root.clone();
        let provider = name.to_string();
        let mut resolving = tokio::task::spawn_blocking(move || {
            let lookup = holon_frontend::integration_vars::preference_var_lookup(
                &holon_config.preferences,
                |var| std::env::var(var).ok(),
                keychain.as_ref(),
            );
            config.into_mcp_config_with(provider, &lookup, &root)
        });
        let joined = match tokio::time::timeout(KEYCHAIN_SLOW, &mut resolving).await {
            Ok(joined) => joined,
            Err(_) => {
                self.record_status(name, IntegrationStatus::WaitingOnKeychain, "")
                    .await;
                self.bus.emit(Condition {
                    subject: name.to_string(),
                    reason: ConditionKind::IntegrationWaitingOnKeychain {
                        integration: name.to_string(),
                    },
                });
                let joined = resolving.await;
                self.bus.clear(&ConditionKey {
                    subject: name.to_string(),
                    kind: ConditionKind::INTEGRATION_WAITING_ON_KEYCHAIN,
                });
                self.record_status(name, IntegrationStatus::Connecting, "")
                    .await;
                joined
            }
        };
        joined.unwrap_or_else(|e| {
            panic!("[IntegrationSupervisor] resolving '{name}'s `${{VAR}}`s panicked: {e}")
        })
    }

    /// Await `connect`, raising `IntegrationConnectSlow` while it runs past
    /// [`CONNECT_SLOW`].
    async fn disclosing_slow<T>(&self, name: &str, connect: impl Future<Output = T>) -> T {
        tokio::pin!(connect);
        match tokio::time::timeout(CONNECT_SLOW, &mut connect).await {
            Ok(done) => done,
            Err(_) => {
                self.bus.emit(Condition {
                    subject: name.to_string(),
                    reason: ConditionKind::IntegrationConnectSlow {
                        integration: name.to_string(),
                        elapsed_secs: CONNECT_SLOW.as_secs(),
                    },
                });
                let done = connect.await;
                self.bus.clear(&ConditionKey {
                    subject: name.to_string(),
                    kind: ConditionKind::INTEGRATION_CONNECT_SLOW,
                });
                done
            }
        }
    }

    /// Install a connected integration into the running engine. Its operations
    /// reach the dispatcher last: that catalog change re-renders the pages over
    /// its tables, which must exist and be primed by then.
    async fn go_live(
        &self,
        engine: &BackendEngine,
        name: &str,
        origin: Option<&str>,
        integration: Arc<McpIntegration>,
    ) -> anyhow::Result<()> {
        integration
            .register_entity_types(&self.type_registry)
            .with_context(|| format!("registering the entity types of '{name}'"))?;
        for table in &integration.fdw_backed_tables {
            engine.register_fdw_table(table).await;
        }
        for grain in &integration.clock_grains {
            engine.hold_clock_grain(*grain).await.with_context(|| {
                format!(
                    "starting the '{}' clock grain '{name}' reads",
                    grain.as_str()
                )
            })?;
        }
        engine
            .add_matview_hook(integration.sync_engine.clone() as Arc<dyn holon_core::MatviewHook>)
            .await;

        let dispatcher = engine.get_dispatcher();
        dispatcher
            .register_provider(Arc::new(ConnectedOperations(integration.clone())))
            .map_err(|e| anyhow::anyhow!("registering the operations of '{name}': {e}"))?;
        for type_def in self.type_registry.all() {
            if type_def.owning_integration() == Some(name) {
                holon::core::type_declaration::derive_write_authority(
                    &type_def,
                    &self.db,
                    &dispatcher,
                )
                .map_err(|e| {
                    anyhow::anyhow!(
                        "deriving the write authority of '{}', mirrored by '{name}': {e}",
                        type_def.name
                    )
                })?;
            }
        }
        if let Err(e) = self.register_list(&dispatcher, name, &integration) {
            warn!("[IntegrationSupervisor] Provider '{name}' runs without its remote list: {e:#}");
            disclose_unusable_config(name, origin, &e, &self.bus);
        }

        // Enqueued through the integration's serialized sync loop, so it never
        // overlaps a notification resync.
        integration
            .request_initial_sync()
            .with_context(|| format!("enqueuing the initial sync of '{name}'"))?;
        // Connecting proves the peer answers, not that its rows land: an
        // integration that syncs earns `Connected` from its first batch.
        let syncs = integration.sync_engine.has_sync_entities();
        self.record_status(
            name,
            if syncs {
                IntegrationStatus::Syncing
            } else {
                IntegrationStatus::Connected
            },
            "",
        )
        .await;
        if let Some(signals) = integration.signal_loss.clone() {
            spawn_disclose_signal_loss(self.bus.clone(), name.to_string(), signals, &self.shutdown);
        }
        if syncs {
            spawn_status_from_sync_health(
                self.db.clone(),
                self.attribution.clone(),
                name.to_string(),
                integration.sync_engine.health().subscribe(),
                &self.shutdown,
            );
        }
        self.connected
            .lock()
            .expect("connected-integration list poisoned")
            .push(integration);
        Ok(())
    }

    /// Serve the remote list `name`'s sidecar declares, if it declares one.
    fn register_list(
        &self,
        dispatcher: &holon::api::operation_dispatcher::OperationDispatcher,
        name: &str,
        integration: &McpIntegration,
    ) -> anyhow::Result<()> {
        let Some(spec) = self
            .configs
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, config)| config.holon.as_ref()?.list_sync.clone())
        else {
            return Ok(());
        };
        let list = crate::remote_list::configured_list(
            name,
            spec,
            integration,
            &self.db,
            &self.type_registry,
        )
        .map_err(|why| anyhow::anyhow!("its `holon.list_sync` cannot be served: {why}"))?;
        dispatcher
            .register_provider(Arc::new(holon_connections::ConfiguredRemoteLists::new(
                Arc::new(holon_connections::SystemRoundClock),
                holon_connections::ConfiguredLists::new(vec![list], Vec::new()),
            )))
            .map_err(|e| anyhow::anyhow!("registering the remote list of '{name}': {e}"))
    }

    async fn record_status(&self, name: &str, status: IntegrationStatus, cause: &str) {
        record_status(&self.db, &self.attribution, name, status, cause).await;
    }
}

/// A connected integration's operations, as the dispatcher routes to them.
struct ConnectedOperations(Arc<McpIntegration>);

#[async_trait::async_trait]
impl OperationProvider for ConnectedOperations {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        self.0.operation_provider.operations()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: holon_core::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        self.0
            .operation_provider
            .execute_operation(entity_name, op_name, params)
            .await
    }
}

/// DI module that registers MCP provider integrations from config files.
///
/// Registers the [`IntegrationSupervisor`] that connects them; the session
/// factory starts it.
pub struct McpIntegrationsModule {
    /// The enablement authority plus the scan of the integrations directory, or
    /// the enriched load error (e.g. malformed YAML for a provider this build
    /// does not ship). The error is surfaced in `configure()` so it propagates
    /// through the module-registration `Result` instead of being swallowed
    /// here.
    loaded: Result<(Arc<IntegrationConfigStore>, LoadedIntegrations), String>,
    /// The active profile's config directory — the only place a sidecar's
    /// credential files may live. Established here, at the one boot site that
    /// knows which profile is running, and passed to every surface that
    /// resolves a credential.
    root: CredentialRoot,
    /// Where `integration.begin_oauth` sends the user to consent. The default
    /// is the desktop's own URL handler; a test supplies one that opens
    /// nothing.
    browser: Arc<dyn BrowserOpener>,
}

impl McpIntegrationsModule {
    /// Create a module for the integrations in `dir`.
    ///
    /// Which ones run is the state store's call — `dir` supplies the store's
    /// files and any content overrides, never the enablement. A directory that
    /// cannot be read, or that holds two files for one provider, is a hard
    /// error: it is captured here and returned from `configure()` (fail loud,
    /// never boot on a half-read integrations directory).
    pub fn from_dir(dir: &Path, config_dir: &Path) -> Self {
        let root = CredentialRoot::new(config_dir);
        let loaded = IntegrationConfigStore::load(dir)
            .map(Arc::new)
            .and_then(|store| load_integration_configs(dir, &store, &root).map(|l| (store, l)))
            .map_err(|e| format!("{e:#}"));
        if let Ok((_, loaded)) = &loaded {
            // Logged here, not at disclosure time: the registry singleton is
            // resolved lazily, so the bus signal may never fire in a container
            // that never touches an integration — the log must not depend on it.
            for s in &loaded.superseded {
                warn!(
                    "[McpIntegrationsModule] Installed sidecar '{}' for provider '{}' was NOT \
                     used: {}. Running the sidecar bundled with this build ('{}') instead. To \
                     silence this, delete the installed file, or re-author it against this \
                     build's schema_version.",
                    s.installed_path.display(),
                    s.provider,
                    s.incompatibility,
                    s.bundled_source
                );
            }
            for s in &loaded.ignored {
                log_ignored_sidecar(s);
            }
            for i in &loaded.inert {
                log_inert_integration(i);
            }
            info!(
                "[McpIntegrationsModule] {} integration(s) enabled from '{}' ({} installed \
                 sidecar(s) superseded by the bundled copy, {} enabling nothing)",
                loaded.configs.len(),
                dir.display(),
                loaded.superseded.len(),
                loaded.ignored.len()
            );
        }
        Self {
            loaded,
            root,
            browser: Arc::new(holon_mcp_client::oauth_bootstrap::SystemBrowser),
        }
    }

    /// Run consent flows through `browser` instead of the desktop's handler.
    pub fn with_browser(mut self, browser: Arc<dyn BrowserOpener>) -> Self {
        self.browser = browser;
        self
    }
}

impl Module for McpIntegrationsModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        let (store, loaded) = self.loaded.as_ref().map_err(|msg| {
            fluxdi::Error::module_lifecycle_failed("McpIntegrationsModule", "configure", msg)
        })?;

        // The enablement authority and the settings list it backs are
        // registered BEFORE the nothing-to-run early return below: a vault with
        // every integration switched off produces no config and no ignored
        // sidecar, and that is exactly the container in which the settings
        // surface is the user's only way to switch one on.
        let store_di = store.clone();
        injector.provide::<IntegrationConfigStore>(Provider::root(move |_| store_di.clone()));
        let settings_vm = Arc::new(IntegrationsSettingsVm::new(
            store.clone(),
            self.root.clone(),
        ));
        let settings_vm_di = settings_vm.clone();
        injector.provide::<IntegrationsSettingsVm>(Provider::root(move |_| settings_vm_di.clone()));

        // The `integration` entity's own operation provider, registered beside
        // the store for the same reason: a vault with every integration off
        // still needs the op, because dispatching it is how one gets switched
        // on.
        let ops_store = store.clone();
        let ops_vm = settings_vm.clone();
        let ops_browser = self.browser.clone();
        injector.provide_into_set::<dyn OperationProvider>(Provider::root(move |inj| {
            Arc::new(
                crate::integrations_operations::IntegrationsOperationProvider::new(
                    ops_store.clone(),
                    ops_vm.clone(),
                    ops_browser.clone(),
                    Arc::new(holon_api::spawner::TokioSpawner::new(
                        tokio::runtime::Handle::try_current().expect(
                            "[McpIntegrationsModule] the operation provider is resolved off a \
                             tokio runtime, so `integration.begin_oauth` would have nowhere to \
                             run its consent flow",
                        ),
                    )),
                    inj.clone(),
                ),
            ) as Arc<dyn OperationProvider>
        }));

        let configs = &loaded.configs;
        let superseded = Arc::new(loaded.superseded.clone());
        let ignored = Arc::new(loaded.ignored.clone());
        let inert = Arc::new(loaded.inert.clone());
        // Nothing to run AND nothing to say: leave the container untouched, so a
        // build with no integrations directory keeps resolving no MCP services
        // at all. Files that enabled nothing are the opposite case — the
        // registry factory is where the disclosure reaches the bus, so it must
        // be registered even when no integration runs.
        if configs.is_empty() && ignored.is_empty() && inert.is_empty() {
            return Ok(());
        }

        // Cross-connector identity check, before anything connects: the case
        // no single sidecar can see, since an entity name is the identity
        // ACROSS integrations.
        holon_mcp_client::assert_no_cross_sidecar_entity_collisions(configs.iter().map(
            |(name, cfg)| {
                (
                    name.as_str(),
                    cfg.entity_prefix.as_deref(),
                    cfg.entities.keys().map(String::as_str),
                )
            },
        ))
        .map_err(|e| {
            fluxdi::Error::module_lifecycle_failed(
                "McpIntegrationsModule",
                "configure",
                &format!(
                    "integration configuration is ambiguous ({e:#}) — continuing would route one \
                     entity's writes to whichever connector the scan reached first."
                ),
            )
        })?;

        let configs = Arc::new(configs.clone());
        let pending_flows = Arc::new(PendingOAuthFlows::new());
        let pending_flows_di = pending_flows.clone();
        injector.provide::<PendingOAuthFlows>(Provider::root(move |_| pending_flows_di.clone()));

        // ONE shared pending-write store for all MCP providers (leases/read-
        // write ruling, increment 4c). Installed on every integration below so
        // all once_only chokepoints and the frontend approve panel coordinate
        // through the same at-most-once state machine. Registered as a DI
        // singleton so the GPUI layer resolves the same handle to render/approve.
        let pending_writes = Arc::new(PendingWriteStore::new());
        let pending_writes_di = pending_writes.clone();
        // fluxdi treats an `Arc<T>`-returning root closure as the shared
        // instance of `T`, so `provide::<PendingWriteStore>` + a closure cloning
        // this Arc registers ONE shared store; `resolve::<PendingWriteStore>`
        // returns that same `Arc<PendingWriteStore>` (mirrors PendingOAuthFlows).
        injector.provide::<PendingWriteStore>(Provider::root(move |_| pending_writes_di.clone()));

        let settings_vm = settings_vm.clone();
        let credential_root = self.root.clone();
        injector.provide::<IntegrationSupervisor>(Provider::root_async(move |resolver| {
            let configs = configs.clone();
            let superseded = superseded.clone();
            let ignored = ignored.clone();
            let inert = inert.clone();
            let settings_vm = settings_vm.clone();
            let credential_root = credential_root.clone();
            let pending_flows = pending_flows.clone();
            let pending_writes = pending_writes.clone();
            async move {
                Shared::new(IntegrationSupervisor {
                    configs,
                    superseded,
                    ignored,
                    inert,
                    settings_vm,
                    credential_root,
                    pending_flows,
                    pending_writes,
                    db: resolver
                        .resolve_async::<dyn holon::di::DbHandleProvider>()
                        .await
                        .handle(),
                    attribution: (*resolver.resolve::<IntegrationAttribution>()).clone(),
                    bus: (*resolver.resolve_async::<Arc<ConditionBus>>().await).clone(),
                    shutdown: resolver.resolve::<SessionShutdown>(),
                    cache_factory: resolver
                        .resolve_async::<dyn holon_core::CacheFactory>()
                        .await,
                    token_store: resolver.resolve_async::<dyn SyncTokenStore>().await,
                    type_registry: resolver.resolve::<TypeRegistry>(),
                    sync_gate: (*resolver.resolve::<SyncGate>()).clone(),
                    // The session's ONE secret store: a session on the in-memory
                    // backend must not have this reader talking to the login
                    // keychain instead.
                    keychain: resolver
                        .resolve_async::<dyn holon_secrets::KeychainStore>()
                        .await,
                    holon_config: resolver.resolve::<holon_frontend::config::HolonConfig>(),
                    started: AtomicBool::new(false),
                    connected: Mutex::new(Vec::new()),
                })
            }
        }));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use holon_api::ConditionBus;
    use holon_api::ConditionKind;

    use super::*;

    /// Drives the REAL sidecar-spawn failure (a `command` that is not on disk)
    /// through the same disclosure the registry factory uses, and asserts the
    /// degraded bus carries the integration name and the connect error.
    #[tokio::test(flavor = "current_thread")]
    async fn dead_sidecar_command_is_disclosed_on_the_degraded_bus() {
        let Err(err) = holon_mcp_client::connect_mcp_child(
            "/nonexistent/holon-test-sidecar",
            &[],
            &HashMap::new(),
        )
        .await
        else {
            panic!("spawning a nonexistent sidecar binary must fail");
        };

        // Disclose BEFORE subscribing — the registry factory runs in boot DI,
        // the only consumer subscribes at window launch.
        let bus = ConditionBus::new();
        disclose_connect_failure("todoist", &err, &bus);

        let mut current = bus.subscribe().current;
        assert_eq!(current.len(), 1);
        let ev = current.remove(0);
        assert_eq!(ev.subject, "todoist");
        let ConditionKind::IntegrationConnectFailed { integration, error } = ev.reason else {
            panic!("expected IntegrationConnectFailed, got {:?}", ev.reason);
        };
        assert_eq!(integration, "todoist");
        assert!(
            error.contains("/nonexistent/holon-test-sidecar"),
            "error must name the binary it could not run: {error}"
        );
        assert_eq!(
            boot_cause(&err),
            "binary not found at /nonexistent/holon-test-sidecar",
            "a configured path that does not exist is a MISSING binary — distinct from a bare \
             name this process cannot see, which needs the opposite remedy"
        );
    }

    /// A connected integration that came up without part of what the peer
    /// publishes must say so with the reason — the entities its resource
    /// templates would have discovered simply do not exist, and a page backed
    /// by one renders blank exactly like a healthy empty result.
    #[tokio::test(flavor = "current_thread")]
    async fn incomplete_discovery_is_disclosed_on_the_degraded_bus() {
        let bus = ConditionBus::new();
        let mut current = bus.subscribe().current;
        assert!(current.is_empty());

        disclose_discovery_incomplete(
            "todoist",
            "list_resource_templates offered another page after MAX_LIST_PAGES (256) pages and 0 \
             items",
            &bus,
        );

        let mut current = bus.subscribe().current;
        assert_eq!(current.len(), 1);
        let ev = current.remove(0);
        assert_eq!(ev.subject, "todoist");
        let ConditionKind::IntegrationDiscoveryIncomplete { integration, error } = ev.reason else {
            panic!(
                "expected IntegrationDiscoveryIncomplete, got {:?}",
                ev.reason
            );
        };
        assert_eq!(integration, "todoist");
        assert!(
            error.contains("MAX_LIST_PAGES"),
            "the disclosure must carry the reason, bound included: {error}"
        );
    }

    /// A peer picks its own error text, and it lands in a toast and in the
    /// integration's row. A megabyte of it is not a disclosure — and cutting it
    /// silently would hide that there was more.
    #[tokio::test(flavor = "current_thread")]
    async fn peer_error_text_is_bounded_in_what_is_disclosed() {
        let bus = ConditionBus::new();
        disclose_connect_failure("todoist", &anyhow::anyhow!("{}", "A".repeat(1 << 20)), &bus);

        let mut current = bus.subscribe().current;
        let ev = current.remove(0);
        let ConditionKind::IntegrationConnectFailed { error, .. } = ev.reason else {
            panic!("expected IntegrationConnectFailed, got {:?}", ev.reason)
        };
        assert!(
            error.len() <= 8192,
            "a peer's error text must be bounded; got {} bytes",
            error.len()
        );
        assert!(
            error.contains("bytes cut"),
            "what was cut must be visible, not silent: {}",
            &error[..error.len().min(200)]
        );
    }

    /// A peer that floods its change signals must say so with the reason: the
    /// rows are current, but every notice it sends now costs a full re-sync of
    /// everything that integration syncs.
    #[tokio::test(flavor = "current_thread")]
    async fn collapsed_change_signals_are_disclosed_on_the_degraded_bus() {
        let bus = ConditionBus::new();
        let mut current = bus.subscribe().current;
        assert!(current.is_empty());

        disclose_signals_collapsed(
            "todoist",
            "MAX_PENDING_SYNC_URIS (256) resource URIs of this connection are already waiting \
             for a re-sync",
            &bus,
        );

        let mut current = bus.subscribe().current;
        assert_eq!(current.len(), 1);
        let ev = current.remove(0);
        assert_eq!(ev.subject, "todoist");
        let ConditionKind::IntegrationChangeSignalsCollapsed {
            integration,
            reason,
        } = ev.reason
        else {
            panic!(
                "expected IntegrationChangeSignalsCollapsed, got {:?}",
                ev.reason
            );
        };
        assert_eq!(integration, "todoist");
        assert!(
            reason.contains("MAX_PENDING_SYNC_URIS"),
            "the disclosure must carry the reason, bound included: {reason}"
        );
    }

    /// The Finder-launch failure: a bare `command` the user has installed, but
    /// not anywhere launchd's minimal `PATH` reaches. The recorded cause must
    /// say so, and say where it looked.
    #[tokio::test(flavor = "current_thread")]
    async fn a_sidecar_command_missing_from_path_is_recorded_as_a_path_failure() {
        let Err(err) = holon_mcp_client::connect_mcp_child(
            "holon-definitely-not-installed-sidecar",
            &[],
            &HashMap::new(),
        )
        .await
        else {
            panic!("spawning an unresolvable sidecar command must fail");
        };

        let cause = boot_cause(&err);
        assert!(
            cause.starts_with("binary 'holon-definitely-not-installed-sidecar' not found on PATH"),
            "the cause must distinguish PATH from a missing file: {cause}"
        );
        assert!(
            cause.contains("searched"),
            "the cause must say where it looked, or a packaging failure is undiagnosable: {cause}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_oauth_is_disclosed_on_the_degraded_bus() {
        let bus = ConditionBus::new();
        disclose_needs_auth("linear", "https://linear.app/oauth/authorize", &bus);

        let mut current = bus.subscribe().current;
        assert_eq!(current.len(), 1);
        let ev = current.remove(0);
        assert_eq!(ev.subject, "linear");
        let ConditionKind::IntegrationNeedsAuth {
            integration,
            auth_url,
        } = ev.reason
        else {
            panic!("expected IntegrationNeedsAuth, got {:?}", ev.reason);
        };
        assert_eq!(integration, "linear");
        assert_eq!(auth_url, "https://linear.app/oauth/authorize");
    }
}
