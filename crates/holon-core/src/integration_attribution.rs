//! Which integration owns a table, and what became of it at boot.
//!
//! An integration that fails to connect still declares its entity tables in
//! its sidecar, so `cc_session` is known to belong to `claude-history` whether
//! or not the sidecar ever started. Without that link the DDL over those
//! tables fails hours later naming five internal identifiers and a matview
//! hash, and the sentence "claude-history is not connected" — which WAS
//! disclosed at boot — appears nowhere near the failure the user sees.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

/// How far an enabled integration got at boot — the axis the integration
/// REGISTRY owns, distinct from the store's `enabled`/`config_status`.
///
/// Keeping "switched on" and "actually working" apart is what lets a broken
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

    /// Whether this state is a SETTLED verdict that the integration is not
    /// running, and so explains a failure over its tables.
    ///
    /// `Pending` is not: it means the registry has not spoken yet, so a
    /// failure under it is unexplained and must stay loud rather than claim
    /// "not connected". `Connected` is not either — its missing table is a
    /// real bug.
    pub fn is_settled_inert(self) -> bool {
        matches!(self, Self::Unavailable | Self::NeedsAuth)
    }
}

/// The integration a table belongs to, with its boot verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableOwner {
    /// Provider name, e.g. `claude-history`.
    pub integration: String,
    /// The name the user sees in the Integrations section.
    pub display_name: String,
    pub status: IntegrationStatus,
    /// The recorded boot cause, e.g. `binary 'claude-history-mcp' not found on
    /// PATH (searched …)`. Empty when there is nothing to add.
    pub cause: String,
}

impl TableOwner {
    /// One sentence naming the integration, its state, and what to do — the
    /// text a degraded surface shows in place of an internal error.
    pub fn disclosure(&self) -> String {
        let mut s = format!(
            "{} is not connected — status: {}",
            self.display_name,
            self.status.label()
        );
        if !self.cause.is_empty() {
            s.push_str(&format!(" ({})", self.cause));
        }
        s
    }
}

/// A missing table that no settled-inert integration explains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnexplainedTable {
    pub table: String,
    /// Its owner when it has one — a connected integration (whose missing
    /// table is a real bug) or one still `Pending`. `None` for a table no
    /// integration declares at all, e.g. a vault table.
    pub owner: Option<TableOwner>,
}

impl UnexplainedTable {
    /// Who to hand this failure to, appended to the loud error.
    pub fn note(&self) -> String {
        match &self.owner {
            Some(owner) => format!(
                "{} belongs to integration '{}' (status: {})",
                self.table,
                owner.integration,
                owner.status.label()
            ),
            None => format!("{} belongs to no integration", self.table),
        }
    }
}

/// How a set of missing tables splits across integration ownership.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissingTableVerdict {
    /// Distinct settled-inert integrations owning at least one missing table —
    /// one entry each however many tables they own.
    pub inert: Vec<TableOwner>,
    /// Everything the inert half does not explain. Non-empty means the failure
    /// stays loud, whatever else is in the list.
    pub unexplained: Vec<UnexplainedTable>,
}

impl MissingTableVerdict {
    /// Whether this failure is FULLY explained by integrations that are known
    /// not to be running — the only case that may be softened to a disclosure.
    pub fn is_fully_explained(&self) -> bool {
        self.unexplained.is_empty() && !self.inert.is_empty()
    }

    /// The disclosure sentences, one per inert integration.
    pub fn disclosures(&self) -> Vec<String> {
        self.inert.iter().map(TableOwner::disclosure).collect()
    }

    /// Every note worth appending to a loud error: who owns the unexplained
    /// tables, plus any inert integrations that co-occurred (dropping those
    /// would hide the one thing the user can act on).
    pub fn notes(&self) -> Vec<String> {
        self.unexplained
            .iter()
            .map(UnexplainedTable::note)
            .chain(self.disclosures())
            .collect()
    }
}

/// Table name → owning integration, declared at boot for EVERY configured
/// integration whether or not it connected.
///
/// Late-filled and shared: the registry resolves long after the render engine
/// is constructed, so consumers hold the handle from the start and read
/// whatever the registry has published by the time a render fails.
#[derive(Clone, Default)]
pub struct IntegrationAttribution {
    by_table: Arc<RwLock<HashMap<String, TableOwner>>>,
}

impl IntegrationAttribution {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `table` belongs to `owner`.
    pub fn declare(&self, table: impl Into<String>, owner: TableOwner) {
        self.write().insert(table.into(), owner);
    }

    /// Replace the boot verdict of every table already declared for
    /// `integration`. The connect loop learns the verdict after it has
    /// declared the tables, and a stale `Pending` would misattribute the
    /// failure it is about to cause.
    pub fn set_status(&self, integration: &str, status: IntegrationStatus, cause: &str) {
        for owner in self.write().values_mut() {
            if owner.integration == integration {
                owner.status = status;
                owner.cause = cause.to_string();
            }
        }
    }

    pub fn owner_of(&self, table: &str) -> Option<TableOwner> {
        self.read().get(table).cloned()
    }

    /// Split `tables` into the part a settled-inert integration explains and
    /// the part nothing explains.
    ///
    /// Both halves matter and neither may swallow the other: one dead
    /// integration in the list must not turn a co-occurring genuine wiring
    /// failure into a calm banner, and a genuine failure must not bury the
    /// disclosure that tells the user what to fix.
    pub fn classify_missing<'a>(
        &self,
        tables: impl IntoIterator<Item = &'a str>,
    ) -> MissingTableVerdict {
        let map = self.read();
        let mut verdict = MissingTableVerdict::default();
        for table in tables {
            match map.get(table) {
                // Distinct: four tables of one dead integration are ONE
                // disclosure, and two dead integrations are two.
                Some(owner) if owner.status.is_settled_inert() => {
                    if !verdict
                        .inert
                        .iter()
                        .any(|o| o.integration == owner.integration)
                    {
                        verdict.inert.push(owner.clone());
                    }
                }
                other => verdict.unexplained.push(UnexplainedTable {
                    table: table.to_string(),
                    owner: other.cloned(),
                }),
            }
        }
        verdict
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, TableOwner>> {
        self.by_table
            .read()
            .expect("integration attribution lock poisoned")
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, TableOwner>> {
        self.by_table
            .write()
            .expect("integration attribution lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(integration: &str, display: &str, status: IntegrationStatus) -> TableOwner {
        TableOwner {
            integration: integration.to_string(),
            display_name: display.to_string(),
            status,
            cause: "binary 'claude-history-mcp' not found on PATH".to_string(),
        }
    }

    fn declare_claude_history(attr: &IntegrationAttribution, status: IntegrationStatus) {
        for table in ["cc_session", "cc_message", "cc_task", "cc_project"] {
            attr.declare(table, owner("claude-history", "Claude History", status));
        }
    }

    #[test]
    fn four_tables_of_one_dead_integration_collapse_to_one_disclosure() {
        let attr = IntegrationAttribution::new();
        declare_claude_history(&attr, IntegrationStatus::Unavailable);

        let verdict = attr.classify_missing(["cc_session", "cc_message", "cc_session", "cc_task"]);

        assert!(verdict.is_fully_explained());
        assert_eq!(verdict.inert.len(), 1, "one integration, one disclosure");
        assert_eq!(
            verdict.disclosures(),
            vec![
                "Claude History is not connected — status: Unavailable (binary \
                 'claude-history-mcp' not found on PATH)"
            ]
        );
    }

    #[test]
    fn two_dead_integrations_each_get_their_own_disclosure() {
        let attr = IntegrationAttribution::new();
        declare_claude_history(&attr, IntegrationStatus::Unavailable);
        attr.declare(
            "td_task",
            owner("todoist", "Todoist", IntegrationStatus::NeedsAuth),
        );

        let verdict = attr.classify_missing(["cc_session", "td_task", "cc_message"]);

        assert!(verdict.is_fully_explained());
        let named: Vec<&str> = verdict
            .inert
            .iter()
            .map(|o| o.integration.as_str())
            .collect();
        assert_eq!(named, vec!["claude-history", "todoist"]);
    }

    /// The masking bug: one dead integration in the list must NOT turn a
    /// co-occurring genuine failure into a calm banner.
    #[test]
    fn a_connected_or_unowned_table_keeps_the_failure_loud_despite_a_dead_neighbour() {
        let attr = IntegrationAttribution::new();
        declare_claude_history(&attr, IntegrationStatus::Unavailable);
        attr.declare(
            "td_task",
            owner("todoist", "Todoist", IntegrationStatus::Connected),
        );

        let verdict = attr.classify_missing(["cc_session", "td_task", "blocks"]);

        assert!(
            !verdict.is_fully_explained(),
            "a dead neighbour must not soften a real failure"
        );
        assert_eq!(verdict.inert.len(), 1, "the disclosure is still carried");
        assert_eq!(
            verdict
                .unexplained
                .iter()
                .map(|u| u.note())
                .collect::<Vec<_>>(),
            vec![
                "td_task belongs to integration 'todoist' (status: Connected)",
                "blocks belongs to no integration",
            ],
            "both the connected owner and the unowned table must be named"
        );
    }

    #[test]
    fn a_connected_integrations_tables_raise_no_disclosure() {
        let attr = IntegrationAttribution::new();
        declare_claude_history(&attr, IntegrationStatus::Connected);

        let verdict = attr.classify_missing(["cc_session"]);
        assert!(
            verdict.inert.is_empty(),
            "a connected integration's missing table is a real bug, not a degraded state"
        );
        assert!(!verdict.is_fully_explained());
    }

    /// `Pending` is "the registry has not spoken yet", not "not running".
    /// Claiming "is not connected" for it would be a wrong claim.
    #[test]
    fn a_pending_integration_does_not_explain_a_failure() {
        let attr = IntegrationAttribution::new();
        declare_claude_history(&attr, IntegrationStatus::Pending);

        let verdict = attr.classify_missing(["cc_session"]);

        assert!(verdict.inert.is_empty());
        assert!(!verdict.is_fully_explained());
        assert_eq!(
            verdict.unexplained[0].note(),
            "cc_session belongs to integration 'claude-history' (status: Pending)"
        );
    }

    #[test]
    fn set_status_repairs_tables_declared_before_the_verdict_was_known() {
        let attr = IntegrationAttribution::new();
        declare_claude_history(&attr, IntegrationStatus::Pending);
        attr.set_status(
            "claude-history",
            IntegrationStatus::Unavailable,
            "binary not found at /nonexistent/claude-history-mcp",
        );

        let owner = attr.owner_of("cc_session").expect("declared above");
        assert_eq!(owner.status, IntegrationStatus::Unavailable);
        assert!(
            owner
                .disclosure()
                .contains("/nonexistent/claude-history-mcp")
        );
    }
}
