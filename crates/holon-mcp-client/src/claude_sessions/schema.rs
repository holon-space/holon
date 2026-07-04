//! Schema module for the `claude_session` table and the
//! `claude_sessions_under_topics` matview.

use async_trait::async_trait;

use holon_core::storage::{Resource, Result, StorageError};
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::schema_module::SchemaModule;
use holon_turso::sql_utils::sql_statements;
use holon_turso::turso::DbHandle;

pub const SESSIONS_UNDER_TOPICS_VIEW: &str = "claude_sessions_under_topics";
/// Synthetic topic id for sessions with no topic association, projected by
/// the view's COALESCE.
pub const UNFILED_TOPIC_ID: &str = "claude_session_topic:unfiled";

pub struct ClaudeSessionSchemaModule;

#[async_trait]
impl SchemaModule for ClaudeSessionSchemaModule {
    fn name(&self) -> &str {
        "claude_sessions"
    }

    fn provides(&self) -> Vec<Resource> {
        vec![
            Resource::schema("claude_session"),
            Resource::schema(SESSIONS_UNDER_TOPICS_VIEW),
        ]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("block_raw")]
    }

    async fn ensure_schema(&self, db_handle: &DbHandle) -> Result<()> {
        for stmt in sql_statements(include_str!("../../sql/claude_session.sql")) {
            db_handle.execute_ddl(stmt).await?;
        }

        reconcile_named_view(
            db_handle,
            SESSIONS_UNDER_TOPICS_VIEW,
            include_str!("../../sql/claude_sessions_under_topics.sql"),
        )
        .await
        .map_err(|e| StorageError::DatabaseError(e.to_string()))?;

        tracing::info!("[ClaudeSessionSchemaModule] claude_session schema ready");
        Ok(())
    }

    fn graph_contributions(
        &self,
    ) -> (
        Vec<holon_api::entity::GraphNodeDef>,
        Vec<holon_api::entity::GraphEdgeDef>,
    ) {
        use holon_api::entity::{GraphEdgeDef, GraphNodeDef};

        let nodes = vec![GraphNodeDef {
            label: "claude_session".into(),
            table_name: "claude_session".into(),
            id_column: "id".into(),
            columns: vec![
                ("id".into(), "id".into()),
                ("project".into(), "project".into()),
                ("first_prompt".into(), "first_prompt".into()),
                ("summary".into(), "summary".into()),
                ("message_count".into(), "message_count".into()),
                ("created_at".into(), "created_at".into()),
                ("modified_at".into(), "modified_at".into()),
                ("last_role".into(), "last_role".into()),
                ("status".into(), "status".into()),
                ("topic_block_id".into(), "topic_block_id".into()),
            ],
        }];

        let edges = vec![GraphEdgeDef {
            edge_name: "FILED_UNDER".into(),
            source_label: Some("claude_session".into()),
            target_label: Some("block".into()),
            fk_table: "claude_session".into(),
            fk_column: "topic_block_id".into(),
            target_table: "block".into(),
            target_id_column: "id".into(),
        }];

        (nodes, edges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provides_table_and_view_requires_block_raw() {
        let m = ClaudeSessionSchemaModule;
        let provides = m.provides();
        assert!(provides.contains(&Resource::schema("claude_session")));
        assert!(provides.contains(&Resource::schema(SESSIONS_UNDER_TOPICS_VIEW)));
        assert_eq!(m.requires(), vec![Resource::schema("block_raw")]);
    }
}
