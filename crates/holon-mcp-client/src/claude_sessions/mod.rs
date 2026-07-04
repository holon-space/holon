//! Sessions-under-topics read-only view (Dogfooding Idea 1, first milestone).
//!
//! Aggregates Claude Code sessions from the claude-history MCP server under
//! Holon topic blocks. Per H8 (Hypotheses.org) no Claude Code hooks are used:
//! session status derives from the modified timestamp + last-message role,
//! and the first prompt is the topic-identification signal.
//!
//! Pipeline: [`source::ClaudeSessionSource`] fetches typed
//! [`boundary::SessionRecord`]s → [`sync::ClaudeSessionSync`] derives
//! [`status::SessionStatus`] + [`topic::associate_topic`] and mirrors rows
//! into `claude_session` → the `claude_sessions_under_topics` matview
//! ([`schema::ClaudeSessionSchemaModule`]) exposes the aggregation read-only.

pub mod boundary;
pub mod schema;
pub mod source;
pub mod status;
pub mod sync;
pub mod topic;

pub use boundary::{LastRole, SessionRecord, normalize_project, parse_session_json};
pub use schema::{ClaudeSessionSchemaModule, SESSIONS_UNDER_TOPICS_VIEW, UNFILED_TOPIC_ID};
pub use source::{ClaudeSessionSource, McpClaudeSessionSource, SESSIONS_URI_TEMPLATE};
pub use status::SessionStatus;
pub use sync::{ClaudeSessionSync, SYNC_INTERVAL, SyncStats};
pub use topic::{Topic, associate_topic};
