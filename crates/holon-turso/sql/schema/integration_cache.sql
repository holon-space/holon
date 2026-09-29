-- The cache tables an integration created, and which provider fills them. A
-- rebuild reads it to disclose which caches it cleared; each connection
-- records its tables again when it connects.
CREATE TABLE IF NOT EXISTS integration_cache (
    table_name TEXT PRIMARY KEY NOT NULL,
    provider TEXT NOT NULL
);
