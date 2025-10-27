-- Claude Code sessions synced from the claude-history MCP (read-only mirror).
-- Written only by ClaudeSessionSync; status/topic columns are derived at
-- sync time from typed boundary values (SessionStatus, associate_topic).
CREATE TABLE IF NOT EXISTS claude_session (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    raw_project TEXT NOT NULL,
    first_prompt TEXT NOT NULL,
    summary TEXT,
    message_count INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    modified_at TEXT NOT NULL,
    last_role TEXT NOT NULL,
    status TEXT NOT NULL,
    status_computed_at TEXT NOT NULL,
    topic_block_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_claude_session_topic ON claude_session(topic_block_id);

CREATE INDEX IF NOT EXISTS idx_claude_session_status ON claude_session(status);
