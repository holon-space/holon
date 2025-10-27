-- Sessions-under-topics read-only view (Dogfooding Idea 1, first milestone).
-- One row per session, annotated with its associated topic block; sessions
-- with no topic association group under the synthetic 'Unfiled sessions'
-- topic. Consumers sort client-side (matview ORDER BY is unreliable, H7).
SELECT
    COALESCE(s.topic_block_id, 'claude_session_topic:unfiled') AS topic_id,
    COALESCE(b.content, 'Unfiled sessions') AS topic_title,
    s.id AS session_id,
    s.project,
    s.raw_project,
    s.first_prompt,
    s.summary,
    s.message_count,
    s.created_at,
    s.modified_at,
    s.last_role,
    s.status,
    s.status_computed_at
FROM claude_session s
LEFT OUTER JOIN block_raw b ON b.id = s.topic_block_id
