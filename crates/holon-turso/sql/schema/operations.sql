CREATE TABLE IF NOT EXISTS operation (
    id INTEGER PRIMARY KEY NOT NULL,
    operation TEXT NOT NULL,
    inverse TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    display_name TEXT NOT NULL,
    entity_name TEXT NOT NULL,
    op_name TEXT NOT NULL,
    _change_origin TEXT
);

CREATE INDEX IF NOT EXISTS idx_operation_entity_name
ON operation(entity_name);

CREATE INDEX IF NOT EXISTS idx_operation_created_at
ON operation(created_at);
