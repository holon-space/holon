//! Boundary types for Claude Code session records (per H8, Hypotheses.org).
//!
//! Raw JSON from the claude-history MCP is parsed HERE into typed records;
//! nothing downstream ever sees raw strings for roles or timestamps.

use chrono::{DateTime, Utc};

/// Role of the last meaningful (user/assistant) message in a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastRole {
    User,
    Assistant,
}

impl LastRole {
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            other => anyhow::bail!("unknown last-message role '{other}' (expected user|assistant)"),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// A Claude Code session as reported by the claude-history MCP session list.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    /// Raw session id (unprefixed uuid).
    pub id: String,
    /// Canonical project (worktree paths normalized, see [`normalize_project`]).
    pub project: String,
    /// Project exactly as reported by the MCP server.
    pub raw_project: String,
    /// The user's opening message — the topic-identification signal.
    pub first_prompt: String,
    pub summary: Option<String>,
    pub message_count: i64,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    pub last_role: LastRole,
}

/// Normalize a claude-history project name to its canonical project.
///
/// Sessions run in git worktrees show up as their own projects, e.g.
/// `-Users-martin-Workspaces-pkm-holon--claude-worktrees-tui-reactive-vm`.
/// The `--claude-worktrees-…` suffix is stripped so all worktree sessions
/// aggregate under the canonical project (H8 side discovery).
pub fn normalize_project(raw: &str) -> String {
    match raw.find("--claude-worktrees-") {
        Some(idx) => raw[..idx].to_string(),
        None => raw.to_string(),
    }
}

fn get_str<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> anyhow::Result<&'a str> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            return v
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("field '{key}' is not a string: {v}"));
        }
    }
    anyhow::bail!("missing required field (tried {keys:?})")
}

fn get_i64(
    obj: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> anyhow::Result<i64> {
    for key in keys {
        if let Some(v) = obj.get(*key) {
            return v
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("field '{key}' is not an integer: {v}"));
        }
    }
    anyhow::bail!("missing required field (tried {keys:?})")
}

fn parse_ts(raw: &str, field: &str) -> anyhow::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| anyhow::anyhow!("field '{field}' is not an ISO-8601 timestamp ('{raw}'): {e}"))
}

/// Parse one session-list JSON object into a [`SessionRecord`].
///
/// Field names follow H8's documented resource shape; both camelCase and
/// snake_case spellings are accepted at this boundary. Any missing or
/// mistyped field is a hard error enriched with the session context —
/// a server-side format change must surface, never produce a half-parsed row.
pub fn parse_session_json(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<SessionRecord> {
    let id = get_str(obj, &["id"])?.to_string();
    let inner = || -> anyhow::Result<SessionRecord> {
        let raw_project = get_str(obj, &["project"])?.to_string();
        let summary = match obj.get("summary") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(other) => anyhow::bail!("field 'summary' is not a string or null: {other}"),
        };
        Ok(SessionRecord {
            id: id.clone(),
            project: normalize_project(&raw_project),
            raw_project,
            first_prompt: get_str(obj, &["firstPrompt", "first_prompt"])?.to_string(),
            summary,
            message_count: get_i64(obj, &["messageCount", "message_count"])?,
            created: parse_ts(get_str(obj, &["created", "created_at"])?, "created")?,
            modified: parse_ts(get_str(obj, &["modified", "modified_at"])?, "modified")?,
            last_role: LastRole::parse(get_str(obj, &["lastRole", "last_role"])?)?,
        })
    };
    inner().map_err(|e| anyhow::anyhow!("session '{id}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_session() -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "id": "809ab486-aaaa-bbbb-cccc-000000000001",
            "project": "-Users-martin-Workspaces-pkm-holon",
            "firstPrompt": "fix the composed PBT boot settle",
            "summary": null,
            "messageCount": 42,
            "created": "2026-07-04T10:00:00Z",
            "modified": "2026-07-04T10:30:00Z",
            "lastRole": "assistant"
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn parses_full_record() {
        let rec = parse_session_json(&full_session()).unwrap();
        assert_eq!(rec.id, "809ab486-aaaa-bbbb-cccc-000000000001");
        assert_eq!(rec.project, "-Users-martin-Workspaces-pkm-holon");
        assert_eq!(rec.last_role, LastRole::Assistant);
        assert_eq!(rec.message_count, 42);
        assert_eq!(rec.summary, None);
    }

    #[test]
    fn snake_case_aliases_accepted() {
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), "s1".into());
        obj.insert("project".into(), "p".into());
        obj.insert("first_prompt".into(), "hello".into());
        obj.insert("message_count".into(), 1.into());
        obj.insert("created".into(), "2026-07-04T10:00:00+00:00".into());
        obj.insert("modified".into(), "2026-07-04T10:00:00+00:00".into());
        obj.insert("last_role".into(), "user".into());
        let rec = parse_session_json(&obj).unwrap();
        assert_eq!(rec.first_prompt, "hello");
        assert_eq!(rec.last_role, LastRole::User);
    }

    #[test]
    fn missing_field_fails_loud_with_session_id() {
        let mut obj = full_session();
        obj.remove("modified");
        let err = parse_session_json(&obj).unwrap_err().to_string();
        assert!(err.contains("809ab486"), "error must name the session: {err}");
        assert!(err.contains("modified"), "error must name the field: {err}");
    }

    #[test]
    fn unknown_role_fails_loud() {
        let mut obj = full_session();
        obj.insert("lastRole".into(), "system".into());
        let err = parse_session_json(&obj).unwrap_err().to_string();
        assert!(err.contains("system"), "{err}");
    }

    #[test]
    fn worktree_project_normalizes_to_canonical() {
        assert_eq!(
            normalize_project(
                "-Users-martin-Workspaces-pkm-holon--claude-worktrees-tui-reactive-vm"
            ),
            "-Users-martin-Workspaces-pkm-holon"
        );
        assert_eq!(
            normalize_project("-Users-martin-Workspaces-pkm-holon"),
            "-Users-martin-Workspaces-pkm-holon"
        );
    }
}
