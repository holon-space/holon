//! Queryable mirror of the integration enablement store — D5.b §4.1a.
//!
//! [`IntegrationConfigStore`] owns integration state and keeps it in filesystem
//! `.state.toml` files with atomic writes (ADR 0030 D3). Nothing can query a
//! set of signals, though, and the left-sidebar Integrations section is a
//! `live_query` — so the store is mirrored into the `integration_state` table
//! and the section reads that.
//!
//! [`IntegrationStateProjector`] is the table's SOLE writer. It re-derives
//! every row from the store on each run rather than accumulating deltas — the
//! stateful-regrouping law of the derived-data contract — so a mirror that
//! drifted for any reason (a missed signal, a file hand-edited between boots)
//! re-converges on the next projection instead of staying wrong.
//!
//! Only the DISPLAY half of the configuration axis is projected
//! ([`ConfigStatus`], via [`IntegrationsSettingsVm`]). `Configuration` itself
//! carries credential LOCATIONS and must never reach a user-queryable,
//! MCP-readable table (§8 R1). `TABLE_COLUMNS` pins the column set so a later
//! field addition is a failing test rather than a silent leak.

use std::sync::Arc;

use futures_signals::signal::SignalExt;
use holon::storage::DbHandle;
use holon::storage::now_utc;
use holon_api::Value;

use crate::integrations_settings::ConfigStatus;
use crate::integrations_settings::IntegrationsSettingsVm;

/// Every column of `integration_state`, in DDL order.
///
/// Asserted by the projection tests: adding a column here is a deliberate act
/// with a test to update, which is what keeps a credential field from arriving
/// unnoticed in a table any user query can read.
pub const TABLE_COLUMNS: &[&str] = &[
    "id",
    "provider_name",
    "enabled",
    "status",
    "config_status",
    "configurable",
    "configure_progress",
    "updated_at",
    "_change_origin",
];

/// How far an enabled integration got at boot — the axis the integration
/// REGISTRY owns, distinct from the store's `enabled`/`config_status`.
///
/// The design defers this (§8 R9) and notes the table takes the extra column
/// without disturbing anything else. The discovery surface wants it: keeping
/// "switched on" and "actually working" apart is what lets a broken
/// integration stay visible AND read as broken, instead of vanishing from the
/// list or rendering like a healthy one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationStatus {
    /// Enabled; the registry has not resolved it yet. Every freshly projected
    /// row starts here.
    Pending,
    /// Connected, operations registered.
    Connected,
    /// Reachable, but waiting on an OAuth grant.
    NeedsAuth,
    /// Enabled but not running: connect failed, or a `${VAR}` it needs is set
    /// neither in the environment nor in settings.
    Unavailable,
}

impl IntegrationStatus {
    /// The word the discovery section prints. Stored rather than derived at
    /// render time, because the section is a `live_query` and SQL is the only
    /// language between the mirror and the row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Connected => "Connected",
            Self::NeedsAuth => "Needs auth",
            Self::Unavailable => "Unavailable",
        }
    }
}

/// The row id for `provider` — the `integration:` entity scheme.
pub fn integration_row_id(provider: &str) -> String {
    format!("integration:{provider}")
}

/// The stored form of the configuration axis: the display enum, lowercased.
fn config_status_value(status: ConfigStatus) -> &'static str {
    match status {
        ConfigStatus::Unconfigured => "unconfigured",
        ConfigStatus::Configured => "configured",
    }
}

/// Mirrors the enablement store into `integration_state`.
///
/// The projector reads the view model rather than the store directly: `rows()`
/// is already the projection that drops credential locations, so the leak in
/// §8 R1 is prevented by construction instead of by remembering to map here.
///
/// Consent-flow progress lives in the view model's own cells rather than the
/// store's files, so this must be the SAME view model the settings surface
/// drives.
pub struct IntegrationStateProjector {
    db: DbHandle,
    vm: Arc<IntegrationsSettingsVm>,
}

impl IntegrationStateProjector {
    pub fn new(db: DbHandle, vm: Arc<IntegrationsSettingsVm>) -> Self {
        Self { db, vm }
    }

    /// What `provider`'s consent flow has to say, or `""` while it has
    /// nothing.
    fn configure_progress(&self, provider: &str) -> String {
        self.vm
            .configure_progress(provider)
            .get_cloned()
            .message()
            .unwrap_or_default()
    }

    /// Re-derive every row from the store.
    ///
    /// Writes one row per BUNDLED provider — enabled or not — so the table
    /// carries the presence axis in full and a disabled provider is
    /// `enabled = 0` rather than an absence that cannot be told from "never
    /// projected". Rows for providers this build no longer bundles are dropped,
    /// which is the mirror-repair leg.
    pub async fn project(&self) -> anyhow::Result<()> {
        let rows = self.vm.rows();
        let updated_at = now_utc();

        for row in &rows {
            self.db
                .execute_values(
                    // `status` is set on insert only. Re-projection runs on every
                    // enablement change, and clobbering a resolved `Connected`
                    // back to `Pending` because an unrelated integration was
                    // toggled would make the column lie. The registry owns that
                    // column after the row exists.
                    "INSERT INTO integration_state \
                     (id, provider_name, enabled, status, config_status, configurable, \
                     configure_progress, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(id) DO UPDATE SET \
                     enabled = excluded.enabled, \
                     config_status = excluded.config_status, \
                     configurable = excluded.configurable, \
                     configure_progress = excluded.configure_progress, \
                     updated_at = excluded.updated_at",
                    vec![
                        Value::String(integration_row_id(row.provider)),
                        Value::String(row.provider.to_string()),
                        Value::Integer(i64::from(row.enabled)),
                        Value::String(IntegrationStatus::Pending.label().to_string()),
                        Value::String(config_status_value(row.status).to_string()),
                        Value::Integer(i64::from(row.configurable)),
                        Value::String(self.configure_progress(row.provider)),
                        Value::String(updated_at.clone()),
                    ],
                )
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "projecting integration '{}' into integration_state: {e}",
                        row.provider
                    )
                })?;
        }

        // Built by position rather than interpolation so a provider name can
        // never reach the statement as sql.
        let projected_ids: Vec<String> = rows
            .iter()
            .map(|r| integration_row_id(r.provider))
            .collect();
        let placeholders = vec!["?"; projected_ids.len()].join(", ");
        self.db
            .execute_values(
                &format!("DELETE FROM integration_state WHERE id NOT IN ({placeholders})"),
                projected_ids.iter().cloned().map(Value::String).collect(),
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!("pruning unbundled providers from integration_state: {e}")
            })?;

        // This mirror IS the projection-visible surface for integrations: the
        // Settings `live_query` and the sidebar read it, and nothing else makes
        // an enablement change visible. Report the projected rows to the e2e
        // latency correlator so a `set_field` interaction closes here, the same
        // way a block mirror closes its interactions from `LiveData::subscribe`.
        // Absent this, a settings `set_field` never sees a block-row delivery
        // for its `integration:` target and expires as `e2e_expired`.
        holon_api::latency_e2e::rows_delivered(
            "integration_state",
            projected_ids.iter().map(|id| {
                (
                    id.as_str(),
                    holon_api::latency_e2e::Observable::BlockRow(None),
                )
            }),
        );

        // The boot log carried NOT ONE line from this projector while the
        // Integrations section was visibly wrong, so the only way to tell
        // whether it had run was to infer it from the rows. One line, naming
        // what landed, is what makes the next occurrence readable.
        tracing::info!(
            providers = rows.len(),
            enabled = rows.iter().filter(|r| r.enabled).count(),
            "[IntegrationStateProjector] projected the enablement store into integration_state"
        );
        Ok(())
    }

    /// Project now, then keep the mirror in step with the store.
    ///
    /// One watcher per provider re-projects the WHOLE store on any change,
    /// rather than patching the row that moved: routing every change through
    /// the same full re-derivation is what makes the mirror self-repairing.
    ///
    /// A projection failure after boot is logged, not fatal — the section goes
    /// stale rather than the app going down, and the next change repairs it.
    /// The INITIAL projection propagates its error, because a mirror that was
    /// never built at all would leave the section silently empty, which is the
    /// escape this table replaces.
    pub async fn start(self: Arc<Self>) -> anyhow::Result<()> {
        self.project().await?;

        for (provider, signal) in self.vm.signals() {
            self.clone().reproject_on(signal.signal_cloned());
            self.clone()
                .reproject_on(self.vm.configure_progress(provider).signal_cloned());
        }
        Ok(())
    }

    /// Re-project whenever `signal` fires.
    fn reproject_on<S>(self: Arc<Self>, signal: S)
    where
        S: futures_signals::signal::Signal + Send + 'static,
        S::Item: Send,
    {
        tokio::spawn(signal.for_each(move |_| {
            let projector = self.clone();
            async move {
                if let Err(e) = projector.project().await {
                    tracing::warn!(
                        "[IntegrationStateProjector] re-projection after a state change failed; \
                         the Integrations section is stale until the next change: {e:#}"
                    );
                }
            }
        }));
    }
}

/// Record the boot outcome of an integration the projector has already placed
/// in the mirror.
///
/// Fails when `provider` has no enabled row: the registry only ever connects
/// what the store enabled, so a status for anybody else means the two have
/// diverged — a wiring bug that must be loud rather than an unattached row.
pub async fn set_integration_status(
    db: &DbHandle,
    provider: &str,
    status: IntegrationStatus,
) -> anyhow::Result<()> {
    let updated = db
        .execute_values(
            "UPDATE integration_state SET status = ? WHERE id = ? AND enabled = 1",
            vec![
                Value::String(status.label().to_string()),
                Value::String(integration_row_id(provider)),
            ],
        )
        .await
        .map_err(|e| anyhow::anyhow!("recording status for integration '{provider}': {e}"))?;

    if updated == 0 {
        anyhow::bail!(
            "integration '{provider}' has no enabled row in the integration_state mirror, so its \
             boot status ({}) cannot be recorded — the connect registry and the enablement store \
             have diverged",
            status.label()
        );
    }
    Ok(())
}
