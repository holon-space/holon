//! Sync engine: pulls sessions from a [`ClaudeSessionSource`], derives status
//! and topic association, and mirrors them into the `claude_session` table.
//!
//! Read-only with respect to Claude Code — the only writes are to Holon's own
//! mirror table. Unchanged rows are not rewritten so the downstream matview
//! sees CDC only for real changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use holon_api::Value;
use holon_turso::turso::DbHandle;

use super::source::ClaudeSessionSource;
use super::status::SessionStatus;
use super::topic::{Topic, associate_topic};

/// Cadence of the periodic sync loop. Coarser than H8's 1-2s attention-routing
/// cadence — sufficient for the read-only topic-aggregation view.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(60);

const ID_SCHEME: &str = "claude_session";

fn prefixed_id(raw: &str) -> String {
    format!("{ID_SCHEME}:{raw}")
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub fetched: usize,
    pub upserted: usize,
    pub deleted: usize,
    pub unchanged: usize,
}

pub struct ClaudeSessionSync {
    source: Arc<dyn ClaudeSessionSource>,
    db: DbHandle,
}

fn value_as_opt_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn value_as_i64(v: Option<&Value>) -> Option<i64> {
    match v {
        Some(Value::Integer(n)) => Some(*n),
        _ => None,
    }
}

/// The columns that participate in the changed-row comparison.
/// `status_computed_at` is deliberately excluded — it only advances when
/// something else changed, otherwise every sync pass would rewrite every row.
#[derive(PartialEq)]
struct RowFingerprint {
    modified_at: String,
    status: &'static str,
    topic_block_id: Option<String>,
    message_count: i64,
    summary: Option<String>,
    first_prompt: String,
}

impl ClaudeSessionSync {
    pub fn new(source: Arc<dyn ClaudeSessionSource>, db: DbHandle) -> Self {
        Self { source, db }
    }

    /// Topic candidates: root blocks (page/document titles).
    async fn load_topics(&self) -> anyhow::Result<Vec<Topic>> {
        let rows = self
            .db
            .query(
                "SELECT id, content FROM block_raw \
                 WHERE (parent_id IS NULL OR parent_id = '') AND content <> ''",
                HashMap::new(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("loading topic blocks: {e}"))?;

        rows.into_iter()
            .map(|row| {
                let block_id = value_as_opt_string(row.get("id"))
                    .ok_or_else(|| anyhow::anyhow!("topic block row without string id: {row:?}"))?;
                let title = value_as_opt_string(row.get("content")).ok_or_else(|| {
                    anyhow::anyhow!("topic block '{block_id}' without string content")
                })?;
                Ok(Topic { block_id, title })
            })
            .collect()
    }

    /// One full sync pass at time `now`: fetch, derive, diff, mirror.
    pub async fn sync_once(&self, now: DateTime<Utc>) -> anyhow::Result<SyncStats> {
        let sessions = self.source.fetch_sessions().await?;
        let topics = self.load_topics().await?;

        let existing_rows = self
            .db
            .query("SELECT * FROM claude_session", HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("reading existing claude_session rows: {e}"))?;
        let mut existing: HashMap<String, RowFingerprint> = existing_rows
            .into_iter()
            .map(|row| {
                let id = value_as_opt_string(row.get("id"))
                    .ok_or_else(|| anyhow::anyhow!("claude_session row without string id"))?;
                let status_raw = value_as_opt_string(row.get("status"))
                    .ok_or_else(|| anyhow::anyhow!("claude_session '{id}' without status"))?;
                let status = match status_raw.as_str() {
                    "active" => SessionStatus::Active.as_str(),
                    "waiting_on_user" => SessionStatus::WaitingOnUser.as_str(),
                    "idle" => SessionStatus::Idle.as_str(),
                    other => anyhow::bail!("claude_session '{id}' has unknown status '{other}'"),
                };
                let fp = RowFingerprint {
                    modified_at: value_as_opt_string(row.get("modified_at"))
                        .ok_or_else(|| anyhow::anyhow!("claude_session '{id}' without modified_at"))?,
                    status,
                    topic_block_id: value_as_opt_string(row.get("topic_block_id")),
                    message_count: value_as_i64(row.get("message_count")).ok_or_else(|| {
                        anyhow::anyhow!("claude_session '{id}' without message_count")
                    })?,
                    summary: value_as_opt_string(row.get("summary")),
                    first_prompt: value_as_opt_string(row.get("first_prompt")).ok_or_else(
                        || anyhow::anyhow!("claude_session '{id}' without first_prompt"),
                    )?,
                };
                Ok((id, fp))
            })
            .collect::<anyhow::Result<_>>()?;

        let mut stats = SyncStats {
            fetched: sessions.len(),
            ..Default::default()
        };

        let mut seen: HashSet<String> = HashSet::new();
        for session in &sessions {
            let id = prefixed_id(&session.id);
            if !seen.insert(id.clone()) {
                anyhow::bail!("duplicate session id '{id}' in fetched session list");
            }

            let status = SessionStatus::derive(session.last_role, session.modified, now);
            let topic = associate_topic(&session.first_prompt, &topics);

            let fresh = RowFingerprint {
                modified_at: session.modified.to_rfc3339(),
                status: status.as_str(),
                topic_block_id: topic.map(|t| t.block_id.clone()),
                message_count: session.message_count,
                summary: session.summary.clone(),
                first_prompt: session.first_prompt.clone(),
            };

            if existing.remove(&id).is_some_and(|old| old == fresh) {
                stats.unchanged += 1;
                continue;
            }

            let opt_str = |v: &Option<String>| match v {
                Some(s) => Value::String(s.clone()),
                None => Value::Null,
            };
            self.db
                .execute_values(
                    "INSERT OR REPLACE INTO claude_session \
                     (id, project, raw_project, first_prompt, summary, message_count, \
                      created_at, modified_at, last_role, status, status_computed_at, \
                      topic_block_id) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    vec![
                        Value::String(id.clone()),
                        Value::String(session.project.clone()),
                        Value::String(session.raw_project.clone()),
                        Value::String(session.first_prompt.clone()),
                        opt_str(&session.summary),
                        Value::Integer(session.message_count),
                        Value::String(session.created.to_rfc3339()),
                        Value::String(fresh.modified_at.clone()),
                        Value::String(session.last_role.as_str().to_string()),
                        Value::String(fresh.status.to_string()),
                        Value::String(now.to_rfc3339()),
                        opt_str(&fresh.topic_block_id),
                    ],
                )
                .await
                .map_err(|e| anyhow::anyhow!("upserting claude_session '{id}': {e}"))?;
            stats.upserted += 1;
        }

        for gone_id in existing.into_keys() {
            self.db
                .execute_values(
                    "DELETE FROM claude_session WHERE id = ?",
                    vec![Value::String(gone_id.clone())],
                )
                .await
                .map_err(|e| anyhow::anyhow!("deleting claude_session '{gone_id}': {e}"))?;
            stats.deleted += 1;
        }

        tracing::info!(
            fetched = stats.fetched,
            upserted = stats.upserted,
            deleted = stats.deleted,
            unchanged = stats.unchanged,
            "[ClaudeSessionSync] sync pass complete"
        );
        Ok(stats)
    }

    /// Run `sync_once` forever at [`SYNC_INTERVAL`]. Errors are disclosed via
    /// `warn!` and the loop continues — a transient MCP failure must not kill
    /// the mirror permanently, but it is never silent.
    pub fn spawn_periodic(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(SYNC_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = self.sync_once(Utc::now()).await {
                    tracing::warn!("[ClaudeSessionSync] sync pass failed: {e:#}");
                }
            }
        })
    }
}
