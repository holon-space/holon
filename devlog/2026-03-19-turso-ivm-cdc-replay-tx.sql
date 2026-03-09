-- Extracted from: /tmp/holon-gpui-tx.log
-- Statements: 1646
-- Time range: 2026-03-19T17:16:01.730043Z .. 2026-03-19T17:16:43.979361Z

-- !SET_CHANGE_CALLBACK 2026-03-19T17:16:01.730043Z

-- Wait 7ms
-- [actor_ddl] 2026-03-19T17:16:01.738034Z
CREATE TABLE IF NOT EXISTS block (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    document_id TEXT,
    depth INTEGER NOT NULL DEFAULT 0,
    sort_key TEXT NOT NULL DEFAULT 'a0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    properties TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT
);

-- Wait 14ms
-- [actor_ddl] 2026-03-19T17:16:01.752293Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block(parent_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T17:16:01.753812Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block(document_id);

-- [actor_ddl] 2026-03-19T17:16:01.754230Z
CREATE TABLE IF NOT EXISTS document (
    id TEXT PRIMARY KEY NOT NULL,
    parent_id TEXT NOT NULL,
    name TEXT NOT NULL,
    sort_key TEXT NOT NULL,
    properties TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.754757Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document(parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.755103Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document(name);

-- [actor_ddl] 2026-03-19T17:16:01.755433Z
CREATE TABLE IF NOT EXISTS directory (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    depth INTEGER NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.755857Z
CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory(parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.756325Z
CREATE TABLE IF NOT EXISTS file (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    document_id TEXT,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.756871Z
CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file(parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.757213Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file(document_id);

-- [actor_ddl] 2026-03-19T17:16:01.757603Z
CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T17:16:01.758697Z
CREATE INDEX IF NOT EXISTS idx_navigation_history_region
ON navigation_history(region);

-- [actor_ddl] 2026-03-19T17:16:01.759242Z
CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- [actor_ddl] 2026-03-19T17:16:01.759867Z
DROP VIEW IF EXISTS focus_roots;

-- [actor_ddl] 2026-03-19T17:16:01.760117Z
DROP VIEW IF EXISTS current_focus;

-- [actor_ddl] 2026-03-19T17:16:01.760175Z
CREATE MATERIALIZED VIEW current_focus AS
SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Wait 5ms
-- [actor_ddl] 2026-03-19T17:16:01.765729Z
-- Resolves focus targets to block IDs. A focus target can be either:
-- - A document URI (doc:xxx) -> root_id = direct children of that document
-- - A block URI (block:xxx) -> root_id = the block itself
-- UNION ALL produces both so downstream queries get a simple equality join.
CREATE MATERIALIZED VIEW focus_roots AS
SELECT cf.region, cf.block_id, b.id AS root_id
FROM current_focus AS cf
JOIN block AS b ON b.parent_id = cf.block_id
UNION ALL
SELECT cf.region, cf.block_id, b.id AS root_id
FROM current_focus AS cf
JOIN block AS b ON b.id = cf.block_id;

-- Wait 7ms
-- [actor_query] 2026-03-19T17:16:01.772744Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- Wait 1ms
-- [actor_query] 2026-03-19T17:16:01.774384Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- [actor_query] 2026-03-19T17:16:01.774979Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- [actor_ddl] 2026-03-19T17:16:01.775299Z
CREATE TABLE IF NOT EXISTS sync_states (
    provider_name TEXT PRIMARY KEY NOT NULL,
    sync_token TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.775903Z
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

-- [actor_ddl] 2026-03-19T17:16:01.776455Z
CREATE INDEX IF NOT EXISTS idx_operation_entity_name
ON operation(entity_name);

-- [actor_ddl] 2026-03-19T17:16:01.776961Z
CREATE INDEX IF NOT EXISTS idx_operation_created_at
ON operation(created_at);

-- [actor_ddl] 2026-03-19T17:16:01.777427Z
CREATE MATERIALIZED VIEW IF NOT EXISTS block_with_path AS
WITH RECURSIVE paths AS (
    -- Base case: root blocks (those whose parent is a document, not another block)
    SELECT
        id,
        parent_id,
        content,
        content_type,
        source_language,
        source_name,
        properties,
        created_at,
        updated_at,
        '/' || id as path,
        id as root_id
    FROM block
    WHERE parent_id LIKE 'doc:%'
       OR parent_id LIKE 'sentinel:%'
    UNION ALL
    -- Recursive case: build path from parent
    SELECT
        b.id,
        b.parent_id,
        b.content,
        b.content_type,
        b.source_language,
        b.source_name,
        b.properties,
        b.created_at,
        b.updated_at,
        p.path || '/' || b.id as path,
        p.root_id
    FROM block b
    INNER JOIN paths p ON b.parent_id = p.id
)
SELECT * FROM paths;

-- Wait 36ms
-- [actor_ddl] 2026-03-19T17:16:01.814415Z
CREATE TABLE IF NOT EXISTS document (
  id TEXT PRIMARY KEY NOT NULL,
  parent_id TEXT NOT NULL,
  name TEXT NOT NULL,
  sort_key TEXT NOT NULL,
  properties TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.814795Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document (parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.814962Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document (name);

-- [actor_query] 2026-03-19T17:16:01.815346Z
INSERT OR IGNORE INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_ddl] 2026-03-19T17:16:01.815940Z
CREATE TABLE IF NOT EXISTS directory (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  depth INTEGER NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.816069Z
CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory (parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.816178Z
CREATE TABLE IF NOT EXISTS file (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  document_id TEXT,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.816288Z
CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file (parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.816363Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file (document_id);

-- [actor_ddl] 2026-03-19T17:16:01.816629Z
CREATE TABLE IF NOT EXISTS block (
  id TEXT PRIMARY KEY NOT NULL,
  parent_id TEXT NOT NULL,
  document_id TEXT NOT NULL,
  content TEXT NOT NULL,
  content_type TEXT NOT NULL,
  source_language TEXT,
  source_name TEXT,
  properties TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.816766Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block (parent_id);

-- [actor_ddl] 2026-03-19T17:16:01.816838Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block (document_id);

-- [actor_ddl] 2026-03-19T17:16:01.816968Z
CREATE TABLE IF NOT EXISTS sync_states (
  provider_name TEXT PRIMARY KEY NOT NULL,
  sync_token TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.817368Z
CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    status TEXT DEFAULT 'confirmed',
    payload TEXT NOT NULL,
    trace_id TEXT,
    command_id TEXT,
    created_at INTEGER NOT NULL,
    processed_by_loro INTEGER DEFAULT 0,
    processed_by_org INTEGER DEFAULT 0,
    processed_by_cache INTEGER DEFAULT 0,
    speculative_id TEXT,
    rejection_reason TEXT
);

-- [actor_ddl] 2026-03-19T17:16:01.818255Z
CREATE INDEX IF NOT EXISTS idx_events_loro_pending
ON events(created_at)
WHERE processed_by_loro = 0 AND origin != 'loro' AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T17:16:01.818935Z
CREATE INDEX IF NOT EXISTS idx_events_org_pending
ON events(created_at)
WHERE processed_by_org = 0 AND origin != 'org' AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T17:16:01.819448Z
CREATE INDEX IF NOT EXISTS idx_events_cache_pending
ON events(created_at)
WHERE processed_by_cache = 0 AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T17:16:01.819938Z
CREATE INDEX IF NOT EXISTS idx_events_aggregate
ON events(aggregate_type, aggregate_id, created_at);

-- [actor_ddl] 2026-03-19T17:16:01.820415Z
CREATE INDEX IF NOT EXISTS idx_events_command
ON events(command_id)
WHERE command_id IS NOT NULL;

-- Wait 1ms
-- [actor_query] 2026-03-19T17:16:01.821779Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T17:16:01.822045Z
CREATE TABLE IF NOT EXISTS "operation" (
  id INTEGER PRIMARY KEY NOT NULL,
  operation TEXT NOT NULL,
  inverse TEXT,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  display_name TEXT NOT NULL,
  entity_name TEXT NOT NULL,
  op_name TEXT NOT NULL
);

-- [actor_ddl] 2026-03-19T17:16:01.822265Z
CREATE INDEX IF NOT EXISTS idx_operation_created_at ON operation (created_at);

-- [actor_query] 2026-03-19T17:16:01.822385Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T17:16:01.822563Z
CREATE INDEX IF NOT EXISTS idx_operation_entity_name ON operation (entity_name);

-- [actor_query] 2026-03-19T17:16:01.822678Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_query] 2026-03-19T17:16:01.822984Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T17:16:01.823170Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_b271926fc3f569a8 AS SELECT * FROM document;

-- Wait 6ms
-- [actor_query] 2026-03-19T17:16:01.829708Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_query] 2026-03-19T17:16:01.829966Z
SELECT * FROM watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T17:16:01.830306Z
SELECT * FROM watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T17:16:01.830425Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_e2453b3c0b29a253';

-- [actor_query] 2026-03-19T17:16:01.830603Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_e2453b3c0b29a253';

-- [actor_query] 2026-03-19T17:16:01.830812Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_e2453b3c0b29a253';

-- [actor_ddl] 2026-03-19T17:16:01.831108Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_e2453b3c0b29a253 AS SELECT id, parent_id, source_language FROM block WHERE content_type = 'source' AND source_language IN ('holon_prql', 'holon_gql', 'holon_sql');

-- Wait 15ms
-- [actor_query] 2026-03-19T17:16:01.846551Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_block';

-- [actor_query] 2026-03-19T17:16:01.846739Z
SELECT * FROM watch_view_e2453b3c0b29a253;

-- [actor_ddl] 2026-03-19T17:16:01.846863Z
CREATE MATERIALIZED VIEW events_view_block AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'block';

-- Wait 10ms
-- [actor_query] 2026-03-19T17:16:01.857564Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T17:16:01.857769Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T17:16:01.857966Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_d77ac41ba85c1706';

-- [actor_ddl] 2026-03-19T17:16:01.858231Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_d77ac41ba85c1706 AS SELECT id, content FROM block WHERE content_type = 'source' AND source_language = 'holon_entity_profile_yaml';

-- Wait 3ms
-- [actor_query] 2026-03-19T17:16:01.861263Z
SELECT * FROM watch_view_d77ac41ba85c1706;

-- [actor_ddl] 2026-03-19T17:16:01.861636Z
CREATE TABLE IF NOT EXISTS nodes (id INTEGER PRIMARY KEY AUTOINCREMENT);

-- [actor_ddl] 2026-03-19T17:16:01.862258Z
CREATE TABLE IF NOT EXISTS edges (id INTEGER PRIMARY KEY AUTOINCREMENT, source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, type TEXT NOT NULL);

-- [actor_ddl] 2026-03-19T17:16:01.862864Z
CREATE TABLE IF NOT EXISTS property_keys (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT UNIQUE NOT NULL);

-- [actor_ddl] 2026-03-19T17:16:01.863505Z
CREATE TABLE IF NOT EXISTS node_labels (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, label TEXT NOT NULL, PRIMARY KEY (node_id, label));

-- [actor_ddl] 2026-03-19T17:16:01.864190Z
CREATE TABLE IF NOT EXISTS node_props_int (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.865059Z
CREATE TABLE IF NOT EXISTS node_props_text (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.865902Z
CREATE TABLE IF NOT EXISTS node_props_real (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value REAL NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_tx_begin] 2026-03-19T17:16:01.866496Z
BEGIN TRANSACTION (256 stmts);

-- [transaction_stmt] 2026-03-19T17:16:01.866519Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8B4QWXXZ6RZTFKJMM', 'directory.created', 'directory', 'Projects', 'org', 'confirmed', '{"data":{"id":"Projects","name":"Projects","parent_id":"null","depth":1},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.866913Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QMD5Q6JTSQA3NA0A', 'directory.created', 'directory', '.jj', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj","name":".jj","parent_id":"null","depth":1}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.867105Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY846BB4SENHN7M1292', 'directory.created', 'directory', '.jj/working_copy', 'org', 'confirmed', '{"data":{"id":".jj/working_copy","name":"working_copy","parent_id":".jj","depth":2},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.867281Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY86H4S2RMB8VPRWZDM', 'directory.created', 'directory', '.jj/repo', 'org', 'confirmed', '{"data":{"id":".jj/repo","name":"repo","parent_id":".jj","depth":2},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.867454Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8JE3TVCNFKV0M15A0', 'directory.created', 'directory', '.jj/repo/op_store', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_store","name":"op_store","parent_id":".jj/repo","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.867654Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY85ADBNXE1NYEYHWC5', 'directory.created', 'directory', '.jj/repo/op_store/operations', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_store/operations","name":"operations","parent_id":".jj/repo/op_store","depth":4}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.867837Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8D629FB65A38WC6KM', 'directory.created', 'directory', '.jj/repo/op_store/views', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_store/views","name":"views","parent_id":".jj/repo/op_store","depth":4}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.868007Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8B4TKZSNYEC84T7EQ', 'directory.created', 'directory', '.jj/repo/op_heads', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_heads","name":"op_heads","parent_id":".jj/repo","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.868178Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Q110S8TMHVJJ8E6V', 'directory.created', 'directory', '.jj/repo/op_heads/heads', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_heads/heads","name":"heads","parent_id":".jj/repo/op_heads","depth":4},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.868351Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8F2C4KS8Z7KVJHEFK', 'directory.created', 'directory', '.jj/repo/index', 'org', 'confirmed', '{"data":{"id":".jj/repo/index","name":"index","parent_id":".jj/repo","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.868525Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8B455WY15P8BHXBEG', 'directory.created', 'directory', '.jj/repo/index/op_links', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/op_links","name":"op_links","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.868702Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8E2QDRBKGRKW0C2A4', 'directory.created', 'directory', '.jj/repo/index/operations', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/index/operations","name":"operations","parent_id":".jj/repo/index","depth":4}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.868878Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8PQC97CZBEHYQJ78Q', 'directory.created', 'directory', '.jj/repo/index/changed_paths', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/changed_paths","name":"changed_paths","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.869052Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8H1Q51RAFDGWS8H9R', 'directory.created', 'directory', '.jj/repo/index/segments', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/index/segments","name":"segments","parent_id":".jj/repo/index","depth":4}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.869229Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8NJWWAY9Z5H3JDJ84', 'directory.created', 'directory', '.jj/repo/submodule_store', 'org', 'confirmed', '{"data":{"id":".jj/repo/submodule_store","name":"submodule_store","parent_id":".jj/repo","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.869414Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QWYX4WHDYFGYDCFJ', 'directory.created', 'directory', '.jj/repo/store', 'org', 'confirmed', '{"data":{"id":".jj/repo/store","name":"store","parent_id":".jj/repo","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.869611Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8KA103JX4NHT1X6RF', 'directory.created', 'directory', '.jj/repo/store/extra', 'org', 'confirmed', '{"data":{"id":".jj/repo/store/extra","name":"extra","parent_id":".jj/repo/store","depth":4},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.869795Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY81BKWTJ9R0YQXFRWG', 'directory.created', 'directory', '.jj/repo/store/extra/heads', 'org', 'confirmed', '{"data":{"id":".jj/repo/store/extra/heads","name":"heads","parent_id":".jj/repo/store/extra","depth":5},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.870023Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8XR7N5CZ4NKV5CB2K', 'directory.created', 'directory', '.git', 'org', 'confirmed', '{"data":{"id":".git","name":".git","parent_id":"null","depth":1},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.870241Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY84XXBN3JAV40JJ44R', 'directory.created', 'directory', '.git/objects', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects","name":"objects","parent_id":".git","depth":2}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.870436Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY85TFE1JT5PZ5Z6ZH7', 'directory.created', 'directory', '.git/objects/61', 'org', 'confirmed', '{"data":{"id":".git/objects/61","name":"61","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.870626Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8VE5SJ6Y0FMXMZJ05', 'directory.created', 'directory', '.git/objects/0d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/0d","name":"0d","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.870814Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8BRJT8TGXN682SP4R', 'directory.created', 'directory', '.git/objects/95', 'org', 'confirmed', '{"data":{"id":".git/objects/95","name":"95","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.871009Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8ZZ0G4K7BCGWXJTX6', 'directory.created', 'directory', '.git/objects/59', 'org', 'confirmed', '{"data":{"id":".git/objects/59","name":"59","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.871195Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY885DVCT2G67HBXAK5', 'directory.created', 'directory', '.git/objects/92', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/92","name":"92","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.871380Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8FV6W2E62KPMP1F44', 'directory.created', 'directory', '.git/objects/0c', 'org', 'confirmed', '{"data":{"id":".git/objects/0c","name":"0c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.871567Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8VMXVWAG1HAT7SEZ9', 'directory.created', 'directory', '.git/objects/66', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/66","name":"66","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.871753Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY88WHE4P643TN8JB6Z', 'directory.created', 'directory', '.git/objects/3e', 'org', 'confirmed', '{"data":{"id":".git/objects/3e","name":"3e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.871938Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8BSNRAAYHJA7N06G5', 'directory.created', 'directory', '.git/objects/50', 'org', 'confirmed', '{"data":{"id":".git/objects/50","name":"50","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.872124Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY88BT6RDHTQ7RP16Q7', 'directory.created', 'directory', '.git/objects/3b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3b","name":"3b","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.872310Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8VCDYMGBN0QB7HPCY', 'directory.created', 'directory', '.git/objects/6f', 'org', 'confirmed', '{"data":{"id":".git/objects/6f","name":"6f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.872503Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8PHKS3W1PEJ02WF40', 'directory.created', 'directory', '.git/objects/03', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/03","name":"03","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.872705Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8NW5S0ZDN8VPETJK4', 'directory.created', 'directory', '.git/objects/9b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/9b","name":"9b","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.872892Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8XYHDWSSQQ2B9Y1Z8', 'directory.created', 'directory', '.git/objects/9e', 'org', 'confirmed', '{"data":{"id":".git/objects/9e","name":"9e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.873081Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8B5ZBRDCPY00F2FCJ', 'directory.created', 'directory', '.git/objects/04', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/04","name":"04","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.873269Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8MX2BYB45GR5NDKVN', 'directory.created', 'directory', '.git/objects/32', 'org', 'confirmed', '{"data":{"id":".git/objects/32","name":"32","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.873456Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY86B2CN7ZZC2QM025R', 'directory.created', 'directory', '.git/objects/35', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/35","name":"35","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.873645Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8MQ662738BBPPPKX5', 'directory.created', 'directory', '.git/objects/69', 'org', 'confirmed', '{"data":{"id":".git/objects/69","name":"69","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.873835Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QK97Z5K1X530FJ8X', 'directory.created', 'directory', '.git/objects/3c', 'org', 'confirmed', '{"data":{"id":".git/objects/3c","name":"3c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.874029Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8R7N23QP6MWZTW2N5', 'directory.created', 'directory', '.git/objects/56', 'org', 'confirmed', '{"data":{"id":".git/objects/56","name":"56","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.874221Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY89J6QPRMMKRR1T84T', 'directory.created', 'directory', '.git/objects/51', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/51","name":"51","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.874412Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8W2WKJNXR9N3R7QTM', 'directory.created', 'directory', '.git/objects/3d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3d","name":"3d","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.874606Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8SK03315WMQXYEX8R', 'directory.created', 'directory', '.git/objects/58', 'org', 'confirmed', '{"data":{"id":".git/objects/58","name":"58","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.874800Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8EHH0P4NGM6P6JXTS', 'directory.created', 'directory', '.git/objects/67', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/67","name":"67","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.874997Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8T1HV4D988QKG908X', 'directory.created', 'directory', '.git/objects/93', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/93","name":"93","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.875195Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY87KEDFWPE6DR01FPA', 'directory.created', 'directory', '.git/objects/94', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/94","name":"94","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.875391Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8ZD69F40EHDQ7ZJA4', 'directory.created', 'directory', '.git/objects/60', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/60","name":"60","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.875589Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY80W90ZTJQ1W0BJS6C', 'directory.created', 'directory', '.git/objects/34', 'org', 'confirmed', '{"data":{"id":".git/objects/34","name":"34","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.875795Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8BZDPHMQTPVA5XH1R', 'directory.created', 'directory', '.git/objects/5a', 'org', 'confirmed', '{"data":{"id":".git/objects/5a","name":"5a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.875990Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WKVKKAN88PZTR68W', 'directory.created', 'directory', '.git/objects/5f', 'org', 'confirmed', '{"data":{"id":".git/objects/5f","name":"5f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.876186Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8XJ816GZ2XHFFT4W7', 'directory.created', 'directory', '.git/objects/33', 'org', 'confirmed', '{"data":{"id":".git/objects/33","name":"33","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.876388Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8X0R9YRAS74Y4YHT0', 'directory.created', 'directory', '.git/objects/05', 'org', 'confirmed', '{"data":{"id":".git/objects/05","name":"05","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.876587Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8S4T5CQJEDSR9TETA', 'directory.created', 'directory', '.git/objects/9c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/9c","name":"9c","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.876788Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY82EXVXARTG5K8C2EH', 'directory.created', 'directory', '.git/objects/02', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/02","name":"02","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.876988Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY89RCRQA4TH83QD64T', 'directory.created', 'directory', '.git/objects/a4', 'org', 'confirmed', '{"data":{"id":".git/objects/a4","name":"a4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.877189Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8RNVD30WN9T73A6G3', 'directory.created', 'directory', '.git/objects/b5', 'org', 'confirmed', '{"data":{"id":".git/objects/b5","name":"b5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.877389Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY813MKCZE26EDCWTQ4', 'directory.created', 'directory', '.git/objects/b2', 'org', 'confirmed', '{"data":{"id":".git/objects/b2","name":"b2","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.877591Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8X9C28G0EZT2VVB1B', 'directory.created', 'directory', '.git/objects/d9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d9","name":"d9","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.877796Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY83P5AFTWK0SZQ908V', 'directory.created', 'directory', '.git/objects/ac', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ac","name":"ac","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.878002Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY80C2VXMSQN239P28A', 'directory.created', 'directory', '.git/objects/ad', 'org', 'confirmed', '{"data":{"id":".git/objects/ad","name":"ad","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.878205Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WXSJEND1KKJ0HDF3', 'directory.created', 'directory', '.git/objects/bb', 'org', 'confirmed', '{"data":{"id":".git/objects/bb","name":"bb","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.878414Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QTMARJ40D54WZN05', 'directory.created', 'directory', '.git/objects/d7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d7","name":"d7","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.878620Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8SCVMS6Z72V932FGN', 'directory.created', 'directory', '.git/objects/d0', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d0","name":"d0","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.878825Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY82NRT708M9G640TZJ', 'directory.created', 'directory', '.git/objects/be', 'org', 'confirmed', '{"data":{"id":".git/objects/be","name":"be","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.879041Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8CGSNDEBWGTQ5KX60', 'directory.created', 'directory', '.git/objects/b3', 'org', 'confirmed', '{"data":{"id":".git/objects/b3","name":"b3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.879251Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8A7T0YMVTNMVVWFDX', 'directory.created', 'directory', '.git/objects/df', 'org', 'confirmed', '{"data":{"id":".git/objects/df","name":"df","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.879457Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY83HYXP509K7VW5JND', 'directory.created', 'directory', '.git/objects/a5', 'org', 'confirmed', '{"data":{"id":".git/objects/a5","name":"a5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.879664Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8YNW3VZAA3G6WVH0T', 'directory.created', 'directory', '.git/objects/bd', 'org', 'confirmed', '{"data":{"id":".git/objects/bd","name":"bd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.879872Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8M5GNZ9620XZPJCMY', 'directory.created', 'directory', '.git/objects/d1', 'org', 'confirmed', '{"data":{"id":".git/objects/d1","name":"d1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.880084Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8A5QQYCVGMM9WGWWR', 'directory.created', 'directory', '.git/objects/d6', 'org', 'confirmed', '{"data":{"id":".git/objects/d6","name":"d6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.880294Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WQ2NQ3WMM01RD14P', 'directory.created', 'directory', '.git/objects/bc', 'org', 'confirmed', '{"data":{"id":".git/objects/bc","name":"bc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.880532Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QVYSK0N2TMZ4M8EH', 'directory.created', 'directory', '.git/objects/ae', 'org', 'confirmed', '{"data":{"id":".git/objects/ae","name":"ae","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.880745Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8G85AVMRMZQHZB9SQ', 'directory.created', 'directory', '.git/objects/d8', 'org', 'confirmed', '{"data":{"id":".git/objects/d8","name":"d8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.880962Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8NXH8D1QP521K3X84', 'directory.created', 'directory', '.git/objects/ab', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ab","name":"ab","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.881184Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8SRA96N4YGE9VC0AP', 'directory.created', 'directory', '.git/objects/e5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e5","name":"e5","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.881401Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WGEE0BPZD878AR3N', 'directory.created', 'directory', '.git/objects/e2', 'org', 'confirmed', '{"data":{"id":".git/objects/e2","name":"e2","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.881617Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY818SRP1W43WM7QKNP', 'directory.created', 'directory', '.git/objects/f4', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f4","name":"f4","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.881832Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8450XXBNDAC119W57', 'directory.created', 'directory', '.git/objects/f3', 'org', 'confirmed', '{"data":{"id":".git/objects/f3","name":"f3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.882048Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8ZK6E6XQ0WBH0Z5VR', 'directory.created', 'directory', '.git/objects/c7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c7","name":"c7","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.882266Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY83NZABKEKY5TP6TZX', 'directory.created', 'directory', '.git/objects/ee', 'org', 'confirmed', '{"data":{"id":".git/objects/ee","name":"ee","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.882493Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8ZBEKFJ1H6TVFYQD1', 'directory.created', 'directory', '.git/objects/c9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c9","name":"c9","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.882709Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8T0GQ95S2VF354VVX', 'directory.created', 'directory', '.git/objects/fd', 'org', 'confirmed', '{"data":{"id":".git/objects/fd","name":"fd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.882926Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY898J4Q6E3NCCESQFW', 'directory.created', 'directory', '.git/objects/f2', 'org', 'confirmed', '{"data":{"id":".git/objects/f2","name":"f2","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.883143Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Z1SYG3201CDYKGKT', 'directory.created', 'directory', '.git/objects/f5', 'org', 'confirmed', '{"data":{"id":".git/objects/f5","name":"f5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.883361Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HRYF9SNDS1H2XBBJ', 'directory.created', 'directory', '.git/objects/cf', 'org', 'confirmed', '{"data":{"id":".git/objects/cf","name":"cf","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.883580Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HARFF3EDGGY9EWXT', 'directory.created', 'directory', '.git/objects/ca', 'org', 'confirmed', '{"data":{"id":".git/objects/ca","name":"ca","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.883801Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Y9QB09E56V9W31TF', 'directory.created', 'directory', '.git/objects/fe', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/fe","name":"fe","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.884022Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY83W6VN3PE0TNT5HRY', 'directory.created', 'directory', '.git/objects/c8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c8","name":"c8","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.884244Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8GCR2PVEAFS9QBSE6', 'directory.created', 'directory', '.git/objects/fb', 'org', 'confirmed', '{"data":{"id":".git/objects/fb","name":"fb","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.884466Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QNKE86HWJFSWQ1VC', 'directory.created', 'directory', '.git/objects/ed', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ed","name":"ed","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.884689Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HPE9RV3CZQ2TBF2N', 'directory.created', 'directory', '.git/objects/c1', 'org', 'confirmed', '{"data":{"id":".git/objects/c1","name":"c1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.884913Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Z3AC16S7BJX1JSG7', 'directory.created', 'directory', '.git/objects/c6', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c6","name":"c6","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.885137Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8QCWJMDP5SH2DZT4N', 'directory.created', 'directory', '.git/objects/ec', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ec","name":"ec","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.885359Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8455E6J63YF6E9H3T', 'directory.created', 'directory', '.git/objects/4e', 'org', 'confirmed', '{"data":{"id":".git/objects/4e","name":"4e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.885573Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8BP23T9NCGMRV3SCH', 'directory.created', 'directory', '.git/objects/18', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/18","name":"18","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.885786Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY87NDMWCXEDWPQDD6Y', 'directory.created', 'directory', '.git/objects/27', 'org', 'confirmed', '{"data":{"id":".git/objects/27","name":"27","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.886038Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8CQFMA42ZBPX53Q9S', 'directory.created', 'directory', '.git/objects/4b', 'org', 'confirmed', '{"data":{"id":".git/objects/4b","name":"4b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.886254Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8BC3ZCJ6MKW2XDFQA', 'directory.created', 'directory', '.git/objects/pack', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/pack","name":"pack","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.886489Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8KSTQWP6W5DNBPT86', 'directory.created', 'directory', '.git/objects/11', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/11","name":"11","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.886706Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY863GSJX95PCY7897A', 'directory.created', 'directory', '.git/objects/7d', 'org', 'confirmed', '{"data":{"id":".git/objects/7d","name":"7d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.886926Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WG866MEKMH12TJ8S', 'directory.created', 'directory', '.git/objects/7c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/7c","name":"7c","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.887147Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8P6PPC15KBYHP3XMZ', 'directory.created', 'directory', '.git/objects/16', 'org', 'confirmed', '{"data":{"id":".git/objects/16","name":"16","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.887381Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8480TGPKHJ3Y3W8E6', 'directory.created', 'directory', '.git/objects/45', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/45","name":"45","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.887611Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WXGXPYAP1S5DMAZ7', 'directory.created', 'directory', '.git/objects/1f', 'org', 'confirmed', '{"data":{"id":".git/objects/1f","name":"1f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.888136Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8RZJRB07H95WRABET', 'directory.created', 'directory', '.git/objects/73', 'org', 'confirmed', '{"data":{"id":".git/objects/73","name":"73","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.888377Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8CBZEXVSCMPE3HMDM', 'directory.created', 'directory', '.git/objects/87', 'org', 'confirmed', '{"data":{"id":".git/objects/87","name":"87","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.888593Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY824Q4FD93AES35EWH', 'directory.created', 'directory', '.git/objects/80', 'org', 'confirmed', '{"data":{"id":".git/objects/80","name":"80","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.888824Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8CGZD84YWBEX9JJT4', 'directory.created', 'directory', '.git/objects/74', 'org', 'confirmed', '{"data":{"id":".git/objects/74","name":"74","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.889053Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY84KAHH93BF8X9EGBB', 'directory.created', 'directory', '.git/objects/1a', 'org', 'confirmed', '{"data":{"id":".git/objects/1a","name":"1a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.889277Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8PVB4FEJ8S9W0JQR2', 'directory.created', 'directory', '.git/objects/28', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/28","name":"28","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.889517Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8CTT9RPKJ6CEPNY4Z', 'directory.created', 'directory', '.git/objects/17', 'org', 'confirmed', '{"data":{"id":".git/objects/17","name":"17","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.889733Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8NKAFYTGPN58E5PRT', 'directory.created', 'directory', '.git/objects/7b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/7b","name":"7b","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.889994Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Y1CCV1M84Q9WAJGH', 'directory.created', 'directory', '.git/objects/8f', 'org', 'confirmed', '{"data":{"id":".git/objects/8f","name":"8f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.890219Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY810W182R863C9RFWE', 'directory.created', 'directory', '.git/objects/7e', 'org', 'confirmed', '{"data":{"id":".git/objects/7e","name":"7e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.890450Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Z5B2HZ6DJ2THR2EX', 'directory.created', 'directory', '.git/objects/10', 'org', 'confirmed', '{"data":{"id":".git/objects/10","name":"10","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.890686Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8RGK6GA2DNE50KCRX', 'directory.created', 'directory', '.git/objects/19', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/19","name":"19","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.890928Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8T8J790YAT4C704K5', 'directory.created', 'directory', '.git/objects/4c', 'org', 'confirmed', '{"data":{"id":".git/objects/4c","name":"4c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.891151Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY80MNQBFVP7PC5K71A', 'directory.created', 'directory', '.git/objects/26', 'org', 'confirmed', '{"data":{"id":".git/objects/26","name":"26","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.891388Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8MBQX83YDNDN608EC', 'directory.created', 'directory', '.git/objects/4d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4d","name":"4d","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.891612Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY80VG4SPHB7MQP84EZ', 'directory.created', 'directory', '.git/objects/75', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/75","name":"75","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.891841Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8MMP3GXFFAY8S912Z', 'directory.created', 'directory', '.git/objects/81', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/81","name":"81","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.892077Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY84155F5C11T2J27C7', 'directory.created', 'directory', '.git/objects/86', 'org', 'confirmed', '{"data":{"id":".git/objects/86","name":"86","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.892305Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY88ZKJ4KV2ZXWRXDPF', 'directory.created', 'directory', '.git/objects/72', 'org', 'confirmed', '{"data":{"id":".git/objects/72","name":"72","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.892540Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8E05APSRFR8G7TW91', 'directory.created', 'directory', '.git/objects/44', 'org', 'confirmed', '{"data":{"id":".git/objects/44","name":"44","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.893089Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Q8R9AJG51APGMTJ2', 'directory.created', 'directory', '.git/objects/2a', 'org', 'confirmed', '{"data":{"id":".git/objects/2a","name":"2a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.893325Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HNFATSKYRTNYWRPA', 'directory.created', 'directory', '.git/objects/2f', 'org', 'confirmed', '{"data":{"id":".git/objects/2f","name":"2f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.893557Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8831NF88PFFXXWYC4', 'directory.created', 'directory', '.git/objects/43', 'org', 'confirmed', '{"data":{"id":".git/objects/43","name":"43","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.893784Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY853Q691TVJCWZSNRX', 'directory.created', 'directory', '.git/objects/88', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/88","name":"88","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.894037Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY814RQDM2BA7VCQ0RS', 'directory.created', 'directory', '.git/objects/9f', 'org', 'confirmed', '{"data":{"id":".git/objects/9f","name":"9f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.894324Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY89CH4VZW2P6FZPK7C', 'directory.created', 'directory', '.git/objects/07', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/07","name":"07","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.894590Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HD4WBNXDMNPAVF3F', 'directory.created', 'directory', '.git/objects/38', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/38","name":"38","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.894829Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8VVN4ATGKNKBD72SB', 'directory.created', 'directory', '.git/objects/00', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/00","name":"00","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.895064Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8M4KK3S8ABXDKDAED', 'directory.created', 'directory', '.git/objects/6e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/6e","name":"6e","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.895296Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY83RT2X4DKSJZSAQ8X', 'directory.created', 'directory', '.git/objects/9a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/9a","name":"9a","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.895530Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8KDJXY7NN2FFCH5JX', 'directory.created', 'directory', '.git/objects/5c', 'org', 'confirmed', '{"data":{"id":".git/objects/5c","name":"5c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.895765Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WNBZDFFC3CEG0QBX', 'directory.created', 'directory', '.git/objects/09', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/09","name":"09","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.895997Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY85DDZW0FGM5D4B0VW', 'directory.created', 'directory', '.git/objects/5d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5d","name":"5d","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.896244Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY83JATS5A2B65EPH4S', 'directory.created', 'directory', '.git/objects/info', 'org', 'confirmed', '{"data":{"id":".git/objects/info","name":"info","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.896476Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY81VF2Q8X09QWAJPSG', 'directory.created', 'directory', '.git/objects/91', 'org', 'confirmed', '{"data":{"id":".git/objects/91","name":"91","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.896730Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8RAJ1QEJDREG43H2Y', 'directory.created', 'directory', '.git/objects/65', 'org', 'confirmed', '{"data":{"id":".git/objects/65","name":"65","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.896970Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8AS95A1XB929JBW8F', 'directory.created', 'directory', '.git/objects/62', 'org', 'confirmed', '{"data":{"id":".git/objects/62","name":"62","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.897206Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8XDHTZXZA2WCNVM65', 'directory.created', 'directory', '.git/objects/96', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/96","name":"96","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.897428Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8BDVDY196JW4QD2EY', 'directory.created', 'directory', '.git/objects/3a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3a","name":"3a","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.897653Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8XT61D1P5SCW5ZXMX', 'directory.created', 'directory', '.git/objects/54', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/54","name":"54","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.897932Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY86PDX8Q219E2ZCVH3', 'directory.created', 'directory', '.git/objects/98', 'org', 'confirmed', '{"data":{"id":".git/objects/98","name":"98","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.898171Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8AN4KM1FGXHD5QRJ2', 'directory.created', 'directory', '.git/objects/53', 'org', 'confirmed', '{"data":{"id":".git/objects/53","name":"53","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.898413Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8X8YCV4TC9XX8NQ80', 'directory.created', 'directory', '.git/objects/3f', 'org', 'confirmed', '{"data":{"id":".git/objects/3f","name":"3f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.898637Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HDN2ZR0GT92H2J56', 'directory.created', 'directory', '.git/objects/30', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/30","name":"30","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.898880Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HAZHACWE3V93FGNG', 'directory.created', 'directory', '.git/objects/5e', 'org', 'confirmed', '{"data":{"id":".git/objects/5e","name":"5e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.899112Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8180136SP95DQBCP3', 'directory.created', 'directory', '.git/objects/5b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5b","name":"5b","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.899351Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY881YPQ55SC12JHKTR', 'directory.created', 'directory', '.git/objects/37', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/37","name":"37","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.899591Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY869P245D7CGBJ9J69', 'directory.created', 'directory', '.git/objects/08', 'org', 'confirmed', '{"data":{"id":".git/objects/08","name":"08","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.899837Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8211WWVA3B7MXDH4K', 'directory.created', 'directory', '.git/objects/6d', 'org', 'confirmed', '{"data":{"id":".git/objects/6d","name":"6d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.900080Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8WTXAN8K0QD711QZT', 'directory.created', 'directory', '.git/objects/01', 'org', 'confirmed', '{"data":{"id":".git/objects/01","name":"01","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.900319Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY837J60QXGFDK6NQ7B', 'directory.created', 'directory', '.git/objects/06', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/06","name":"06","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.900565Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8VGHZ7HZS7GCW9G6H', 'directory.created', 'directory', '.git/objects/6c', 'org', 'confirmed', '{"data":{"id":".git/objects/6c","name":"6c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.900809Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8HXHTDEMKWAAGAV8N', 'directory.created', 'directory', '.git/objects/39', 'org', 'confirmed', '{"data":{"id":".git/objects/39","name":"39","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.901403Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8S2XZAKS3AZNVEESW', 'directory.created', 'directory', '.git/objects/99', 'org', 'confirmed', '{"data":{"id":".git/objects/99","name":"99","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.901652Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8KJX63T1Z0NJCQNG8', 'directory.created', 'directory', '.git/objects/52', 'org', 'confirmed', '{"data":{"id":".git/objects/52","name":"52","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.901901Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8JD0WQ1499YQZ212W', 'directory.created', 'directory', '.git/objects/55', 'org', 'confirmed', '{"data":{"id":".git/objects/55","name":"55","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.902160Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8G975C5QQ7WTK01HW', 'directory.created', 'directory', '.git/objects/97', 'org', 'confirmed', '{"data":{"id":".git/objects/97","name":"97","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.902412Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8FFGYR01V4Q9C8A4S', 'directory.created', 'directory', '.git/objects/0a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/0a","name":"0a","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.902643Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8NH4EMPD1V9CTBXQ2', 'directory.created', 'directory', '.git/objects/90', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/90","name":"90","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.902894Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY813DJMXF43FT46JHW', 'directory.created', 'directory', '.git/objects/bf', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/bf","name":"bf","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.903132Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8N40X1GG310TK5C2Y', 'directory.created', 'directory', '.git/objects/d3', 'org', 'confirmed', '{"data":{"id":".git/objects/d3","name":"d3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.903385Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY82TJW6PFYPC9YPMFS', 'directory.created', 'directory', '.git/objects/d4', 'org', 'confirmed', '{"data":{"id":".git/objects/d4","name":"d4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.903625Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY871ZSWMVCGP2RFEC9', 'directory.created', 'directory', '.git/objects/ba', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ba","name":"ba","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.903886Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY85T5WB13XD6KAJSTK', 'directory.created', 'directory', '.git/objects/a0', 'org', 'confirmed', '{"data":{"id":".git/objects/a0","name":"a0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.904131Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY88PKG5J8HWCCN9WEC', 'directory.created', 'directory', '.git/objects/a7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a7","name":"a7","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.904391Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY81QN49SXYYWWX8925', 'directory.created', 'directory', '.git/objects/b8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/b8","name":"b8","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.904634Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY82VVN3AAB9KQQKXGE', 'directory.created', 'directory', '.git/objects/b1', 'org', 'confirmed', '{"data":{"id":".git/objects/b1","name":"b1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.904891Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY88KK4EA0AB55T1S06', 'directory.created', 'directory', '.git/objects/dd', 'org', 'confirmed', '{"data":{"id":".git/objects/dd","name":"dd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.905146Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Z7PFJG2ETA99BD4J', 'directory.created', 'directory', '.git/objects/dc', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/dc","name":"dc","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.905393Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8A596C3GN7AP232SC', 'directory.created', 'directory', '.git/objects/b6', 'org', 'confirmed', '{"data":{"id":".git/objects/b6","name":"b6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.905648Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8KJRZ2BQVHCMVMY21', 'directory.created', 'directory', '.git/objects/a9', 'org', 'confirmed', '{"data":{"id":".git/objects/a9","name":"a9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.905904Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8TYHMF9Q502GZSCR0', 'directory.created', 'directory', '.git/objects/d5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d5","name":"d5","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.906167Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY85Q6NC6KXP424D3N7', 'directory.created', 'directory', '.git/objects/d2', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d2","name":"d2","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.906435Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY84M84C55C9QQRN5M8', 'directory.created', 'directory', '.git/objects/aa', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/aa","name":"aa","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.906711Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8Y7M96B9XNG57BMW2', 'directory.created', 'directory', '.git/objects/af', 'org', 'confirmed', '{"data":{"id":".git/objects/af","name":"af","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.906963Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8SESSKMY7QKNNVXN8', 'directory.created', 'directory', '.git/objects/b7', 'org', 'confirmed', '{"data":{"id":".git/objects/b7","name":"b7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.907220Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8KCXSQSXS7FR0RPC9', 'directory.created', 'directory', '.git/objects/db', 'org', 'confirmed', '{"data":{"id":".git/objects/db","name":"db","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.907474Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY84TD31W1BG8DZSMV9', 'directory.created', 'directory', '.git/objects/a8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a8","name":"a8","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.907725Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8RYN1P9YTFF62CN3V', 'directory.created', 'directory', '.git/objects/de', 'org', 'confirmed', '{"data":{"id":".git/objects/de","name":"de","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.907985Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8ZB0SEBE4JFXGXPWE', 'directory.created', 'directory', '.git/objects/b0', 'org', 'confirmed', '{"data":{"id":".git/objects/b0","name":"b0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.908242Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY86YTC4BW04NNYWKJV', 'directory.created', 'directory', '.git/objects/b9', 'org', 'confirmed', '{"data":{"id":".git/objects/b9","name":"b9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.908507Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8NBHZ6K04J8TE8QYJ', 'directory.created', 'directory', '.git/objects/a1', 'org', 'confirmed', '{"data":{"id":".git/objects/a1","name":"a1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.909144Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY89D0VNXJN58RZ9K17', 'directory.created', 'directory', '.git/objects/ef', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ef","name":"ef","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.909408Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8PX4XSS6HRPHR5ZTG', 'directory.created', 'directory', '.git/objects/c3', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c3","name":"c3","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.909674Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY88BZZKZEHBN38WH3D', 'directory.created', 'directory', '.git/objects/c4', 'org', 'confirmed', '{"data":{"id":".git/objects/c4","name":"c4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.909953Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8DW4FAYPPSHV22J0T', 'directory.created', 'directory', '.git/objects/ea', 'org', 'confirmed', '{"data":{"id":".git/objects/ea","name":"ea","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.910216Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY919QJM8WN4KJFRH8X', 'directory.created', 'directory', '.git/objects/e1', 'org', 'confirmed', '{"data":{"id":".git/objects/e1","name":"e1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.910478Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9G1Y05F23RW5YFWZP', 'directory.created', 'directory', '.git/objects/cd', 'org', 'confirmed', '{"data":{"id":".git/objects/cd","name":"cd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.910756Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9WXJVKFCFSSMZJ56A', 'directory.created', 'directory', '.git/objects/cc', 'org', 'confirmed', '{"data":{"id":".git/objects/cc","name":"cc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.911019Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY915N8WNGV6SW0CA3V', 'directory.created', 'directory', '.git/objects/e6', 'org', 'confirmed', '{"data":{"id":".git/objects/e6","name":"e6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.911282Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9CRHGGY4TFV718EQP', 'directory.created', 'directory', '.git/objects/f9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f9","name":"f9","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.911540Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY932XXXA6ZK2D2JZ19', 'directory.created', 'directory', '.git/objects/f0', 'org', 'confirmed', '{"data":{"id":".git/objects/f0","name":"f0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.911802Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9KVS77KR2WHP7HMH6', 'directory.created', 'directory', '.git/objects/f7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f7","name":"f7","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.912434Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY96PY743MYDWVV3FGG', 'directory.created', 'directory', '.git/objects/e8', 'org', 'confirmed', '{"data":{"id":".git/objects/e8","name":"e8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.912708Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9R6DT95K0D033TF89', 'directory.created', 'directory', '.git/objects/fa', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/fa","name":"fa","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.912975Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9DYDT54SFR0NCBV8J', 'directory.created', 'directory', '.git/objects/ff', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ff","name":"ff","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.913241Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY90Y83SEVJKG1BW7TD', 'directory.created', 'directory', '.git/objects/c5', 'org', 'confirmed', '{"data":{"id":".git/objects/c5","name":"c5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.913514Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY996A5P5YYGBWB5SB8', 'directory.created', 'directory', '.git/objects/f6', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f6","name":"f6","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.913775Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY926VZREH0R3NS3ENW', 'directory.created', 'directory', '.git/objects/e9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e9","name":"e9","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.914437Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9MJ8XMZDWQKDNVNBE', 'directory.created', 'directory', '.git/objects/f1', 'org', 'confirmed', '{"data":{"id":".git/objects/f1","name":"f1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.914728Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9A2MXVETYSYRYTBKC', 'directory.created', 'directory', '.git/objects/e7', 'org', 'confirmed', '{"data":{"id":".git/objects/e7","name":"e7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.915009Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY963M41SCY22F520PT', 'directory.created', 'directory', '.git/objects/cb', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/cb","name":"cb","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.915669Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9TCKG2NRGXKX1ES7C', 'directory.created', 'directory', '.git/objects/f8', 'org', 'confirmed', '{"data":{"id":".git/objects/f8","name":"f8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.916368Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY95J4044WQP9TNSVVY', 'directory.created', 'directory', '.git/objects/ce', 'org', 'confirmed', '{"data":{"id":".git/objects/ce","name":"ce","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.916643Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9VC0M4TQ91BSV5ZBW', 'directory.created', 'directory', '.git/objects/e0', 'org', 'confirmed', '{"data":{"id":".git/objects/e0","name":"e0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.916907Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9ZDJT8V3F60KGBARN', 'directory.created', 'directory', '.git/objects/46', 'org', 'confirmed', '{"data":{"id":".git/objects/46","name":"46","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.917164Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9SQ71Y7CS0PKS51SA', 'directory.created', 'directory', '.git/objects/2c', 'org', 'confirmed', '{"data":{"id":".git/objects/2c","name":"2c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.917434Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9NG5GPXDQ9Q40HNCW', 'directory.created', 'directory', '.git/objects/79', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/79","name":"79","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.917697Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9EB5DYS40DDTQ2R7N', 'directory.created', 'directory', '.git/objects/2d', 'org', 'confirmed', '{"data":{"id":".git/objects/2d","name":"2d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.917984Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY91X51VBXGWZ1DR92C', 'directory.created', 'directory', '.git/objects/41', 'org', 'confirmed', '{"data":{"id":".git/objects/41","name":"41","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.918242Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9NSAZGVV60F2FMGW6', 'directory.created', 'directory', '.git/objects/1b', 'org', 'confirmed', '{"data":{"id":".git/objects/1b","name":"1b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.918511Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9TEHJ92X7V744TWP3', 'directory.created', 'directory', '.git/objects/77', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/77","name":"77","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.918776Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9QTQEEF7078ZB3K6K', 'directory.created', 'directory', '.git/objects/48', 'org', 'confirmed', '{"data":{"id":".git/objects/48","name":"48","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.919058Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9TY3V7G7X8N18J2E1', 'directory.created', 'directory', '.git/objects/1e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1e","name":"1e","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.919321Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9RJSJYY2BCX27W589', 'directory.created', 'directory', '.git/objects/84', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/84","name":"84","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.919600Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9QCKWB67ARD4620G3', 'directory.created', 'directory', '.git/objects/4a', 'org', 'confirmed', '{"data":{"id":".git/objects/4a","name":"4a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.919875Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9KC7798P11N767MVG', 'directory.created', 'directory', '.git/objects/24', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/24","name":"24","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.920141Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9Y705TVN394BF7CZ9', 'directory.created', 'directory', '.git/objects/23', 'org', 'confirmed', '{"data":{"id":".git/objects/23","name":"23","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.920416Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9DRXN987PH185MYRF', 'directory.created', 'directory', '.git/objects/4f', 'org', 'confirmed', '{"data":{"id":".git/objects/4f","name":"4f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.920687Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9B105GJS79GCMQKCS', 'directory.created', 'directory', '.git/objects/8d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8d","name":"8d","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.920974Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9EJ8NVRH21NGPXGX3', 'directory.created', 'directory', '.git/objects/15', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/15","name":"15","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.921731Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9ZK26ZRY7HVK5CS9X', 'directory.created', 'directory', '.git/objects/12', 'org', 'confirmed', '{"data":{"id":".git/objects/12","name":"12","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.922010Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9YXKAH8PS1SJDGYH3', 'directory.created', 'directory', '.git/objects/85', 'org', 'confirmed', '{"data":{"id":".git/objects/85","name":"85","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.922280Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY97BRGAY7TNZN2R8T4', 'directory.created', 'directory', '.git/objects/1d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1d","name":"1d","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.922553Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9KHEXXQ35PYDQK495', 'directory.created', 'directory', '.git/objects/71', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/71","name":"71","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.922831Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9GH8XKKAVVTB90V6N', 'directory.created', 'directory', '.git/objects/76', 'org', 'confirmed', '{"data":{"id":".git/objects/76","name":"76","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.923099Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY97WBFNS6ZPC9R77S7', 'directory.created', 'directory', '.git/objects/1c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1c","name":"1c","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.923386Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9FVMQMFQAAPY0WFZT', 'directory.created', 'directory', '.git/objects/82', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/82","name":"82","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.923648Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9AV7SXVCX2JC0N028', 'directory.created', 'directory', '.git/objects/49', 'org', 'confirmed', '{"data":{"id":".git/objects/49","name":"49","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.923929Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY94G3ZRM34TZWRPV3K', 'directory.created', 'directory', '.git/objects/40', 'org', 'confirmed', '{"data":{"id":".git/objects/40","name":"40","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.924214Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9PGVR2V8MAZ935482', 'directory.created', 'directory', '.git/objects/2e', 'org', 'confirmed', '{"data":{"id":".git/objects/2e","name":"2e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.924499Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9CRJM4M81B0TK59JE', 'directory.created', 'directory', '.git/objects/2b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/2b","name":"2b","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.925235Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY977VN09DVB1NFC5MY', 'directory.created', 'directory', '.git/objects/47', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/47","name":"47","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.925515Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9DTZT0M8NS1WWD2V8', 'directory.created', 'directory', '.git/objects/78', 'org', 'confirmed', '{"data":{"id":".git/objects/78","name":"78","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.926311Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY96BW25YJADX8G72NZ', 'directory.created', 'directory', '.git/objects/8b', 'org', 'confirmed', '{"data":{"id":".git/objects/8b","name":"8b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.926610Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9C9027981XF68GK7F', 'directory.created', 'directory', '.git/objects/13', 'org', 'confirmed', '{"data":{"id":".git/objects/13","name":"13","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.926899Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9C9Q1BC6K4ERY71XG', 'directory.created', 'directory', '.git/objects/7a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/7a","name":"7a","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.927656Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9CPXW56W5CKKWSCP7', 'directory.created', 'directory', '.git/objects/14', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/14","name":"14","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.927942Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9TRYRB7PTDDA3FN7Y', 'directory.created', 'directory', '.git/objects/8e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8e","name":"8e","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.928213Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY916AEW3128ZWCTE9V', 'directory.created', 'directory', '.git/objects/22', 'org', 'confirmed', '{"data":{"id":".git/objects/22","name":"22","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.928969Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9J6N2AQHD2HGZK5YM', 'directory.created', 'directory', '.git/objects/25', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/25","name":"25","parent_id":".git/objects","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.929753Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9QNF6TCPW92EPKV18', 'directory.created', 'directory', '.git/rr-cache', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/rr-cache","name":"rr-cache","parent_id":".git","depth":2}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.930041Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9HHXR6XQKE5JFTJQ1', 'directory.created', 'directory', '.git/info', 'org', 'confirmed', '{"data":{"id":".git/info","name":"info","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.930313Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY97CP5PJTZ7RY6X9EK', 'directory.created', 'directory', '.git/logs', 'org', 'confirmed', '{"data":{"id":".git/logs","name":"logs","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.930578Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9R0131XVEJQHA7TZ1', 'directory.created', 'directory', '.git/logs/refs', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/logs/refs","name":"refs","parent_id":".git/logs","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.930848Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY95AFB6A5M00Q9RGVC', 'directory.created', 'directory', '.git/logs/refs/heads', 'org', 'confirmed', '{"data":{"id":".git/logs/refs/heads","name":"heads","parent_id":".git/logs/refs","depth":4},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.931134Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9NPKEWMSP76AAETY5', 'directory.created', 'directory', '.git/hooks', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/hooks","name":"hooks","parent_id":".git","depth":2}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.931406Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY93NWJ4FG0EQB76HTE', 'directory.created', 'directory', '.git/refs', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/refs","name":"refs","parent_id":".git","depth":2}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.931695Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9K578NERJT71CNX7Y', 'directory.created', 'directory', '.git/refs/heads', 'org', 'confirmed', '{"data":{"id":".git/refs/heads","name":"heads","parent_id":".git/refs","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.931968Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9XXVHJSVENRV5ZE0Y', 'directory.created', 'directory', '.git/refs/tags', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/refs/tags","name":"tags","parent_id":".git/refs","depth":3}}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.932244Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9FG56ZCYYR3CS6ASM', 'directory.created', 'directory', '.git/refs/jj', 'org', 'confirmed', '{"data":{"id":".git/refs/jj","name":"jj","parent_id":".git/refs","depth":3},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.932522Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY9HWK5JTD1S4RJ0229', 'directory.created', 'directory', '.git/refs/jj/keep', 'org', 'confirmed', '{"data":{"id":".git/refs/jj/keep","name":"keep","parent_id":".git/refs/jj","depth":4},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561865, NULL, NULL);

-- [actor_tx_commit] 2026-03-19T17:16:01.932816Z
COMMIT;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T17:16:01.934564Z
CREATE TABLE IF NOT EXISTS node_props_bool (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_query] 2026-03-19T17:16:01.935420Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_tx_begin] 2026-03-19T17:16:01.935822Z
BEGIN TRANSACTION (4 stmts);

-- [transaction_stmt] 2026-03-19T17:16:01.935844Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY81A42QMGC345X7CVR', 'file.created', 'file', 'file:index.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:index.org","name":"index.org","parent_id":"null","content_hash":"2c45843e5c445c10c43f30dc4aaf59018fe6696700adf391a4347650b1977af2","document_id":null}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.936147Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8C3FZFBNVS8CBC9Z2', 'file.created', 'file', 'file:__default__.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:__default__.org","name":"__default__.org","parent_id":"null","content_hash":"9fd72b98d2fdcc99b3a0b4132dd515fa62233e6482c4ae90d39f429f40826f78","document_id":null}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.936432Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY87T8AFCCVZJGMM5V5', 'file.created', 'file', 'file:ClaudeCode.org', 'org', 'confirmed', '{"data":{"id":"file:ClaudeCode.org","name":"ClaudeCode.org","parent_id":"null","content_hash":"e57d79f0cf908c2c3b5a4ef5e5c8f4a5044c05dd4c05fa94ab2f2ae845336566","document_id":null},"change_type":"created"}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.936715Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHNY8RS2263JAXV33PF63', 'file.created', 'file', 'file:Projects/Holon.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:Projects/Holon.org","name":"Holon.org","parent_id":"Projects","content_hash":"b42533dcc01eb91e5e075876075c2768286edbad397affec99ec19eacb1e7154","document_id":null}}', '00000000000000000000004000000001', NULL, 1773940561864, NULL, NULL);

-- [actor_tx_commit] 2026-03-19T17:16:01.936995Z
COMMIT;

-- [actor_ddl] 2026-03-19T17:16:01.937229Z
CREATE TABLE IF NOT EXISTS node_props_json (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.937931Z
CREATE TABLE IF NOT EXISTS edge_props_int (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.938560Z
CREATE TABLE IF NOT EXISTS edge_props_text (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.939220Z
CREATE TABLE IF NOT EXISTS edge_props_real (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value REAL NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.939940Z
CREATE TABLE IF NOT EXISTS edge_props_bool (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.940761Z
CREATE TABLE IF NOT EXISTS edge_props_json (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T17:16:01.941374Z
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);

-- [actor_ddl] 2026-03-19T17:16:01.941985Z
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);

-- [actor_ddl] 2026-03-19T17:16:01.942579Z
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(type);

-- [actor_ddl] 2026-03-19T17:16:01.943153Z
CREATE INDEX IF NOT EXISTS idx_node_labels_label ON node_labels(label, node_id);

-- [actor_ddl] 2026-03-19T17:16:01.943826Z
CREATE INDEX IF NOT EXISTS idx_property_keys_key ON property_keys(key);

-- [actor_ddl] 2026-03-19T17:16:01.944457Z
CREATE INDEX IF NOT EXISTS idx_node_props_int_key_value ON node_props_int(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T17:16:01.945351Z
CREATE INDEX IF NOT EXISTS idx_node_props_text_key_value ON node_props_text(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T17:16:01.945992Z
CREATE INDEX IF NOT EXISTS idx_node_props_real_key_value ON node_props_real(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T17:16:01.946622Z
CREATE INDEX IF NOT EXISTS idx_node_props_bool_key_value ON node_props_bool(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T17:16:01.947457Z
CREATE INDEX IF NOT EXISTS idx_node_props_json_key_value ON node_props_json(key_id, node_id);

-- [actor_ddl] 2026-03-19T17:16:01.948096Z
CREATE INDEX IF NOT EXISTS idx_edge_props_int_key_value ON edge_props_int(key_id, value, edge_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T17:16:01.949123Z
CREATE INDEX IF NOT EXISTS idx_edge_props_text_key_value ON edge_props_text(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T17:16:01.949768Z
CREATE INDEX IF NOT EXISTS idx_edge_props_real_key_value ON edge_props_real(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T17:16:01.950374Z
CREATE INDEX IF NOT EXISTS idx_edge_props_bool_key_value ON edge_props_bool(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T17:16:01.950977Z
CREATE INDEX IF NOT EXISTS idx_edge_props_json_key_value ON edge_props_json(key_id, edge_id);

-- Wait 31ms
-- [actor_tx_begin] 2026-03-19T17:16:01.982570Z
BEGIN TRANSACTION (24 stmts);

-- [transaction_stmt] 2026-03-19T17:16:01.982597Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content", "document_id", "parent_id", "content_type", "id", "properties") VALUES (1773940561981, 1773940561939, 'Holon Layout', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'text', 'block:root-layout', '{"ID":"root-layout","sequence":0}');

-- [transaction_stmt] 2026-03-19T17:16:01.982896Z
INSERT OR REPLACE INTO block ("document_id", "id", "parent_id", "content", "content_type", "created_at", "source_language", "updated_at", "properties") VALUES ('doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:root-layout::src::0', 'block:root-layout', 'MATCH (root:block)<-[:CHILD_OF]-(d:block)
WHERE root.id = ''block:root-layout'' AND d.content_type = ''text''
RETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority
ORDER BY d.properties.sequence
', 'source', 1773940561939, 'holon_gql', 1773940561981, '{"ID":"root-layout::src::0","sequence":1}');

-- [transaction_stmt] 2026-03-19T17:16:01.983097Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "created_at", "source_language", "parent_id", "content_type", "updated_at", "properties") VALUES ('doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:holon-app-layout::render::0', 'columns(#{gap: 4, sort_key: col("sequence"), item_template: block_ref()})
', 1773940561939, 'render', 'block:root-layout', 'source', 1773940561981, '{"sequence":2,"ID":"holon-app-layout::render::0"}');

-- [transaction_stmt] 2026-03-19T17:16:01.983268Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "id", "parent_id", "document_id", "content", "content_type", "properties") VALUES (1773940561981, 1773940561979, 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'block:root-layout', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'Left Sidebar', 'text', '{"ID":"e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","sequence":3,"collapse_to":"drawer"}');

-- [transaction_stmt] 2026-03-19T17:16:01.983421Z
INSERT OR REPLACE INTO block ("document_id", "id", "content_type", "created_at", "updated_at", "source_language", "content", "parent_id", "properties") VALUES ('doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:block:left_sidebar::render::0', 'source', 1773940561979, 1773940561981, 'render', 'list(#{sortkey: "name", item_template: selectable(row(icon("notebook"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})
', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', '{"sequence":4,"ID":"block:left_sidebar::render::0"}');

-- [transaction_stmt] 2026-03-19T17:16:01.983588Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "content", "document_id", "id", "source_language", "content_type", "updated_at", "properties") VALUES (1773940561979, 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'from document
filter name != "" && name != "index" && name != "__default__"
', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:block:left_sidebar::src::0', 'holon_prql', 'source', 1773940561981, '{"sequence":5,"ID":"block:left_sidebar::src::0"}');

-- [transaction_stmt] 2026-03-19T17:16:01.983749Z
INSERT OR REPLACE INTO block ("parent_id", "content", "id", "created_at", "updated_at", "document_id", "content_type", "properties") VALUES ('block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'All Documents', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 1773940561980, 1773940561981, 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'text', '{"sequence":6,"ID":"e8b05308-37ed-49a6-9c94-bccf9e3499bc"}');

-- [transaction_stmt] 2026-03-19T17:16:01.983900Z
INSERT OR REPLACE INTO block ("id", "created_at", "content_type", "document_id", "parent_id", "updated_at", "content", "properties") VALUES ('block:66c6aae4-4829-4d54-b92f-6638fda03368', 1773940561980, 'text', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 1773940561981, 'Test', '{"sequence":7,"ID":"66c6aae4-4829-4d54-b92f-6638fda03368"}');

-- [transaction_stmt] 2026-03-19T17:16:01.984050Z
INSERT OR REPLACE INTO block ("updated_at", "content", "created_at", "id", "document_id", "content_type", "parent_id", "properties") VALUES (1773940561981, 'Favorites', 1773940561980, 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'text', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', '{"ID":"88862721-ed4f-43ba-9222-f84f17c6692e","sequence":8}');

-- [transaction_stmt] 2026-03-19T17:16:01.984203Z
INSERT OR REPLACE INTO block ("id", "document_id", "content", "created_at", "updated_at", "content_type", "parent_id", "properties") VALUES ('block:a5d47f54-8632-412b-8844-7762121788b6', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'Recently Opened', 1773940561980, 1773940561981, 'text', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', '{"ID":"a5d47f54-8632-412b-8844-7762121788b6","sequence":9}');

-- [transaction_stmt] 2026-03-19T17:16:01.984354Z
INSERT OR REPLACE INTO block ("parent_id", "content", "updated_at", "created_at", "content_type", "document_id", "id", "properties") VALUES ('block:root-layout', 'Main Panel', 1773940561981, 1773940561980, 'text', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', '{"sequence":10,"ID":"03ad3820-2c9d-42d1-85f4-8b5695df22fa"}');

-- [transaction_stmt] 2026-03-19T17:16:01.984506Z
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "updated_at", "content_type", "document_id", "id", "source_language", "properties") VALUES (1773940561980, 'MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block)
WHERE fr.region = ''main'' AND root.id = fr.root_id AND d.content_type <> ''source''
RETURN d, d.properties.sequence AS sequence
ORDER BY d.properties.sequence
', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 1773940561981, 'source', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:main::src::0', 'holon_gql', '{"sequence":11,"ID":"main::src::0"}');

-- [transaction_stmt] 2026-03-19T17:16:01.984676Z
INSERT OR REPLACE INTO block ("source_language", "updated_at", "parent_id", "content", "content_type", "id", "created_at", "document_id", "properties") VALUES ('render', 1773940561981, 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})
', 'source', 'block:main::render::0', 1773940561980, 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', '{"sequence":12,"ID":"main::render::0"}');

-- [transaction_stmt] 2026-03-19T17:16:01.984840Z
INSERT OR REPLACE INTO block ("id", "updated_at", "created_at", "document_id", "content", "parent_id", "content_type", "properties") VALUES ('block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 1773940561981, 1773940561980, 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'Graph View', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'text', '{"ID":"aaca22e0-1b52-479b-891e-c55dcfc308f4","sequence":13}');

-- [transaction_stmt] 2026-03-19T17:16:01.984993Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "source_language", "content", "created_at", "id", "updated_at", "parent_id", "properties") VALUES ('doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'source', 'render', 'list(#{item_template: row(text(col("content")))})
', 1773940561980, 'block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1', 1773940561981, 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', '{"sequence":14,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1"}');

-- [transaction_stmt] 2026-03-19T17:16:01.985184Z
INSERT OR REPLACE INTO block ("id", "content_type", "content", "created_at", "source_language", "document_id", "parent_id", "updated_at", "properties") VALUES ('block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0', 'source', 'MATCH (b:block) WHERE b.content_type = ''text'' RETURN b
', 1773940561980, 'holon_gql', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 1773940561981, '{"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0","sequence":15}');

-- [transaction_stmt] 2026-03-19T17:16:01.985348Z
INSERT OR REPLACE INTO block ("content", "updated_at", "id", "created_at", "content_type", "parent_id", "document_id", "properties") VALUES ('Right Sidebar', 1773940561981, 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 1773940561980, 'text', 'block:root-layout', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', '{"collapse_to":"drawer","sequence":16,"ID":"cf7e0570-0e50-46ae-8b33-8c4b4f82e79c"}');

-- [transaction_stmt] 2026-03-19T17:16:01.985503Z
INSERT OR REPLACE INTO block ("source_language", "id", "parent_id", "created_at", "content", "document_id", "updated_at", "content_type", "properties") VALUES ('render', 'block:block:right_sidebar::render::0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 1773940561980, 'list(#{item_template: render_entity()})
', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 1773940561981, 'source', '{"ID":"block:right_sidebar::render::0","sequence":17}');

-- [transaction_stmt] 2026-03-19T17:16:01.985663Z
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "source_language", "document_id", "content_type", "updated_at", "content", "properties") VALUES ('block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'block:block:right_sidebar::src::0', 1773940561980, 'holon_prql', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'source', 1773940561981, 'from children
', '{"ID":"block:right_sidebar::src::0","sequence":18}');

-- [transaction_stmt] 2026-03-19T17:16:01.985823Z
INSERT OR REPLACE INTO block ("id", "updated_at", "content", "document_id", "content_type", "parent_id", "created_at", "properties") VALUES ('block:510a2669-402e-4d35-a161-4a2c259ed519', 1773940561981, 'Another pointer that gets shuffled around', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'text', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 1773940561981, '{"ID":"510a2669-402e-4d35-a161-4a2c259ed519","sequence":19}');

-- [transaction_stmt] 2026-03-19T17:16:01.985984Z
INSERT OR REPLACE INTO block ("created_at", "id", "parent_id", "content", "content_type", "updated_at", "document_id", "properties") VALUES (1773940561981, 'block:cffccf2a-7792-4b6d-a600-f8b31dc086b0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'Context Panel is reactive again!', 'text', 1773940561981, 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', '{"ID":"cffccf2a-7792-4b6d-a600-f8b31dc086b0","sequence":20}');

-- [transaction_stmt] 2026-03-19T17:16:01.986145Z
INSERT OR REPLACE INTO block ("id", "content", "updated_at", "parent_id", "content_type", "document_id", "created_at", "properties") VALUES ('block:4510fef8-f1c5-47b8-805b-8cd2c4905909', 'Quick Capture', 1773940561981, 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'text', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 1773940561981, '{"sequence":21,"ID":"4510fef8-f1c5-47b8-805b-8cd2c4905909"}');

-- [transaction_stmt] 2026-03-19T17:16:01.986304Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "content", "created_at", "id", "parent_id", "document_id", "properties") VALUES ('text', 1773940561981, 'Block Profiles', 1773940561981, 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', '{"sequence":22,"ID":"0c5c95a1-5202-427f-b714-86bec42fae89"}');

-- [transaction_stmt] 2026-03-19T17:16:01.986465Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "created_at", "id", "content", "content_type", "parent_id", "source_language", "properties") VALUES ('doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761', 1773940561981, 1773940561981, 'block:block:blocks-profile::src::0', 'entity_name: block
computed:
  is_task: ''= task_state != ()''
  is_source: ''= content_type == "source"''
  has_query_source: ''= query_source(id) != ()''
default:
  render: ''row(icon("orgmode"), spacer(8), editable_text(col("content")))''
variants:
  - name: query_block
    condition: ''= has_query_source''
    render: ''block_ref()''
  - name: task
    condition: ''= is_task''
    render: ''row(state_toggle(col("task_state")), spacer(8), editable_text(col("content")))''
  - name: source
    condition: ''= is_source''
    render: ''source_editor(#{language: col("source_language"), content: col("content")})''
', 'source', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 'holon_entity_profile_yaml', '{"sequence":23,"ID":"block:blocks-profile::src::0"}');

-- [actor_tx_commit] 2026-03-19T17:16:01.986681Z
COMMIT;

-- Wait 9ms
-- [actor_query] 2026-03-19T17:16:01.996255Z
SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%';

-- [actor_tx_begin] 2026-03-19T17:16:01.996539Z
BEGIN TRANSACTION (24 stmts);

-- [transaction_stmt] 2026-03-19T17:16:01.996560Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y8G0BQMP5RDR3CKG4', 'block.created', 'block', 'block:root-layout', 'sql', 'confirmed', '{"data":{"updated_at":1773940561981,"created_at":1773940561939,"content":"Holon Layout","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","parent_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content_type":"text","id":"block:root-layout","properties":{"ID":"root-layout","sequence":0}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.996922Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YW5RZMMWE353TMTRB', 'block.created', 'block', 'block:root-layout::src::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","id":"block:root-layout::src::0","parent_id":"block:root-layout","content":"MATCH (root:block)<-[:CHILD_OF]-(d:block)\\nWHERE root.id = ''block:root-layout'' AND d.content_type = ''text''\\nRETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority\\nORDER BY d.properties.sequence\\n","content_type":"source","created_at":1773940561939,"source_language":"holon_gql","updated_at":1773940561981,"properties":{"sequence":1,"ID":"root-layout::src::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.997241Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YJAKKTQBN118NZE8Z', 'block.created', 'block', 'block:holon-app-layout::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","id":"block:holon-app-layout::render::0","content":"columns(#{gap: 4, sort_key: col(\\"sequence\\"), item_template: block_ref()})\\n","created_at":1773940561939,"source_language":"render","parent_id":"block:root-layout","content_type":"source","updated_at":1773940561981,"properties":{"ID":"holon-app-layout::render::0","sequence":2}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.997551Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y5S0M62XPBP5A5SVJ', 'block.created', 'block', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'sql', 'confirmed', '{"data":{"updated_at":1773940561981,"created_at":1773940561979,"id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","parent_id":"block:root-layout","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content":"Left Sidebar","content_type":"text","properties":{"sequence":3,"ID":"e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","collapse_to":"drawer"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.997850Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y9H2F1H2SP0T57529', 'block.created', 'block', 'block:block:left_sidebar::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","id":"block:block:left_sidebar::render::0","content_type":"source","created_at":1773940561979,"updated_at":1773940561981,"source_language":"render","content":"list(#{sortkey: \\"name\\", item_template: selectable(row(icon(\\"notebook\\"), spacer(6), text(col(\\"name\\"))), #{action: navigation_focus(#{region: \\"main\\", block_id: col(\\"id\\")})})})\\n","parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","properties":{"sequence":4,"ID":"block:left_sidebar::render::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.998181Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y13XG8QS2SR1B3QC6', 'block.created', 'block', 'block:block:left_sidebar::src::0', 'sql', 'confirmed', '{"data":{"created_at":1773940561979,"parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","content":"from document\\nfilter name != \\"\\" && name != \\"index\\" && name != \\"__default__\\"\\n","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","id":"block:block:left_sidebar::src::0","source_language":"holon_prql","content_type":"source","updated_at":1773940561981,"properties":{"sequence":5,"ID":"block:left_sidebar::src::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.998479Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YJWAQER0P1QD6VTBE', 'block.created', 'block', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'sql', 'confirmed', '{"data":{"parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","content":"All Documents","id":"block:e8b05308-37ed-49a6-9c94-bccf9e3499bc","created_at":1773940561980,"updated_at":1773940561981,"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content_type":"text","properties":{"sequence":6,"ID":"e8b05308-37ed-49a6-9c94-bccf9e3499bc"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.999406Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YE2TY28HBW1FRPDHD', 'block.created', 'block', 'block:66c6aae4-4829-4d54-b92f-6638fda03368', 'sql', 'confirmed', '{"data":{"id":"block:66c6aae4-4829-4d54-b92f-6638fda03368","created_at":1773940561980,"content_type":"text","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","parent_id":"block:e8b05308-37ed-49a6-9c94-bccf9e3499bc","updated_at":1773940561981,"content":"Test","properties":{"ID":"66c6aae4-4829-4d54-b92f-6638fda03368","sequence":7}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:01.999704Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y43FF6SJ5953139FC', 'block.created', 'block', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'sql', 'confirmed', '{"data":{"updated_at":1773940561981,"content":"Favorites","created_at":1773940561980,"id":"block:88862721-ed4f-43ba-9222-f84f17c6692e","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content_type":"text","parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","properties":{"sequence":8,"ID":"88862721-ed4f-43ba-9222-f84f17c6692e"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.000002Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y7MC7NJR6E74BTDJV', 'block.created', 'block', 'block:a5d47f54-8632-412b-8844-7762121788b6', 'sql', 'confirmed', '{"data":{"id":"block:a5d47f54-8632-412b-8844-7762121788b6","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content":"Recently Opened","created_at":1773940561980,"updated_at":1773940561981,"content_type":"text","parent_id":"block:88862721-ed4f-43ba-9222-f84f17c6692e","properties":{"sequence":9,"ID":"a5d47f54-8632-412b-8844-7762121788b6"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.000300Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YEAR0CS6XYSXVPEGA', 'block.created', 'block', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'sql', 'confirmed', '{"data":{"parent_id":"block:root-layout","content":"Main Panel","updated_at":1773940561981,"created_at":1773940561980,"content_type":"text","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","properties":{"sequence":10,"ID":"03ad3820-2c9d-42d1-85f4-8b5695df22fa"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.000597Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YE1KMXXZ6QJXQSRT2', 'block.created', 'block', 'block:main::src::0', 'sql', 'confirmed', '{"data":{"created_at":1773940561980,"content":"MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block)\\nWHERE fr.region = ''main'' AND root.id = fr.root_id AND d.content_type <> ''source''\\nRETURN d, d.properties.sequence AS sequence\\nORDER BY d.properties.sequence\\n","parent_id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","updated_at":1773940561981,"content_type":"source","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","id":"block:main::src::0","source_language":"holon_gql","properties":{"sequence":11,"ID":"main::src::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.000912Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YJD2RJ4N3CQSGCDMH', 'block.created', 'block', 'block:main::render::0', 'sql', 'confirmed', '{"data":{"source_language":"render","updated_at":1773940561981,"parent_id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","content":"tree(#{parent_id: col(\\"parent_id\\"), sortkey: col(\\"sequence\\"), item_template: render_entity()})\\n","content_type":"source","id":"block:main::render::0","created_at":1773940561980,"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","properties":{"sequence":12,"ID":"main::render::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.001219Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YDNWDWWP5TNB24081', 'block.created', 'block', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'sql', 'confirmed', '{"data":{"id":"block:aaca22e0-1b52-479b-891e-c55dcfc308f4","updated_at":1773940561981,"created_at":1773940561980,"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content":"Graph View","parent_id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","content_type":"text","properties":{"ID":"aaca22e0-1b52-479b-891e-c55dcfc308f4","sequence":13}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.002108Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YNG1S6C7WD6ZEKYQA', 'block.created', 'block', 'block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1', 'sql', 'confirmed', '{"data":{"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content_type":"source","source_language":"render","content":"list(#{item_template: row(text(col(\\"content\\")))})\\n","created_at":1773940561980,"id":"block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1","updated_at":1773940561981,"parent_id":"block:aaca22e0-1b52-479b-891e-c55dcfc308f4","properties":{"sequence":14,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.002418Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y62926SKXEBCFGABB', 'block.created', 'block', 'block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0', 'sql', 'confirmed', '{"data":{"id":"block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0","content_type":"source","content":"MATCH (b:block) WHERE b.content_type = ''text'' RETURN b\\n","created_at":1773940561980,"source_language":"holon_gql","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","parent_id":"block:aaca22e0-1b52-479b-891e-c55dcfc308f4","updated_at":1773940561981,"properties":{"sequence":15,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.002723Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YXT59BC9WBDH41WM7', 'block.created', 'block', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'sql', 'confirmed', '{"data":{"content":"Right Sidebar","updated_at":1773940561981,"id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","created_at":1773940561980,"content_type":"text","parent_id":"block:root-layout","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","properties":{"ID":"cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","collapse_to":"drawer","sequence":16}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.003029Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YGVA0K9JHNHWWVGWM', 'block.created', 'block', 'block:block:right_sidebar::render::0', 'sql', 'confirmed', '{"data":{"source_language":"render","id":"block:block:right_sidebar::render::0","parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","created_at":1773940561980,"content":"list(#{item_template: render_entity()})\\n","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","updated_at":1773940561981,"content_type":"source","properties":{"ID":"block:right_sidebar::render::0","sequence":17}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.003333Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YSQ05GECAGD0877DK', 'block.created', 'block', 'block:block:right_sidebar::src::0', 'sql', 'confirmed', '{"data":{"parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","id":"block:block:right_sidebar::src::0","created_at":1773940561980,"source_language":"holon_prql","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content_type":"source","updated_at":1773940561981,"content":"from children\\n","properties":{"sequence":18,"ID":"block:right_sidebar::src::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.004262Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YX4H76D7CNS08DBYM', 'block.created', 'block', 'block:510a2669-402e-4d35-a161-4a2c259ed519', 'sql', 'confirmed', '{"data":{"id":"block:510a2669-402e-4d35-a161-4a2c259ed519","updated_at":1773940561981,"content":"Another pointer that gets shuffled around","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","content_type":"text","parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","created_at":1773940561981,"properties":{"sequence":19,"ID":"510a2669-402e-4d35-a161-4a2c259ed519"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.004568Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YSTP6PEMM2FXHXPRR', 'block.created', 'block', 'block:cffccf2a-7792-4b6d-a600-f8b31dc086b0', 'sql', 'confirmed', '{"data":{"created_at":1773940561981,"id":"block:cffccf2a-7792-4b6d-a600-f8b31dc086b0","parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","content":"Context Panel is reactive again!","content_type":"text","updated_at":1773940561981,"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","properties":{"ID":"cffccf2a-7792-4b6d-a600-f8b31dc086b0","sequence":20}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.004873Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1Y79E37Y2JC71Y62AP', 'block.created', 'block', 'block:4510fef8-f1c5-47b8-805b-8cd2c4905909', 'sql', 'confirmed', '{"data":{"id":"block:4510fef8-f1c5-47b8-805b-8cd2c4905909","content":"Quick Capture","updated_at":1773940561981,"parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","content_type":"text","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","created_at":1773940561981,"properties":{"sequence":21,"ID":"4510fef8-f1c5-47b8-805b-8cd2c4905909"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.005179Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YMRKF7RMMQ1CSFNZS', 'block.created', 'block', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940561981,"content":"Block Profiles","created_at":1773940561981,"id":"block:0c5c95a1-5202-427f-b714-86bec42fae89","parent_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","properties":{"sequence":22,"ID":"0c5c95a1-5202-427f-b714-86bec42fae89"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.006118Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP1YNP70Y9BH543NYBYK', 'block.created', 'block', 'block:block:blocks-profile::src::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761","updated_at":1773940561981,"created_at":1773940561981,"id":"block:block:blocks-profile::src::0","content":"entity_name: block\\n\\ncomputed:\\n  is_task: ''= task_state != ()''\\n  is_source: ''= content_type == \\"source\\"''\\n  has_query_source: ''= query_source(id) != ()''\\n\\ndefault:\\n  render: ''row(icon(\\"orgmode\\"), spacer(8), editable_text(col(\\"content\\")))''\\n\\nvariants:\\n  - name: query_block\\n    condition: ''= has_query_source''\\n    render: ''block_ref()''\\n  - name: task\\n    condition: ''= is_task''\\n    render: ''row(state_toggle(col(\\"task_state\\")), spacer(8), editable_text(col(\\"content\\")))''\\n  - name: source\\n    condition: ''= is_source''\\n    render: ''source_editor(#{language: col(\\"source_language\\"), content: col(\\"content\\")})''\\n","content_type":"source","parent_id":"block:0c5c95a1-5202-427f-b714-86bec42fae89","source_language":"holon_entity_profile_yaml","properties":{"sequence":23,"ID":"block:blocks-profile::src::0"}}}', NULL, NULL, 1773940561982, NULL, NULL);

-- [actor_tx_commit] 2026-03-19T17:16:02.006452Z
COMMIT;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T17:16:02.008074Z
DROP VIEW IF EXISTS watch_view_b271926fc3f569a8;

-- Wait 1ms
-- [actor_query] 2026-03-19T17:16:02.009498Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T17:16:02.009758Z
DROP VIEW IF EXISTS watch_view_e2453b3c0b29a253;

-- Wait 1ms
-- [actor_query] 2026-03-19T17:16:02.011162Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_e2453b3c0b29a253';

-- [actor_ddl] 2026-03-19T17:16:02.011399Z
DROP VIEW IF EXISTS watch_view_d77ac41ba85c1706;

-- [actor_query] 2026-03-19T17:16:02.012046Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_query] 2026-03-19T17:16:02.012298Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T17:16:02.012537Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_1570347602dda3f9';

-- [actor_ddl] 2026-03-19T17:16:02.012714Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_1570347602dda3f9 AS SELECT id, parent_id, content, content_type, source_language, block._change_origin AS _change_origin FROM block;

-- Wait 7ms
-- [actor_query] 2026-03-19T17:16:02.019892Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_dd27958f4ec0f8e7';

-- [actor_ddl] 2026-03-19T17:16:02.020089Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_dd27958f4ec0f8e7 AS SELECT id, content, block._change_origin AS _change_origin FROM block WHERE content_type = 'text';

-- Wait 3ms
-- [actor_tx_begin] 2026-03-19T17:16:02.024087Z
BEGIN TRANSACTION (14 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.024107Z
INSERT OR REPLACE INTO block ("content", "content_type", "updated_at", "created_at", "document_id", "parent_id", "id", "properties") VALUES ('Holon Layout', 'text', 1773940562020, 1773940562012, 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'block:default-layout-root', '{"sequence":0,"ID":"default-layout-root"}');

-- [transaction_stmt] 2026-03-19T17:16:02.024342Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "created_at", "parent_id", "source_language", "content_type", "updated_at", "properties") VALUES ('doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'block:default-layout-root::render::0', 'columns(#{gap: 4, item_template: block_ref()})
', 1773940562012, 'block:default-layout-root', 'render', 'source', 1773940562020, '{"ID":"default-layout-root::render::0","sequence":1}');

-- [transaction_stmt] 2026-03-19T17:16:02.024532Z
INSERT OR REPLACE INTO block ("source_language", "created_at", "id", "updated_at", "document_id", "content_type", "content", "parent_id", "properties") VALUES ('holon_prql', 1773940562012, 'block:default-layout-root::src::0', 1773940562020, 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'source', 'from children
filter content_type != "source"
derive {
  seq = s"json_extract(properties, ''$.\"column-order\"'')" ?? 999999,
  collapse_to = s"json_extract(properties, ''$.\"collapse-to\"'')",
  ideal_width = s"json_extract(properties, ''$.\"ideal-width\"'')",
  priority = s"json_extract(properties, ''$.\"column-priority\"'')"
}
sort seq
', 'block:default-layout-root', '{"sequence":2,"ID":"default-layout-root::src::0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.024749Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "id", "updated_at", "parent_id", "document_id", "content", "properties") VALUES ('text', 1773940562013, 'block:default-left-sidebar', 1773940562020, 'block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'Left Sidebar', '{"sequence":3,"ID":"default-left-sidebar"}');

-- [transaction_stmt] 2026-03-19T17:16:02.024915Z
INSERT OR REPLACE INTO block ("updated_at", "id", "content_type", "parent_id", "document_id", "source_language", "content", "created_at", "properties") VALUES (1773940562020, 'block:default-left-sidebar::render::0', 'source', 'block:default-left-sidebar', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'render', 'list(#{sortkey: "name", item_template: clickable(row(icon("folder"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})
', 1773940562013, '{"ID":"default-left-sidebar::render::0","sequence":4}');

-- [transaction_stmt] 2026-03-19T17:16:02.025097Z
INSERT OR REPLACE INTO block ("created_at", "source_language", "content_type", "updated_at", "document_id", "content", "parent_id", "id", "properties") VALUES (1773940562013, 'holon_prql', 'source', 1773940562020, 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'from document
filter name != ""
', 'block:default-left-sidebar', 'block:default-left-sidebar::src::0', '{"sequence":5,"ID":"default-left-sidebar::src::0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.025263Z
INSERT OR REPLACE INTO block ("id", "created_at", "content_type", "document_id", "parent_id", "updated_at", "content", "properties") VALUES ('block:default-main-panel', 1773940562013, 'text', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'block:default-layout-root', 1773940562020, 'Main Panel', '{"ID":"default-main-panel","sequence":6}');

-- [transaction_stmt] 2026-03-19T17:16:02.025423Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "id", "content", "created_at", "document_id", "source_language", "updated_at", "properties") VALUES ('source', 'block:default-main-panel', 'block:default-main-panel::src::0', 'MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d
', 1773940562013, 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'holon_gql', 1773940562020, '{"ID":"default-main-panel::src::0","sequence":7}');

-- [transaction_stmt] 2026-03-19T17:16:02.025599Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "id", "updated_at", "source_language", "parent_id", "created_at", "content", "properties") VALUES ('doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'source', 'block:default-main-panel::render::0', 1773940562020, 'render', 'block:default-main-panel', 1773940562013, 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})
', '{"ID":"default-main-panel::render::0","sequence":8}');

-- [transaction_stmt] 2026-03-19T17:16:02.025772Z
INSERT OR REPLACE INTO block ("document_id", "id", "created_at", "content", "content_type", "parent_id", "updated_at", "properties") VALUES ('doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'block:default-right-sidebar', 1773940562013, 'Right Sidebar', 'text', 'block:default-layout-root', 1773940562020, '{"sequence":9,"ID":"default-right-sidebar"}');

-- [transaction_stmt] 2026-03-19T17:16:02.025937Z
INSERT OR REPLACE INTO block ("document_id", "source_language", "parent_id", "created_at", "content_type", "content", "updated_at", "id", "properties") VALUES ('doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'render', 'block:default-right-sidebar', 1773940562013, 'source', 'list(#{item_template: render_entity()})
', 1773940562020, 'block:default-right-sidebar::render::0', '{"ID":"default-right-sidebar::render::0","sequence":10}');

-- [transaction_stmt] 2026-03-19T17:16:02.026108Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "id", "parent_id", "created_at", "source_language", "updated_at", "content", "properties") VALUES ('doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'source', 'block:default-right-sidebar::src::0', 'block:default-right-sidebar', 1773940562013, 'holon_prql', 1773940562020, 'from children
', '{"sequence":11,"ID":"default-right-sidebar::src::0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.026275Z
INSERT OR REPLACE INTO block ("content", "parent_id", "id", "document_id", "content_type", "created_at", "updated_at", "properties") VALUES ('Block Profiles', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'block:default-block-profiles', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'text', 1773940562013, 1773940562020, '{"sequence":12,"ID":"default-block-profiles"}');

-- [transaction_stmt] 2026-03-19T17:16:02.026446Z
INSERT OR REPLACE INTO block ("id", "document_id", "content", "updated_at", "source_language", "parent_id", "created_at", "content_type", "properties") VALUES ('block:default-block-profiles::src::0', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'entity_name: block
computed:
  is_task: ''= task_state != ()''
  is_source: ''= content_type == "source"''
  has_query_source: ''= query_source(id) != ()''
  todo_states: ''= if document_id != () { let d = document(document_id); if d != () { d.todo_keywords } else { () } } else { () }''
default:
  render: ''row(icon("orgmode"), spacer(8), editable_text(col("content")))''
variants:
  - name: query_block
    condition: ''= has_query_source''
    render: ''block_ref()''
  - name: task
    condition: ''= is_task''
    render: ''row(state_toggle(col("task_state"), #{states: col("todo_states")}), spacer(8), editable_text(col("content")))''
  - name: source
    condition: ''= is_source''
    render: ''source_editor(#{language: col("source_language"), content: col("content")})''
', 1773940562020, 'holon_entity_profile_yaml', 'block:default-block-profiles', 1773940562013, 'source', '{"sequence":13,"ID":"default-block-profiles::src::0"}');

-- [actor_tx_commit] 2026-03-19T17:16:02.026681Z
COMMIT;

-- Wait 6ms
-- [actor_query] 2026-03-19T17:16:02.032836Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_block';

-- [actor_query] 2026-03-19T17:16:02.033146Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_directory';

-- [actor_tx_begin] 2026-03-19T17:16:02.033313Z
BEGIN TRANSACTION (14 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.033335Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP342Z54GCG8RPPVE633', 'block.created', 'block', 'block:default-layout-root', 'sql', 'confirmed', '{"data":{"content":"Holon Layout","content_type":"text","updated_at":1773940562020,"created_at":1773940562012,"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","parent_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","id":"block:default-layout-root","properties":{"sequence":0,"ID":"default-layout-root"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.033735Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34WP60PGSYZGMV9BVQ', 'block.created', 'block', 'block:default-layout-root::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","id":"block:default-layout-root::render::0","content":"columns(#{gap: 4, item_template: block_ref()})\\n","created_at":1773940562012,"parent_id":"block:default-layout-root","source_language":"render","content_type":"source","updated_at":1773940562020,"properties":{"sequence":1,"ID":"default-layout-root::render::0"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.034664Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34M9SAHBRKENC4N57H', 'block.created', 'block', 'block:default-layout-root::src::0', 'sql', 'confirmed', '{"data":{"source_language":"holon_prql","created_at":1773940562012,"id":"block:default-layout-root::src::0","updated_at":1773940562020,"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content_type":"source","content":"from children\\nfilter content_type != \\"source\\"\\nderive {\\n  seq = s\\"json_extract(properties, ''$.\\\\\\"column-order\\\\\\"'')\\" ?? 999999,\\n  collapse_to = s\\"json_extract(properties, ''$.\\\\\\"collapse-to\\\\\\"'')\\",\\n  ideal_width = s\\"json_extract(properties, ''$.\\\\\\"ideal-width\\\\\\"'')\\",\\n  priority = s\\"json_extract(properties, ''$.\\\\\\"column-priority\\\\\\"'')\\"\\n}\\nsort seq\\n","parent_id":"block:default-layout-root","properties":{"sequence":2,"ID":"default-layout-root::src::0"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.034998Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP343K2BGRBRQK81EDCM', 'block.created', 'block', 'block:default-left-sidebar', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773940562013,"id":"block:default-left-sidebar","updated_at":1773940562020,"parent_id":"block:default-layout-root","document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content":"Left Sidebar","properties":{"sequence":3,"ID":"default-left-sidebar"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.035313Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34QNJ5A5HXEBP8GCJN', 'block.created', 'block', 'block:default-left-sidebar::render::0', 'sql', 'confirmed', '{"data":{"updated_at":1773940562020,"id":"block:default-left-sidebar::render::0","content_type":"source","parent_id":"block:default-left-sidebar","document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","source_language":"render","content":"list(#{sortkey: \\"name\\", item_template: clickable(row(icon(\\"folder\\"), spacer(6), text(col(\\"name\\"))), #{action: navigation_focus(#{region: \\"main\\", block_id: col(\\"id\\")})})})\\n","created_at":1773940562013,"properties":{"ID":"default-left-sidebar::render::0","sequence":4}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.036252Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34VW3RJPMJC6GW1NDZ', 'block.created', 'block', 'block:default-left-sidebar::src::0', 'sql', 'confirmed', '{"data":{"created_at":1773940562013,"source_language":"holon_prql","content_type":"source","updated_at":1773940562020,"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content":"from document\\nfilter name != \\"\\"\\n","parent_id":"block:default-left-sidebar","id":"block:default-left-sidebar::src::0","properties":{"ID":"default-left-sidebar::src::0","sequence":5}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.036569Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP341TZHC22C8T18SAFE', 'block.created', 'block', 'block:default-main-panel', 'sql', 'confirmed', '{"data":{"id":"block:default-main-panel","created_at":1773940562013,"content_type":"text","document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","parent_id":"block:default-layout-root","updated_at":1773940562020,"content":"Main Panel","properties":{"sequence":6,"ID":"default-main-panel"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.037485Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34Q88NHN42EQ7WJK3B', 'block.created', 'block', 'block:default-main-panel::src::0', 'sql', 'confirmed', '{"data":{"content_type":"source","parent_id":"block:default-main-panel","id":"block:default-main-panel::src::0","content":"MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d\\n","created_at":1773940562013,"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","source_language":"holon_gql","updated_at":1773940562020,"properties":{"sequence":7,"ID":"default-main-panel::src::0"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.037805Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34FJFK6909EDSZ6X3X', 'block.created', 'block', 'block:default-main-panel::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content_type":"source","id":"block:default-main-panel::render::0","updated_at":1773940562020,"source_language":"render","parent_id":"block:default-main-panel","created_at":1773940562013,"content":"tree(#{parent_id: col(\\"parent_id\\"), sortkey: col(\\"sequence\\"), item_template: render_entity()})\\n","properties":{"ID":"default-main-panel::render::0","sequence":8}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.038755Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34NANRRKB81G89255W', 'block.created', 'block', 'block:default-right-sidebar', 'sql', 'confirmed', '{"data":{"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","id":"block:default-right-sidebar","created_at":1773940562013,"content":"Right Sidebar","content_type":"text","parent_id":"block:default-layout-root","updated_at":1773940562020,"properties":{"sequence":9,"ID":"default-right-sidebar"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.039634Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34D06GE2FW22193C40', 'block.created', 'block', 'block:default-right-sidebar::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","source_language":"render","parent_id":"block:default-right-sidebar","created_at":1773940562013,"content_type":"source","content":"list(#{item_template: render_entity()})\\n","updated_at":1773940562020,"id":"block:default-right-sidebar::render::0","properties":{"ID":"default-right-sidebar::render::0","sequence":10}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.040545Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP340N4HYKKDDQ7GFR6B', 'block.created', 'block', 'block:default-right-sidebar::src::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content_type":"source","id":"block:default-right-sidebar::src::0","parent_id":"block:default-right-sidebar","created_at":1773940562013,"source_language":"holon_prql","updated_at":1773940562020,"content":"from children\\n","properties":{"ID":"default-right-sidebar::src::0","sequence":11}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.041527Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34KVY9H9E3PYG4GK23', 'block.created', 'block', 'block:default-block-profiles', 'sql', 'confirmed', '{"data":{"content":"Block Profiles","parent_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","id":"block:default-block-profiles","document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content_type":"text","created_at":1773940562013,"updated_at":1773940562020,"properties":{"ID":"default-block-profiles","sequence":12}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.041853Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP34351A356190R8XC3C', 'block.created', 'block', 'block:default-block-profiles::src::0', 'sql', 'confirmed', '{"data":{"id":"block:default-block-profiles::src::0","document_id":"doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b","content":"entity_name: block\\n\\ncomputed:\\n  is_task: ''= task_state != ()''\\n  is_source: ''= content_type == \\"source\\"''\\n  has_query_source: ''= query_source(id) != ()''\\n  todo_states: ''= if document_id != () { let d = document(document_id); if d != () { d.todo_keywords } else { () } } else { () }''\\n\\ndefault:\\n  render: ''row(icon(\\"orgmode\\"), spacer(8), editable_text(col(\\"content\\")))''\\n\\nvariants:\\n  - name: query_block\\n    condition: ''= has_query_source''\\n    render: ''block_ref()''\\n  - name: task\\n    condition: ''= is_task''\\n    render: ''row(state_toggle(col(\\"task_state\\"), #{states: col(\\"todo_states\\")}), spacer(8), editable_text(col(\\"content\\")))''\\n  - name: source\\n    condition: ''= is_source''\\n    render: ''source_editor(#{language: col(\\"source_language\\"), content: col(\\"content\\")})''\\n","updated_at":1773940562020,"source_language":"holon_entity_profile_yaml","parent_id":"block:default-block-profiles","created_at":1773940562013,"content_type":"source","properties":{"sequence":13,"ID":"default-block-profiles::src::0"}}}', NULL, NULL, 1773940562020, NULL, NULL);

-- [actor_tx_commit] 2026-03-19T17:16:02.042200Z
COMMIT;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T17:16:02.043391Z
CREATE MATERIALIZED VIEW events_view_directory AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'directory';

-- Wait 37ms
-- [actor_query] 2026-03-19T17:16:02.081297Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_file';

-- [actor_ddl] 2026-03-19T17:16:02.081515Z
CREATE MATERIALIZED VIEW events_view_file AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'file';

-- Wait 12ms
-- [actor_tx_begin] 2026-03-19T17:16:02.093708Z
BEGIN TRANSACTION (14 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.093733Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'Holon Layout', 'text', NULL, NULL, '{"ID":"default-layout-root","sequence":0}', 1773940562012, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.094452Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-layout-root::render::0', 'block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'columns(#{gap: 4, item_template: block_ref()})\n', 'source', 'render', NULL, '{"sequence":1,"ID":"default-layout-root::render::0"}', 1773940562012, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.094829Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-layout-root::src::0', 'block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'from children\nfilter content_type != "source"\nderive {\n  seq = s"json_extract(properties, ''$.\\"column-order\\"'')" ?? 999999,\n  collapse_to = s"json_extract(properties, ''$.\\"collapse-to\\"'')",\n  ideal_width = s"json_extract(properties, ''$.\\"ideal-width\\"'')",\n  priority = s"json_extract(properties, ''$.\\"column-priority\\"'')"\n}\nsort seq\n', 'source', 'holon_prql', NULL, '{"sequence":2,"ID":"default-layout-root::src::0"}', 1773940562012, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.095225Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-left-sidebar', 'block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'Left Sidebar', 'text', NULL, NULL, '{"ID":"default-left-sidebar","sequence":3}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.095708Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-left-sidebar::render::0', 'block:default-left-sidebar', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'list(#{sortkey: "name", item_template: clickable(row(icon("folder"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})\n', 'source', 'render', NULL, '{"sequence":4,"ID":"default-left-sidebar::render::0"}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.096067Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-left-sidebar::src::0', 'block:default-left-sidebar', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'from document\nfilter name != ""\n', 'source', 'holon_prql', NULL, '{"sequence":5,"ID":"default-left-sidebar::src::0"}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.096399Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-main-panel', 'block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'Main Panel', 'text', NULL, NULL, '{"ID":"default-main-panel","sequence":6}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.096725Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-main-panel::src::0', 'block:default-main-panel', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d\n', 'source', 'holon_gql', NULL, '{"sequence":7,"ID":"default-main-panel::src::0"}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.097067Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-main-panel::render::0', 'block:default-main-panel', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})\n', 'source', 'render', NULL, '{"ID":"default-main-panel::render::0","sequence":8}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.097403Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-right-sidebar', 'block:default-layout-root', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'Right Sidebar', 'text', NULL, NULL, '{"sequence":9,"ID":"default-right-sidebar"}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.097726Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-right-sidebar::render::0', 'block:default-right-sidebar', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'list(#{item_template: render_entity()})\n', 'source', 'render', NULL, '{"ID":"default-right-sidebar::render::0","sequence":10}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.098048Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-right-sidebar::src::0', 'block:default-right-sidebar', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'from children\n', 'source', 'holon_prql', NULL, '{"ID":"default-right-sidebar::src::0","sequence":11}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.098372Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-block-profiles', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'Block Profiles', 'text', NULL, NULL, '{"ID":"default-block-profiles","sequence":12}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.098718Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-block-profiles::src::0', 'block:default-block-profiles', 'doc:1b2f1c05-d6b7-431d-908e-849938a2ce2b', 'entity_name: block\n\ncomputed:\n  is_task: ''= task_state != ()''\n  is_source: ''= content_type == "source"''\n  has_query_source: ''= query_source(id) != ()''\n  todo_states: ''= if document_id != () { let d = document(document_id); if d != () { d.todo_keywords } else { () } } else { () }''\n\ndefault:\n  render: ''row(icon("orgmode"), spacer(8), editable_text(col("content")))''\n\nvariants:\n  - name: query_block\n    condition: ''= has_query_source''\n    render: ''block_ref()''\n  - name: task\n    condition: ''= is_task''\n    render: ''row(state_toggle(col("task_state"), #{states: col("todo_states")}), spacer(8), editable_text(col("content")))''\n  - name: source\n    condition: ''= is_source''\n    render: ''source_editor(#{language: col("source_language"), content: col("content")})''\n', 'source', 'holon_entity_profile_yaml', NULL, '{"ID":"default-block-profiles::src::0","sequence":13}', 1773940562013, 1773940562020, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [actor_tx_commit] 2026-03-19T17:16:02.099140Z
COMMIT;

-- Wait 17ms
-- [actor_query] 2026-03-19T17:16:02.116341Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_query] 2026-03-19T17:16:02.116684Z
SELECT id FROM block WHERE id = 'block:root-layout';

-- [actor_exec] 2026-03-19T17:16:02.116893Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 1ms
-- [actor_query] 2026-03-19T17:16:02.118124Z
SELECT document_id FROM block WHERE id = 'block:root-layout' AND document_id != 'doc:__default__';

-- [actor_exec] 2026-03-19T17:16:02.118293Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.119020Z
DELETE FROM block WHERE document_id = 'doc:__default__';

-- Wait 1ms
-- [actor_exec] 2026-03-19T17:16:02.120092Z
DELETE FROM document WHERE id = 'doc:__default__';

-- [actor_exec] 2026-03-19T17:16:02.120199Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.120957Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_tx_begin] 2026-03-19T17:16:02.121612Z
BEGIN TRANSACTION (10 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.121633Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "document_id", "content_type", "created_at", "content", "properties") VALUES (1773940562121, 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'block:cc-history-root', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'text', 1773940562117, 'Claude Code History', '{"ID":"cc-history-root","sequence":0}');

-- [transaction_stmt] 2026-03-19T17:16:02.121846Z
INSERT OR REPLACE INTO block ("content_type", "id", "document_id", "updated_at", "content", "parent_id", "created_at", "properties") VALUES ('text', 'block:cc-projects', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 1773940562121, 'Projects', 'block:cc-history-root', 1773940562117, '{"ID":"cc-projects","sequence":1}');

-- [transaction_stmt] 2026-03-19T17:16:02.122021Z
INSERT OR REPLACE INTO block ("parent_id", "source_language", "id", "content", "content_type", "document_id", "created_at", "updated_at", "properties") VALUES ('block:cc-projects', 'holon_prql', 'block:block:cc-projects::src::0', 'from cc_project
select {id, original_path, session_count, last_activity}
sort {-last_activity}
', 'source', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 1773940562117, 1773940562121, '{"ID":"block:cc-projects::src::0","sequence":2}');

-- [transaction_stmt] 2026-03-19T17:16:02.122207Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "created_at", "document_id", "source_language", "content", "id", "content_type", "properties") VALUES ('block:cc-projects', 1773940562121, 1773940562117, 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'render', 'list(#{item_template: row(text(col("original_path")), spacer(16), text(col("session_count")), spacer(8), text(col("last_activity")))})
', 'block:block:cc-projects::render::0', 'source', '{"sequence":3,"ID":"block:cc-projects::render::0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.122392Z
INSERT OR REPLACE INTO block ("id", "parent_id", "content_type", "content", "document_id", "updated_at", "created_at", "properties") VALUES ('block:cc-sessions', 'block:cc-history-root', 'text', 'Recent Sessions', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 1773940562121, 1773940562118, '{"ID":"cc-sessions","sequence":4}');

-- [transaction_stmt] 2026-03-19T17:16:02.122562Z
INSERT OR REPLACE INTO block ("id", "updated_at", "parent_id", "document_id", "created_at", "content", "content_type", "source_language", "properties") VALUES ('block:block:cc-sessions::src::0', 1773940562121, 'block:cc-sessions', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 1773940562118, 'from cc_session
filter message_count > 0
select {id, first_prompt, message_count, model, modified, git_branch}
sort {-modified}
take 30
', 'source', 'holon_prql', '{"ID":"block:cc-sessions::src::0","sequence":5}');

-- [transaction_stmt] 2026-03-19T17:16:02.122744Z
INSERT OR REPLACE INTO block ("created_at", "source_language", "updated_at", "parent_id", "content_type", "document_id", "content", "id", "properties") VALUES (1773940562118, 'render', 1773940562121, 'block:cc-sessions', 'source', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'list(#{item_template: row(text(col("first_prompt")), spacer(16), text(col("message_count")), spacer(8), text(col("modified")))})
', 'block:block:cc-sessions::render::0', '{"sequence":6,"ID":"block:cc-sessions::render::0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.122927Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "content_type", "updated_at", "content", "document_id", "id", "properties") VALUES ('block:cc-history-root', 1773940562118, 'text', 1773940562121, 'Tasks', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'block:cc-tasks', '{"sequence":7,"ID":"cc-tasks"}');

-- [transaction_stmt] 2026-03-19T17:16:02.123103Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "parent_id", "content_type", "created_at", "id", "source_language", "content", "properties") VALUES (1773940562121, 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'block:cc-tasks', 'source', 1773940562118, 'block:block:cc-tasks::src::0', 'holon_prql', 'from cc_task
filter status == "in_progress"
select {id, subject, status, created_at}
sort {-created_at}
', '{"ID":"block:cc-tasks::src::0","sequence":8}');

-- [transaction_stmt] 2026-03-19T17:16:02.123298Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "content", "document_id", "source_language", "id", "created_at", "parent_id", "properties") VALUES (1773940562121, 'source', 'list(#{item_template: row(text(col("status")), spacer(8), text(col("subject")))})
', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'render', 'block:block:cc-tasks::render::0', 1773940562118, 'block:cc-tasks', '{"ID":"block:cc-tasks::render::0","sequence":9}');

-- [actor_tx_commit] 2026-03-19T17:16:02.123473Z
COMMIT;

-- Wait 5ms
-- [actor_exec] 2026-03-19T17:16:02.128543Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_tx_begin] 2026-03-19T17:16:02.129397Z
BEGIN TRANSACTION (10 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.129419Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69SYWG3WC5PZD952ME', 'block.created', 'block', 'block:cc-history-root', 'sql', 'confirmed', '{"data":{"updated_at":1773940562121,"parent_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","id":"block:cc-history-root","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","content_type":"text","created_at":1773940562117,"content":"Claude Code History","properties":{"ID":"cc-history-root","sequence":0}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.129804Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69CKECX4VTG9JGVXBQ', 'block.created', 'block', 'block:cc-projects', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:cc-projects","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","updated_at":1773940562121,"content":"Projects","parent_id":"block:cc-history-root","created_at":1773940562117,"properties":{"ID":"cc-projects","sequence":1}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.130169Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69B66SP35JJFSV41DT', 'block.created', 'block', 'block:block:cc-projects::src::0', 'sql', 'confirmed', '{"data":{"parent_id":"block:cc-projects","source_language":"holon_prql","id":"block:block:cc-projects::src::0","content":"from cc_project\\nselect {id, original_path, session_count, last_activity}\\nsort {-last_activity}\\n","content_type":"source","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","created_at":1773940562117,"updated_at":1773940562121,"properties":{"ID":"block:cc-projects::src::0","sequence":2}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.130521Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69007MGND3HQKCNJZS', 'block.created', 'block', 'block:block:cc-projects::render::0', 'sql', 'confirmed', '{"data":{"parent_id":"block:cc-projects","updated_at":1773940562121,"created_at":1773940562117,"document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","source_language":"render","content":"list(#{item_template: row(text(col(\\"original_path\\")), spacer(16), text(col(\\"session_count\\")), spacer(8), text(col(\\"last_activity\\")))})\\n","id":"block:block:cc-projects::render::0","content_type":"source","properties":{"ID":"block:cc-projects::render::0","sequence":3}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.131435Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP698JE2JQFSNATRWTYS', 'block.created', 'block', 'block:cc-sessions', 'sql', 'confirmed', '{"data":{"id":"block:cc-sessions","parent_id":"block:cc-history-root","content_type":"text","content":"Recent Sessions","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","updated_at":1773940562121,"created_at":1773940562118,"properties":{"ID":"cc-sessions","sequence":4}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.131764Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP698XH4XE334D3GAFJ9', 'block.created', 'block', 'block:block:cc-sessions::src::0', 'sql', 'confirmed', '{"data":{"id":"block:block:cc-sessions::src::0","updated_at":1773940562121,"parent_id":"block:cc-sessions","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","created_at":1773940562118,"content":"from cc_session\\nfilter message_count > 0\\nselect {id, first_prompt, message_count, model, modified, git_branch}\\nsort {-modified}\\ntake 30\\n","content_type":"source","source_language":"holon_prql","properties":{"ID":"block:cc-sessions::src::0","sequence":5}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.132094Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69YA5K2RRT6F6RDKXE', 'block.created', 'block', 'block:block:cc-sessions::render::0', 'sql', 'confirmed', '{"data":{"created_at":1773940562118,"source_language":"render","updated_at":1773940562121,"parent_id":"block:cc-sessions","content_type":"source","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","content":"list(#{item_template: row(text(col(\\"first_prompt\\")), spacer(16), text(col(\\"message_count\\")), spacer(8), text(col(\\"modified\\")))})\\n","id":"block:block:cc-sessions::render::0","properties":{"sequence":6,"ID":"block:cc-sessions::render::0"}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.132425Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69PSYM6B9J9Q4M2TAV', 'block.created', 'block', 'block:cc-tasks', 'sql', 'confirmed', '{"data":{"parent_id":"block:cc-history-root","created_at":1773940562118,"content_type":"text","updated_at":1773940562121,"content":"Tasks","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","id":"block:cc-tasks","properties":{"sequence":7,"ID":"cc-tasks"}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.132739Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP69BKQ7X040GZSVXN87', 'block.created', 'block', 'block:block:cc-tasks::src::0', 'sql', 'confirmed', '{"data":{"updated_at":1773940562121,"document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","parent_id":"block:cc-tasks","content_type":"source","created_at":1773940562118,"id":"block:block:cc-tasks::src::0","source_language":"holon_prql","content":"from cc_task\\nfilter status == \\"in_progress\\"\\nselect {id, subject, status, created_at}\\nsort {-created_at}\\n","properties":{"sequence":8,"ID":"block:cc-tasks::src::0"}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.134156Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP692KD10TZPFVDBZ4J1', 'block.created', 'block', 'block:block:cc-tasks::render::0', 'sql', 'confirmed', '{"data":{"updated_at":1773940562121,"content_type":"source","content":"list(#{item_template: row(text(col(\\"status\\")), spacer(8), text(col(\\"subject\\")))})\\n","document_id":"doc:f753ea35-2fb1-4a73-90b5-04d65940a091","source_language":"render","id":"block:block:cc-tasks::render::0","created_at":1773940562118,"parent_id":"block:cc-tasks","properties":{"sequence":9,"ID":"block:cc-tasks::render::0"}}}', NULL, NULL, 1773940562121, NULL, NULL);

-- [actor_tx_commit] 2026-03-19T17:16:02.134522Z
COMMIT;

-- Wait 1ms
-- [actor_exec] 2026-03-19T17:16:02.136086Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 1ms
-- [actor_exec] 2026-03-19T17:16:02.137960Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 1ms
-- [actor_exec] 2026-03-19T17:16:02.139866Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:02.140583Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_exec] 2026-03-19T17:16:02.140839Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:02.141508Z
SELECT * FROM document WHERE id = $id LIMIT 1;

-- [actor_exec] 2026-03-19T17:16:02.141658Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:02.142317Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_exec] 2026-03-19T17:16:02.142563Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.143292Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.143959Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.144604Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 34ms
-- [actor_tx_begin] 2026-03-19T17:16:02.178834Z
BEGIN TRANSACTION (259 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.178860Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "created_at", "content", "content_type", "document_id", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 1773940562147, 'Phase 1: Core Outliner', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', '{"sequence":0,"ID":"599b60af-960d-4c9c-b222-d3d9de95c513"}');

-- [transaction_stmt] 2026-03-19T17:16:02.179150Z
INSERT OR REPLACE INTO block ("id", "updated_at", "document_id", "created_at", "parent_id", "content", "content_type", "properties") VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562147, 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'MCP Server Frontend [/]', 'text', '{"ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb","sequence":1,"task_state":"DOING"}');

-- [transaction_stmt] 2026-03-19T17:16:02.179354Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content", "content_type", "updated_at", "id", "created_at", "properties") VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Context parameter support ($context_id, $context_parent_id)', 'text', 1773940562172, 'block:db59d038-8a47-43e9-9502-0472b493a6b9', 1773940562147, '{"sequence":2,"ID":"db59d038-8a47-43e9-9502-0472b493a6b9"}');

-- [transaction_stmt] 2026-03-19T17:16:02.179541Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content", "created_at", "content_type", "id", "updated_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'MCP server (stdio + HTTP modes)', 1773940562147, 'text', 'block:95ad6166-c03c-4417-a435-349e88b8e90a', 1773940562172, '{"sequence":3,"ID":"95ad6166-c03c-4417-a435-349e88b8e90a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.179727Z
INSERT OR REPLACE INTO block ("id", "created_at", "content", "parent_id", "updated_at", "document_id", "content_type", "properties") VALUES ('block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 1773940562147, 'MCP tools for query execution and operations', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"sequence":4,"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b"}');

-- [transaction_stmt] 2026-03-19T17:16:02.179913Z
INSERT OR REPLACE INTO block ("content_type", "content", "id", "created_at", "document_id", "updated_at", "parent_id", "properties") VALUES ('text', 'Block Operations [/]', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 1773940562147, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 'block:599b60af-960d-4c9c-b222-d3d9de95c513', '{"ID":"661368d9-e4bd-4722-b5c2-40f32006c643","sequence":5}');

-- [transaction_stmt] 2026-03-19T17:16:02.180082Z
INSERT OR REPLACE INTO block ("content_type", "id", "content", "created_at", "updated_at", "document_id", "parent_id", "properties") VALUES ('text', 'block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'Block hierarchy (parent/child, indent/outdent)', 1773940562147, 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', '{"sequence":6,"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007"}');

-- [transaction_stmt] 2026-03-19T17:16:02.180264Z
INSERT OR REPLACE INTO block ("id", "document_id", "created_at", "content_type", "parent_id", "content", "updated_at", "properties") VALUES ('block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562147, 'text', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'Split block operation', 1773940562172, '{"sequence":7,"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34"}');

-- [transaction_stmt] 2026-03-19T17:16:02.180440Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "content", "updated_at", "content_type", "id", "document_id", "properties") VALUES (1773940562147, 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'Block CRUD (create, read, update, delete)', 1773940562172, 'text', 'block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":8,"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb"}');

-- [transaction_stmt] 2026-03-19T17:16:02.180611Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "content_type", "document_id", "updated_at", "content", "properties") VALUES ('block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 1773940562148, 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 'Block movement (move_up, move_down, move_block)', '{"sequence":9,"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63"}');

-- [transaction_stmt] 2026-03-19T17:16:02.180800Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content", "id", "updated_at", "content_type", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'Undo/redo system (UndoStack + persistent OperationLogStore)', 'block:225edb45-f670-445a-9162-18c150210ee6', 1773940562172, 'text', 1773940562148, '{"ID":"225edb45-f670-445a-9162-18c150210ee6","task_state":"DONE","sequence":10}');

-- [transaction_stmt] 2026-03-19T17:16:02.180985Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "updated_at", "created_at", "parent_id", "id", "content", "properties") VALUES ('text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 1773940562148, 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'Storage & Data Layer [/]', '{"sequence":11,"ID":"444b24f6-d412-43c4-a14b-6e725b673cee"}');

-- [transaction_stmt] 2026-03-19T17:16:02.181165Z
INSERT OR REPLACE INTO block ("id", "parent_id", "document_id", "content", "content_type", "updated_at", "created_at", "properties") VALUES ('block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Schema Module system with topological dependency ordering', 'text', 1773940562172, 1773940562148, '{"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae","sequence":12}');

-- [transaction_stmt] 2026-03-19T17:16:02.181347Z
INSERT OR REPLACE INTO block ("created_at", "content", "document_id", "id", "updated_at", "content_type", "parent_id", "properties") VALUES (1773940562148, 'Fractional indexing for block ordering', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 1773940562172, 'text', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699","sequence":13}');

-- [transaction_stmt] 2026-03-19T17:16:02.181554Z
INSERT OR REPLACE INTO block ("id", "content_type", "document_id", "created_at", "updated_at", "parent_id", "content", "properties") VALUES ('block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562148, 1773940562172, 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'Turso (embedded SQLite) backend with connection pooling', '{"sequence":14,"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57"}');

-- [transaction_stmt] 2026-03-19T17:16:02.181732Z
INSERT OR REPLACE INTO block ("content_type", "id", "document_id", "created_at", "updated_at", "content", "parent_id", "properties") VALUES ('text', 'block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562148, 1773940562172, 'QueryableCache wrapping DataSource with local caching', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280","sequence":15}');

-- [transaction_stmt] 2026-03-19T17:16:02.181915Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "id", "content_type", "parent_id", "document_id", "content", "properties") VALUES (1773940562148, 1773940562172, 'block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'text', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Entity derive macro (#[derive(Entity)]) for schema generation', '{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}');

-- [transaction_stmt] 2026-03-19T17:16:02.182094Z
INSERT OR REPLACE INTO block ("updated_at", "id", "content", "parent_id", "document_id", "content_type", "created_at", "properties") VALUES (1773940562172, 'block:d318cae4-759d-487b-a909-81940223ecc1', 'CDC (Change Data Capture) streaming from storage to UI', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562148, '{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}');

-- [transaction_stmt] 2026-03-19T17:16:02.182279Z
INSERT OR REPLACE INTO block ("content", "id", "updated_at", "content_type", "created_at", "document_id", "parent_id", "properties") VALUES ('Command sourcing infrastructure (append-only operation log)', 'block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 1773940562172, 'text', 1773940562148, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222","task_state":"DONE","sequence":18}');

-- [transaction_stmt] 2026-03-19T17:16:02.182469Z
INSERT OR REPLACE INTO block ("id", "parent_id", "created_at", "updated_at", "content_type", "content", "document_id", "properties") VALUES ('block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773940562148, 1773940562172, 'text', 'Procedural Macros [/]', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","sequence":19}');

-- [transaction_stmt] 2026-03-19T17:16:02.182649Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "updated_at", "id", "content_type", "document_id", "content", "properties") VALUES (1773940562149, 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 1773940562172, 'block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '#[operations_trait] macro for operation dispatch generation', '{"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4","sequence":20}');

-- [transaction_stmt] 2026-03-19T17:16:02.182836Z
INSERT OR REPLACE INTO block ("content", "created_at", "content_type", "id", "document_id", "updated_at", "parent_id", "properties") VALUES ('#[triggered_by(...)] for operation availability', 1773940562149, 'text', 'block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', '{"sequence":21,"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44"}');

-- [transaction_stmt] 2026-03-19T17:16:02.183019Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "document_id", "content_type", "id", "parent_id", "content", "properties") VALUES (1773940562149, 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'Type inference for OperationDescriptor parameters', '{"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd","sequence":22}');

-- [transaction_stmt] 2026-03-19T17:16:02.183201Z
INSERT OR REPLACE INTO block ("content", "parent_id", "created_at", "content_type", "id", "updated_at", "document_id", "properties") VALUES ('#[affects(...)] for field-level reactivity', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 1773940562149, 'text', 'block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2","sequence":23}');

-- [transaction_stmt] 2026-03-19T17:16:02.183385Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "document_id", "id", "updated_at", "content", "parent_id", "properties") VALUES ('text', 1773940562149, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 1773940562172, 'Performance [/]', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', '{"sequence":24,"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.183562Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "id", "content", "content_type", "parent_id", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 'block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'Virtual scrolling and lazy loading', 'text', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 1773940562149, '{"ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73","sequence":25,"task_state":"DOING"}');

-- [transaction_stmt] 2026-03-19T17:16:02.183749Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "parent_id", "document_id", "content", "updated_at", "id", "properties") VALUES (1773940562149, 'text', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Connection pooling for Turso', 1773940562172, 'block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', '{"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","task_state":"DOING","sequence":26}');

-- [transaction_stmt] 2026-03-19T17:16:02.183936Z
INSERT OR REPLACE INTO block ("content", "id", "content_type", "created_at", "parent_id", "document_id", "updated_at", "properties") VALUES ('Full-text search indexing (Tantivy)', 'block:e0567a06-5a62-4957-9457-c55a6661cee5', 'text', 1773940562149, 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, '{"ID":"e0567a06-5a62-4957-9457-c55a6661cee5","sequence":27}');

-- [transaction_stmt] 2026-03-19T17:16:02.184140Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "content", "document_id", "id", "content_type", "created_at", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773940562172, 'Cross-Device Sync [/]', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'text', 1773940562149, '{"sequence":28,"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34"}');

-- [transaction_stmt] 2026-03-19T17:16:02.184340Z
INSERT OR REPLACE INTO block ("content", "parent_id", "content_type", "created_at", "updated_at", "document_id", "id", "properties") VALUES ('CollaborativeDoc with ALPN routing', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'text', 1773940562149, 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:43f329da-cfb4-4764-b599-06f4b6272f91', '{"sequence":29,"ID":"43f329da-cfb4-4764-b599-06f4b6272f91"}');

-- [transaction_stmt] 2026-03-19T17:16:02.184539Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "content_type", "created_at", "id", "content", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 'text', 1773940562149, 'block:7aef40b2-14e1-4df0-a825-18603c55d198', 'Offline-first with background sync', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', '{"ID":"7aef40b2-14e1-4df0-a825-18603c55d198","sequence":30}');

-- [transaction_stmt] 2026-03-19T17:16:02.184874Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "created_at", "document_id", "id", "content", "content_type", "properties") VALUES (1773940562172, 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 1773940562150, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:e148d7b7-c505-4201-83b7-36986a981a56', 'Iroh P2P transport for Loro documents', 'text', '{"ID":"e148d7b7-c505-4201-83b7-36986a981a56","sequence":31}');

-- [transaction_stmt] 2026-03-19T17:16:02.185100Z
INSERT OR REPLACE INTO block ("created_at", "id", "content", "updated_at", "document_id", "content_type", "parent_id", "properties") VALUES (1773940562150, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'Dependency Injection [/]', 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', '{"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9","sequence":32}');

-- [transaction_stmt] 2026-03-19T17:16:02.185296Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "created_at", "id", "content_type", "parent_id", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562172, 1773940562150, 'block:b980e51f-0c91-4708-9a17-3d41284974b2', 'text', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'OperationDispatcher routing to providers', '{"sequence":33,"ID":"b980e51f-0c91-4708-9a17-3d41284974b2"}');

-- [transaction_stmt] 2026-03-19T17:16:02.185477Z
INSERT OR REPLACE INTO block ("id", "updated_at", "document_id", "parent_id", "content_type", "content", "created_at", "properties") VALUES ('block:97cc8506-47d2-44cb-bdca-8e9a507953a0', 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'text', 'BackendEngine as main orchestration point', 1773940562150, '{"sequence":34,"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.185656Z
INSERT OR REPLACE INTO block ("content", "updated_at", "document_id", "id", "parent_id", "content_type", "created_at", "properties") VALUES ('ferrous-di based service composition', 1773940562172, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'text', 1773940562150, '{"sequence":35,"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8"}');

-- [transaction_stmt] 2026-03-19T17:16:02.185834Z
INSERT OR REPLACE INTO block ("updated_at", "id", "document_id", "content", "created_at", "parent_id", "content_type", "properties") VALUES (1773940562172, 'block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'SchemaRegistry with topological initialization', 1773940562150, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'text', '{"sequence":36,"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47"}');

-- [transaction_stmt] 2026-03-19T17:16:02.186011Z
INSERT OR REPLACE INTO block ("content", "updated_at", "parent_id", "document_id", "id", "content_type", "created_at", "properties") VALUES ('Query & Render Pipeline [/]', 1773940562172, 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'text', 1773940562150, '{"sequence":37,"ID":"b489c622-6c87-4bf6-8d35-787eb732d670"}');

-- [transaction_stmt] 2026-03-19T17:16:02.186186Z
INSERT OR REPLACE INTO block ("id", "document_id", "content", "content_type", "updated_at", "parent_id", "created_at", "properties") VALUES ('block:1bbec456-7217-4477-a49c-0b8422e441e9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)', 'text', 1773940562173, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 1773940562150, '{"ID":"1bbec456-7217-4477-a49c-0b8422e441e9","sequence":38}');

-- [transaction_stmt] 2026-03-19T17:16:02.186362Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "id", "content_type", "created_at", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:2b1c341e-5da2-4207-a609-f4af6d7ceebd', 'text', 1773940562150, 'Automatic operation wiring (lineage analysis → widget binding)', '{"task_state":"DOING","ID":"2b1c341e-5da2-4207-a609-f4af6d7ceebd","sequence":39}');

-- [transaction_stmt] 2026-03-19T17:16:02.186727Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "content", "updated_at", "content_type", "document_id", "properties") VALUES ('block:2d44d7df-5d7d-4cfe-9061-459c7578e334', 1773940562150, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'GQL (graph query) support via EAV schema', 1773940562173, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"2d44d7df-5d7d-4cfe-9061-459c7578e334","task_state":"DOING","sequence":40}');

-- [transaction_stmt] 2026-03-19T17:16:02.186899Z
INSERT OR REPLACE INTO block ("id", "content", "parent_id", "document_id", "updated_at", "content_type", "created_at", "properties") VALUES ('block:54ed1be5-765e-4884-87ab-02268e0208c7', 'PRQL compilation (PRQL → SQL + RenderSpec)', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'text', 1773940562150, '{"ID":"54ed1be5-765e-4884-87ab-02268e0208c7","sequence":41}');

-- [transaction_stmt] 2026-03-19T17:16:02.187070Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "content_type", "parent_id", "document_id", "id", "content", "properties") VALUES (1773940562150, 1773940562173, 'text', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:5384c1da-f058-4321-8401-929b3570c2a5', 'RenderSpec tree for declarative UI description', '{"sequence":42,"ID":"5384c1da-f058-4321-8401-929b3570c2a5"}');

-- [transaction_stmt] 2026-03-19T17:16:02.187251Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "id", "updated_at", "document_id", "content_type", "content", "properties") VALUES (1773940562151, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'Unified execute_query supporting PRQL/GQL/SQL', '{"ID":"fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","sequence":43,"task_state":"DONE"}');

-- [transaction_stmt] 2026-03-19T17:16:02.187603Z
INSERT OR REPLACE INTO block ("id", "content", "document_id", "created_at", "content_type", "updated_at", "parent_id", "properties") VALUES ('block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 'SQL direct execution support', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562151, 'text', 1773940562173, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', '{"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","sequence":44,"task_state":"DOING"}');

-- [transaction_stmt] 2026-03-19T17:16:02.187768Z
INSERT OR REPLACE INTO block ("created_at", "id", "parent_id", "document_id", "updated_at", "content", "content_type", "properties") VALUES (1773940562151, 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'Loro CRDT Integration [/]', 'text', '{"sequence":45,"ID":"d9374dc3-05fc-40b2-896d-f88bb8a33c92"}');

-- [transaction_stmt] 2026-03-19T17:16:02.187932Z
INSERT OR REPLACE INTO block ("content", "updated_at", "created_at", "document_id", "content_type", "id", "parent_id", "properties") VALUES ('LoroBackend implementing CoreOperations trait', 1773940562173, 1773940562151, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', '{"sequence":46,"ID":"b1dc3ad3-574b-472a-b74b-e3ea29a433e6"}');

-- [transaction_stmt] 2026-03-19T17:16:02.188292Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "updated_at", "parent_id", "content", "document_id", "id", "properties") VALUES ('text', 1773940562151, 1773940562173, 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'LoroDocumentStore for managing CRDT documents on disk', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957', '{"ID":"ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","task_state":"DOING","sequence":47}');

-- [transaction_stmt] 2026-03-19T17:16:02.188460Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "content", "document_id", "id", "parent_id", "content_type", "properties") VALUES (1773940562151, 1773940562173, 'LoroBlockOperations as OperationProvider routing writes through CRDT', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:35652c3f-720c-4e20-ab90-5e25e1429733', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'text', '{"sequence":48,"ID":"35652c3f-720c-4e20-ab90-5e25e1429733"}');

-- [transaction_stmt] 2026-03-19T17:16:02.188621Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content_type", "id", "content", "updated_at", "parent_id", "properties") VALUES (1773940562151, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2', 'Cycle detection in move_block', 1773940562173, 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', '{"ID":"090731e3-38ae-4bf1-b5ec-dbb33eae4fb2","sequence":49}');

-- [transaction_stmt] 2026-03-19T17:16:02.188781Z
INSERT OR REPLACE INTO block ("created_at", "id", "parent_id", "content", "document_id", "updated_at", "content_type", "properties") VALUES (1773940562151, 'block:ddf208e4-9b73-422d-b8ab-4ec58b328907', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'Loro-to-Turso materialization (CRDT → SQL cache → CDC)', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'text', '{"sequence":50,"ID":"ddf208e4-9b73-422d-b8ab-4ec58b328907"}');

-- [transaction_stmt] 2026-03-19T17:16:02.188943Z
INSERT OR REPLACE INTO block ("parent_id", "id", "updated_at", "content_type", "created_at", "content", "document_id", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 1773940562173, 'text', 1773940562151, 'Org-Mode Sync [/]', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","sequence":51}');

-- [transaction_stmt] 2026-03-19T17:16:02.189099Z
INSERT OR REPLACE INTO block ("content", "document_id", "parent_id", "content_type", "created_at", "id", "updated_at", "properties") VALUES ('OrgRenderer as single path for producing org text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'text', 1773940562151, 'block:7bc5f362-0bf9-45a1-b2b7-6882585ed169', 1773940562173, '{"ID":"7bc5f362-0bf9-45a1-b2b7-6882585ed169","sequence":52}');

-- [transaction_stmt] 2026-03-19T17:16:02.189263Z
INSERT OR REPLACE INTO block ("updated_at", "id", "document_id", "created_at", "content_type", "content", "parent_id", "properties") VALUES (1773940562173, 'block:8eab3453-25d2-4e7a-89f8-f9f79be939c9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562151, 'text', 'Document identity & aliases (UUID ↔ file path mapping)', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', '{"sequence":53,"ID":"8eab3453-25d2-4e7a-89f8-f9f79be939c9"}');

-- [transaction_stmt] 2026-03-19T17:16:02.189430Z
INSERT OR REPLACE INTO block ("parent_id", "content", "id", "document_id", "updated_at", "content_type", "created_at", "properties") VALUES ('block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'OrgSyncController with echo suppression', 'block:fc60da1b-6065-4d36-8551-5479ff145df0', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'text', 1773940562152, '{"sequence":54,"ID":"fc60da1b-6065-4d36-8551-5479ff145df0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.189590Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "id", "created_at", "parent_id", "document_id", "content", "properties") VALUES ('text', 1773940562173, 'block:6e5a1157-b477-45a1-892f-57807b4d969b', 1773940562152, 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Bidirectional sync (file changes ↔ block changes)', '{"ID":"6e5a1157-b477-45a1-892f-57807b4d969b","sequence":55}');

-- [transaction_stmt] 2026-03-19T17:16:02.189756Z
INSERT OR REPLACE INTO block ("content", "created_at", "updated_at", "id", "parent_id", "content_type", "document_id", "properties") VALUES ('Org file parsing (headlines, properties, source blocks)', 1773940562152, 1773940562173, 'block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":56,"ID":"6e4dab75-cd13-4c5e-9168-bf266d11aa3f"}');

-- [transaction_stmt] 2026-03-19T17:16:02.189935Z
INSERT OR REPLACE INTO block ("id", "parent_id", "document_id", "content", "content_type", "updated_at", "created_at", "properties") VALUES ('block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Flutter Frontend [/]', 'text', 1773940562173, 1773940562152, '{"ID":"bb3bc716-ca9a-438a-936d-03631e2ee929","sequence":57}');

-- [transaction_stmt] 2026-03-19T17:16:02.190091Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "document_id", "content_type", "content", "id", "parent_id", "properties") VALUES (1773940562173, 1773940562152, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'FFI bridge via flutter_rust_bridge', 'block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', '{"ID":"b4753cd8-47ea-4f7d-bd00-e1ec563aa43f","sequence":58}');

-- [transaction_stmt] 2026-03-19T17:16:02.190256Z
INSERT OR REPLACE INTO block ("id", "content_type", "created_at", "updated_at", "content", "parent_id", "document_id", "properties") VALUES ('block:3289bc82-f8a9-4cad-8545-ad1fee9dc282', 'text', 1773940562152, 1773940562173, 'Navigation system (history, cursor, focus)', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"task_state":"DOING","ID":"3289bc82-f8a9-4cad-8545-ad1fee9dc282","sequence":59}');

-- [transaction_stmt] 2026-03-19T17:16:02.190428Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "id", "parent_id", "content", "updated_at", "content_type", "properties") VALUES (1773940562152, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:ebca0a24-f6f6-4c49-8a27-9d9973acf737', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'Block editor (outliner interactions)', 1773940562173, 'text', '{"ID":"ebca0a24-f6f6-4c49-8a27-9d9973acf737","sequence":60}');

-- [transaction_stmt] 2026-03-19T17:16:02.190595Z
INSERT OR REPLACE INTO block ("id", "parent_id", "content", "document_id", "updated_at", "content_type", "created_at", "properties") VALUES ('block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'Reactive UI updates from CDC change streams', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'text', 1773940562152, '{"sequence":61,"ID":"eb7e34f8-19f5-48f5-a22d-8f62493bafdd"}');

-- [transaction_stmt] 2026-03-19T17:16:02.190777Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "parent_id", "id", "content", "created_at", "updated_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'block:7a0a4905-59c5-4277-8114-1e9ca9d425e3', 'Three-column layout (sidebar, main, right panel)', 1773940562152, 1773940562173, '{"ID":"7a0a4905-59c5-4277-8114-1e9ca9d425e3","sequence":62}');

-- [transaction_stmt] 2026-03-19T17:16:02.190948Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "created_at", "id", "parent_id", "content", "document_id", "properties") VALUES (1773940562173, 'text', 1773940562152, 'block:19d7b512-e5e0-469c-917b-eb27d7a38bed', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'Flutter desktop app shell', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"19d7b512-e5e0-469c-917b-eb27d7a38bed","sequence":63}');

-- [transaction_stmt] 2026-03-19T17:16:02.191117Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "updated_at", "content", "id", "document_id", "created_at", "properties") VALUES ('text', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773940562173, 'Petri-Net Task Ranking (WSJF) [/]', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562152, '{"ID":"afe4f75c-7948-4d4c-9724-4bfab7d47d88","sequence":64}');

-- [transaction_stmt] 2026-03-19T17:16:02.191293Z
INSERT OR REPLACE INTO block ("content", "parent_id", "id", "document_id", "created_at", "content_type", "updated_at", "properties") VALUES ('Prototype blocks with =computed Rhai expressions', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562153, 'text', 1773940562173, '{"ID":"d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","sequence":65,"task_state":"DOING"}');

-- [transaction_stmt] 2026-03-19T17:16:02.191463Z
INSERT OR REPLACE INTO block ("content_type", "id", "created_at", "updated_at", "parent_id", "document_id", "content", "properties") VALUES ('text', 'block:2d399fd7-79d8-41f1-846b-31dabcec208a', 1773940562153, 1773940562173, 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Verb dictionary (~30 German + English verbs → transition types)', '{"ID":"2d399fd7-79d8-41f1-846b-31dabcec208a","sequence":66}');

-- [transaction_stmt] 2026-03-19T17:16:02.191638Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "id", "parent_id", "content_type", "content", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:2385f4e3-25e1-4911-bf75-77cefd394206', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'text', 'rank_tasks() engine with tiebreak ordering', 1773940562153, '{"ID":"2385f4e3-25e1-4911-bf75-77cefd394206","task_state":"DOING","sequence":67}');

-- [transaction_stmt] 2026-03-19T17:16:02.191826Z
INSERT OR REPLACE INTO block ("updated_at", "content", "content_type", "document_id", "id", "created_at", "parent_id", "properties") VALUES (1773940562173, 'Block → Petri Net materialization (petri.rs)', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:cae619f2-26fe-464e-b67a-0a04f76543c9', 1773940562153, 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', '{"ID":"cae619f2-26fe-464e-b67a-0a04f76543c9","task_state":"DOING","sequence":68}');

-- [transaction_stmt] 2026-03-19T17:16:02.191996Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "document_id", "content_type", "content", "created_at", "properties") VALUES (1773940562173, 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'block:eaee1c9b-5466-428f-8dbb-f4882ccdb066', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'Self Descriptor (person block with is_self: true)', 1773940562153, '{"ID":"eaee1c9b-5466-428f-8dbb-f4882ccdb066","sequence":69,"task_state":"DOING"}');

-- [transaction_stmt] 2026-03-19T17:16:02.192186Z
INSERT OR REPLACE INTO block ("id", "created_at", "updated_at", "content", "parent_id", "document_id", "content_type", "properties") VALUES ('block:023da362-ce5d-4a3b-827a-29e745d6f778', 1773940562153, 1773940562173, 'WSJF scoring (priority_weight × urgency_weight + position_weight)', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"task_state":"DOING","sequence":70,"ID":"023da362-ce5d-4a3b-827a-29e745d6f778"}');

-- [transaction_stmt] 2026-03-19T17:16:02.192390Z
INSERT OR REPLACE INTO block ("updated_at", "id", "content_type", "document_id", "content", "parent_id", "created_at", "properties") VALUES (1773940562173, 'block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Task syntax parser (@, ?, >, [[links]])', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 1773940562153, '{"ID":"46a8c75e-8ab8-4a5a-b4af-a1388f6a4812","sequence":71}');

-- [transaction_stmt] 2026-03-19T17:16:02.192559Z
INSERT OR REPLACE INTO block ("parent_id", "content", "created_at", "content_type", "document_id", "id", "updated_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 2: First Integration (Todoist) [/]
Goal: Prove hybrid architecture', 1773940562153, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 1773940562173, '{"sequence":72,"ID":"29c0aa5f-d9ca-46f3-8601-6023f87cefbd"}');

-- [transaction_stmt] 2026-03-19T17:16:02.192732Z
INSERT OR REPLACE INTO block ("updated_at", "id", "document_id", "content_type", "content", "parent_id", "created_at", "properties") VALUES (1773940562173, 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'Reconciliation [/]', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 1773940562153, '{"sequence":73,"ID":"00fa0916-2681-4699-9554-44fcb8e2ea6a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.192905Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "created_at", "id", "document_id", "updated_at", "content", "properties") VALUES ('block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'text', 1773940562153, 'block:632af903-5459-4d44-921a-43145e20dc82', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'Sync token management to prevent duplicate processing', '{"ID":"632af903-5459-4d44-921a-43145e20dc82","sequence":74}');

-- [transaction_stmt] 2026-03-19T17:16:02.193085Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content_type", "parent_id", "document_id", "id", "content", "properties") VALUES (1773940562173, 1773940562153, 'text', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:78f9d6e3-42d4-4975-910d-3728e23410b1', 'Conflict detection and resolution UI', '{"sequence":75,"ID":"78f9d6e3-42d4-4975-910d-3728e23410b1"}');

-- [transaction_stmt] 2026-03-19T17:16:02.193274Z
INSERT OR REPLACE INTO block ("content", "content_type", "document_id", "id", "updated_at", "parent_id", "created_at", "properties") VALUES ('Last-write-wins for concurrent edits', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:fa2854d1-2751-4a07-8f83-70c2f9c6c190', 1773940562173, 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 1773940562154, '{"ID":"fa2854d1-2751-4a07-8f83-70c2f9c6c190","sequence":76}');

-- [transaction_stmt] 2026-03-19T17:16:02.193457Z
INSERT OR REPLACE INTO block ("id", "document_id", "content_type", "created_at", "updated_at", "parent_id", "content", "properties") VALUES ('block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562154, 1773940562173, 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'Operation Queue & Offline Support [/]', '{"sequence":77,"ID":"043ed925-6bf2-4db3-baf8-2277f1a5afaa"}');

-- [transaction_stmt] 2026-03-19T17:16:02.193643Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "created_at", "id", "updated_at", "content", "content_type", "properties") VALUES ('block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562154, 'block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3', 1773940562173, 'Offline operation queue with retry logic', 'text', '{"ID":"5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","sequence":78}');

-- [transaction_stmt] 2026-03-19T17:16:02.193834Z
INSERT OR REPLACE INTO block ("content", "parent_id", "document_id", "content_type", "id", "created_at", "updated_at", "properties") VALUES ('Sync status indicators (synced, pending, conflict, error)', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05', 1773940562154, 1773940562173, '{"ID":"7de8d37b-49ba-4ada-9b1e-df1c41c0db05","sequence":79}');

-- [transaction_stmt] 2026-03-19T17:16:02.194012Z
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "id", "content_type", "document_id", "updated_at", "properties") VALUES (1773940562154, 'Optimistic updates with ID mapping (internal ↔ external)', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'block:302eb0c5-56fe-4980-8292-bae8a9a0450a', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, '{"sequence":80,"ID":"302eb0c5-56fe-4980-8292-bae8a9a0450a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.194194Z
INSERT OR REPLACE INTO block ("document_id", "content", "updated_at", "created_at", "parent_id", "content_type", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist-Specific Features [/]', 1773940562173, 1773940562154, 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'text', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', '{"sequence":81,"ID":"b1b2037e-b2e9-45db-8cb9-2ed783ede2ce"}');

-- [transaction_stmt] 2026-03-19T17:16:02.194381Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "content_type", "content", "parent_id", "updated_at", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562154, 'text', 'Bi-directional task completion sync', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 1773940562173, 'block:a27cd79b-63bd-4704-b20f-f3b595838e89', '{"ID":"a27cd79b-63bd-4704-b20f-f3b595838e89","sequence":82}');

-- [transaction_stmt] 2026-03-19T17:16:02.194552Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "id", "updated_at", "content_type", "content", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562154, 'block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22', 1773940562173, 'text', 'Todoist due dates → deadline penalty functions', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', '{"ID":"ab2868f6-ac6a-48de-b56f-ffa755f6cd22","sequence":83}');

-- [transaction_stmt] 2026-03-19T17:16:02.195146Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "updated_at", "content", "parent_id", "id", "content_type", "properties") VALUES (1773940562154, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, '@person labels → delegation/waiting_for tracking', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'block:f6e32a19-a659-47f7-b2dc-24142c6616f7', 'text', '{"sequence":84,"ID":"f6e32a19-a659-47f7-b2dc-24142c6616f7"}');

-- [transaction_stmt] 2026-03-19T17:16:02.195424Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "id", "created_at", "updated_at", "parent_id", "content", "properties") VALUES ('text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:19923c1b-89ab-42f3-97a2-d78e994a2e1c', 1773940562154, 1773940562173, 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'Todoist priority → WSJF CoD weight mapping', '{"ID":"19923c1b-89ab-42f3-97a2-d78e994a2e1c","sequence":85}');

-- [transaction_stmt] 2026-03-19T17:16:02.195629Z
INSERT OR REPLACE INTO block ("id", "parent_id", "created_at", "document_id", "content", "updated_at", "content_type", "properties") VALUES ('block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 1773940562155, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'MCP Client Bridge [/]', 1773940562173, 'text', '{"ID":"f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","sequence":86}');

-- [transaction_stmt] 2026-03-19T17:16:02.195825Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "id", "created_at", "updated_at", "parent_id", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:4d30926a-54c4-40b4-978e-eeca2d273fd1', 1773940562155, 1773940562173, 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'Tool name normalization (kebab-case ↔ snake_case)', '{"ID":"4d30926a-54c4-40b4-978e-eeca2d273fd1","sequence":87}');

-- [transaction_stmt] 2026-03-19T17:16:02.196016Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "id", "updated_at", "content", "created_at", "content_type", "properties") VALUES ('block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467', 1773940562173, 'McpOperationProvider converting MCP tool schemas → OperationDescriptors', 1773940562155, 'text', '{"sequence":88,"ID":"c30b7e5a-4e9f-41e8-ab19-e803c93dc467"}');

-- [transaction_stmt] 2026-03-19T17:16:02.196202Z
INSERT OR REPLACE INTO block ("content", "updated_at", "id", "parent_id", "content_type", "created_at", "document_id", "properties") VALUES ('holon-mcp-client crate for connecting to external MCP servers', 1773940562173, 'block:836bab0e-5ac1-4df1-9f40-4005320c406e', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'text', 1773940562155, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"836bab0e-5ac1-4df1-9f40-4005320c406e","sequence":89}');

-- [transaction_stmt] 2026-03-19T17:16:02.196392Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "created_at", "updated_at", "document_id", "id", "content", "properties") VALUES ('block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'text', 1773940562155, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:ceb59dae-6090-41be-aff7-89de33ec600a', 'YAML sidecar for UI annotations (affected_fields, triggered_by, preconditions)', '{"sequence":90,"ID":"ceb59dae-6090-41be-aff7-89de33ec600a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.196579Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "parent_id", "created_at", "content", "id", "document_id", "properties") VALUES ('text', 1773940562173, 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 1773940562155, 'JSON Schema → TypeHint mapping', 'block:419e493f-c2de-47c2-a612-787db669cd89', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":91,"ID":"419e493f-c2de-47c2-a612-787db669cd89"}');

-- [transaction_stmt] 2026-03-19T17:16:02.196767Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "updated_at", "content_type", "content", "parent_id", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562155, 1773940562173, 'text', 'Todoist API Integration [/]', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', '{"sequence":92,"ID":"bdce9ec2-1508-47e9-891e-e12a7b228fcc"}');

-- [transaction_stmt] 2026-03-19T17:16:02.196955Z
INSERT OR REPLACE INTO block ("content_type", "id", "content", "updated_at", "document_id", "created_at", "parent_id", "properties") VALUES ('text', 'block:e9398514-1686-4fef-a44a-5fef1742d004', 'TodoistOperationProvider for operation routing', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562155, 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', '{"sequence":93,"ID":"e9398514-1686-4fef-a44a-5fef1742d004"}');

-- [transaction_stmt] 2026-03-19T17:16:02.197165Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "updated_at", "id", "document_id", "content_type", "content", "properties") VALUES (1773940562155, 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 1773940562173, 'block:9670e586-5cda-42a2-8071-efaf855fd5d4', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'Todoist REST API client', '{"sequence":94,"ID":"9670e586-5cda-42a2-8071-efaf855fd5d4"}');

-- [transaction_stmt] 2026-03-19T17:16:02.197362Z
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "updated_at", "id", "content_type", "document_id", "properties") VALUES (1773940562155, 'Todoist entity types (tasks, projects, sections, labels)', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 1773940562173, 'block:f41aeaa5-fe1d-45a5-806d-1f815040a33d', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"f41aeaa5-fe1d-45a5-806d-1f815040a33d","sequence":95}');

-- [transaction_stmt] 2026-03-19T17:16:02.197557Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "parent_id", "id", "created_at", "document_id", "content", "properties") VALUES ('text', 1773940562173, 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe', 1773940562155, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'TodoistSyncProvider with incremental sync tokens', '{"sequence":96,"ID":"d041e942-f3a1-4b7d-80b8-7de6eb289ebe"}');

-- [transaction_stmt] 2026-03-19T17:16:02.197755Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "document_id", "created_at", "content", "id", "updated_at", "properties") VALUES ('text', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562155, 'TodoistTaskDataSource implementing DataSource<TodoistTask>', 'block:f3b43be1-5503-4b1a-a724-fc657b47e18c', 1773940562173, '{"ID":"f3b43be1-5503-4b1a-a724-fc657b47e18c","sequence":97}');

-- [transaction_stmt] 2026-03-19T17:16:02.197951Z
INSERT OR REPLACE INTO block ("content", "created_at", "parent_id", "content_type", "id", "document_id", "updated_at", "properties") VALUES ('Phase 3: Multiple Integrations [/]
Goal: Validate type unification scales', 1773940562156, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, '{"ID":"88810f15-a95b-4343-92e2-909c5113cc9c","sequence":98}');

-- [transaction_stmt] 2026-03-19T17:16:02.198154Z
INSERT OR REPLACE INTO block ("updated_at", "id", "document_id", "content", "content_type", "parent_id", "created_at", "properties") VALUES (1773940562173, 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Unified Item Types [/]', 'text', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 1773940562156, '{"sequence":99,"ID":"9ea38e3d-383e-4c27-9533-d53f1f8b1fb2"}');

-- [transaction_stmt] 2026-03-19T17:16:02.198358Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "id", "content", "parent_id", "content_type", "document_id", "properties") VALUES (1773940562156, 1773940562173, 'block:5b1e8251-be26-4099-b169-a330cc16f0a6', 'Macro-generated serialization boilerplate', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"5b1e8251-be26-4099-b169-a330cc16f0a6","sequence":100}');

-- [transaction_stmt] 2026-03-19T17:16:02.198544Z
INSERT OR REPLACE INTO block ("id", "content", "content_type", "created_at", "document_id", "parent_id", "updated_at", "properties") VALUES ('block:5b49aefd-e14f-4151-bf9e-ccccae3545ec', 'Trait-based protocol for common task interface', 'text', 1773940562156, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 1773940562173, '{"sequence":101,"ID":"5b49aefd-e14f-4151-bf9e-ccccae3545ec"}');

-- [transaction_stmt] 2026-03-19T17:16:02.198733Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "id", "content_type", "created_at", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447', 'text', 1773940562156, 'Extension structs for system-specific features', '{"ID":"e6162a0a-e9ae-494e-b3f5-4cf98cb2f447","sequence":102}');

-- [transaction_stmt] 2026-03-19T17:16:02.198921Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "id", "content", "content_type", "parent_id", "document_id", "properties") VALUES (1773940562156, 1773940562173, 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'Cross-System Features [/]', 'text', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"d6ab6d5f-68ae-404a-bcad-b5db61586634","sequence":103}');

-- [transaction_stmt] 2026-03-19T17:16:02.199106Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "document_id", "id", "parent_id", "content", "created_at", "properties") VALUES ('text', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:5403c088-a551-4ca6-8830-34e00d5e5820', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'Context Bundles assembling related items from all sources', 1773940562156, '{"ID":"5403c088-a551-4ca6-8830-34e00d5e5820","sequence":104}');

-- [transaction_stmt] 2026-03-19T17:16:02.199295Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "content_type", "content", "id", "parent_id", "document_id", "properties") VALUES (1773940562156, 1773940562173, 'text', 'Embedding third-party items anywhere in the graph', 'block:091caad8-1689-472d-9130-e3c855c510a8', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":105,"ID":"091caad8-1689-472d-9130-e3c855c510a8"}');

-- [transaction_stmt] 2026-03-19T17:16:02.200122Z
INSERT OR REPLACE INTO block ("content", "created_at", "content_type", "document_id", "parent_id", "id", "updated_at", "properties") VALUES ('Unified search across all systems', 1773940562156, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'block:cfb257f0-1a9c-426c-ab24-940eb18853ea', 1773940562173, '{"sequence":106,"ID":"cfb257f0-1a9c-426c-ab24-940eb18853ea"}');

-- [transaction_stmt] 2026-03-19T17:16:02.200312Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "document_id", "updated_at", "parent_id", "content", "id", "properties") VALUES ('text', 1773940562156, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'P.A.R.A. project-based organization with auto-linking', 'block:52a440c1-4099-4911-8d9d-e2d583dbdde7', '{"sequence":107,"ID":"52a440c1-4099-4911-8d9d-e2d583dbdde7"}');

-- [transaction_stmt] 2026-03-19T17:16:02.200508Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "id", "parent_id", "document_id", "created_at", "content", "properties") VALUES ('text', 1773940562173, 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562157, 'Additional Integrations [/]', '{"sequence":108,"ID":"34fa9276-cc30-4fcb-95b5-a97b5d708757"}');

-- [transaction_stmt] 2026-03-19T17:16:02.200706Z
INSERT OR REPLACE INTO block ("parent_id", "id", "updated_at", "document_id", "content", "created_at", "content_type", "properties") VALUES ('block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'block:9240c0d7-d60a-46e0-8265-ceacfbf04d50', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Linear integration (cycles, projects)', 1773940562157, 'text', '{"sequence":109,"ID":"9240c0d7-d60a-46e0-8265-ceacfbf04d50"}');

-- [transaction_stmt] 2026-03-19T17:16:02.200897Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "created_at", "document_id", "content", "id", "parent_id", "properties") VALUES (1773940562173, 'text', 1773940562157, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Google Calendar integration (events as time tokens)', 'block:8ea813ff-b355-4165-b377-fbdef4d3d7d8', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', '{"ID":"8ea813ff-b355-4165-b377-fbdef4d3d7d8","sequence":110}');

-- [transaction_stmt] 2026-03-19T17:16:02.201096Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "id", "content", "content_type", "document_id", "parent_id", "properties") VALUES (1773940562173, 1773940562157, 'block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29', 'Gmail integration (email threads, labels)', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', '{"ID":"ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29","sequence":111}');

-- [transaction_stmt] 2026-03-19T17:16:02.201304Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content", "id", "parent_id", "content_type", "updated_at", "properties") VALUES (1773940562157, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'JIRA integration (sprints, story points, epics)', 'block:f583e6d9-f67d-4997-a658-ed00149a34cc', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'text', 1773940562173, '{"sequence":112,"ID":"f583e6d9-f67d-4997-a658-ed00149a34cc"}');

-- [transaction_stmt] 2026-03-19T17:16:02.201515Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "id", "content_type", "parent_id", "content", "updated_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562157, 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'text', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'GPUI Components', 1773940562173, '{"ID":"9fed69a3-9180-4eba-a778-fa93bc398064","sequence":113}');

-- [transaction_stmt] 2026-03-19T17:16:02.201699Z
INSERT OR REPLACE INTO block ("id", "document_id", "updated_at", "content", "content_type", "created_at", "parent_id", "properties") VALUES ('block:9f523ce8-5449-4a2f-81c8-8ee08399fc31', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'https://github.com/MeowLynxSea/yororen-ui', 'text', 1773940562157, 'block:9fed69a3-9180-4eba-a778-fa93bc398064', '{"sequence":114,"ID":"9f523ce8-5449-4a2f-81c8-8ee08399fc31"}');

-- [transaction_stmt] 2026-03-19T17:16:02.202211Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "content", "updated_at", "parent_id", "id", "document_id", "properties") VALUES (1773940562157, 'text', 'Pomodoro
https://github.com/rubbieKelvin/bmo', 1773940562173, 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'block:fd965570-883d-48f7-82b0-92ba257b2597', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"fd965570-883d-48f7-82b0-92ba257b2597","sequence":115}');

-- [transaction_stmt] 2026-03-19T17:16:02.202413Z
INSERT OR REPLACE INTO block ("content", "document_id", "content_type", "parent_id", "id", "created_at", "updated_at", "properties") VALUES ('Diff viewer
https://github.com/BlixtWallet/hunk', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'block:9657e201-4426-4091-891b-eb40e299d81d', 1773940562157, 1773940562173, '{"sequence":116,"ID":"9657e201-4426-4091-891b-eb40e299d81d"}');

-- [transaction_stmt] 2026-03-19T17:16:02.202931Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "updated_at", "content", "document_id", "content_type", "properties") VALUES ('block:9fed69a3-9180-4eba-a778-fa93bc398064', 1773940562157, 'block:61a47437-c394-42db-b195-3dabbd5d87ab', 1773940562173, 'Animation
https://github.com/chi11321/gpui-animation', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"sequence":117,"ID":"61a47437-c394-42db-b195-3dabbd5d87ab"}');

-- [transaction_stmt] 2026-03-19T17:16:02.203120Z
INSERT OR REPLACE INTO block ("updated_at", "content", "id", "document_id", "content_type", "created_at", "parent_id", "properties") VALUES (1773940562173, 'Editor
https://github.com/iamnbutler/gpui-editor', 'block:5841efc0-cfe6-4e69-9dbc-9f627693e59a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562157, 'block:9fed69a3-9180-4eba-a778-fa93bc398064', '{"sequence":118,"ID":"5841efc0-cfe6-4e69-9dbc-9f627693e59a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.203315Z
INSERT OR REPLACE INTO block ("parent_id", "content", "document_id", "created_at", "updated_at", "id", "content_type", "properties") VALUES ('block:9fed69a3-9180-4eba-a778-fa93bc398064', 'WebView
https://github.com/longbridge/wef', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562158, 1773940562173, 'block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c', 'text', '{"sequence":119,"ID":"482c5cbb-dd4f-4225-9329-ca9ca0beea4c"}');

-- [transaction_stmt] 2026-03-19T17:16:02.203848Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "created_at", "parent_id", "document_id", "content", "id", "properties") VALUES ('text', 1773940562173, 1773940562158, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 4: AI Foundation [/]
Goal: Infrastructure for AI features', 'block:7b960cd0-3478-412b-b96f-15822117ac14', '{"ID":"7b960cd0-3478-412b-b96f-15822117ac14","sequence":120}');

-- [transaction_stmt] 2026-03-19T17:16:02.204049Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "document_id", "created_at", "content_type", "id", "content", "properties") VALUES ('block:7b960cd0-3478-412b-b96f-15822117ac14', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562158, 'text', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'Agent Embedding', '{"sequence":121,"ID":"553f3545-4ec7-44e5-bccf-3d6443f22ecc"}');

-- [transaction_stmt] 2026-03-19T17:16:02.204234Z
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "content", "updated_at", "document_id", "content_type", "properties") VALUES ('block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 1773940562158, 'Via Terminal', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"sequence":122,"ID":"d4c1533f-3a67-4314-b430-0e24bd62ce34"}');

-- [transaction_stmt] 2026-03-19T17:16:02.205082Z
INSERT OR REPLACE INTO block ("id", "content", "document_id", "parent_id", "content_type", "created_at", "updated_at", "properties") VALUES ('block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e', 'Okena
A fast, native terminal multiplexer built in Rust with GPUI
https://github.com/contember/okena', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'text', 1773940562158, 1773940562173, '{"sequence":123,"ID":"6e2fd9a2-6f39-48d2-b323-935fc18a3f5e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.205629Z
INSERT OR REPLACE INTO block ("parent_id", "content", "created_at", "updated_at", "document_id", "content_type", "id", "properties") VALUES ('block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'PMux
https://github.com/zhoujinliang/pmux', 1773940562158, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e', '{"ID":"c4b1ce62-0ad1-4c33-90fe-d7463f40800e","sequence":124}');

-- [transaction_stmt] 2026-03-19T17:16:02.205823Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "content_type", "created_at", "content", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'text', 1773940562158, 'Slick
https://github.com/tristanpoland/Slick', 'block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e', '{"sequence":125,"ID":"e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.206360Z
INSERT OR REPLACE INTO block ("content", "created_at", "parent_id", "content_type", "document_id", "updated_at", "id", "properties") VALUES ('https://github.com/zortax/gpui-terminal', 1773940562158, 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:d50a9a7a-0155-4778-ac99-5f83555a1952', '{"sequence":126,"ID":"d50a9a7a-0155-4778-ac99-5f83555a1952"}');

-- [transaction_stmt] 2026-03-19T17:16:02.206976Z
INSERT OR REPLACE INTO block ("id", "updated_at", "created_at", "parent_id", "content", "document_id", "content_type", "properties") VALUES ('block:cf102b47-01db-427b-97b6-3c066d9dba24', 1773940562173, 1773940562158, 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'https://github.com/Xuanwo/gpui-ghostty', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"ID":"cf102b47-01db-427b-97b6-3c066d9dba24","sequence":127}');

-- [transaction_stmt] 2026-03-19T17:16:02.207497Z
INSERT OR REPLACE INTO block ("parent_id", "id", "updated_at", "document_id", "content_type", "created_at", "content", "properties") VALUES ('block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562158, 'Via Chat', '{"ID":"1236a3b4-6e03-421a-a94b-fce9d7dc123c","sequence":128}');

-- [transaction_stmt] 2026-03-19T17:16:02.208009Z
INSERT OR REPLACE INTO block ("content", "document_id", "parent_id", "updated_at", "created_at", "content_type", "id", "properties") VALUES ('coop
https://github.com/lumehq/coop?tab=readme-ov-file', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 1773940562173, 1773940562158, 'text', 'block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847', '{"ID":"f47a6df7-abfc-47b8-bdfe-f19eaf35b847","sequence":129}');

-- [transaction_stmt] 2026-03-19T17:16:02.208583Z
INSERT OR REPLACE INTO block ("updated_at", "id", "parent_id", "content", "document_id", "content_type", "created_at", "properties") VALUES (1773940562173, 'block:671593d9-a9c6-4716-860b-8410c8616539', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'Embeddings & Search [/]', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562159, '{"ID":"671593d9-a9c6-4716-860b-8410c8616539","sequence":130}');

-- [transaction_stmt] 2026-03-19T17:16:02.208774Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content_type", "updated_at", "id", "parent_id", "content", "properties") VALUES (1773940562159, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562173, 'block:d58b8367-14eb-4895-9e56-ffa7ff716d59', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'Local vector embeddings (sentence-transformers)', '{"sequence":131,"ID":"d58b8367-14eb-4895-9e56-ffa7ff716d59"}');

-- [transaction_stmt] 2026-03-19T17:16:02.208968Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "content", "parent_id", "id", "created_at", "updated_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'Semantic search using local embeddings', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc', 1773940562159, 1773940562173, '{"ID":"5f3e7d1e-af67-4699-a591-fd9291bf0cdc","sequence":132}');

-- [transaction_stmt] 2026-03-19T17:16:02.209139Z
INSERT OR REPLACE INTO block ("id", "content", "document_id", "content_type", "parent_id", "updated_at", "created_at", "properties") VALUES ('block:96f4647c-8b74-4b08-8952-4f87820aed86', 'Entity linking (manual first, then automatic)', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:671593d9-a9c6-4716-860b-8410c8616539', 1773940562173, 1773940562159, '{"ID":"96f4647c-8b74-4b08-8952-4f87820aed86","sequence":133}');

-- [transaction_stmt] 2026-03-19T17:16:02.209322Z
INSERT OR REPLACE INTO block ("content", "parent_id", "content_type", "updated_at", "document_id", "id", "created_at", "properties") VALUES ('Tantivy full-text search integration', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'text', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:0da39f39-6635-4f9b-a468-34310147bea9', 1773940562159, '{"ID":"0da39f39-6635-4f9b-a468-34310147bea9","sequence":134}');

-- [transaction_stmt] 2026-03-19T17:16:02.209505Z
INSERT OR REPLACE INTO block ("id", "content_type", "created_at", "content", "parent_id", "document_id", "updated_at", "properties") VALUES ('block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'text', 1773940562159, 'Self Digital Twin [/]', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, '{"sequence":135,"ID":"439af07e-3237-420c-8bc0-c71aeb37c61a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.209696Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "document_id", "content_type", "id", "parent_id", "content", "properties") VALUES (1773940562159, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'Energy/focus/flow_depth dynamics', '{"ID":"5f3e8ef3-df52-4fb9-80c1-ccb81be40412","sequence":136}');

-- [transaction_stmt] 2026-03-19T17:16:02.209916Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "created_at", "content_type", "parent_id", "updated_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:30406a65-8e66-4589-b070-3a1b4db6e4e0', 'Peripheral awareness modeling', 1773940562159, 'text', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 1773940562173, '{"sequence":137,"ID":"30406a65-8e66-4589-b070-3a1b4db6e4e0"}');

-- [transaction_stmt] 2026-03-19T17:16:02.210122Z
INSERT OR REPLACE INTO block ("content", "parent_id", "content_type", "id", "created_at", "updated_at", "document_id", "properties") VALUES ('Observable signals (window switches, typing cadence)', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'text', 'block:bed11feb-a634-4f8d-b930-f0021ec0512b', 1773940562159, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":138,"ID":"bed11feb-a634-4f8d-b930-f0021ec0512b"}');

-- [transaction_stmt] 2026-03-19T17:16:02.210316Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "created_at", "parent_id", "id", "content_type", "content", "properties") VALUES (1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562159, 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'block:11c9c8bb-b72e-4752-8b6c-846e45920418', 'text', 'Mental slots tracking (materialized view of open transitions)', '{"sequence":139,"ID":"11c9c8bb-b72e-4752-8b6c-846e45920418"}');

-- [transaction_stmt] 2026-03-19T17:16:02.210504Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "document_id", "parent_id", "content", "content_type", "id", "properties") VALUES (1773940562159, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'Logging & Training Data [/]', 'text', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', '{"sequence":140,"ID":"b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5"}');

-- [transaction_stmt] 2026-03-19T17:16:02.210697Z
INSERT OR REPLACE INTO block ("content", "created_at", "content_type", "updated_at", "document_id", "id", "parent_id", "properties") VALUES ('Conflict logging system (capture every conflict + resolution)', 1773940562160, 'text', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:a186c88f-6ca5-49e2-8a0d-19632cb689fc', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', '{"ID":"a186c88f-6ca5-49e2-8a0d-19632cb689fc","sequence":141}');

-- [transaction_stmt] 2026-03-19T17:16:02.210897Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "content", "created_at", "document_id", "content_type", "id", "properties") VALUES ('block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 1773940562173, 'Pattern logging for Guide to learn from', 1773940562160, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:f342692d-5414-4c48-89fe-ed8f9ccf2172', '{"sequence":142,"ID":"f342692d-5414-4c48-89fe-ed8f9ccf2172"}');

-- [transaction_stmt] 2026-03-19T17:16:02.211099Z
INSERT OR REPLACE INTO block ("content", "parent_id", "created_at", "content_type", "updated_at", "id", "document_id", "properties") VALUES ('Behavioral logging for search ranking', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 1773940562160, 'text', 1773940562173, 'block:30f04064-a58e-416d-b0d2-7533637effe8', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"30f04064-a58e-416d-b0d2-7533637effe8","sequence":143}');

-- [transaction_stmt] 2026-03-19T17:16:02.211298Z
INSERT OR REPLACE INTO block ("content", "created_at", "content_type", "parent_id", "document_id", "updated_at", "id", "properties") VALUES ('Objective Function Engine [/]', 1773940562160, 'text', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:84151cf1-696a-420f-b73c-4947b0a4437e', '{"sequence":144,"ID":"84151cf1-696a-420f-b73c-4947b0a4437e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.211485Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "parent_id", "content", "updated_at", "id", "content_type", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562160, 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'Evaluate token attributes via PRQL → scalar score', 1773940562173, 'block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7', 'text', '{"sequence":145,"ID":"fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7"}');

-- [transaction_stmt] 2026-03-19T17:16:02.211670Z
INSERT OR REPLACE INTO block ("created_at", "id", "content", "updated_at", "content_type", "parent_id", "document_id", "properties") VALUES (1773940562160, 'block:480f2628-c49f-4940-9e26-572ea23f25a3', 'Store weights as prototype block properties', 1773940562173, 'text', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":146,"ID":"480f2628-c49f-4940-9e26-572ea23f25a3"}');

-- [transaction_stmt] 2026-03-19T17:16:02.211866Z
INSERT OR REPLACE INTO block ("id", "document_id", "parent_id", "content", "content_type", "updated_at", "created_at", "properties") VALUES ('block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'Support multiple goal types (achievement, maintenance, process)', 'text', 1773940562173, 1773940562160, '{"sequence":147,"ID":"e4e93198-6617-4c7c-b8f7-4b2d8188a77e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.212056Z
INSERT OR REPLACE INTO block ("document_id", "id", "updated_at", "created_at", "parent_id", "content", "content_type", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 1773940562173, 1773940562160, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 5: AI Features [/]
Goal: Three AI services operational', 'text', '{"sequence":148,"ID":"8b962d6c-0246-4119-8826-d517e2357f21"}');

-- [transaction_stmt] 2026-03-19T17:16:02.212238Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "parent_id", "content_type", "content", "updated_at", "id", "properties") VALUES (1773940562160, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'text', 'The Guide (Growth) [/]', 1773940562173, 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', '{"ID":"567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","sequence":149}');

-- [transaction_stmt] 2026-03-19T17:16:02.212428Z
INSERT OR REPLACE INTO block ("parent_id", "content", "created_at", "id", "document_id", "content_type", "updated_at", "properties") VALUES ('block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'Velocity and capacity analysis', 1773940562160, 'block:37c082de-d10a-4f11-82ad-5fb3316bb3e4', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562173, '{"sequence":150,"ID":"37c082de-d10a-4f11-82ad-5fb3316bb3e4"}');

-- [transaction_stmt] 2026-03-19T17:16:02.212618Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "document_id", "id", "created_at", "content", "parent_id", "properties") VALUES (1773940562173, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:52bedd69-85ec-448d-81b6-0099bd413149', 1773940562160, 'Stuck task identification (postponement tracking)', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', '{"ID":"52bedd69-85ec-448d-81b6-0099bd413149","sequence":151}');

-- [transaction_stmt] 2026-03-19T17:16:02.212811Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "document_id", "content_type", "updated_at", "content", "properties") VALUES ('block:2b5ec929-a22d-4d7f-8640-66495331a40d', 1773940562161, 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562173, 'Shadow Work prompts for avoided tasks', '{"ID":"2b5ec929-a22d-4d7f-8640-66495331a40d","sequence":152}');

-- [transaction_stmt] 2026-03-19T17:16:02.213007Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "parent_id", "created_at", "content", "id", "content_type", "properties") VALUES (1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 1773940562161, 'Growth tracking and visualization', 'block:dd9075a4-5c64-4d6b-9661-7937897337d3', 'text', '{"ID":"dd9075a4-5c64-4d6b-9661-7937897337d3","sequence":153}');

-- [transaction_stmt] 2026-03-19T17:16:02.213192Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "id", "parent_id", "updated_at", "content_type", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562161, 'block:15a61916-b0c1-4d24-9046-4e066a312401', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 1773940562173, 'text', 'Pattern recognition across time', '{"ID":"15a61916-b0c1-4d24-9046-4e066a312401","sequence":154}');

-- [transaction_stmt] 2026-03-19T17:16:02.213390Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "updated_at", "content", "content_type", "parent_id", "id", "properties") VALUES (1773940562161, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'Intelligent Conflict Reconciliation [/]', 'text', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', '{"ID":"8ae21b36-6f48-41f1-80d9-bb7ce43b4545","sequence":155}');

-- [transaction_stmt] 2026-03-19T17:16:02.213581Z
INSERT OR REPLACE INTO block ("parent_id", "content", "updated_at", "document_id", "content_type", "created_at", "id", "properties") VALUES ('block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'LLM-based resolution for low-confidence cases', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562161, 'block:0db1be3e-ae11-4341-8aa8-b1d80e22963a', '{"sequence":156,"ID":"0db1be3e-ae11-4341-8aa8-b1d80e22963a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.213788Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "id", "content", "created_at", "updated_at", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:314e7db7-fb5e-40b6-ac10-a589ff3c809d', 'Rule-based conflict resolver', 1773940562161, 1773940562173, 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', '{"ID":"314e7db7-fb5e-40b6-ac10-a589ff3c809d","sequence":157}');

-- [transaction_stmt] 2026-03-19T17:16:02.213986Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "updated_at", "document_id", "created_at", "content", "id", "properties") VALUES ('block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'text', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562161, 'Train classifier on logged conflicts', 'block:655e2f77-d02e-4347-aa5f-dcd03ac140eb', '{"sequence":158,"ID":"655e2f77-d02e-4347-aa5f-dcd03ac140eb"}');

-- [transaction_stmt] 2026-03-19T17:16:02.214189Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "id", "updated_at", "content", "document_id", "content_type", "properties") VALUES (1773940562161, 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'block:3bbdc016-4f08-49e4-b550-ba3d09a03933', 1773940562173, 'Conflict resolution UI with reasoning display', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"sequence":159,"ID":"3bbdc016-4f08-49e4-b550-ba3d09a03933"}');

-- [transaction_stmt] 2026-03-19T17:16:02.214385Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "content", "created_at", "content_type", "id", "document_id", "properties") VALUES (1773940562173, 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'AI Trust Ladder [/]', 1773940562161, 'text', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":160,"ID":"be9e6d6e-f995-4a27-bd5e-b2f70f12c93e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.214578Z
INSERT OR REPLACE INTO block ("id", "parent_id", "created_at", "document_id", "updated_at", "content_type", "content", "properties") VALUES ('block:8a72f072-cc14-4e5f-987c-72bd27d94ced', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 1773940562161, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'text', 'Level 3 (Agentic) with permission prompts', '{"ID":"8a72f072-cc14-4e5f-987c-72bd27d94ced","sequence":161}');

-- [transaction_stmt] 2026-03-19T17:16:02.214776Z
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "content_type", "content", "document_id", "updated_at", "properties") VALUES ('block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'block:c2289c19-1733-476e-9b50-43da1d70221f', 1773940562161, 'text', 'Level 4 (Autonomous) for power users', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, '{"sequence":162,"ID":"c2289c19-1733-476e-9b50-43da1d70221f"}');

-- [transaction_stmt] 2026-03-19T17:16:02.214979Z
INSERT OR REPLACE INTO block ("updated_at", "content", "content_type", "id", "created_at", "parent_id", "document_id", "properties") VALUES (1773940562173, 'Level 2 (Advisory) features', 'text', 'block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0', 1773940562162, 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","sequence":163}');

-- [transaction_stmt] 2026-03-19T17:16:02.215165Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "content_type", "updated_at", "content", "document_id", "properties") VALUES ('block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 1773940562162, 'block:84706843-7132-4c12-a2ae-32fb7109982c', 'text', 1773940562173, 'Per-feature trust tracking', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"84706843-7132-4c12-a2ae-32fb7109982c","sequence":164}');

-- [transaction_stmt] 2026-03-19T17:16:02.215367Z
INSERT OR REPLACE INTO block ("id", "parent_id", "content", "created_at", "document_id", "content_type", "updated_at", "properties") VALUES ('block:66b47313-a556-4628-954e-1da7fb1d402d', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'Trust level visualization UI', 1773940562162, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562173, '{"ID":"66b47313-a556-4628-954e-1da7fb1d402d","sequence":165}');

-- [transaction_stmt] 2026-03-19T17:16:02.215565Z
INSERT OR REPLACE INTO block ("content", "document_id", "created_at", "content_type", "parent_id", "updated_at", "id", "properties") VALUES ('Background Enrichment Agents [/]', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562162, 'text', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 1773940562173, 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', '{"sequence":166,"ID":"d1e6541b-0c6b-4065-aea5-ad9057dc5bb5"}');

-- [transaction_stmt] 2026-03-19T17:16:02.215759Z
INSERT OR REPLACE INTO block ("updated_at", "id", "parent_id", "content_type", "content", "created_at", "document_id", "properties") VALUES (1773940562173, 'block:2618de83-3d90-4dc6-b586-98f95e351fb5', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'text', 'Infer likely token types from context', 1773940562162, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"2618de83-3d90-4dc6-b586-98f95e351fb5","sequence":167}');

-- [transaction_stmt] 2026-03-19T17:16:02.216450Z
INSERT OR REPLACE INTO block ("content_type", "content", "parent_id", "updated_at", "document_id", "id", "created_at", "properties") VALUES ('text', 'Suggest dependencies between siblings', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec', 1773940562162, '{"ID":"edd212e6-16a9-4dfd-95f9-e2a2a3a55eec","sequence":168}');

-- [transaction_stmt] 2026-03-19T17:16:02.217064Z
INSERT OR REPLACE INTO block ("content", "id", "updated_at", "parent_id", "document_id", "content_type", "created_at", "properties") VALUES ('Suggest [[links]] for plain-text nouns (local LLM)', 'block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda', 1773940562173, 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562162, '{"ID":"44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda","sequence":169}');

-- [transaction_stmt] 2026-03-19T17:16:02.217264Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "content", "document_id", "content_type", "created_at", "properties") VALUES (1773940562173, 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'block:2ff960fa-38a4-42dd-8eb0-77e15c89659e', 'Classify tasks as question/delegation/action', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562162, '{"sequence":170,"ID":"2ff960fa-38a4-42dd-8eb0-77e15c89659e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.217455Z
INSERT OR REPLACE INTO block ("content", "parent_id", "document_id", "created_at", "id", "updated_at", "content_type", "properties") VALUES ('Suggest via: routes for questions', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562162, 'block:864527d2-65d4-4716-a65e-73a868c7e63b', 1773940562173, 'text', '{"sequence":171,"ID":"864527d2-65d4-4716-a65e-73a868c7e63b"}');

-- [transaction_stmt] 2026-03-19T17:16:02.217649Z
INSERT OR REPLACE INTO block ("content", "parent_id", "id", "created_at", "updated_at", "content_type", "document_id", "properties") VALUES ('The Integrator (Wholeness) [/]', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 1773940562162, 1773940562173, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":172,"ID":"8a4a658e-d773-4528-8c61-ff3e5e425f47"}');

-- [transaction_stmt] 2026-03-19T17:16:02.218348Z
INSERT OR REPLACE INTO block ("parent_id", "content", "created_at", "updated_at", "document_id", "id", "content_type", "properties") VALUES ('block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'Smart linking suggestions', 1773940562162, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a', 'text', '{"sequence":173,"ID":"2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.218545Z
INSERT OR REPLACE INTO block ("document_id", "content", "content_type", "updated_at", "created_at", "id", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Context Bundle assembly for Flow mode', 'text', 1773940562173, 1773940562163, 'block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', '{"sequence":174,"ID":"4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6"}');

-- [transaction_stmt] 2026-03-19T17:16:02.218733Z
INSERT OR REPLACE INTO block ("parent_id", "content", "content_type", "created_at", "document_id", "updated_at", "id", "properties") VALUES ('block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'Cross-system deduplication', 'text', 1773940562163, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:7efa2454-274c-4304-8641-e3b8171c5b5a', '{"sequence":175,"ID":"7efa2454-274c-4304-8641-e3b8171c5b5a"}');

-- [transaction_stmt] 2026-03-19T17:16:02.218921Z
INSERT OR REPLACE INTO block ("id", "content", "created_at", "parent_id", "updated_at", "content_type", "document_id", "properties") VALUES ('block:311aa51c-88af-446f-8cb6-b791b9740665', 'Related item discovery', 1773940562163, 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 1773940562173, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":176,"ID":"311aa51c-88af-446f-8cb6-b791b9740665"}');

-- [transaction_stmt] 2026-03-19T17:16:02.219117Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "content", "updated_at", "content_type", "document_id", "id", "properties") VALUES (1773940562163, 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'Automatic entity linking via embeddings', 1773940562173, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:9b6b2563-21b8-4286-9fac-dbdddc1a79be', '{"ID":"9b6b2563-21b8-4286-9fac-dbdddc1a79be","sequence":177}');

-- [transaction_stmt] 2026-03-19T17:16:02.219313Z
INSERT OR REPLACE INTO block ("document_id", "id", "updated_at", "content_type", "parent_id", "content", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 1773940562173, 'text', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'The Watcher (Awareness) [/]', 1773940562163, '{"ID":"d385afbe-5bc9-4341-b879-6d14b8d763bc","sequence":178}');

-- [transaction_stmt] 2026-03-19T17:16:02.219506Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "updated_at", "id", "content", "content_type", "parent_id", "properties") VALUES (1773940562163, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23', 'Risk and deadline tracking', 'text', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', '{"sequence":179,"ID":"244abb7d-ef0f-4768-9e4e-b4bd7f3eec23"}');

-- [transaction_stmt] 2026-03-19T17:16:02.219694Z
INSERT OR REPLACE INTO block ("id", "created_at", "content", "content_type", "updated_at", "parent_id", "document_id", "properties") VALUES ('block:f9a2e27c-218f-402a-b405-b6b14b498bcf', 1773940562163, 'Capacity analysis across all systems', 'text', 1773940562173, 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"f9a2e27c-218f-402a-b405-b6b14b498bcf","sequence":180}');

-- [transaction_stmt] 2026-03-19T17:16:02.219891Z
INSERT OR REPLACE INTO block ("content", "document_id", "id", "created_at", "parent_id", "updated_at", "content_type", "properties") VALUES ('Cross-system monitoring and alerts', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4', 1773940562163, 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 1773940562173, 'text', '{"ID":"92d9dee2-3c16-4d14-9d54-1a93313ee1f4","sequence":181}');

-- [transaction_stmt] 2026-03-19T17:16:02.220081Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "updated_at", "parent_id", "content", "created_at", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562173, 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'Daily/weekly synthesis for Orient mode', 1773940562163, 'block:e6c28ce7-c659-49e7-874b-334f05852cc4', '{"sequence":182,"ID":"e6c28ce7-c659-49e7-874b-334f05852cc4"}');

-- [transaction_stmt] 2026-03-19T17:16:02.220274Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "created_at", "updated_at", "id", "content", "content_type", "properties") VALUES ('block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562163, 1773940562173, 'block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519', 'Dependency chain analysis', 'text', '{"sequence":183,"ID":"1ffa7eb6-174a-4bed-85d2-9c47d9d55519"}');

-- [transaction_stmt] 2026-03-19T17:16:02.220472Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "id", "parent_id", "document_id", "content", "updated_at", "properties") VALUES (1773940562163, 'text', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 6: Flow Optimization [/]
Goal: Users achieve flow states regularly', 1773940562173, '{"ID":"c74fcc72-883d-4788-911a-0632f6145e4d","sequence":184}');

-- [transaction_stmt] 2026-03-19T17:16:02.220662Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "updated_at", "content", "created_at", "content_type", "id", "properties") VALUES ('block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'Self DT Work Rhythms [/]', 1773940562164, 'text', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', '{"ID":"f908d928-db6f-495e-a941-22fcdfdba73a","sequence":185}');

-- [transaction_stmt] 2026-03-19T17:16:02.220861Z
INSERT OR REPLACE INTO block ("id", "document_id", "content_type", "content", "updated_at", "parent_id", "created_at", "properties") VALUES ('block:0570c0bf-84b4-4734-b6f3-25242a12a154', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'Emergent break suggestions from energy/focus dynamics', 1773940562173, 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 1773940562164, '{"sequence":186,"ID":"0570c0bf-84b4-4734-b6f3-25242a12a154"}');

-- [transaction_stmt] 2026-03-19T17:16:02.221073Z
INSERT OR REPLACE INTO block ("id", "content", "updated_at", "created_at", "document_id", "parent_id", "content_type", "properties") VALUES ('block:9d85cad6-1e74-499a-8d8e-899c5553c3d6', 'Flow depth tracking with peripheral awareness alerts', 1773940562173, 1773940562164, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'text', '{"ID":"9d85cad6-1e74-499a-8d8e-899c5553c3d6","sequence":187}');

-- [transaction_stmt] 2026-03-19T17:16:02.221268Z
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "updated_at", "document_id", "content_type", "id", "properties") VALUES (1773940562164, 'Quick task suggestions during breaks (2-minute rule)', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:adc7803b-9318-4ca5-877b-83f213445aba', '{"ID":"adc7803b-9318-4ca5-877b-83f213445aba","sequence":188}');

-- [transaction_stmt] 2026-03-19T17:16:02.221467Z
INSERT OR REPLACE INTO block ("parent_id", "content", "created_at", "updated_at", "id", "content_type", "document_id", "properties") VALUES ('block:c74fcc72-883d-4788-911a-0632f6145e4d', 'Three Modes [/]', 1773940562164, 1773940562173, 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"b5771daa-0208-43fe-a890-ef1fcebf5f2f","sequence":189}');

-- [transaction_stmt] 2026-03-19T17:16:02.221655Z
INSERT OR REPLACE INTO block ("document_id", "id", "parent_id", "updated_at", "created_at", "content_type", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:be15792f-21f3-476f-8b5f-e2e6b478b864', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 1773940562173, 1773940562164, 'text', 'Orient mode (Watcher Dashboard, daily/weekly review)', '{"sequence":190,"ID":"be15792f-21f3-476f-8b5f-e2e6b478b864"}');

-- [transaction_stmt] 2026-03-19T17:16:02.222377Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "content_type", "id", "updated_at", "document_id", "content", "properties") VALUES ('block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 1773940562164, 'text', 'block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Flow mode (single task focus, context on demand)', '{"sequence":191,"ID":"c68e8d5a-3f4b-4e8c-a887-2341e9b98bde"}');

-- [transaction_stmt] 2026-03-19T17:16:02.222585Z
INSERT OR REPLACE INTO block ("content", "content_type", "document_id", "updated_at", "parent_id", "id", "created_at", "properties") VALUES ('Capture mode (global hotkey, quick input overlay)', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963', 1773940562164, '{"sequence":192,"ID":"b1b2db9a-fc0d-4f51-98ae-9c5ab056a963"}');

-- [transaction_stmt] 2026-03-19T17:16:02.223305Z
INSERT OR REPLACE INTO block ("content", "created_at", "updated_at", "content_type", "parent_id", "id", "document_id", "properties") VALUES ('Review Workflows [/]', 1773940562164, 1773940562173, 'text', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"a3e31c87-d10b-432e-987c-0371e730f753","sequence":193}');

-- [transaction_stmt] 2026-03-19T17:16:02.223499Z
INSERT OR REPLACE INTO block ("id", "document_id", "created_at", "parent_id", "updated_at", "content_type", "content", "properties") VALUES ('block:4c020c67-1726-46d8-92e3-b9e0dbc90b62', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562164, 'block:a3e31c87-d10b-432e-987c-0371e730f753', 1773940562173, 'text', 'Daily orientation ("What does today look like?")', '{"ID":"4c020c67-1726-46d8-92e3-b9e0dbc90b62","sequence":194}');

-- [transaction_stmt] 2026-03-19T17:16:02.223701Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "created_at", "content_type", "id", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:a3e31c87-d10b-432e-987c-0371e730f753', 1773940562164, 'text', 'block:0906f769-52eb-47a2-917a-f9b57b7e80d1', 'Inbox zero workflow', '{"sequence":195,"ID":"0906f769-52eb-47a2-917a-f9b57b7e80d1"}');

-- [transaction_stmt] 2026-03-19T17:16:02.224383Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "document_id", "parent_id", "content", "id", "content_type", "properties") VALUES (1773940562173, 1773940562164, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'Weekly review (comprehensive synthesis)', 'block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f', 'text', '{"sequence":196,"ID":"091e7648-5314-4b4d-8e9c-bd7e0b8efc6f"}');

-- [transaction_stmt] 2026-03-19T17:16:02.224574Z
INSERT OR REPLACE INTO block ("id", "content", "content_type", "parent_id", "created_at", "updated_at", "document_id", "properties") VALUES ('block:240acff4-cf06-445e-99ee-42040da1bb84', 'Context Bundles in Flow [/]', 'text', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 1773940562165, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":197,"ID":"240acff4-cf06-445e-99ee-42040da1bb84"}');

-- [transaction_stmt] 2026-03-19T17:16:02.225288Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content_type", "created_at", "content", "updated_at", "id", "properties") VALUES ('block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562165, 'Hide distractions, show progress', 1773940562173, 'block:90702048-5baf-4732-96fb-ddae16824257', '{"ID":"90702048-5baf-4732-96fb-ddae16824257","sequence":198}');

-- [transaction_stmt] 2026-03-19T17:16:02.225483Z
INSERT OR REPLACE INTO block ("updated_at", "content", "content_type", "document_id", "parent_id", "id", "created_at", "properties") VALUES (1773940562173, 'Slide-in context panel from edge', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130', 1773940562165, '{"ID":"e4aeb8f0-4c63-48f6-b745-92a89cfd4130","sequence":199}');

-- [transaction_stmt] 2026-03-19T17:16:02.225682Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content_type", "id", "updated_at", "content", "created_at", "properties") VALUES ('block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:3907168e-eaf8-48ee-8ccc-6dfef069371e', 1773940562173, 'Assemble all related items for focused task', 1773940562165, '{"sequence":200,"ID":"3907168e-eaf8-48ee-8ccc-6dfef069371e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.225878Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "id", "document_id", "created_at", "content", "parent_id", "properties") VALUES (1773940562173, 'text', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562165, 'Progressive Concealment [/]', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', '{"sequence":201,"ID":"e233124d-8711-4dd4-8153-c884f889bc07"}');

-- [transaction_stmt] 2026-03-19T17:16:02.226606Z
INSERT OR REPLACE INTO block ("content", "content_type", "updated_at", "parent_id", "id", "created_at", "document_id", "properties") VALUES ('Peripheral element dimming during sustained typing', 'text', 1773940562173, 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'block:70485255-a2be-4356-bb9e-967270878b7e', 1773940562165, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":202,"ID":"70485255-a2be-4356-bb9e-967270878b7e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.226800Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "document_id", "id", "parent_id", "created_at", "content", "properties") VALUES ('text', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 1773940562165, 'Focused block emphasis, surrounding content fades', '{"ID":"ea7f8d72-f963-4a51-ab4f-d10f981eafcc","sequence":203}');

-- [transaction_stmt] 2026-03-19T17:16:02.227008Z
INSERT OR REPLACE INTO block ("document_id", "content", "content_type", "parent_id", "created_at", "updated_at", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Automatic visibility restore on cursor movement', 'text', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 1773940562165, 1773940562173, 'block:30a71e2f-f070-4745-947d-c443a86a7149', '{"sequence":204,"ID":"30a71e2f-f070-4745-947d-c443a86a7149"}');

-- [transaction_stmt] 2026-03-19T17:16:02.227201Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "content", "document_id", "created_at", "id", "parent_id", "properties") VALUES (1773940562173, 'text', 'Phase 7: Team Features [/]
Goal: Teams leverage individual excellence', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562165, 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":205,"ID":"4c647dfe-0639-4064-8ab6-491d57c7e367"}');

-- [transaction_stmt] 2026-03-19T17:16:02.227926Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "content", "id", "document_id", "updated_at", "content_type", "properties") VALUES ('block:4c647dfe-0639-4064-8ab6-491d57c7e367', 1773940562165, 'Delegation System [/]', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'text', '{"sequence":206,"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede"}');

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.229082Z
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "id", "updated_at", "document_id", "content_type", "properties") VALUES (1773940562165, '@[[Person]]: syntax for delegation sub-nets', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"sequence":207,"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264"}');

-- [transaction_stmt] 2026-03-19T17:16:02.229798Z
INSERT OR REPLACE INTO block ("parent_id", "id", "updated_at", "document_id", "content", "content_type", "created_at", "properties") VALUES ('block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'block:fbbce845-023e-438b-963e-471833c51505', 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Waiting-for tracking (automatic from delegation patterns)', 'text', 1773940562166, '{"ID":"fbbce845-023e-438b-963e-471833c51505","sequence":208}');

-- [transaction_stmt] 2026-03-19T17:16:02.230000Z
INSERT OR REPLACE INTO block ("updated_at", "content", "parent_id", "id", "created_at", "document_id", "content_type", "properties") VALUES (1773940562173, 'Delegation status sync with external systems', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0', 1773940562166, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"sequence":209,"ID":"25e19c99-63c2-4edb-8fb1-deb1daf4baf0"}');

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.231831Z
INSERT OR REPLACE INTO block ("id", "content", "updated_at", "parent_id", "created_at", "content_type", "document_id", "properties") VALUES ('block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc', '@anyone: team pool transitions', 1773940562173, 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 1773940562166, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":210,"ID":"938f03b8-6129-4eda-9c5f-31a76ad8b8dc"}');

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.233025Z
INSERT OR REPLACE INTO block ("updated_at", "content", "content_type", "created_at", "document_id", "id", "parent_id", "properties") VALUES (1773940562173, 'Sharing & Collaboration [/]', 'text', 1773940562166, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', '{"ID":"5bdf3ba6-f617-4bc1-93c2-15d84d925e01","sequence":211}');

-- [transaction_stmt] 2026-03-19T17:16:02.233733Z
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "updated_at", "content", "document_id", "content_type", "properties") VALUES ('block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'block:88b467b1-5a46-4b64-acb3-fcf9f377030e', 1773940562166, 1773940562173, 'Collaborative editing', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', '{"ID":"88b467b1-5a46-4b64-acb3-fcf9f377030e","sequence":212}');

-- [transaction_stmt] 2026-03-19T17:16:02.234489Z
INSERT OR REPLACE INTO block ("content", "id", "updated_at", "content_type", "created_at", "parent_id", "document_id", "properties") VALUES ('Shared views and dashboards', 'block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da', 1773940562173, 'text', 1773940562166, 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":213,"ID":"f3ce62cd-5817-4a7c-81f6-7a7077aff7da"}');

-- [transaction_stmt] 2026-03-19T17:16:02.234684Z
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "content_type", "document_id", "updated_at", "id", "properties") VALUES (1773940562166, 'Read-only sharing for documentation', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:135c74b1-8341-4719-b5d1-492eb26e2189', '{"sequence":214,"ID":"135c74b1-8341-4719-b5d1-492eb26e2189"}');

-- [transaction_stmt] 2026-03-19T17:16:02.234950Z
INSERT OR REPLACE INTO block ("document_id", "id", "content_type", "content", "parent_id", "updated_at", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'text', 'Competition analysis', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 1773940562173, 1773940562166, '{"sequence":215,"ID":"e0f90f1e-5468-4229-9b6d-438b31f09ed6"}');

-- [transaction_stmt] 2026-03-19T17:16:02.235160Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "content_type", "document_id", "id", "created_at", "content", "properties") VALUES ('block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 1773940562173, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:ceb203d0-0b59-4aa0-a840-2e4763234112', 1773940562166, 'https://github.com/3xpyth0n/ideon
Organize repositories, notes, links and more on a shared infinite canvas.', '{"ID":"ceb203d0-0b59-4aa0-a840-2e4763234112","sequence":216}');

-- [transaction_stmt] 2026-03-19T17:16:02.235373Z
INSERT OR REPLACE INTO block ("id", "document_id", "content", "content_type", "created_at", "updated_at", "parent_id", "properties") VALUES ('block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-Cutting Concerns [/]', 'text', 1773940562166, 1773940562173, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":217,"ID":"f407a7ec-c924-4a38-96e0-7e73472e7353"}');

-- [transaction_stmt] 2026-03-19T17:16:02.235567Z
INSERT OR REPLACE INTO block ("id", "parent_id", "updated_at", "content_type", "content", "created_at", "document_id", "properties") VALUES ('block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773940562173, 'text', 'Privacy & Security [/]', 1773940562166, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":218,"ID":"ad1d8307-134f-4a34-b58e-07d6195b2466"}');

-- [transaction_stmt] 2026-03-19T17:16:02.235766Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "updated_at", "id", "content", "parent_id", "created_at", "properties") VALUES ('text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562173, 'block:717db234-61eb-41ef-a8bf-b67e870f9aa6', 'Plugin sandboxing (WASM)', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773940562166, '{"sequence":219,"ID":"717db234-61eb-41ef-a8bf-b67e870f9aa6"}');

-- [transaction_stmt] 2026-03-19T17:16:02.235952Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "content_type", "updated_at", "document_id", "content", "properties") VALUES ('block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773940562167, 'block:75604518-b736-4653-a2a3-941215e798c7', 'text', 1773940562174, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Self-hosted LLM option (Ollama/vLLM)', '{"sequence":220,"ID":"75604518-b736-4653-a2a3-941215e798c7"}');

-- [transaction_stmt] 2026-03-19T17:16:02.236142Z
INSERT OR REPLACE INTO block ("document_id", "content", "created_at", "content_type", "updated_at", "id", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Optional cloud LLM with explicit consent', 1773940562167, 'text', 1773940562174, 'block:bfaedc82-3bc7-4b16-8314-273721ea997f', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', '{"ID":"bfaedc82-3bc7-4b16-8314-273721ea997f","sequence":221}');

-- [transaction_stmt] 2026-03-19T17:16:02.236322Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "updated_at", "id", "document_id", "content", "created_at", "properties") VALUES ('text', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773940562174, 'block:4b96f182-61e5-4f0e-861d-1a7d2413abe7', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Local-first by default (all data on device)', 1773940562167, '{"sequence":222,"ID":"4b96f182-61e5-4f0e-861d-1a7d2413abe7"}');

-- [transaction_stmt] 2026-03-19T17:16:02.236508Z
INSERT OR REPLACE INTO block ("id", "content_type", "document_id", "content", "created_at", "updated_at", "parent_id", "properties") VALUES ('block:eac105ca-efda-4976-9856-6c39a9b1502e', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Petri-Net Advanced [/]', 1773940562167, 1773940562174, 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', '{"sequence":223,"ID":"eac105ca-efda-4976-9856-6c39a9b1502e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.236689Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "id", "content", "document_id", "updated_at", "created_at", "properties") VALUES ('block:eac105ca-efda-4976-9856-6c39a9b1502e', 'text', 'block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59', 'SOP extraction from repeated interaction patterns', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562174, 1773940562167, '{"ID":"0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59","sequence":224}');

-- [transaction_stmt] 2026-03-19T17:16:02.236871Z
INSERT OR REPLACE INTO block ("content_type", "id", "created_at", "document_id", "content", "updated_at", "parent_id", "properties") VALUES ('text', 'block:143d071e-2b90-4f93-98d3-7aa5d3a14933', 1773940562167, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Delegation sub-nets (waiting_for pattern)', 1773940562174, 'block:eac105ca-efda-4976-9856-6c39a9b1502e', '{"sequence":225,"ID":"143d071e-2b90-4f93-98d3-7aa5d3a14933"}');

-- [transaction_stmt] 2026-03-19T17:16:02.237056Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "content_type", "content", "parent_id", "id", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562174, 'text', 'Token type hierarchy with mixins', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'block:cc499de0-f953-4f41-b795-0864b366d8ab', 1773940562167, '{"sequence":226,"ID":"cc499de0-f953-4f41-b795-0864b366d8ab"}');

-- [transaction_stmt] 2026-03-19T17:16:02.237238Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content", "document_id", "content_type", "id", "parent_id", "properties") VALUES (1773940562174, 1773940562167, 'Projections as views on flat net (Kanban, SOP, pipeline)', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', '{"sequence":227,"ID":"bd99d866-66ed-4474-8a4d-7ac1c1b08fbb"}');

-- [transaction_stmt] 2026-03-19T17:16:02.237422Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "updated_at", "content_type", "content", "id", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562167, 1773940562174, 'text', 'Question/Information tokens with confidence tracking', 'block:4041eb2e-23a6-4fea-9a69-0c152a6311e8', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', '{"sequence":228,"ID":"4041eb2e-23a6-4fea-9a69-0c152a6311e8"}');

-- [transaction_stmt] 2026-03-19T17:16:02.237608Z
INSERT OR REPLACE INTO block ("id", "created_at", "content_type", "updated_at", "parent_id", "document_id", "content", "properties") VALUES ('block:1e1027d2-4c0f-4975-ba59-c3c601d1f661', 1773940562167, 'text', 1773940562174, 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Simulation engine (fork marking, compare scenarios)', '{"sequence":229,"ID":"1e1027d2-4c0f-4975-ba59-c3c601d1f661"}');

-- [transaction_stmt] 2026-03-19T17:16:02.237792Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "updated_at", "document_id", "id", "content", "parent_id", "properties") VALUES ('text', 1773940562167, 1773940562174, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:a80f6d58-c876-48f5-8bfe-69390a8f9bde', 'Browser plugin for web app Digital Twins', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', '{"ID":"a80f6d58-c876-48f5-8bfe-69390a8f9bde","sequence":230}');

-- [transaction_stmt] 2026-03-19T17:16:02.238006Z
INSERT OR REPLACE INTO block ("id", "updated_at", "content", "content_type", "parent_id", "document_id", "created_at", "properties") VALUES ('block:723a51a9-3861-429c-bb10-f73c01f8463d', 1773940562174, 'PRQL Automation [/]', 'text', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562168, '{"ID":"723a51a9-3861-429c-bb10-f73c01f8463d","sequence":231}');

-- [transaction_stmt] 2026-03-19T17:16:02.238195Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "document_id", "content_type", "id", "content", "created_at", "properties") VALUES ('block:723a51a9-3861-429c-bb10-f73c01f8463d', 1773940562174, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7', 'Cross-system status propagation rules', 1773940562168, '{"ID":"e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7","sequence":232}');

-- [transaction_stmt] 2026-03-19T17:16:02.238379Z
INSERT OR REPLACE INTO block ("id", "content_type", "created_at", "document_id", "content", "parent_id", "updated_at", "properties") VALUES ('block:c1338a15-080b-4dba-bbdc-87b6b8467f28', 'text', 1773940562168, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Auto-tag blocks based on content analysis', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 1773940562174, '{"ID":"c1338a15-080b-4dba-bbdc-87b6b8467f28","sequence":233}');

-- [transaction_stmt] 2026-03-19T17:16:02.238563Z
INSERT OR REPLACE INTO block ("id", "parent_id", "updated_at", "content", "document_id", "content_type", "created_at", "properties") VALUES ('block:5707965a-6578-443c-aeff-bf40170edea9', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 1773940562174, 'PRQL-based automation rules (query → action)', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562168, '{"ID":"5707965a-6578-443c-aeff-bf40170edea9","sequence":234}');

-- [transaction_stmt] 2026-03-19T17:16:02.238750Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "created_at", "content_type", "updated_at", "id", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773940562168, 'text', 1773940562174, 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'Platform Support [/]', '{"sequence":235,"ID":"8e2b4ddd-e428-4950-bc41-76ee8a0e27ce"}');

-- [transaction_stmt] 2026-03-19T17:16:02.238935Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "id", "document_id", "created_at", "content", "updated_at", "properties") VALUES ('block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'text', 'block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562168, 'Android mobile', 1773940562174, '{"sequence":236,"ID":"4c4ff372-c3b9-44e6-9d46-33b7a4e7882e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.239129Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "document_id", "content", "updated_at", "content_type", "properties") VALUES ('block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857', 1773940562168, 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'WASM compatibility (MaybeSendSync trait)', 1773940562174, 'text', '{"sequence":237,"ID":"e5b9db2d-f39a-439d-99f8-b4e7c4ff6857"}');

-- [transaction_stmt] 2026-03-19T17:16:02.239310Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "updated_at", "content", "id", "parent_id", "document_id", "properties") VALUES ('text', 1773940562168, 1773940562174, 'Windows desktop', 'block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"d61290d4-e1f6-41e7-89e0-a7ed7a6662db","sequence":238}');

-- [transaction_stmt] 2026-03-19T17:16:02.239508Z
INSERT OR REPLACE INTO block ("id", "parent_id", "created_at", "content_type", "document_id", "content", "updated_at", "properties") VALUES ('block:1e729eef-3fff-43cb-8d13-499a8a8d4203', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 1773940562168, 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'iOS mobile', 1773940562174, '{"sequence":239,"ID":"1e729eef-3fff-43cb-8d13-499a8a8d4203"}');

-- [transaction_stmt] 2026-03-19T17:16:02.239686Z
INSERT OR REPLACE INTO block ("created_at", "id", "content", "updated_at", "content_type", "parent_id", "document_id", "properties") VALUES (1773940562168, 'block:500b7aae-5c3b-4dd5-a3c8-373fe746990b', 'Linux desktop', 1773940562174, 'text', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"500b7aae-5c3b-4dd5-a3c8-373fe746990b","sequence":240}');

-- [transaction_stmt] 2026-03-19T17:16:02.240362Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "id", "content_type", "content", "parent_id", "created_at", "properties") VALUES (1773940562174, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:a79ab251-4685-4728-b98b-0a652774f06c', 'text', 'macOS desktop (Flutter)', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 1773940562168, '{"ID":"a79ab251-4685-4728-b98b-0a652774f06c","sequence":241}');

-- [transaction_stmt] 2026-03-19T17:16:02.240561Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content", "updated_at", "id", "content_type", "created_at", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'UI/UX Design System [/]', 1773940562174, 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'text', 1773940562169, '{"sequence":242,"ID":"ac137431-daf6-4741-9808-6dc71c13e7c6"}');

-- [transaction_stmt] 2026-03-19T17:16:02.241253Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "id", "parent_id", "content", "content_type", "document_id", "properties") VALUES (1773940562174, 1773940562169, 'block:a85de368-9546-446d-ad61-17b72c7dbc3e', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'Which-Key navigation system (Space → mnemonic keys)', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"a85de368-9546-446d-ad61-17b72c7dbc3e","sequence":243}');

-- [transaction_stmt] 2026-03-19T17:16:02.241463Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "content", "id", "created_at", "content_type", "parent_id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562174, 'Micro-interactions (checkbox animation, smooth reorder)', 'block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9', 1773940562169, 'text', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', '{"sequence":244,"ID":"1cea6bd3-680f-46c3-bdbc-5989da5ed7d9"}');

-- [transaction_stmt] 2026-03-19T17:16:02.241645Z
INSERT OR REPLACE INTO block ("id", "parent_id", "document_id", "content_type", "updated_at", "created_at", "content", "properties") VALUES ('block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 1773940562174, 1773940562169, 'Light and dark themes', '{"sequence":245,"ID":"d1fbee2c-3a11-4adc-a3db-fd93f5b117e3"}');

-- [transaction_stmt] 2026-03-19T17:16:02.241852Z
INSERT OR REPLACE INTO block ("content", "id", "document_id", "created_at", "updated_at", "content_type", "parent_id", "properties") VALUES ('Color palette (warm, professional, calm technology)', 'block:beeec959-ba87-4c57-9531-c1d7f24d2b2c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562169, 1773940562174, 'text', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', '{"ID":"beeec959-ba87-4c57-9531-c1d7f24d2b2c","sequence":246}');

-- [transaction_stmt] 2026-03-19T17:16:02.242042Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "created_at", "content_type", "content", "parent_id", "id", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562174, 1773940562169, 'text', 'Typography system (Inter + JetBrains Mono)', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'block:d36014da-518a-4da5-b360-218d027ee104', '{"sequence":247,"ID":"d36014da-518a-4da5-b360-218d027ee104"}');

-- [transaction_stmt] 2026-03-19T17:16:02.242226Z
INSERT OR REPLACE INTO block ("id", "content_type", "parent_id", "content", "document_id", "updated_at", "created_at", "properties") VALUES ('block:01806047-9cf8-42fe-8391-6d608bfade9e', 'text', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'LogSeq replacement', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562174, 1773940562169, '{"sequence":248,"ID":"01806047-9cf8-42fe-8391-6d608bfade9e"}');

-- [transaction_stmt] 2026-03-19T17:16:02.242412Z
INSERT OR REPLACE INTO block ("content", "document_id", "content_type", "parent_id", "created_at", "id", "updated_at", "properties") VALUES ('Editing experience', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 1773940562169, 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 1773940562174, '{"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","sequence":249}');

-- [transaction_stmt] 2026-03-19T17:16:02.243120Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "id", "parent_id", "document_id", "content_type", "content", "properties") VALUES (1773940562169, 1773940562174, 'block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'GitHub Flavored Markdown parser & renderer for GPUI
https://github.com/joris-gallot/gpui-gfm', '{"sequence":250,"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2"}');

-- [transaction_stmt] 2026-03-19T17:16:02.243324Z
INSERT OR REPLACE INTO block ("content", "content_type", "updated_at", "document_id", "parent_id", "created_at", "id", "properties") VALUES ('Desktop Markdown viewer built with Rust and GPUI
https://github.com/chunghha/markdown_viewer', 'text', 1773940562174, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 1773940562169, 'block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', '{"sequence":251,"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8"}');

-- [transaction_stmt] 2026-03-19T17:16:02.243524Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content_type", "id", "content", "parent_id", "document_id", "properties") VALUES (1773940562174, 1773940562169, 'text', 'block:f7730a68-6268-4e65-ac93-3fdf79e92133', 'Markdown Editor and Viewer
https://github.com/kumarUjjawal/aster', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133","sequence":252}');

-- [transaction_stmt] 2026-03-19T17:16:02.243713Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "id", "created_at", "parent_id", "updated_at", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 1773940562170, 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 1773940562174, 'PDF Viewer & Annotator', '{"ID":"8594ab7c-5f36-44cf-8f92-248b31508441","sequence":253}');

-- [transaction_stmt] 2026-03-19T17:16:02.243917Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content_type", "updated_at", "id", "created_at", "content", "properties") VALUES ('doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'text', 1773940562174, 'block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 1773940562170, 'Combining gpui and hayro for a little application that render pdfs
https://github.com/vincenthz/gpui-hayro?tab=readme-ov-file', '{"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c","sequence":254}');

-- [transaction_stmt] 2026-03-19T17:16:02.244572Z
INSERT OR REPLACE INTO block ("content", "updated_at", "created_at", "parent_id", "document_id", "content_type", "id", "properties") VALUES ('Libera Reader
Modern, performance-oriented desktop e-book reader built with Rust and GPUI.
https://github.com/RikaKit2/libera-reader', 1773940562174, 1773940562170, 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'text', 'block:b95a19a6-5448-42f0-af06-177e95e27f49', '{"sequence":255,"ID":"b95a19a6-5448-42f0-af06-177e95e27f49"}');

-- [transaction_stmt] 2026-03-19T17:16:02.245257Z
INSERT OR REPLACE INTO block ("content_type", "id", "parent_id", "created_at", "content", "updated_at", "document_id", "properties") VALUES ('text', 'block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 1773940562170, 'Monica: On-screen anotation software
https://github.com/tasuren/monica', 1773940562174, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '{"sequence":256,"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad"}');

-- [transaction_stmt] 2026-03-19T17:16:02.245475Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "updated_at", "content", "content_type", "parent_id", "id", "properties") VALUES (1773940562170, 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 1773940562174, 'Graph vis', 'text', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', '{"sequence":257,"ID":"419b2df8-0121-4532-8dcd-21f04df806d8"}');

-- [transaction_stmt] 2026-03-19T17:16:02.245660Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "created_at", "content", "id", "document_id", "parent_id", "properties") VALUES ('text', 1773940562174, 1773940562170, 'https://github.com/jerlendds/gpug', 'block:f520a9ff-71bf-4a72-8777-9864bad7c535', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', '{"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535","sequence":258}');

-- [actor_tx_commit] 2026-03-19T17:16:02.245853Z
COMMIT;

-- Wait 212ms
-- [actor_tx_begin] 2026-03-19T17:16:02.458828Z
BEGIN TRANSACTION (10 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.458871Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-history-root', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'Claude Code History', 'text', NULL, NULL, '{"sequence":0,"ID":"cc-history-root"}', 1773940562117, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.459517Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-projects', 'block:cc-history-root', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'Projects', 'text', NULL, NULL, '{"ID":"cc-projects","sequence":1}', 1773940562117, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.459887Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-projects::src::0', 'block:cc-projects', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'from cc_project\nselect {id, original_path, session_count, last_activity}\nsort {-last_activity}\n', 'source', 'holon_prql', NULL, '{"ID":"block:cc-projects::src::0","sequence":2}', 1773940562117, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.460247Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-projects::render::0', 'block:cc-projects', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'list(#{item_template: row(text(col("original_path")), spacer(16), text(col("session_count")), spacer(8), text(col("last_activity")))})\n', 'source', 'render', NULL, '{"ID":"block:cc-projects::render::0","sequence":3}', 1773940562117, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.460608Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-sessions', 'block:cc-history-root', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'Recent Sessions', 'text', NULL, NULL, '{"sequence":4,"ID":"cc-sessions"}', 1773940562118, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.460977Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-sessions::src::0', 'block:cc-sessions', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'from cc_session\nfilter message_count > 0\nselect {id, first_prompt, message_count, model, modified, git_branch}\nsort {-modified}\ntake 30\n', 'source', 'holon_prql', NULL, '{"sequence":5,"ID":"block:cc-sessions::src::0"}', 1773940562118, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.461457Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-sessions::render::0', 'block:cc-sessions', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'list(#{item_template: row(text(col("first_prompt")), spacer(16), text(col("message_count")), spacer(8), text(col("modified")))})\n', 'source', 'render', NULL, '{"ID":"block:cc-sessions::render::0","sequence":6}', 1773940562118, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.461814Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-tasks', 'block:cc-history-root', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'Tasks', 'text', NULL, NULL, '{"ID":"cc-tasks","sequence":7}', 1773940562118, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.462161Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-tasks::src::0', 'block:cc-tasks', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'from cc_task\nfilter status == "in_progress"\nselect {id, subject, status, created_at}\nsort {-created_at}\n', 'source', 'holon_prql', NULL, '{"ID":"block:cc-tasks::src::0","sequence":8}', 1773940562118, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.462519Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-tasks::render::0', 'block:cc-tasks', 'doc:f753ea35-2fb1-4a73-90b5-04d65940a091', 'list(#{item_template: row(text(col("status")), spacer(8), text(col("subject")))})\n', 'source', 'render', NULL, '{"sequence":9,"ID":"block:cc-tasks::render::0"}', 1773940562118, 1773940562121, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [actor_tx_commit] 2026-03-19T17:16:02.462867Z
COMMIT;

-- Wait 12ms
-- [actor_exec] 2026-03-19T17:16:02.475154Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_tx_begin] 2026-03-19T17:16:02.476143Z
BEGIN TRANSACTION (259 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.476167Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YVJVMX10Q8T6YFRTY', 'block.created', 'block', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'sql', 'confirmed', '{"data":{"parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"created_at":1773940562147,"content":"Phase 1: Core Outliner","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","properties":{"ID":"599b60af-960d-4c9c-b222-d3d9de95c513","sequence":0}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.476522Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YTMSSX8JBKBNTT95G', 'block.created', 'block', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'sql', 'confirmed', '{"data":{"id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562147,"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content":"MCP Server Frontend [/]","content_type":"text","properties":{"task_state":"DOING","sequence":1,"ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.476825Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y4536JGEDJ4VKGDAG', 'block.created', 'block', 'block:db59d038-8a47-43e9-9502-0472b493a6b9', 'sql', 'confirmed', '{"data":{"parent_id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Context parameter support ($context_id, $context_parent_id)","content_type":"text","updated_at":1773940562172,"id":"block:db59d038-8a47-43e9-9502-0472b493a6b9","created_at":1773940562147,"properties":{"ID":"db59d038-8a47-43e9-9502-0472b493a6b9","sequence":2}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.477708Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YG9MGS0T7J8G1PQBV', 'block.created', 'block', 'block:95ad6166-c03c-4417-a435-349e88b8e90a', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","content":"MCP server (stdio + HTTP modes)","created_at":1773940562147,"content_type":"text","id":"block:95ad6166-c03c-4417-a435-349e88b8e90a","updated_at":1773940562172,"properties":{"ID":"95ad6166-c03c-4417-a435-349e88b8e90a","sequence":3}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.478010Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YXV4FCNADNMK561FH', 'block.created', 'block', 'block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 'sql', 'confirmed', '{"data":{"id":"block:d365c9ef-c9aa-49ee-bd19-960c0e12669b","created_at":1773940562147,"content":"MCP tools for query execution and operations","parent_id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"sequence":4,"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.478308Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y5E3HHV8GVN1Y1CDZ', 'block.created', 'block', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"Block Operations [/]","id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","created_at":1773940562147,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","properties":{"sequence":5,"ID":"661368d9-e4bd-4722-b5c2-40f32006c643"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.479148Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y70AXEFC43SQG8X22', 'block.created', 'block', 'block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:346e7a61-62a5-4813-8fd1-5deea67d9007","content":"Block hierarchy (parent/child, indent/outdent)","created_at":1773940562147,"updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","properties":{"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007","sequence":6}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.479442Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YTM3MF5M10PJFR26M', 'block.created', 'block', 'block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'sql', 'confirmed', '{"data":{"id":"block:4fb5e908-31a0-47fb-8280-fe01cebada34","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562147,"content_type":"text","parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content":"Split block operation","updated_at":1773940562172,"properties":{"sequence":7,"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.479752Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YQM8MY3SWE0H522YD', 'block.created', 'block', 'block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'sql', 'confirmed', '{"data":{"created_at":1773940562147,"parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content":"Block CRUD (create, read, update, delete)","updated_at":1773940562172,"content_type":"text","id":"block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":8,"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.480049Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YRBW4PRG25M32R1MW', 'block.created', 'block', 'block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 'sql', 'confirmed', '{"data":{"id":"block:c3ad7889-3d40-4d07-88fb-adf569e50a63","created_at":1773940562148,"parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"content":"Block movement (move_up, move_down, move_block)","properties":{"sequence":9,"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.480342Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YJE7NJWWP8GW2788K', 'block.created', 'block', 'block:225edb45-f670-445a-9162-18c150210ee6', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content":"Undo/redo system (UndoStack + persistent OperationLogStore)","id":"block:225edb45-f670-445a-9162-18c150210ee6","updated_at":1773940562172,"content_type":"text","created_at":1773940562148,"properties":{"sequence":10,"task_state":"DONE","ID":"225edb45-f670-445a-9162-18c150210ee6"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.481179Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y4KW6Y8SFMJZ2QPBC', 'block.created', 'block', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"created_at":1773940562148,"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","content":"Storage & Data Layer [/]","properties":{"ID":"444b24f6-d412-43c4-a14b-6e725b673cee","sequence":11}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.481479Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YCHBW2V5X8EJD8016', 'block.created', 'block', 'block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 'sql', 'confirmed', '{"data":{"id":"block:c5007917-6723-49e2-95d4-c8bd3c7659ae","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Schema Module system with topological dependency ordering","content_type":"text","updated_at":1773940562172,"created_at":1773940562148,"properties":{"sequence":12,"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.481775Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y4KV5EZF33KKYQ0QV', 'block.created', 'block', 'block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'sql', 'confirmed', '{"data":{"created_at":1773940562148,"content":"Fractional indexing for block ordering","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:ecafcad8-15e9-4883-9f4a-79b9631b2699","updated_at":1773940562172,"content_type":"text","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","properties":{"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699","sequence":13}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.482627Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y2SH101FR74ZPYTTH', 'block.created', 'block', 'block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'sql', 'confirmed', '{"data":{"id":"block:1e0cf8f7-28e1-4748-a682-ce07be956b57","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562148,"updated_at":1773940562172,"parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","content":"Turso (embedded SQLite) backend with connection pooling","properties":{"sequence":14,"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.482934Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YMWG4F78SN71V6YF9', 'block.created', 'block', 'block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:eff0db85-3eb2-4c9b-ac02-3c2773193280","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562148,"updated_at":1773940562172,"content":"QueryableCache wrapping DataSource with local caching","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","properties":{"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280","sequence":15}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.483243Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y2BNE5G6Y2EG964W6', 'block.created', 'block', 'block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'sql', 'confirmed', '{"data":{"created_at":1773940562148,"updated_at":1773940562172,"id":"block:d4ae0e9f-d370-49e7-b777-bd8274305ad7","content_type":"text","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Entity derive macro (#[derive(Entity)]) for schema generation","properties":{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.484080Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YNZSEP478RE02E967', 'block.created', 'block', 'block:d318cae4-759d-487b-a909-81940223ecc1', 'sql', 'confirmed', '{"data":{"updated_at":1773940562172,"id":"block:d318cae4-759d-487b-a909-81940223ecc1","content":"CDC (Change Data Capture) streaming from storage to UI","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562148,"properties":{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.484387Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YC874BD6P3BW9E02Y', 'block.created', 'block', 'block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 'sql', 'confirmed', '{"data":{"content":"Command sourcing infrastructure (append-only operation log)","id":"block:d587e8d0-8e96-4b98-8a8f-f18f47e45222","updated_at":1773940562172,"content_type":"text","created_at":1773940562148,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","properties":{"sequence":18,"task_state":"DONE","ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.485248Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YQENFHT84EPM9NKYP', 'block.created', 'block', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'sql', 'confirmed', '{"data":{"id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","created_at":1773940562148,"updated_at":1773940562172,"content_type":"text","content":"Procedural Macros [/]","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","sequence":19}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.486076Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YKMB1V25SY216XDR5', 'block.created', 'block', 'block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 'sql', 'confirmed', '{"data":{"created_at":1773940562149,"parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","updated_at":1773940562172,"id":"block:b90a254f-145b-4e0d-96ca-ad6139f13ce4","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"#[operations_trait] macro for operation dispatch generation","properties":{"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4","sequence":20}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.486935Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YGE4WRXNY5FH3FH1X', 'block.created', 'block', 'block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', 'sql', 'confirmed', '{"data":{"content":"#[triggered_by(...)] for operation availability","created_at":1773940562149,"content_type":"text","id":"block:5657317c-dedf-4ae5-9db0-83bd3c92fc44","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","properties":{"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44","sequence":21}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.487780Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YPEAQB24DMBZVM98V', 'block.created', 'block', 'block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 'sql', 'confirmed', '{"data":{"created_at":1773940562149,"updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd","parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","content":"Type inference for OperationDescriptor parameters","properties":{"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd","sequence":22}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.488682Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YJQHB9QY9XVK7ASHX', 'block.created', 'block', 'block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', 'sql', 'confirmed', '{"data":{"content":"#[affects(...)] for field-level reactivity","parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","created_at":1773940562149,"content_type":"text","id":"block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2","updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2","sequence":23}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.488993Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YYSX1Z545JD8VDS2Z', 'block.created', 'block', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773940562149,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","updated_at":1773940562172,"content":"Performance [/]","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","properties":{"sequence":24,"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.489283Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YVC79DY07PXQ5MK2S', 'block.created', 'block', 'block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"id":"block:6463c700-3e8b-42a7-ae49-ce13520f8c73","content":"Virtual scrolling and lazy loading","content_type":"text","parent_id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","created_at":1773940562149,"properties":{"sequence":25,"task_state":"DOING","ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.489587Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y9PMHQHVZSF2WKJX0', 'block.created', 'block', 'block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', 'sql', 'confirmed', '{"data":{"created_at":1773940562149,"content_type":"text","parent_id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Connection pooling for Turso","updated_at":1773940562172,"id":"block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","properties":{"task_state":"DOING","sequence":26,"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.489898Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YKT0FX0ZPV3W2M002', 'block.created', 'block', 'block:e0567a06-5a62-4957-9457-c55a6661cee5', 'sql', 'confirmed', '{"data":{"content":"Full-text search indexing (Tantivy)","id":"block:e0567a06-5a62-4957-9457-c55a6661cee5","content_type":"text","created_at":1773940562149,"parent_id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"properties":{"ID":"e0567a06-5a62-4957-9457-c55a6661cee5","sequence":27}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.490192Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YK8R6EQE17DH70KXX', 'block.created', 'block', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'sql', 'confirmed', '{"data":{"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","updated_at":1773940562172,"content":"Cross-Device Sync [/]","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","content_type":"text","created_at":1773940562149,"properties":{"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","sequence":28}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.490494Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YZPYRD8SWHJGZHS7S', 'block.created', 'block', 'block:43f329da-cfb4-4764-b599-06f4b6272f91', 'sql', 'confirmed', '{"data":{"content":"CollaborativeDoc with ALPN routing","parent_id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","content_type":"text","created_at":1773940562149,"updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:43f329da-cfb4-4764-b599-06f4b6272f91","properties":{"sequence":29,"ID":"43f329da-cfb4-4764-b599-06f4b6272f91"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.490798Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YJ9FH2QTHAKCGVJKB', 'block.created', 'block', 'block:7aef40b2-14e1-4df0-a825-18603c55d198', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"content_type":"text","created_at":1773940562149,"id":"block:7aef40b2-14e1-4df0-a825-18603c55d198","content":"Offline-first with background sync","parent_id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","properties":{"ID":"7aef40b2-14e1-4df0-a825-18603c55d198","sequence":30}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.491122Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YWSEHPK2CZ4E88KKN', 'block.created', 'block', 'block:e148d7b7-c505-4201-83b7-36986a981a56', 'sql', 'confirmed', '{"data":{"updated_at":1773940562172,"parent_id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","created_at":1773940562150,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:e148d7b7-c505-4201-83b7-36986a981a56","content":"Iroh P2P transport for Loro documents","content_type":"text","properties":{"ID":"e148d7b7-c505-4201-83b7-36986a981a56","sequence":31}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.492105Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YE788VDW659QFTNYX', 'block.created', 'block', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'sql', 'confirmed', '{"data":{"created_at":1773940562150,"id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","content":"Dependency Injection [/]","updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","properties":{"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9","sequence":32}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.492910Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YNRKXWBX4HMZA3DEH', 'block.created', 'block', 'block:b980e51f-0c91-4708-9a17-3d41284974b2', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562172,"created_at":1773940562150,"id":"block:b980e51f-0c91-4708-9a17-3d41284974b2","content_type":"text","parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","content":"OperationDispatcher routing to providers","properties":{"ID":"b980e51f-0c91-4708-9a17-3d41284974b2","sequence":33}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.493229Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YQCDNAZJW66WRCN96', 'block.created', 'block', 'block:97cc8506-47d2-44cb-bdca-8e9a507953a0', 'sql', 'confirmed', '{"data":{"id":"block:97cc8506-47d2-44cb-bdca-8e9a507953a0","updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","content_type":"text","content":"BackendEngine as main orchestration point","created_at":1773940562150,"properties":{"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0","sequence":34}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.493535Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7YNAR3Q786NX9F8GBQ', 'block.created', 'block', 'block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'sql', 'confirmed', '{"data":{"content":"ferrous-di based service composition","updated_at":1773940562172,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:1c1f07b1-c801-47b2-8480-931cfb7930a8","parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","content_type":"text","created_at":1773940562150,"properties":{"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8","sequence":35}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.493849Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y71409QVAN1FTDDPY', 'block.created', 'block', 'block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', 'sql', 'confirmed', '{"data":{"updated_at":1773940562172,"id":"block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"SchemaRegistry with topological initialization","created_at":1773940562150,"parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","content_type":"text","properties":{"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47","sequence":36}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.494147Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y10FTMP6PH8HWV1W6', 'block.created', 'block', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'sql', 'confirmed', '{"data":{"content":"Query & Render Pipeline [/]","updated_at":1773940562172,"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","content_type":"text","created_at":1773940562150,"properties":{"sequence":37,"ID":"b489c622-6c87-4bf6-8d35-787eb732d670"}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.494465Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Y7KC058S249H4ENCW', 'block.created', 'block', 'block:1bbec456-7217-4477-a49c-0b8422e441e9', 'sql', 'confirmed', '{"data":{"id":"block:1bbec456-7217-4477-a49c-0b8422e441e9","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)","content_type":"text","updated_at":1773940562173,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","created_at":1773940562150,"properties":{"ID":"1bbec456-7217-4477-a49c-0b8422e441e9","sequence":38}}}', NULL, NULL, 1773940562174, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.495273Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZR38DFPEVPP1AEW0Y', 'block.created', 'block', 'block:2b1c341e-5da2-4207-a609-f4af6d7ceebd', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","id":"block:2b1c341e-5da2-4207-a609-f4af6d7ceebd","content_type":"text","created_at":1773940562150,"content":"Automatic operation wiring (lineage analysis → widget binding)","properties":{"ID":"2b1c341e-5da2-4207-a609-f4af6d7ceebd","task_state":"DOING","sequence":39}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.495578Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZJ4H6T19FNF5SHFPM', 'block.created', 'block', 'block:2d44d7df-5d7d-4cfe-9061-459c7578e334', 'sql', 'confirmed', '{"data":{"id":"block:2d44d7df-5d7d-4cfe-9061-459c7578e334","created_at":1773940562150,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","content":"GQL (graph query) support via EAV schema","updated_at":1773940562173,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":40,"ID":"2d44d7df-5d7d-4cfe-9061-459c7578e334","task_state":"DOING"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.495885Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZKZ497ANJE2MNCT5W', 'block.created', 'block', 'block:54ed1be5-765e-4884-87ab-02268e0208c7', 'sql', 'confirmed', '{"data":{"id":"block:54ed1be5-765e-4884-87ab-02268e0208c7","content":"PRQL compilation (PRQL → SQL + RenderSpec)","parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content_type":"text","created_at":1773940562150,"properties":{"ID":"54ed1be5-765e-4884-87ab-02268e0208c7","sequence":41}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.496185Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z0KNMSCQMYCTWXC09', 'block.created', 'block', 'block:5384c1da-f058-4321-8401-929b3570c2a5', 'sql', 'confirmed', '{"data":{"created_at":1773940562150,"updated_at":1773940562173,"content_type":"text","parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:5384c1da-f058-4321-8401-929b3570c2a5","content":"RenderSpec tree for declarative UI description","properties":{"sequence":42,"ID":"5384c1da-f058-4321-8401-929b3570c2a5"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.496998Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZFE4A6B5M4MTPJ9GG', 'block.created', 'block', 'block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc', 'sql', 'confirmed', '{"data":{"created_at":1773940562151,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","id":"block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"Unified execute_query supporting PRQL/GQL/SQL","properties":{"sequence":43,"ID":"fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","task_state":"DONE"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.497323Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZKEGVXTW5H1A8W7YM', 'block.created', 'block', 'block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 'sql', 'confirmed', '{"data":{"id":"block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","content":"SQL direct execution support","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562151,"content_type":"text","updated_at":1773940562173,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","properties":{"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","task_state":"DOING","sequence":44}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.497634Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z92YCY6G2ASQ4DS01', 'block.created', 'block', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'sql', 'confirmed', '{"data":{"created_at":1773940562151,"id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content":"Loro CRDT Integration [/]","content_type":"text","properties":{"sequence":45,"ID":"d9374dc3-05fc-40b2-896d-f88bb8a33c92"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.498667Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZY77T5T9XR3W66X7Q', 'block.created', 'block', 'block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6', 'sql', 'confirmed', '{"data":{"content":"LoroBackend implementing CoreOperations trait","updated_at":1773940562173,"created_at":1773940562151,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","properties":{"sequence":46,"ID":"b1dc3ad3-574b-472a-b74b-e3ea29a433e6"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.499499Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z7RHAB5PMV12JY3FD', 'block.created', 'block', 'block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773940562151,"updated_at":1773940562173,"parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","content":"LoroDocumentStore for managing CRDT documents on disk","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","properties":{"ID":"ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","task_state":"DOING","sequence":47}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.499809Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZZEFH2RWG9BRRREXY', 'block.created', 'block', 'block:35652c3f-720c-4e20-ab90-5e25e1429733', 'sql', 'confirmed', '{"data":{"created_at":1773940562151,"updated_at":1773940562173,"content":"LoroBlockOperations as OperationProvider routing writes through CRDT","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:35652c3f-720c-4e20-ab90-5e25e1429733","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","content_type":"text","properties":{"sequence":48,"ID":"35652c3f-720c-4e20-ab90-5e25e1429733"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.500626Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZF6YVXAA6R86PX54Y', 'block.created', 'block', 'block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2', 'sql', 'confirmed', '{"data":{"created_at":1773940562151,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2","content":"Cycle detection in move_block","updated_at":1773940562173,"parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","properties":{"sequence":49,"ID":"090731e3-38ae-4bf1-b5ec-dbb33eae4fb2"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.500937Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZQV8D8R2Q9NZ9VHZV', 'block.created', 'block', 'block:ddf208e4-9b73-422d-b8ab-4ec58b328907', 'sql', 'confirmed', '{"data":{"created_at":1773940562151,"id":"block:ddf208e4-9b73-422d-b8ab-4ec58b328907","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","content":"Loro-to-Turso materialization (CRDT → SQL cache → CDC)","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content_type":"text","properties":{"ID":"ddf208e4-9b73-422d-b8ab-4ec58b328907","sequence":50}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.501253Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZX98YVFGWWP7E9J9H', 'block.created', 'block', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'sql', 'confirmed', '{"data":{"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","updated_at":1773940562173,"content_type":"text","created_at":1773940562151,"content":"Org-Mode Sync [/]","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","sequence":51}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.502093Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZE7KAEE7J02P2RE13', 'block.created', 'block', 'block:7bc5f362-0bf9-45a1-b2b7-6882585ed169', 'sql', 'confirmed', '{"data":{"content":"OrgRenderer as single path for producing org text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","content_type":"text","created_at":1773940562151,"id":"block:7bc5f362-0bf9-45a1-b2b7-6882585ed169","updated_at":1773940562173,"properties":{"sequence":52,"ID":"7bc5f362-0bf9-45a1-b2b7-6882585ed169"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.503649Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZG735TCSFR4K7VNNE', 'block.created', 'block', 'block:8eab3453-25d2-4e7a-89f8-f9f79be939c9', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"id":"block:8eab3453-25d2-4e7a-89f8-f9f79be939c9","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562151,"content_type":"text","content":"Document identity & aliases (UUID ↔ file path mapping)","parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","properties":{"ID":"8eab3453-25d2-4e7a-89f8-f9f79be939c9","sequence":53}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.504450Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZK37C122ZCNMTZT68', 'block.created', 'block', 'block:fc60da1b-6065-4d36-8551-5479ff145df0', 'sql', 'confirmed', '{"data":{"parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","content":"OrgSyncController with echo suppression","id":"block:fc60da1b-6065-4d36-8551-5479ff145df0","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content_type":"text","created_at":1773940562152,"properties":{"sequence":54,"ID":"fc60da1b-6065-4d36-8551-5479ff145df0"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.505248Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z4X1TEBQRQP2V640G', 'block.created', 'block', 'block:6e5a1157-b477-45a1-892f-57807b4d969b', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"id":"block:6e5a1157-b477-45a1-892f-57807b4d969b","created_at":1773940562152,"parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Bidirectional sync (file changes ↔ block changes)","properties":{"sequence":55,"ID":"6e5a1157-b477-45a1-892f-57807b4d969b"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.506137Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZHX9X811QPJM9DXDM', 'block.created', 'block', 'block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f', 'sql', 'confirmed', '{"data":{"content":"Org file parsing (headlines, properties, source blocks)","created_at":1773940562152,"updated_at":1773940562173,"id":"block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f","parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":56,"ID":"6e4dab75-cd13-4c5e-9168-bf266d11aa3f"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.507096Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z9RH4T5YYGAA2TFE6', 'block.created', 'block', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'sql', 'confirmed', '{"data":{"id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Flutter Frontend [/]","content_type":"text","updated_at":1773940562173,"created_at":1773940562152,"properties":{"ID":"bb3bc716-ca9a-438a-936d-03631e2ee929","sequence":57}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.508135Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZT4RC1R4AK5MHZQ3F', 'block.created', 'block', 'block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"created_at":1773940562152,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"FFI bridge via flutter_rust_bridge","id":"block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","properties":{"sequence":58,"ID":"b4753cd8-47ea-4f7d-bd00-e1ec563aa43f"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.508447Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZDZVPTZF3NH9MY8MP', 'block.created', 'block', 'block:3289bc82-f8a9-4cad-8545-ad1fee9dc282', 'sql', 'confirmed', '{"data":{"id":"block:3289bc82-f8a9-4cad-8545-ad1fee9dc282","content_type":"text","created_at":1773940562152,"updated_at":1773940562173,"content":"Navigation system (history, cursor, focus)","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"task_state":"DOING","ID":"3289bc82-f8a9-4cad-8545-ad1fee9dc282","sequence":59}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.508749Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZKSYW3GNG0NZQF9YD', 'block.created', 'block', 'block:ebca0a24-f6f6-4c49-8a27-9d9973acf737', 'sql', 'confirmed', '{"data":{"created_at":1773940562152,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:ebca0a24-f6f6-4c49-8a27-9d9973acf737","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","content":"Block editor (outliner interactions)","updated_at":1773940562173,"content_type":"text","properties":{"sequence":60,"ID":"ebca0a24-f6f6-4c49-8a27-9d9973acf737"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.509057Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZVCC6SA29PC6VW1X6', 'block.created', 'block', 'block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd', 'sql', 'confirmed', '{"data":{"id":"block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","content":"Reactive UI updates from CDC change streams","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content_type":"text","created_at":1773940562152,"properties":{"sequence":61,"ID":"eb7e34f8-19f5-48f5-a22d-8f62493bafdd"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.509372Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z709MJAJHVFM577F1', 'block.created', 'block', 'block:7a0a4905-59c5-4277-8114-1e9ca9d425e3', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","id":"block:7a0a4905-59c5-4277-8114-1e9ca9d425e3","content":"Three-column layout (sidebar, main, right panel)","created_at":1773940562152,"updated_at":1773940562173,"properties":{"ID":"7a0a4905-59c5-4277-8114-1e9ca9d425e3","sequence":62}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.509685Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZJDSVVZTGPYCJ4PVQ', 'block.created', 'block', 'block:19d7b512-e5e0-469c-917b-eb27d7a38bed', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content_type":"text","created_at":1773940562152,"id":"block:19d7b512-e5e0-469c-917b-eb27d7a38bed","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","content":"Flutter desktop app shell","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"19d7b512-e5e0-469c-917b-eb27d7a38bed","sequence":63}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.510017Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZTWAVSK4K5JF7ZBEY', 'block.created', 'block', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","updated_at":1773940562173,"content":"Petri-Net Task Ranking (WSJF) [/]","id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562152,"properties":{"ID":"afe4f75c-7948-4d4c-9724-4bfab7d47d88","sequence":64}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.510330Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZJJC9FBABHYFZG32N', 'block.created', 'block', 'block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7', 'sql', 'confirmed', '{"data":{"content":"Prototype blocks with =computed Rhai expressions","parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","id":"block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562153,"content_type":"text","updated_at":1773940562173,"properties":{"sequence":65,"ID":"d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","task_state":"DOING"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.510635Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZYGBSCZVJ3HJGCFKG', 'block.created', 'block', 'block:2d399fd7-79d8-41f1-846b-31dabcec208a', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:2d399fd7-79d8-41f1-846b-31dabcec208a","created_at":1773940562153,"updated_at":1773940562173,"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Verb dictionary (~30 German + English verbs → transition types)","properties":{"ID":"2d399fd7-79d8-41f1-846b-31dabcec208a","sequence":66}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.510955Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZHC8F8Z4J71FNB64V', 'block.created', 'block', 'block:2385f4e3-25e1-4911-bf75-77cefd394206', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:2385f4e3-25e1-4911-bf75-77cefd394206","parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","content_type":"text","content":"rank_tasks() engine with tiebreak ordering","created_at":1773940562153,"properties":{"task_state":"DOING","sequence":67,"ID":"2385f4e3-25e1-4911-bf75-77cefd394206"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.511758Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZPCM7HTFWV05ND1ZQ', 'block.created', 'block', 'block:cae619f2-26fe-464e-b67a-0a04f76543c9', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content":"Block → Petri Net materialization (petri.rs)","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:cae619f2-26fe-464e-b67a-0a04f76543c9","created_at":1773940562153,"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","properties":{"sequence":68,"ID":"cae619f2-26fe-464e-b67a-0a04f76543c9","task_state":"DOING"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.512094Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZV3N0608SF99E3QC2', 'block.created', 'block', 'block:eaee1c9b-5466-428f-8dbb-f4882ccdb066', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","id":"block:eaee1c9b-5466-428f-8dbb-f4882ccdb066","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"Self Descriptor (person block with is_self: true)","created_at":1773940562153,"properties":{"sequence":69,"ID":"eaee1c9b-5466-428f-8dbb-f4882ccdb066","task_state":"DOING"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.512414Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZR2VW2JGWKYKQB5FP', 'block.created', 'block', 'block:023da362-ce5d-4a3b-827a-29e745d6f778', 'sql', 'confirmed', '{"data":{"id":"block:023da362-ce5d-4a3b-827a-29e745d6f778","created_at":1773940562153,"updated_at":1773940562173,"content":"WSJF scoring (priority_weight × urgency_weight + position_weight)","parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"task_state":"DOING","ID":"023da362-ce5d-4a3b-827a-29e745d6f778","sequence":70}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.512731Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z0HAC7XCH5916W6AD', 'block.created', 'block', 'block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"id":"block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Task syntax parser (@, ?, >, [[links]])","parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","created_at":1773940562153,"properties":{"sequence":71,"ID":"46a8c75e-8ab8-4a5a-b4af-a1388f6a4812"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.513048Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z6S72QGG01BJN4P7B', 'block.created', 'block', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'sql', 'confirmed', '{"data":{"parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Phase 2: First Integration (Todoist) [/]\\nGoal: Prove hybrid architecture","created_at":1773940562153,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","updated_at":1773940562173,"properties":{"sequence":72,"ID":"29c0aa5f-d9ca-46f3-8601-6023f87cefbd"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.513369Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z4D6KP8AA1G30VJRT', 'block.created', 'block', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"Reconciliation [/]","parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","created_at":1773940562153,"properties":{"sequence":73,"ID":"00fa0916-2681-4699-9554-44fcb8e2ea6a"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.513691Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZB8PVZ7MGWX72QQ37', 'block.created', 'block', 'block:632af903-5459-4d44-921a-43145e20dc82', 'sql', 'confirmed', '{"data":{"parent_id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","content_type":"text","created_at":1773940562153,"id":"block:632af903-5459-4d44-921a-43145e20dc82","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content":"Sync token management to prevent duplicate processing","properties":{"sequence":74,"ID":"632af903-5459-4d44-921a-43145e20dc82"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.514021Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZYF3ERCYSBCYMTNMZ', 'block.created', 'block', 'block:78f9d6e3-42d4-4975-910d-3728e23410b1', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"created_at":1773940562153,"content_type":"text","parent_id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:78f9d6e3-42d4-4975-910d-3728e23410b1","content":"Conflict detection and resolution UI","properties":{"sequence":75,"ID":"78f9d6e3-42d4-4975-910d-3728e23410b1"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.514836Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZMQDG5YFA3TK5TC53', 'block.created', 'block', 'block:fa2854d1-2751-4a07-8f83-70c2f9c6c190', 'sql', 'confirmed', '{"data":{"content":"Last-write-wins for concurrent edits","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:fa2854d1-2751-4a07-8f83-70c2f9c6c190","updated_at":1773940562173,"parent_id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","created_at":1773940562154,"properties":{"ID":"fa2854d1-2751-4a07-8f83-70c2f9c6c190","sequence":76}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.515220Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZQ1VADB365HQ4HN5K', 'block.created', 'block', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'sql', 'confirmed', '{"data":{"id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562154,"updated_at":1773940562173,"parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","content":"Operation Queue & Offline Support [/]","properties":{"sequence":77,"ID":"043ed925-6bf2-4db3-baf8-2277f1a5afaa"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.515571Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z1Z5GM70W2EZCG5Z1', 'block.created', 'block', 'block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3', 'sql', 'confirmed', '{"data":{"parent_id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562154,"id":"block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","updated_at":1773940562173,"content":"Offline operation queue with retry logic","content_type":"text","properties":{"ID":"5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","sequence":78}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.515898Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZR2RR2QYJWVFA0Q5J', 'block.created', 'block', 'block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05', 'sql', 'confirmed', '{"data":{"content":"Sync status indicators (synced, pending, conflict, error)","parent_id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05","created_at":1773940562154,"updated_at":1773940562173,"properties":{"ID":"7de8d37b-49ba-4ada-9b1e-df1c41c0db05","sequence":79}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.516644Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z3NHMEC77WQNTNRTF', 'block.created', 'block', 'block:302eb0c5-56fe-4980-8292-bae8a9a0450a', 'sql', 'confirmed', '{"data":{"created_at":1773940562154,"content":"Optimistic updates with ID mapping (internal ↔ external)","parent_id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","id":"block:302eb0c5-56fe-4980-8292-bae8a9a0450a","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"properties":{"ID":"302eb0c5-56fe-4980-8292-bae8a9a0450a","sequence":80}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.516947Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z1KPC2M7FH2RBDM3F', 'block.created', 'block', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Todoist-Specific Features [/]","updated_at":1773940562173,"created_at":1773940562154,"parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","content_type":"text","id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","properties":{"sequence":81,"ID":"b1b2037e-b2e9-45db-8cb9-2ed783ede2ce"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.517688Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z15Y9CNJNDRDF1QV3', 'block.created', 'block', 'block:a27cd79b-63bd-4704-b20f-f3b595838e89', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562154,"content_type":"text","content":"Bi-directional task completion sync","parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","updated_at":1773940562173,"id":"block:a27cd79b-63bd-4704-b20f-f3b595838e89","properties":{"ID":"a27cd79b-63bd-4704-b20f-f3b595838e89","sequence":82}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.518006Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZF7NJH715G02KRRYC', 'block.created', 'block', 'block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562154,"id":"block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22","updated_at":1773940562173,"content_type":"text","content":"Todoist due dates → deadline penalty functions","parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","properties":{"ID":"ab2868f6-ac6a-48de-b56f-ffa755f6cd22","sequence":83}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.518788Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z9ZEC9RJBQ3286PW4', 'block.created', 'block', 'block:f6e32a19-a659-47f7-b2dc-24142c6616f7', 'sql', 'confirmed', '{"data":{"created_at":1773940562154,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content":"@person labels → delegation/waiting_for tracking","parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","id":"block:f6e32a19-a659-47f7-b2dc-24142c6616f7","content_type":"text","properties":{"ID":"f6e32a19-a659-47f7-b2dc-24142c6616f7","sequence":84}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.519122Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZMZAG7TQSV3D9GDZ5', 'block.created', 'block', 'block:19923c1b-89ab-42f3-97a2-d78e994a2e1c', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:19923c1b-89ab-42f3-97a2-d78e994a2e1c","created_at":1773940562154,"updated_at":1773940562173,"parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","content":"Todoist priority → WSJF CoD weight mapping","properties":{"sequence":85,"ID":"19923c1b-89ab-42f3-97a2-d78e994a2e1c"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.519906Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z15DPG2H84KJGQ2ZX', 'block.created', 'block', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'sql', 'confirmed', '{"data":{"id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","created_at":1773940562155,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"MCP Client Bridge [/]","updated_at":1773940562173,"content_type":"text","properties":{"ID":"f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","sequence":86}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.520657Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z5JJ6KCT128RMJ9GN', 'block.created', 'block', 'block:4d30926a-54c4-40b4-978e-eeca2d273fd1', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:4d30926a-54c4-40b4-978e-eeca2d273fd1","created_at":1773940562155,"updated_at":1773940562173,"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","content":"Tool name normalization (kebab-case ↔ snake_case)","properties":{"ID":"4d30926a-54c4-40b4-978e-eeca2d273fd1","sequence":87}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.521512Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZY7XH0NNK4B4NEMWN', 'block.created', 'block', 'block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467', 'sql', 'confirmed', '{"data":{"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467","updated_at":1773940562173,"content":"McpOperationProvider converting MCP tool schemas → OperationDescriptors","created_at":1773940562155,"content_type":"text","properties":{"sequence":88,"ID":"c30b7e5a-4e9f-41e8-ab19-e803c93dc467"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.521830Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZRH5VC8CZK7VAK9JF', 'block.created', 'block', 'block:836bab0e-5ac1-4df1-9f40-4005320c406e', 'sql', 'confirmed', '{"data":{"content":"holon-mcp-client crate for connecting to external MCP servers","updated_at":1773940562173,"id":"block:836bab0e-5ac1-4df1-9f40-4005320c406e","parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","content_type":"text","created_at":1773940562155,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"836bab0e-5ac1-4df1-9f40-4005320c406e","sequence":89}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.522182Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZPGNSY1H0A9JJHPSH', 'block.created', 'block', 'block:ceb59dae-6090-41be-aff7-89de33ec600a', 'sql', 'confirmed', '{"data":{"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","content_type":"text","created_at":1773940562155,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:ceb59dae-6090-41be-aff7-89de33ec600a","content":"YAML sidecar for UI annotations (affected_fields, triggered_by, preconditions)","properties":{"ID":"ceb59dae-6090-41be-aff7-89de33ec600a","sequence":90}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.522519Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZZ015AZHNEG3WZXC3', 'block.created', 'block', 'block:419e493f-c2de-47c2-a612-787db669cd89', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","created_at":1773940562155,"content":"JSON Schema → TypeHint mapping","id":"block:419e493f-c2de-47c2-a612-787db669cd89","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"419e493f-c2de-47c2-a612-787db669cd89","sequence":91}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.522850Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7Z9FPPKB6P71CP5JDR', 'block.created', 'block', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562155,"updated_at":1773940562173,"content_type":"text","content":"Todoist API Integration [/]","parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","properties":{"sequence":92,"ID":"bdce9ec2-1508-47e9-891e-e12a7b228fcc"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.523173Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZTHZNZSB97VPF77K6', 'block.created', 'block', 'block:e9398514-1686-4fef-a44a-5fef1742d004', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:e9398514-1686-4fef-a44a-5fef1742d004","content":"TodoistOperationProvider for operation routing","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562155,"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","properties":{"sequence":93,"ID":"e9398514-1686-4fef-a44a-5fef1742d004"}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.523529Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP7ZYS955Y9YSH5QKD8Z', 'block.created', 'block', 'block:9670e586-5cda-42a2-8071-efaf855fd5d4', 'sql', 'confirmed', '{"data":{"created_at":1773940562155,"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","updated_at":1773940562173,"id":"block:9670e586-5cda-42a2-8071-efaf855fd5d4","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"Todoist REST API client","properties":{"ID":"9670e586-5cda-42a2-8071-efaf855fd5d4","sequence":94}}}', NULL, NULL, 1773940562175, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.523861Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP804YCAEH67A488NZF9', 'block.created', 'block', 'block:f41aeaa5-fe1d-45a5-806d-1f815040a33d', 'sql', 'confirmed', '{"data":{"created_at":1773940562155,"content":"Todoist entity types (tasks, projects, sections, labels)","parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","updated_at":1773940562173,"id":"block:f41aeaa5-fe1d-45a5-806d-1f815040a33d","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":95,"ID":"f41aeaa5-fe1d-45a5-806d-1f815040a33d"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.524166Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP806Z02J0QK4N8GV1PP', 'block.created', 'block', 'block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","id":"block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe","created_at":1773940562155,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"TodoistSyncProvider with incremental sync tokens","properties":{"sequence":96,"ID":"d041e942-f3a1-4b7d-80b8-7de6eb289ebe"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.524482Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP806Z4BAQ5Y0C417JJ2', 'block.created', 'block', 'block:f3b43be1-5503-4b1a-a724-fc657b47e18c', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562155,"content":"TodoistTaskDataSource implementing DataSource<TodoistTask>","id":"block:f3b43be1-5503-4b1a-a724-fc657b47e18c","updated_at":1773940562173,"properties":{"sequence":97,"ID":"f3b43be1-5503-4b1a-a724-fc657b47e18c"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.524794Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80NKBDHVVVQ7QQX73D', 'block.created', 'block', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'sql', 'confirmed', '{"data":{"content":"Phase 3: Multiple Integrations [/]\\nGoal: Validate type unification scales","created_at":1773940562156,"parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"properties":{"ID":"88810f15-a95b-4343-92e2-909c5113cc9c","sequence":98}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.525189Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80D99CAJT2STW2NV0Z', 'block.created', 'block', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Unified Item Types [/]","content_type":"text","parent_id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","created_at":1773940562156,"properties":{"sequence":99,"ID":"9ea38e3d-383e-4c27-9533-d53f1f8b1fb2"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.525521Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80S8X0MM7AF6BFHN6N', 'block.created', 'block', 'block:5b1e8251-be26-4099-b169-a330cc16f0a6', 'sql', 'confirmed', '{"data":{"created_at":1773940562156,"updated_at":1773940562173,"id":"block:5b1e8251-be26-4099-b169-a330cc16f0a6","content":"Macro-generated serialization boilerplate","parent_id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"5b1e8251-be26-4099-b169-a330cc16f0a6","sequence":100}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.525849Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80RGRWEXYXGXQA43HB', 'block.created', 'block', 'block:5b49aefd-e14f-4151-bf9e-ccccae3545ec', 'sql', 'confirmed', '{"data":{"id":"block:5b49aefd-e14f-4151-bf9e-ccccae3545ec","content":"Trait-based protocol for common task interface","content_type":"text","created_at":1773940562156,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","updated_at":1773940562173,"properties":{"sequence":101,"ID":"5b49aefd-e14f-4151-bf9e-ccccae3545ec"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.526173Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80FKN9ZARV4WWN203Q', 'block.created', 'block', 'block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"parent_id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","id":"block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447","content_type":"text","created_at":1773940562156,"content":"Extension structs for system-specific features","properties":{"ID":"e6162a0a-e9ae-494e-b3f5-4cf98cb2f447","sequence":102}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.526504Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80JJW3T65D8VYMMA85', 'block.created', 'block', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'sql', 'confirmed', '{"data":{"created_at":1773940562156,"updated_at":1773940562173,"id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","content":"Cross-System Features [/]","content_type":"text","parent_id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"d6ab6d5f-68ae-404a-bcad-b5db61586634","sequence":103}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.526827Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80PN2R8YRQDFJDVEQZ', 'block.created', 'block', 'block:5403c088-a551-4ca6-8830-34e00d5e5820', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:5403c088-a551-4ca6-8830-34e00d5e5820","parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","content":"Context Bundles assembling related items from all sources","created_at":1773940562156,"properties":{"ID":"5403c088-a551-4ca6-8830-34e00d5e5820","sequence":104}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.527606Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP803Z6V2SNANFT92F5F', 'block.created', 'block', 'block:091caad8-1689-472d-9130-e3c855c510a8', 'sql', 'confirmed', '{"data":{"created_at":1773940562156,"updated_at":1773940562173,"content_type":"text","content":"Embedding third-party items anywhere in the graph","id":"block:091caad8-1689-472d-9130-e3c855c510a8","parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":105,"ID":"091caad8-1689-472d-9130-e3c855c510a8"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.527943Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP805WGQAWEVSJYSAYGF', 'block.created', 'block', 'block:cfb257f0-1a9c-426c-ab24-940eb18853ea', 'sql', 'confirmed', '{"data":{"content":"Unified search across all systems","created_at":1773940562156,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","id":"block:cfb257f0-1a9c-426c-ab24-940eb18853ea","updated_at":1773940562173,"properties":{"sequence":106,"ID":"cfb257f0-1a9c-426c-ab24-940eb18853ea"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.528280Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80HYQF95EPYZYFFBCV', 'block.created', 'block', 'block:52a440c1-4099-4911-8d9d-e2d583dbdde7', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773940562156,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","content":"P.A.R.A. project-based organization with auto-linking","id":"block:52a440c1-4099-4911-8d9d-e2d583dbdde7","properties":{"sequence":107,"ID":"52a440c1-4099-4911-8d9d-e2d583dbdde7"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.528615Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80DMEMFZ4YHYAH4KYS', 'block.created', 'block', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","parent_id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562157,"content":"Additional Integrations [/]","properties":{"sequence":108,"ID":"34fa9276-cc30-4fcb-95b5-a97b5d708757"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.528943Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80A54K5ZSG3KVC9GVN', 'block.created', 'block', 'block:9240c0d7-d60a-46e0-8265-ceacfbf04d50', 'sql', 'confirmed', '{"data":{"parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","id":"block:9240c0d7-d60a-46e0-8265-ceacfbf04d50","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Linear integration (cycles, projects)","created_at":1773940562157,"content_type":"text","properties":{"ID":"9240c0d7-d60a-46e0-8265-ceacfbf04d50","sequence":109}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.529699Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80RT2BMXSEESE9D3EP', 'block.created', 'block', 'block:8ea813ff-b355-4165-b377-fbdef4d3d7d8', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content_type":"text","created_at":1773940562157,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Google Calendar integration (events as time tokens)","id":"block:8ea813ff-b355-4165-b377-fbdef4d3d7d8","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","properties":{"ID":"8ea813ff-b355-4165-b377-fbdef4d3d7d8","sequence":110}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.530055Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP800BBW16TCEY09G96V', 'block.created', 'block', 'block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"created_at":1773940562157,"id":"block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29","content":"Gmail integration (email threads, labels)","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","properties":{"sequence":111,"ID":"ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.530384Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80RM85WNWMV47S2B8W', 'block.created', 'block', 'block:f583e6d9-f67d-4997-a658-ed00149a34cc', 'sql', 'confirmed', '{"data":{"created_at":1773940562157,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"JIRA integration (sprints, story points, epics)","id":"block:f583e6d9-f67d-4997-a658-ed00149a34cc","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","content_type":"text","updated_at":1773940562173,"properties":{"ID":"f583e6d9-f67d-4997-a658-ed00149a34cc","sequence":112}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.530740Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80E9JEZFKV9CTZNHK4', 'block.created', 'block', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562157,"id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","content_type":"text","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","content":"GPUI Components","updated_at":1773940562173,"properties":{"ID":"9fed69a3-9180-4eba-a778-fa93bc398064","sequence":113}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.531099Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP808TV4Z46JA059TYW8', 'block.created', 'block', 'block:9f523ce8-5449-4a2f-81c8-8ee08399fc31', 'sql', 'confirmed', '{"data":{"id":"block:9f523ce8-5449-4a2f-81c8-8ee08399fc31","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content":"https://github.com/MeowLynxSea/yororen-ui","content_type":"text","created_at":1773940562157,"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","properties":{"sequence":114,"ID":"9f523ce8-5449-4a2f-81c8-8ee08399fc31"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.531475Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80MPVQ68WJGCCW2YG4', 'block.created', 'block', 'block:fd965570-883d-48f7-82b0-92ba257b2597', 'sql', 'confirmed', '{"data":{"created_at":1773940562157,"content_type":"text","content":"Pomodoro\\nhttps://github.com/rubbieKelvin/bmo","updated_at":1773940562173,"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","id":"block:fd965570-883d-48f7-82b0-92ba257b2597","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":115,"ID":"fd965570-883d-48f7-82b0-92ba257b2597"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.531867Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80Z1BMJNPN9DJRTA3Y', 'block.created', 'block', 'block:9657e201-4426-4091-891b-eb40e299d81d', 'sql', 'confirmed', '{"data":{"content":"Diff viewer\\nhttps://github.com/BlixtWallet/hunk","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","id":"block:9657e201-4426-4091-891b-eb40e299d81d","created_at":1773940562157,"updated_at":1773940562173,"properties":{"ID":"9657e201-4426-4091-891b-eb40e299d81d","sequence":116}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.532217Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80HPNX596J6V8WTXAH', 'block.created', 'block', 'block:61a47437-c394-42db-b195-3dabbd5d87ab', 'sql', 'confirmed', '{"data":{"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","created_at":1773940562157,"id":"block:61a47437-c394-42db-b195-3dabbd5d87ab","updated_at":1773940562173,"content":"Animation\\nhttps://github.com/chi11321/gpui-animation","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"ID":"61a47437-c394-42db-b195-3dabbd5d87ab","sequence":117}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.532552Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80HM23PYYQD4V76H1P', 'block.created', 'block', 'block:5841efc0-cfe6-4e69-9dbc-9f627693e59a', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content":"Editor\\nhttps://github.com/iamnbutler/gpui-editor","id":"block:5841efc0-cfe6-4e69-9dbc-9f627693e59a","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562157,"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","properties":{"ID":"5841efc0-cfe6-4e69-9dbc-9f627693e59a","sequence":118}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.533911Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP809Y0PJ8QF2YFD7JBG', 'block.created', 'block', 'block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c', 'sql', 'confirmed', '{"data":{"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","content":"WebView\\nhttps://github.com/longbridge/wef","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562158,"updated_at":1773940562173,"id":"block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c","content_type":"text","properties":{"ID":"482c5cbb-dd4f-4225-9329-ca9ca0beea4c","sequence":119}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.534683Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80EQXGT9TX4XC5TKVV', 'block.created', 'block', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"created_at":1773940562158,"parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Phase 4: AI Foundation [/]\\nGoal: Infrastructure for AI features","id":"block:7b960cd0-3478-412b-b96f-15822117ac14","properties":{"ID":"7b960cd0-3478-412b-b96f-15822117ac14","sequence":120}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.535538Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80CFA2B3KJAFJ0N4RH', 'block.created', 'block', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'sql', 'confirmed', '{"data":{"parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562158,"content_type":"text","id":"block:553f3545-4ec7-44e5-bccf-3d6443f22ecc","content":"Agent Embedding","properties":{"ID":"553f3545-4ec7-44e5-bccf-3d6443f22ecc","sequence":121}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.535894Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80Q3H717C5Q0HXK7XD', 'block.created', 'block', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'sql', 'confirmed', '{"data":{"parent_id":"block:553f3545-4ec7-44e5-bccf-3d6443f22ecc","id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","created_at":1773940562158,"content":"Via Terminal","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"ID":"d4c1533f-3a67-4314-b430-0e24bd62ce34","sequence":122}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.536262Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80CK2QAAEH9T8XTNXB', 'block.created', 'block', 'block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e', 'sql', 'confirmed', '{"data":{"id":"block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e","content":"Okena\\nA fast, native terminal multiplexer built in Rust with GPUI\\nhttps://github.com/contember/okena","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content_type":"text","created_at":1773940562158,"updated_at":1773940562173,"properties":{"sequence":123,"ID":"6e2fd9a2-6f39-48d2-b323-935fc18a3f5e"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.536615Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80JD81H6N2E4HKC7C8', 'block.created', 'block', 'block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e', 'sql', 'confirmed', '{"data":{"parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content":"PMux\\nhttps://github.com/zhoujinliang/pmux","created_at":1773940562158,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e","properties":{"ID":"c4b1ce62-0ad1-4c33-90fe-d7463f40800e","sequence":124}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.536959Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP804PR3ABC5ZTNKC929', 'block.created', 'block', 'block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content_type":"text","created_at":1773940562158,"content":"Slick\\nhttps://github.com/tristanpoland/Slick","id":"block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e","properties":{"sequence":125,"ID":"e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.537286Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80C9C7YKCCTX925XES', 'block.created', 'block', 'block:d50a9a7a-0155-4778-ac99-5f83555a1952', 'sql', 'confirmed', '{"data":{"content":"https://github.com/zortax/gpui-terminal","created_at":1773940562158,"parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:d50a9a7a-0155-4778-ac99-5f83555a1952","properties":{"ID":"d50a9a7a-0155-4778-ac99-5f83555a1952","sequence":126}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.537611Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80VJGQ40TVJB2246EH', 'block.created', 'block', 'block:cf102b47-01db-427b-97b6-3c066d9dba24', 'sql', 'confirmed', '{"data":{"id":"block:cf102b47-01db-427b-97b6-3c066d9dba24","updated_at":1773940562173,"created_at":1773940562158,"parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content":"https://github.com/Xuanwo/gpui-ghostty","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"ID":"cf102b47-01db-427b-97b6-3c066d9dba24","sequence":127}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.537939Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80S4V4D25HD0PPBPN0', 'block.created', 'block', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'sql', 'confirmed', '{"data":{"parent_id":"block:553f3545-4ec7-44e5-bccf-3d6443f22ecc","id":"block:1236a3b4-6e03-421a-a94b-fce9d7dc123c","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562158,"content":"Via Chat","properties":{"sequence":128,"ID":"1236a3b4-6e03-421a-a94b-fce9d7dc123c"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.538267Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80HWHKYZ82R8NQF8FV', 'block.created', 'block', 'block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847', 'sql', 'confirmed', '{"data":{"content":"coop\\nhttps://github.com/lumehq/coop?tab=readme-ov-file","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:1236a3b4-6e03-421a-a94b-fce9d7dc123c","updated_at":1773940562173,"created_at":1773940562158,"content_type":"text","id":"block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847","properties":{"ID":"f47a6df7-abfc-47b8-bdfe-f19eaf35b847","sequence":129}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.538612Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80QJTTE5XHDJ3JVFSN', 'block.created', 'block', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"id":"block:671593d9-a9c6-4716-860b-8410c8616539","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","content":"Embeddings & Search [/]","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562159,"properties":{"ID":"671593d9-a9c6-4716-860b-8410c8616539","sequence":130}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.538975Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80B3D93CNPG7AJ6YD2', 'block.created', 'block', 'block:d58b8367-14eb-4895-9e56-ffa7ff716d59', 'sql', 'confirmed', '{"data":{"created_at":1773940562159,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","updated_at":1773940562173,"id":"block:d58b8367-14eb-4895-9e56-ffa7ff716d59","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","content":"Local vector embeddings (sentence-transformers)","properties":{"sequence":131,"ID":"d58b8367-14eb-4895-9e56-ffa7ff716d59"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.539298Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP806FCZ3QNS9S04CMRW', 'block.created', 'block', 'block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"Semantic search using local embeddings","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","id":"block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc","created_at":1773940562159,"updated_at":1773940562173,"properties":{"sequence":132,"ID":"5f3e7d1e-af67-4699-a591-fd9291bf0cdc"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.539655Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80MVAHXMHT1AZJ94WF', 'block.created', 'block', 'block:96f4647c-8b74-4b08-8952-4f87820aed86', 'sql', 'confirmed', '{"data":{"id":"block:96f4647c-8b74-4b08-8952-4f87820aed86","content":"Entity linking (manual first, then automatic)","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","updated_at":1773940562173,"created_at":1773940562159,"properties":{"sequence":133,"ID":"96f4647c-8b74-4b08-8952-4f87820aed86"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.540005Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80VB3DGMR0NB9Z8RKJ', 'block.created', 'block', 'block:0da39f39-6635-4f9b-a468-34310147bea9', 'sql', 'confirmed', '{"data":{"content":"Tantivy full-text search integration","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","content_type":"text","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:0da39f39-6635-4f9b-a468-34310147bea9","created_at":1773940562159,"properties":{"ID":"0da39f39-6635-4f9b-a468-34310147bea9","sequence":134}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.540337Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80RNHAZDS19NBKG7R6', 'block.created', 'block', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'sql', 'confirmed', '{"data":{"id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","content_type":"text","created_at":1773940562159,"content":"Self Digital Twin [/]","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"properties":{"sequence":135,"ID":"439af07e-3237-420c-8bc0-c71aeb37c61a"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.540673Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80ZN7T9YWWBZDDRG29', 'block.created', 'block', 'block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412', 'sql', 'confirmed', '{"data":{"created_at":1773940562159,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412","parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","content":"Energy/focus/flow_depth dynamics","properties":{"ID":"5f3e8ef3-df52-4fb9-80c1-ccb81be40412","sequence":136}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.541431Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80N2JXVGKFMSPQPF3Y', 'block.created', 'block', 'block:30406a65-8e66-4589-b070-3a1b4db6e4e0', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:30406a65-8e66-4589-b070-3a1b4db6e4e0","content":"Peripheral awareness modeling","created_at":1773940562159,"content_type":"text","parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","updated_at":1773940562173,"properties":{"ID":"30406a65-8e66-4589-b070-3a1b4db6e4e0","sequence":137}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.541778Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80QA6Z51NWEAW5ERC0', 'block.created', 'block', 'block:bed11feb-a634-4f8d-b930-f0021ec0512b', 'sql', 'confirmed', '{"data":{"content":"Observable signals (window switches, typing cadence)","parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","content_type":"text","id":"block:bed11feb-a634-4f8d-b930-f0021ec0512b","created_at":1773940562159,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":138,"ID":"bed11feb-a634-4f8d-b930-f0021ec0512b"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.542160Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP807VJV4A0VWNNE5K2K', 'block.created', 'block', 'block:11c9c8bb-b72e-4752-8b6c-846e45920418', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562159,"parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","id":"block:11c9c8bb-b72e-4752-8b6c-846e45920418","content_type":"text","content":"Mental slots tracking (materialized view of open transitions)","properties":{"sequence":139,"ID":"11c9c8bb-b72e-4752-8b6c-846e45920418"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.542506Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80CR324GA98Y1ZH9P7', 'block.created', 'block', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'sql', 'confirmed', '{"data":{"created_at":1773940562159,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","content":"Logging & Training Data [/]","content_type":"text","id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","properties":{"ID":"b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","sequence":140}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.543645Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80DK5HXRB1MD35B66W', 'block.created', 'block', 'block:a186c88f-6ca5-49e2-8a0d-19632cb689fc', 'sql', 'confirmed', '{"data":{"content":"Conflict logging system (capture every conflict + resolution)","created_at":1773940562160,"content_type":"text","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:a186c88f-6ca5-49e2-8a0d-19632cb689fc","parent_id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","properties":{"ID":"a186c88f-6ca5-49e2-8a0d-19632cb689fc","sequence":141}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.543997Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80G28NDNBD00ZXER2P', 'block.created', 'block', 'block:f342692d-5414-4c48-89fe-ed8f9ccf2172', 'sql', 'confirmed', '{"data":{"parent_id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","updated_at":1773940562173,"content":"Pattern logging for Guide to learn from","created_at":1773940562160,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:f342692d-5414-4c48-89fe-ed8f9ccf2172","properties":{"sequence":142,"ID":"f342692d-5414-4c48-89fe-ed8f9ccf2172"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.545006Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP802VT7WBHKWFJVP8GG', 'block.created', 'block', 'block:30f04064-a58e-416d-b0d2-7533637effe8', 'sql', 'confirmed', '{"data":{"content":"Behavioral logging for search ranking","parent_id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","created_at":1773940562160,"content_type":"text","updated_at":1773940562173,"id":"block:30f04064-a58e-416d-b0d2-7533637effe8","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":143,"ID":"30f04064-a58e-416d-b0d2-7533637effe8"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.545339Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80QETBB97EH9BFYZRH', 'block.created', 'block', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'sql', 'confirmed', '{"data":{"content":"Objective Function Engine [/]","created_at":1773940562160,"content_type":"text","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","properties":{"sequence":144,"ID":"84151cf1-696a-420f-b73c-4947b0a4437e"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.545674Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP803MC6TP06MXTN40BT', 'block.created', 'block', 'block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562160,"parent_id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","content":"Evaluate token attributes via PRQL → scalar score","updated_at":1773940562173,"id":"block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7","content_type":"text","properties":{"sequence":145,"ID":"fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.546020Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80NG6JN6TSYTY59E1B', 'block.created', 'block', 'block:480f2628-c49f-4940-9e26-572ea23f25a3', 'sql', 'confirmed', '{"data":{"created_at":1773940562160,"id":"block:480f2628-c49f-4940-9e26-572ea23f25a3","content":"Store weights as prototype block properties","updated_at":1773940562173,"content_type":"text","parent_id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"480f2628-c49f-4940-9e26-572ea23f25a3","sequence":146}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.546783Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8087QQD9QT32ZEQC4K', 'block.created', 'block', 'block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e', 'sql', 'confirmed', '{"data":{"id":"block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","content":"Support multiple goal types (achievement, maintenance, process)","content_type":"text","updated_at":1773940562173,"created_at":1773940562160,"properties":{"ID":"e4e93198-6617-4c7c-b8f7-4b2d8188a77e","sequence":147}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.547145Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80713TSWTG6GK1ZRDR', 'block.created', 'block', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:8b962d6c-0246-4119-8826-d517e2357f21","updated_at":1773940562173,"created_at":1773940562160,"parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Phase 5: AI Features [/]\\nGoal: Three AI services operational","content_type":"text","properties":{"sequence":148,"ID":"8b962d6c-0246-4119-8826-d517e2357f21"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.547510Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80RFNYM4QZFNJWZMFW', 'block.created', 'block', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'sql', 'confirmed', '{"data":{"created_at":1773940562160,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content_type":"text","content":"The Guide (Growth) [/]","updated_at":1773940562173,"id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","properties":{"sequence":149,"ID":"567e74d4-05c4-4f98-8ce1-1b78a8c7fd78"}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.547864Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80CTGC072K7WV55R0H', 'block.created', 'block', 'block:37c082de-d10a-4f11-82ad-5fb3316bb3e4', 'sql', 'confirmed', '{"data":{"parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","content":"Velocity and capacity analysis","created_at":1773940562160,"id":"block:37c082de-d10a-4f11-82ad-5fb3316bb3e4","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","updated_at":1773940562173,"properties":{"ID":"37c082de-d10a-4f11-82ad-5fb3316bb3e4","sequence":150}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.548612Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP80QCCN0H82ZGAW2AC6', 'block.created', 'block', 'block:52bedd69-85ec-448d-81b6-0099bd413149', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:52bedd69-85ec-448d-81b6-0099bd413149","created_at":1773940562160,"content":"Stuck task identification (postponement tracking)","parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","properties":{"ID":"52bedd69-85ec-448d-81b6-0099bd413149","sequence":151}}}', NULL, NULL, 1773940562176, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.549366Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81ER986DMREQ5DCR3P', 'block.created', 'block', 'block:2b5ec929-a22d-4d7f-8640-66495331a40d', 'sql', 'confirmed', '{"data":{"id":"block:2b5ec929-a22d-4d7f-8640-66495331a40d","created_at":1773940562161,"parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","updated_at":1773940562173,"content":"Shadow Work prompts for avoided tasks","properties":{"sequence":152,"ID":"2b5ec929-a22d-4d7f-8640-66495331a40d"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.550122Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP811HD0KME5QZEY4Y8K', 'block.created', 'block', 'block:dd9075a4-5c64-4d6b-9661-7937897337d3', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","created_at":1773940562161,"content":"Growth tracking and visualization","id":"block:dd9075a4-5c64-4d6b-9661-7937897337d3","content_type":"text","properties":{"ID":"dd9075a4-5c64-4d6b-9661-7937897337d3","sequence":153}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.550871Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81XYVY586NN8C1VMTV', 'block.created', 'block', 'block:15a61916-b0c1-4d24-9046-4e066a312401', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562161,"id":"block:15a61916-b0c1-4d24-9046-4e066a312401","parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","updated_at":1773940562173,"content_type":"text","content":"Pattern recognition across time","properties":{"ID":"15a61916-b0c1-4d24-9046-4e066a312401","sequence":154}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.552349Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81C0VHG936KF7R1MCE', 'block.created', 'block', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'sql', 'confirmed', '{"data":{"created_at":1773940562161,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content":"Intelligent Conflict Reconciliation [/]","content_type":"text","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","properties":{"sequence":155,"ID":"8ae21b36-6f48-41f1-80d9-bb7ce43b4545"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.553526Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81RYZMS7D894G8HCMH', 'block.created', 'block', 'block:0db1be3e-ae11-4341-8aa8-b1d80e22963a', 'sql', 'confirmed', '{"data":{"parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","content":"LLM-based resolution for low-confidence cases","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562161,"id":"block:0db1be3e-ae11-4341-8aa8-b1d80e22963a","properties":{"sequence":156,"ID":"0db1be3e-ae11-4341-8aa8-b1d80e22963a"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.553856Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81CMJ367HA8KWJX33J', 'block.created', 'block', 'block:314e7db7-fb5e-40b6-ac10-a589ff3c809d', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:314e7db7-fb5e-40b6-ac10-a589ff3c809d","content":"Rule-based conflict resolver","created_at":1773940562161,"updated_at":1773940562173,"parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","properties":{"ID":"314e7db7-fb5e-40b6-ac10-a589ff3c809d","sequence":157}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.554184Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81RHN0X5B2J0MHF04K', 'block.created', 'block', 'block:655e2f77-d02e-4347-aa5f-dcd03ac140eb', 'sql', 'confirmed', '{"data":{"parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","content_type":"text","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562161,"content":"Train classifier on logged conflicts","id":"block:655e2f77-d02e-4347-aa5f-dcd03ac140eb","properties":{"sequence":158,"ID":"655e2f77-d02e-4347-aa5f-dcd03ac140eb"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.554523Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP817DMW120GDTXA60G5', 'block.created', 'block', 'block:3bbdc016-4f08-49e4-b550-ba3d09a03933', 'sql', 'confirmed', '{"data":{"created_at":1773940562161,"parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","id":"block:3bbdc016-4f08-49e4-b550-ba3d09a03933","updated_at":1773940562173,"content":"Conflict resolution UI with reasoning display","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"sequence":159,"ID":"3bbdc016-4f08-49e4-b550-ba3d09a03933"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.554857Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP813RD6C22FN428RTXQ', 'block.created', 'block', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content":"AI Trust Ladder [/]","created_at":1773940562161,"content_type":"text","id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","sequence":160}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.555930Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81S3B18CSFDXR88FTS', 'block.created', 'block', 'block:8a72f072-cc14-4e5f-987c-72bd27d94ced', 'sql', 'confirmed', '{"data":{"id":"block:8a72f072-cc14-4e5f-987c-72bd27d94ced","parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","created_at":1773940562161,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content_type":"text","content":"Level 3 (Agentic) with permission prompts","properties":{"ID":"8a72f072-cc14-4e5f-987c-72bd27d94ced","sequence":161}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.556281Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81K6510WKFY0XKX0HN', 'block.created', 'block', 'block:c2289c19-1733-476e-9b50-43da1d70221f', 'sql', 'confirmed', '{"data":{"parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","id":"block:c2289c19-1733-476e-9b50-43da1d70221f","created_at":1773940562161,"content_type":"text","content":"Level 4 (Autonomous) for power users","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"properties":{"ID":"c2289c19-1733-476e-9b50-43da1d70221f","sequence":162}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.556640Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81SC9XSDH426RAH40D', 'block.created', 'block', 'block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content":"Level 2 (Advisory) features","content_type":"text","id":"block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","created_at":1773940562162,"parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","sequence":163}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.557671Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP816YNV1GG1AWZWCXPY', 'block.created', 'block', 'block:84706843-7132-4c12-a2ae-32fb7109982c', 'sql', 'confirmed', '{"data":{"parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","created_at":1773940562162,"id":"block:84706843-7132-4c12-a2ae-32fb7109982c","content_type":"text","updated_at":1773940562173,"content":"Per-feature trust tracking","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"84706843-7132-4c12-a2ae-32fb7109982c","sequence":164}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.558387Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP816N2TZ2ZKWN52RQTE', 'block.created', 'block', 'block:66b47313-a556-4628-954e-1da7fb1d402d', 'sql', 'confirmed', '{"data":{"id":"block:66b47313-a556-4628-954e-1da7fb1d402d","parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","content":"Trust level visualization UI","created_at":1773940562162,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","updated_at":1773940562173,"properties":{"ID":"66b47313-a556-4628-954e-1da7fb1d402d","sequence":165}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.559429Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81AM5G28C8MD4JNTQ1', 'block.created', 'block', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'sql', 'confirmed', '{"data":{"content":"Background Enrichment Agents [/]","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562162,"content_type":"text","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","updated_at":1773940562173,"id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","properties":{"sequence":166,"ID":"d1e6541b-0c6b-4065-aea5-ad9057dc5bb5"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.559777Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81T3N543EXCYD2RNYJ', 'block.created', 'block', 'block:2618de83-3d90-4dc6-b586-98f95e351fb5', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"id":"block:2618de83-3d90-4dc6-b586-98f95e351fb5","parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","content_type":"text","content":"Infer likely token types from context","created_at":1773940562162,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"2618de83-3d90-4dc6-b586-98f95e351fb5","sequence":167}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.560519Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81N81JBNBWD4QP18F0', 'block.created', 'block', 'block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"Suggest dependencies between siblings","parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec","created_at":1773940562162,"properties":{"sequence":168,"ID":"edd212e6-16a9-4dfd-95f9-e2a2a3a55eec"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.560850Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81SXHV93V6HPDVGQ30', 'block.created', 'block', 'block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda', 'sql', 'confirmed', '{"data":{"content":"Suggest [[links]] for plain-text nouns (local LLM)","id":"block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda","updated_at":1773940562173,"parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562162,"properties":{"ID":"44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda","sequence":169}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.561573Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81QX7K8N88T1HFXYHF', 'block.created', 'block', 'block:2ff960fa-38a4-42dd-8eb0-77e15c89659e', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","id":"block:2ff960fa-38a4-42dd-8eb0-77e15c89659e","content":"Classify tasks as question/delegation/action","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562162,"properties":{"sequence":170,"ID":"2ff960fa-38a4-42dd-8eb0-77e15c89659e"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.562401Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP818WF87RNJSCP7X68X', 'block.created', 'block', 'block:864527d2-65d4-4716-a65e-73a868c7e63b', 'sql', 'confirmed', '{"data":{"content":"Suggest via: routes for questions","parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562162,"id":"block:864527d2-65d4-4716-a65e-73a868c7e63b","updated_at":1773940562173,"content_type":"text","properties":{"ID":"864527d2-65d4-4716-a65e-73a868c7e63b","sequence":171}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.562734Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP813KJ50786KNZTJGQX', 'block.created', 'block', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'sql', 'confirmed', '{"data":{"content":"The Integrator (Wholeness) [/]","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","created_at":1773940562162,"updated_at":1773940562173,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":172,"ID":"8a4a658e-d773-4528-8c61-ff3e5e425f47"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.563076Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81Q6KKB2N1HNZNYBM9', 'block.created', 'block', 'block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a', 'sql', 'confirmed', '{"data":{"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","content":"Smart linking suggestions","created_at":1773940562162,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","content_type":"text","properties":{"ID":"2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","sequence":173}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.563407Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP817CCAGQT09PJK6C5B', 'block.created', 'block', 'block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Context Bundle assembly for Flow mode","content_type":"text","updated_at":1773940562173,"created_at":1773940562163,"id":"block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6","parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","properties":{"ID":"4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6","sequence":174}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.563750Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8122NE8BFY9CK7TG0B', 'block.created', 'block', 'block:7efa2454-274c-4304-8641-e3b8171c5b5a', 'sql', 'confirmed', '{"data":{"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","content":"Cross-system deduplication","content_type":"text","created_at":1773940562163,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:7efa2454-274c-4304-8641-e3b8171c5b5a","properties":{"ID":"7efa2454-274c-4304-8641-e3b8171c5b5a","sequence":175}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.564077Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8190BCE06WQMS4HN7E', 'block.created', 'block', 'block:311aa51c-88af-446f-8cb6-b791b9740665', 'sql', 'confirmed', '{"data":{"id":"block:311aa51c-88af-446f-8cb6-b791b9740665","content":"Related item discovery","created_at":1773940562163,"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","updated_at":1773940562173,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":176,"ID":"311aa51c-88af-446f-8cb6-b791b9740665"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.564389Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81G2CBNXKZC7ECQE3R', 'block.created', 'block', 'block:9b6b2563-21b8-4286-9fac-dbdddc1a79be', 'sql', 'confirmed', '{"data":{"created_at":1773940562163,"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","content":"Automatic entity linking via embeddings","updated_at":1773940562173,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:9b6b2563-21b8-4286-9fac-dbdddc1a79be","properties":{"sequence":177,"ID":"9b6b2563-21b8-4286-9fac-dbdddc1a79be"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.564720Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP816TH793EFVGC06DAR', 'block.created', 'block', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","updated_at":1773940562173,"content_type":"text","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content":"The Watcher (Awareness) [/]","created_at":1773940562163,"properties":{"ID":"d385afbe-5bc9-4341-b879-6d14b8d763bc","sequence":178}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.565041Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81DH1A38GZ7RKQ7JTE', 'block.created', 'block', 'block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23', 'sql', 'confirmed', '{"data":{"created_at":1773940562163,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23","content":"Risk and deadline tracking","content_type":"text","parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","properties":{"sequence":179,"ID":"244abb7d-ef0f-4768-9e4e-b4bd7f3eec23"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.565355Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81X4ADZTATQD0AAN90', 'block.created', 'block', 'block:f9a2e27c-218f-402a-b405-b6b14b498bcf', 'sql', 'confirmed', '{"data":{"id":"block:f9a2e27c-218f-402a-b405-b6b14b498bcf","created_at":1773940562163,"content":"Capacity analysis across all systems","content_type":"text","updated_at":1773940562173,"parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":180,"ID":"f9a2e27c-218f-402a-b405-b6b14b498bcf"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.565678Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP817C8PGNST50HJ42QA', 'block.created', 'block', 'block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4', 'sql', 'confirmed', '{"data":{"content":"Cross-system monitoring and alerts","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4","created_at":1773940562163,"parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","updated_at":1773940562173,"content_type":"text","properties":{"sequence":181,"ID":"92d9dee2-3c16-4d14-9d54-1a93313ee1f4"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.566013Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81N97T31PGJJ8YEY95', 'block.created', 'block', 'block:e6c28ce7-c659-49e7-874b-334f05852cc4', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","updated_at":1773940562173,"parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","content":"Daily/weekly synthesis for Orient mode","created_at":1773940562163,"id":"block:e6c28ce7-c659-49e7-874b-334f05852cc4","properties":{"sequence":182,"ID":"e6c28ce7-c659-49e7-874b-334f05852cc4"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.566339Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81ACCWBR8PC62C03ZP', 'block.created', 'block', 'block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519', 'sql', 'confirmed', '{"data":{"parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562163,"updated_at":1773940562173,"id":"block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519","content":"Dependency chain analysis","content_type":"text","properties":{"sequence":183,"ID":"1ffa7eb6-174a-4bed-85d2-9c47d9d55519"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.566668Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81RF2ZBV23HTERX2XR', 'block.created', 'block', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'sql', 'confirmed', '{"data":{"created_at":1773940562163,"content_type":"text","id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Phase 6: Flow Optimization [/]\\nGoal: Users achieve flow states regularly","updated_at":1773940562173,"properties":{"sequence":184,"ID":"c74fcc72-883d-4788-911a-0632f6145e4d"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.567006Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP811DHNWD3NC9T9DKJE', 'block.created', 'block', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'sql', 'confirmed', '{"data":{"parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content":"Self DT Work Rhythms [/]","created_at":1773940562164,"content_type":"text","id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","properties":{"sequence":185,"ID":"f908d928-db6f-495e-a941-22fcdfdba73a"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.567338Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81T6THCHPEH8MEK27J', 'block.created', 'block', 'block:0570c0bf-84b4-4734-b6f3-25242a12a154', 'sql', 'confirmed', '{"data":{"id":"block:0570c0bf-84b4-4734-b6f3-25242a12a154","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"Emergent break suggestions from energy/focus dynamics","updated_at":1773940562173,"parent_id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","created_at":1773940562164,"properties":{"ID":"0570c0bf-84b4-4734-b6f3-25242a12a154","sequence":186}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.567688Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81TYXA2XR4MJWPZ7XA', 'block.created', 'block', 'block:9d85cad6-1e74-499a-8d8e-899c5553c3d6', 'sql', 'confirmed', '{"data":{"id":"block:9d85cad6-1e74-499a-8d8e-899c5553c3d6","content":"Flow depth tracking with peripheral awareness alerts","updated_at":1773940562173,"created_at":1773940562164,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","content_type":"text","properties":{"ID":"9d85cad6-1e74-499a-8d8e-899c5553c3d6","sequence":187}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.568033Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP818NT4BWV1ZR1YQ0BC', 'block.created', 'block', 'block:adc7803b-9318-4ca5-877b-83f213445aba', 'sql', 'confirmed', '{"data":{"created_at":1773940562164,"content":"Quick task suggestions during breaks (2-minute rule)","parent_id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:adc7803b-9318-4ca5-877b-83f213445aba","properties":{"sequence":188,"ID":"adc7803b-9318-4ca5-877b-83f213445aba"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.568385Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81HGFJWH3GK66E1E83', 'block.created', 'block', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'sql', 'confirmed', '{"data":{"parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","content":"Three Modes [/]","created_at":1773940562164,"updated_at":1773940562173,"id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":189,"ID":"b5771daa-0208-43fe-a890-ef1fcebf5f2f"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.568761Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP814680D5B5343EFACN', 'block.created', 'block', 'block:be15792f-21f3-476f-8b5f-e2e6b478b864', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:be15792f-21f3-476f-8b5f-e2e6b478b864","parent_id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","updated_at":1773940562173,"created_at":1773940562164,"content_type":"text","content":"Orient mode (Watcher Dashboard, daily/weekly review)","properties":{"ID":"be15792f-21f3-476f-8b5f-e2e6b478b864","sequence":190}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.569112Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP813D8W0EB6DTJQ5RNS', 'block.created', 'block', 'block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde', 'sql', 'confirmed', '{"data":{"parent_id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","created_at":1773940562164,"content_type":"text","id":"block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Flow mode (single task focus, context on demand)","properties":{"ID":"c68e8d5a-3f4b-4e8c-a887-2341e9b98bde","sequence":191}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.569437Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81M7F7P4J3AS5KA1YM', 'block.created', 'block', 'block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963', 'sql', 'confirmed', '{"data":{"content":"Capture mode (global hotkey, quick input overlay)","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"parent_id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","id":"block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963","created_at":1773940562164,"properties":{"ID":"b1b2db9a-fc0d-4f51-98ae-9c5ab056a963","sequence":192}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.569776Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81ZV09FFMB9RA6JXET', 'block.created', 'block', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'sql', 'confirmed', '{"data":{"content":"Review Workflows [/]","created_at":1773940562164,"updated_at":1773940562173,"content_type":"text","parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","id":"block:a3e31c87-d10b-432e-987c-0371e730f753","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":193,"ID":"a3e31c87-d10b-432e-987c-0371e730f753"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.570482Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81FQAB12M09JW1968T', 'block.created', 'block', 'block:4c020c67-1726-46d8-92e3-b9e0dbc90b62', 'sql', 'confirmed', '{"data":{"id":"block:4c020c67-1726-46d8-92e3-b9e0dbc90b62","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562164,"parent_id":"block:a3e31c87-d10b-432e-987c-0371e730f753","updated_at":1773940562173,"content_type":"text","content":"Daily orientation (\\"What does today look like?\\")","properties":{"ID":"4c020c67-1726-46d8-92e3-b9e0dbc90b62","sequence":194}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.570880Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81B3JQ9KECYZS7107Y', 'block.created', 'block', 'block:0906f769-52eb-47a2-917a-f9b57b7e80d1', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"parent_id":"block:a3e31c87-d10b-432e-987c-0371e730f753","created_at":1773940562164,"content_type":"text","id":"block:0906f769-52eb-47a2-917a-f9b57b7e80d1","content":"Inbox zero workflow","properties":{"ID":"0906f769-52eb-47a2-917a-f9b57b7e80d1","sequence":195}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.571236Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP816F8WVC0R67SEYGHG', 'block.created', 'block', 'block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"created_at":1773940562164,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:a3e31c87-d10b-432e-987c-0371e730f753","content":"Weekly review (comprehensive synthesis)","id":"block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f","content_type":"text","properties":{"ID":"091e7648-5314-4b4d-8e9c-bd7e0b8efc6f","sequence":196}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.571948Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP818R5DTX2D65B1FME6', 'block.created', 'block', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'sql', 'confirmed', '{"data":{"id":"block:240acff4-cf06-445e-99ee-42040da1bb84","content":"Context Bundles in Flow [/]","content_type":"text","parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","created_at":1773940562165,"updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":197,"ID":"240acff4-cf06-445e-99ee-42040da1bb84"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.572293Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81QWTKF2S2PW5EAG3D', 'block.created', 'block', 'block:90702048-5baf-4732-96fb-ddae16824257', 'sql', 'confirmed', '{"data":{"parent_id":"block:240acff4-cf06-445e-99ee-42040da1bb84","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562165,"content":"Hide distractions, show progress","updated_at":1773940562173,"id":"block:90702048-5baf-4732-96fb-ddae16824257","properties":{"sequence":198,"ID":"90702048-5baf-4732-96fb-ddae16824257"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.572626Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81DE40ZHKGB2B0Z6CY', 'block.created', 'block', 'block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content":"Slide-in context panel from edge","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:240acff4-cf06-445e-99ee-42040da1bb84","id":"block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130","created_at":1773940562165,"properties":{"sequence":199,"ID":"e4aeb8f0-4c63-48f6-b745-92a89cfd4130"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.572997Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP816Q7QBT98WWEAPXKF', 'block.created', 'block', 'block:3907168e-eaf8-48ee-8ccc-6dfef069371e', 'sql', 'confirmed', '{"data":{"parent_id":"block:240acff4-cf06-445e-99ee-42040da1bb84","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:3907168e-eaf8-48ee-8ccc-6dfef069371e","updated_at":1773940562173,"content":"Assemble all related items for focused task","created_at":1773940562165,"properties":{"sequence":200,"ID":"3907168e-eaf8-48ee-8ccc-6dfef069371e"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.573752Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81D39H355EWR2B15PZ', 'block.created', 'block', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content_type":"text","id":"block:e233124d-8711-4dd4-8153-c884f889bc07","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562165,"content":"Progressive Concealment [/]","parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","properties":{"sequence":201,"ID":"e233124d-8711-4dd4-8153-c884f889bc07"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.574139Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81NRPK03MNCND0QNXP', 'block.created', 'block', 'block:70485255-a2be-4356-bb9e-967270878b7e', 'sql', 'confirmed', '{"data":{"content":"Peripheral element dimming during sustained typing","content_type":"text","updated_at":1773940562173,"parent_id":"block:e233124d-8711-4dd4-8153-c884f889bc07","id":"block:70485255-a2be-4356-bb9e-967270878b7e","created_at":1773940562165,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"70485255-a2be-4356-bb9e-967270878b7e","sequence":202}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.574495Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81VH60TJVSRKWP1F16', 'block.created', 'block', 'block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc","parent_id":"block:e233124d-8711-4dd4-8153-c884f889bc07","created_at":1773940562165,"content":"Focused block emphasis, surrounding content fades","properties":{"ID":"ea7f8d72-f963-4a51-ab4f-d10f981eafcc","sequence":203}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.574815Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81MW7EMRSN9NKK1X2D', 'block.created', 'block', 'block:30a71e2f-f070-4745-947d-c443a86a7149', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Automatic visibility restore on cursor movement","content_type":"text","parent_id":"block:e233124d-8711-4dd4-8153-c884f889bc07","created_at":1773940562165,"updated_at":1773940562173,"id":"block:30a71e2f-f070-4745-947d-c443a86a7149","properties":{"sequence":204,"ID":"30a71e2f-f070-4745-947d-c443a86a7149"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.575144Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81KQA45FEQJVJJ8H6R', 'block.created', 'block', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content_type":"text","content":"Phase 7: Team Features [/]\\nGoal: Teams leverage individual excellence","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562165,"id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":205,"ID":"4c647dfe-0639-4064-8ab6-491d57c7e367"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.575861Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81QC531SDH09TSYKW3', 'block.created', 'block', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'sql', 'confirmed', '{"data":{"parent_id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","created_at":1773940562165,"content":"Delegation System [/]","id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"content_type":"text","properties":{"sequence":206,"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.576589Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP813HZ5NT56JMNCCPGY', 'block.created', 'block', 'block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 'sql', 'confirmed', '{"data":{"created_at":1773940562165,"content":"@[[Person]]: syntax for delegation sub-nets","parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","id":"block:15c4b164-b29f-4fb0-b882-e6408f2e3264","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"sequence":207,"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.577334Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81Y7BAP20KB6FTTPHF', 'block.created', 'block', 'block:fbbce845-023e-438b-963e-471833c51505', 'sql', 'confirmed', '{"data":{"parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","id":"block:fbbce845-023e-438b-963e-471833c51505","updated_at":1773940562173,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Waiting-for tracking (automatic from delegation patterns)","content_type":"text","created_at":1773940562166,"properties":{"ID":"fbbce845-023e-438b-963e-471833c51505","sequence":208}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.577692Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP812AVJY5178DT6HSGX', 'block.created', 'block', 'block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content":"Delegation status sync with external systems","parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","id":"block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0","created_at":1773940562166,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"sequence":209,"ID":"25e19c99-63c2-4edb-8fb1-deb1daf4baf0"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.578516Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP81ETWHX5FFBSJCKD1D', 'block.created', 'block', 'block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc', 'sql', 'confirmed', '{"data":{"id":"block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc","content":"@anyone: team pool transitions","updated_at":1773940562173,"parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","created_at":1773940562166,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"938f03b8-6129-4eda-9c5f-31a76ad8b8dc","sequence":210}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.578861Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8129GVKAWBMKSN6QZ6', 'block.created', 'block', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'sql', 'confirmed', '{"data":{"updated_at":1773940562173,"content":"Sharing & Collaboration [/]","content_type":"text","created_at":1773940562166,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","parent_id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","properties":{"sequence":211,"ID":"5bdf3ba6-f617-4bc1-93c2-15d84d925e01"}}}', NULL, NULL, 1773940562177, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.579217Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82D3KTGA2RHQC2RVNX', 'block.created', 'block', 'block:88b467b1-5a46-4b64-acb3-fcf9f377030e', 'sql', 'confirmed', '{"data":{"parent_id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","id":"block:88b467b1-5a46-4b64-acb3-fcf9f377030e","created_at":1773940562166,"updated_at":1773940562173,"content":"Collaborative editing","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","properties":{"sequence":212,"ID":"88b467b1-5a46-4b64-acb3-fcf9f377030e"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.579571Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8207X572HVHHXDDZGJ', 'block.created', 'block', 'block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da', 'sql', 'confirmed', '{"data":{"content":"Shared views and dashboards","id":"block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da","updated_at":1773940562173,"content_type":"text","created_at":1773940562166,"parent_id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":213,"ID":"f3ce62cd-5817-4a7c-81f6-7a7077aff7da"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.579923Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82389N2X4KPTVHNR2Y', 'block.created', 'block', 'block:135c74b1-8341-4719-b5d1-492eb26e2189', 'sql', 'confirmed', '{"data":{"created_at":1773940562166,"content":"Read-only sharing for documentation","parent_id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:135c74b1-8341-4719-b5d1-492eb26e2189","properties":{"sequence":214,"ID":"135c74b1-8341-4719-b5d1-492eb26e2189"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.581226Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82DXNS024AA9ZAM5K3', 'block.created', 'block', 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:e0f90f1e-5468-4229-9b6d-438b31f09ed6","content_type":"text","content":"Competition analysis","parent_id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","updated_at":1773940562173,"created_at":1773940562166,"properties":{"sequence":215,"ID":"e0f90f1e-5468-4229-9b6d-438b31f09ed6"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.581585Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP827ECXVAKDE5CE53HQ', 'block.created', 'block', 'block:ceb203d0-0b59-4aa0-a840-2e4763234112', 'sql', 'confirmed', '{"data":{"parent_id":"block:e0f90f1e-5468-4229-9b6d-438b31f09ed6","updated_at":1773940562173,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:ceb203d0-0b59-4aa0-a840-2e4763234112","created_at":1773940562166,"content":"https://github.com/3xpyth0n/ideon\\nOrganize repositories, notes, links and more on a shared infinite canvas.","properties":{"ID":"ceb203d0-0b59-4aa0-a840-2e4763234112","sequence":216}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.581945Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82EMT8TQAT16XPC7WA', 'block.created', 'block', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'sql', 'confirmed', '{"data":{"id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Cross-Cutting Concerns [/]","content_type":"text","created_at":1773940562166,"updated_at":1773940562173,"parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":217,"ID":"f407a7ec-c924-4a38-96e0-7e73472e7353"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.582316Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82V0E4FCACQRX3HBR8', 'block.created', 'block', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'sql', 'confirmed', '{"data":{"id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","updated_at":1773940562173,"content_type":"text","content":"Privacy & Security [/]","created_at":1773940562166,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":218,"ID":"ad1d8307-134f-4a34-b58e-07d6195b2466"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.582678Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82VG7ESAZR1V81VCBX', 'block.created', 'block', 'block:717db234-61eb-41ef-a8bf-b67e870f9aa6', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562173,"id":"block:717db234-61eb-41ef-a8bf-b67e870f9aa6","content":"Plugin sandboxing (WASM)","parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","created_at":1773940562166,"properties":{"sequence":219,"ID":"717db234-61eb-41ef-a8bf-b67e870f9aa6"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.583066Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82V3HKPHM8V427T1YE', 'block.created', 'block', 'block:75604518-b736-4653-a2a3-941215e798c7', 'sql', 'confirmed', '{"data":{"parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","created_at":1773940562167,"id":"block:75604518-b736-4653-a2a3-941215e798c7","content_type":"text","updated_at":1773940562174,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Self-hosted LLM option (Ollama/vLLM)","properties":{"sequence":220,"ID":"75604518-b736-4653-a2a3-941215e798c7"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.583423Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP829D8EHZN5R8KWRXA5', 'block.created', 'block', 'block:bfaedc82-3bc7-4b16-8314-273721ea997f', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Optional cloud LLM with explicit consent","created_at":1773940562167,"content_type":"text","updated_at":1773940562174,"id":"block:bfaedc82-3bc7-4b16-8314-273721ea997f","parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","properties":{"ID":"bfaedc82-3bc7-4b16-8314-273721ea997f","sequence":221}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.583772Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82S29VESZ1KCT3445P', 'block.created', 'block', 'block:4b96f182-61e5-4f0e-861d-1a7d2413abe7', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","updated_at":1773940562174,"id":"block:4b96f182-61e5-4f0e-861d-1a7d2413abe7","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Local-first by default (all data on device)","created_at":1773940562167,"properties":{"ID":"4b96f182-61e5-4f0e-861d-1a7d2413abe7","sequence":222}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.584146Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82SSYM8JK3KH8DY31S', 'block.created', 'block', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'sql', 'confirmed', '{"data":{"id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Petri-Net Advanced [/]","created_at":1773940562167,"updated_at":1773940562174,"parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","properties":{"ID":"eac105ca-efda-4976-9856-6c39a9b1502e","sequence":223}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.584495Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82QPFTZZEBQFZCJN2Q', 'block.created', 'block', 'block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59', 'sql', 'confirmed', '{"data":{"parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","content_type":"text","id":"block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59","content":"SOP extraction from repeated interaction patterns","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562174,"created_at":1773940562167,"properties":{"ID":"0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59","sequence":224}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.584875Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82JV21TF5DHEQZ3CSZ', 'block.created', 'block', 'block:143d071e-2b90-4f93-98d3-7aa5d3a14933', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:143d071e-2b90-4f93-98d3-7aa5d3a14933","created_at":1773940562167,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Delegation sub-nets (waiting_for pattern)","updated_at":1773940562174,"parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","properties":{"ID":"143d071e-2b90-4f93-98d3-7aa5d3a14933","sequence":225}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.585224Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82R4M8DKMTJQCBN5KJ', 'block.created', 'block', 'block:cc499de0-f953-4f41-b795-0864b366d8ab', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562174,"content_type":"text","content":"Token type hierarchy with mixins","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","id":"block:cc499de0-f953-4f41-b795-0864b366d8ab","created_at":1773940562167,"properties":{"ID":"cc499de0-f953-4f41-b795-0864b366d8ab","sequence":226}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.586161Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82SA3DH8WRXVCXBMCK', 'block.created', 'block', 'block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb', 'sql', 'confirmed', '{"data":{"updated_at":1773940562174,"created_at":1773940562167,"content":"Projections as views on flat net (Kanban, SOP, pipeline)","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","properties":{"sequence":227,"ID":"bd99d866-66ed-4474-8a4d-7ac1c1b08fbb"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.586508Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8232N38H41XZJ5JBDR', 'block.created', 'block', 'block:4041eb2e-23a6-4fea-9a69-0c152a6311e8', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562167,"updated_at":1773940562174,"content_type":"text","content":"Question/Information tokens with confidence tracking","id":"block:4041eb2e-23a6-4fea-9a69-0c152a6311e8","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","properties":{"ID":"4041eb2e-23a6-4fea-9a69-0c152a6311e8","sequence":228}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.586853Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8298KAS8KXSZH1GAQQ', 'block.created', 'block', 'block:1e1027d2-4c0f-4975-ba59-c3c601d1f661', 'sql', 'confirmed', '{"data":{"id":"block:1e1027d2-4c0f-4975-ba59-c3c601d1f661","created_at":1773940562167,"content_type":"text","updated_at":1773940562174,"parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Simulation engine (fork marking, compare scenarios)","properties":{"sequence":229,"ID":"1e1027d2-4c0f-4975-ba59-c3c601d1f661"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.588136Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82F2ZQK6687SF1KEK5', 'block.created', 'block', 'block:a80f6d58-c876-48f5-8bfe-69390a8f9bde', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773940562167,"updated_at":1773940562174,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:a80f6d58-c876-48f5-8bfe-69390a8f9bde","content":"Browser plugin for web app Digital Twins","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","properties":{"sequence":230,"ID":"a80f6d58-c876-48f5-8bfe-69390a8f9bde"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.588481Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82JXRSNAPW3PP87WA6', 'block.created', 'block', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'sql', 'confirmed', '{"data":{"id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","updated_at":1773940562174,"content":"PRQL Automation [/]","content_type":"text","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562168,"properties":{"sequence":231,"ID":"723a51a9-3861-429c-bb10-f73c01f8463d"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.588838Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP824MZYP4ZRABC4BGC4', 'block.created', 'block', 'block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7', 'sql', 'confirmed', '{"data":{"parent_id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","updated_at":1773940562174,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7","content":"Cross-system status propagation rules","created_at":1773940562168,"properties":{"sequence":232,"ID":"e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.589182Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82K3XNFN9839B1KM9D', 'block.created', 'block', 'block:c1338a15-080b-4dba-bbdc-87b6b8467f28', 'sql', 'confirmed', '{"data":{"id":"block:c1338a15-080b-4dba-bbdc-87b6b8467f28","content_type":"text","created_at":1773940562168,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"Auto-tag blocks based on content analysis","parent_id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","updated_at":1773940562174,"properties":{"ID":"c1338a15-080b-4dba-bbdc-87b6b8467f28","sequence":233}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.589556Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82GMDAC0XSK9QAP8XJ', 'block.created', 'block', 'block:5707965a-6578-443c-aeff-bf40170edea9', 'sql', 'confirmed', '{"data":{"id":"block:5707965a-6578-443c-aeff-bf40170edea9","parent_id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","updated_at":1773940562174,"content":"PRQL-based automation rules (query → action)","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","created_at":1773940562168,"properties":{"sequence":234,"ID":"5707965a-6578-443c-aeff-bf40170edea9"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.589907Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82XKR5C1R37NSKG3X9', 'block.created', 'block', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","created_at":1773940562168,"content_type":"text","updated_at":1773940562174,"id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","content":"Platform Support [/]","properties":{"ID":"8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","sequence":235}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.590248Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82EF8JMF9ARCDGQRFW', 'block.created', 'block', 'block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e', 'sql', 'confirmed', '{"data":{"parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","content_type":"text","id":"block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562168,"content":"Android mobile","updated_at":1773940562174,"properties":{"sequence":236,"ID":"4c4ff372-c3b9-44e6-9d46-33b7a4e7882e"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.590611Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82DWF8WHZBTWWNZ4TX', 'block.created', 'block', 'block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857', 'sql', 'confirmed', '{"data":{"id":"block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857","created_at":1773940562168,"parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"WASM compatibility (MaybeSendSync trait)","updated_at":1773940562174,"content_type":"text","properties":{"sequence":237,"ID":"e5b9db2d-f39a-439d-99f8-b4e7c4ff6857"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.590964Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82DWSCCK21RARZP2YR', 'block.created', 'block', 'block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773940562168,"updated_at":1773940562174,"content":"Windows desktop","id":"block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":238,"ID":"d61290d4-e1f6-41e7-89e0-a7ed7a6662db"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.591333Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82QE5A8X4V85YN3SHZ', 'block.created', 'block', 'block:1e729eef-3fff-43cb-8d13-499a8a8d4203', 'sql', 'confirmed', '{"data":{"id":"block:1e729eef-3fff-43cb-8d13-499a8a8d4203","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","created_at":1773940562168,"content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"iOS mobile","updated_at":1773940562174,"properties":{"sequence":239,"ID":"1e729eef-3fff-43cb-8d13-499a8a8d4203"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.591693Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP823D7BSMZNJYQHM66J', 'block.created', 'block', 'block:500b7aae-5c3b-4dd5-a3c8-373fe746990b', 'sql', 'confirmed', '{"data":{"created_at":1773940562168,"id":"block:500b7aae-5c3b-4dd5-a3c8-373fe746990b","content":"Linux desktop","updated_at":1773940562174,"content_type":"text","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":240,"ID":"500b7aae-5c3b-4dd5-a3c8-373fe746990b"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.592443Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8287QES2V3F3SJQT9V', 'block.created', 'block', 'block:a79ab251-4685-4728-b98b-0a652774f06c', 'sql', 'confirmed', '{"data":{"updated_at":1773940562174,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","id":"block:a79ab251-4685-4728-b98b-0a652774f06c","content_type":"text","content":"macOS desktop (Flutter)","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","created_at":1773940562168,"properties":{"sequence":241,"ID":"a79ab251-4685-4728-b98b-0a652774f06c"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.592824Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP827N85PA8C8EC6467F', 'block.created', 'block', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","content":"UI/UX Design System [/]","updated_at":1773940562174,"id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","content_type":"text","created_at":1773940562169,"properties":{"sequence":242,"ID":"ac137431-daf6-4741-9808-6dc71c13e7c6"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.593208Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82D6Y8V3QHBE7FQD47', 'block.created', 'block', 'block:a85de368-9546-446d-ad61-17b72c7dbc3e', 'sql', 'confirmed', '{"data":{"updated_at":1773940562174,"created_at":1773940562169,"id":"block:a85de368-9546-446d-ad61-17b72c7dbc3e","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","content":"Which-Key navigation system (Space → mnemonic keys)","content_type":"text","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":243,"ID":"a85de368-9546-446d-ad61-17b72c7dbc3e"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.593576Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82M2M0YZ37J7BKGSRN', 'block.created', 'block', 'block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562174,"content":"Micro-interactions (checkbox animation, smooth reorder)","id":"block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9","created_at":1773940562169,"content_type":"text","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","properties":{"sequence":244,"ID":"1cea6bd3-680f-46c3-bdbc-5989da5ed7d9"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.594414Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82DPPDGRE9JAXJD25S', 'block.created', 'block', 'block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3', 'sql', 'confirmed', '{"data":{"id":"block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","updated_at":1773940562174,"created_at":1773940562169,"content":"Light and dark themes","properties":{"sequence":245,"ID":"d1fbee2c-3a11-4adc-a3db-fd93f5b117e3"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.594757Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP828333BM5Y8Y52TWSY', 'block.created', 'block', 'block:beeec959-ba87-4c57-9531-c1d7f24d2b2c', 'sql', 'confirmed', '{"data":{"content":"Color palette (warm, professional, calm technology)","id":"block:beeec959-ba87-4c57-9531-c1d7f24d2b2c","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","created_at":1773940562169,"updated_at":1773940562174,"content_type":"text","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","properties":{"ID":"beeec959-ba87-4c57-9531-c1d7f24d2b2c","sequence":246}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.595104Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82VNRRS6VN1AP4ADNX', 'block.created', 'block', 'block:d36014da-518a-4da5-b360-218d027ee104', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562174,"created_at":1773940562169,"content_type":"text","content":"Typography system (Inter + JetBrains Mono)","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","id":"block:d36014da-518a-4da5-b360-218d027ee104","properties":{"ID":"d36014da-518a-4da5-b360-218d027ee104","sequence":247}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.595474Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82DEW4VBCQ96CZBKD8', 'block.created', 'block', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'sql', 'confirmed', '{"data":{"id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","content_type":"text","parent_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content":"LogSeq replacement","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562174,"created_at":1773940562169,"properties":{"sequence":248,"ID":"01806047-9cf8-42fe-8391-6d608bfade9e"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.595815Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP820PT2F3GZ92HDGGGK', 'block.created', 'block', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'sql', 'confirmed', '{"data":{"content":"Editing experience","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","parent_id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","created_at":1773940562169,"id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","updated_at":1773940562174,"properties":{"sequence":249,"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.597229Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82BAMMWTX1VZ3RMKQJ', 'block.created', 'block', 'block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'sql', 'confirmed', '{"data":{"created_at":1773940562169,"updated_at":1773940562174,"id":"block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2","parent_id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","content":"GitHub Flavored Markdown parser & renderer for GPUI\\nhttps://github.com/joris-gallot/gpui-gfm","properties":{"sequence":250,"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.597585Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82V8AFE71DJX4552TD', 'block.created', 'block', 'block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', 'sql', 'confirmed', '{"data":{"content":"Desktop Markdown viewer built with Rust and GPUI\\nhttps://github.com/chunghha/markdown_viewer","content_type":"text","updated_at":1773940562174,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","created_at":1773940562169,"id":"block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8","properties":{"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8","sequence":251}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.598941Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP822TJBWQ4TN8DXZHJQ', 'block.created', 'block', 'block:f7730a68-6268-4e65-ac93-3fdf79e92133', 'sql', 'confirmed', '{"data":{"updated_at":1773940562174,"created_at":1773940562169,"content_type":"text","id":"block:f7730a68-6268-4e65-ac93-3fdf79e92133","content":"Markdown Editor and Viewer\\nhttps://github.com/kumarUjjawal/aster","parent_id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133","sequence":252}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.599308Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82SK43RCHY9QTHR8KF', 'block.created', 'block', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","created_at":1773940562170,"parent_id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","updated_at":1773940562174,"content":"PDF Viewer & Annotator","properties":{"ID":"8594ab7c-5f36-44cf-8f92-248b31508441","sequence":253}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.599692Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP8220RM71DTH7VR31S7', 'block.created', 'block', 'block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 'sql', 'confirmed', '{"data":{"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","content_type":"text","updated_at":1773940562174,"id":"block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c","created_at":1773940562170,"content":"Combining gpui and hayro for a little application that render pdfs\\nhttps://github.com/vincenthz/gpui-hayro?tab=readme-ov-file","properties":{"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c","sequence":254}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.600052Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82DTW1Z6NK76QEXJVG', 'block.created', 'block', 'block:b95a19a6-5448-42f0-af06-177e95e27f49', 'sql', 'confirmed', '{"data":{"content":"Libera Reader\\nModern, performance-oriented desktop e-book reader built with Rust and GPUI.\\nhttps://github.com/RikaKit2/libera-reader","updated_at":1773940562174,"created_at":1773940562170,"parent_id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","content_type":"text","id":"block:b95a19a6-5448-42f0-af06-177e95e27f49","properties":{"ID":"b95a19a6-5448-42f0-af06-177e95e27f49","sequence":255}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.600402Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82464XEWZAH22E061M', 'block.created', 'block', 'block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:812924a9-0bc2-41a7-8820-1c60a40bd1ad","parent_id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","created_at":1773940562170,"content":"Monica: On-screen anotation software\\nhttps://github.com/tasuren/monica","updated_at":1773940562174,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","properties":{"sequence":256,"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.601115Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82WX95DY52VKFS2S4F', 'block.created', 'block', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', 'sql', 'confirmed', '{"data":{"created_at":1773940562170,"document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","updated_at":1773940562174,"content":"Graph vis","content_type":"text","parent_id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","id":"block:419b2df8-0121-4532-8dcd-21f04df806d8","properties":{"sequence":257,"ID":"419b2df8-0121-4532-8dcd-21f04df806d8"}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [transaction_stmt] 2026-03-19T17:16:02.601863Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3HHP82E3XZWDDBB1HC8PXP', 'block.created', 'block', 'block:f520a9ff-71bf-4a72-8777-9864bad7c535', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773940562174,"created_at":1773940562170,"content":"https://github.com/jerlendds/gpug","id":"block:f520a9ff-71bf-4a72-8777-9864bad7c535","document_id":"doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5","parent_id":"block:419b2df8-0121-4532-8dcd-21f04df806d8","properties":{"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535","sequence":258}}}', NULL, NULL, 1773940562178, NULL, NULL);

-- [actor_tx_commit] 2026-03-19T17:16:02.602204Z
COMMIT;

-- Wait 15ms
-- [actor_exec] 2026-03-19T17:16:02.618072Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_exec] 2026-03-19T17:16:02.626991Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.627823Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.628735Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:02.635759Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:02.636670Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_tx_begin] 2026-03-19T17:16:02.637402Z
BEGIN TRANSACTION (259 stmts);

-- [transaction_stmt] 2026-03-19T17:16:02.637425Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 1: Core Outliner', 'text', NULL, NULL, '{"sequence":0,"ID":"599b60af-960d-4c9c-b222-d3d9de95c513"}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.637907Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'MCP Server Frontend [/]', 'text', NULL, NULL, '{"task_state":"DOING","ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb","sequence":1}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.638341Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:db59d038-8a47-43e9-9502-0472b493a6b9', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Context parameter support ($context_id, $context_parent_id)', 'text', NULL, NULL, '{"sequence":2,"ID":"db59d038-8a47-43e9-9502-0472b493a6b9"}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.638751Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:95ad6166-c03c-4417-a435-349e88b8e90a', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'MCP server (stdio + HTTP modes)', 'text', NULL, NULL, '{"sequence":3,"ID":"95ad6166-c03c-4417-a435-349e88b8e90a"}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.639155Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'MCP tools for query execution and operations', 'text', NULL, NULL, '{"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b","sequence":4}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.639567Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:661368d9-e4bd-4722-b5c2-40f32006c643', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Block Operations [/]', 'text', NULL, NULL, '{"ID":"661368d9-e4bd-4722-b5c2-40f32006c643","sequence":5}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.640120Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Block hierarchy (parent/child, indent/outdent)', 'text', NULL, NULL, '{"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007","sequence":6}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.640504Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Split block operation', 'text', NULL, NULL, '{"sequence":7,"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34"}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.640876Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Block CRUD (create, read, update, delete)', 'text', NULL, NULL, '{"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb","sequence":8}', 1773940562147, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.641262Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Block movement (move_up, move_down, move_block)', 'text', NULL, NULL, '{"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63","sequence":9}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.641643Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:225edb45-f670-445a-9162-18c150210ee6', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Undo/redo system (UndoStack + persistent OperationLogStore)', 'text', NULL, NULL, '{"ID":"225edb45-f670-445a-9162-18c150210ee6","task_state":"DONE","sequence":10}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.642029Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Storage & Data Layer [/]', 'text', NULL, NULL, '{"sequence":11,"ID":"444b24f6-d412-43c4-a14b-6e725b673cee"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.642416Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Schema Module system with topological dependency ordering', 'text', NULL, NULL, '{"sequence":12,"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.642808Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Fractional indexing for block ordering', 'text', NULL, NULL, '{"sequence":13,"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.643187Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Turso (embedded SQLite) backend with connection pooling', 'text', NULL, NULL, '{"sequence":14,"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.643565Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'QueryableCache wrapping DataSource with local caching', 'text', NULL, NULL, '{"sequence":15,"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.643929Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Entity derive macro (#[derive(Entity)]) for schema generation', 'text', NULL, NULL, '{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.644326Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d318cae4-759d-487b-a909-81940223ecc1', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'CDC (Change Data Capture) streaming from storage to UI', 'text', NULL, NULL, '{"ID":"d318cae4-759d-487b-a909-81940223ecc1","sequence":17}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.644714Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Command sourcing infrastructure (append-only operation log)', 'text', NULL, NULL, '{"ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222","sequence":18,"task_state":"DONE"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.645096Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Procedural Macros [/]', 'text', NULL, NULL, '{"sequence":19,"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72"}', 1773940562148, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.645477Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '#[operations_trait] macro for operation dispatch generation', 'text', NULL, NULL, '{"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4","sequence":20}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.645868Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '#[triggered_by(...)] for operation availability', 'text', NULL, NULL, '{"sequence":21,"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44"}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.646252Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Type inference for OperationDescriptor parameters', 'text', NULL, NULL, '{"sequence":22,"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd"}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.646627Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '#[affects(...)] for field-level reactivity', 'text', NULL, NULL, '{"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2","sequence":23}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.647004Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Performance [/]', 'text', NULL, NULL, '{"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a","sequence":24}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.647386Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Virtual scrolling and lazy loading', 'text', NULL, NULL, '{"sequence":25,"ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73","task_state":"DOING"}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.647770Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Connection pooling for Turso', 'text', NULL, NULL, '{"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","task_state":"DOING","sequence":26}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.648147Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e0567a06-5a62-4957-9457-c55a6661cee5', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Full-text search indexing (Tantivy)', 'text', NULL, NULL, '{"sequence":27,"ID":"e0567a06-5a62-4957-9457-c55a6661cee5"}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.648680Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-Device Sync [/]', 'text', NULL, NULL, '{"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","sequence":28}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.649062Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:43f329da-cfb4-4764-b599-06f4b6272f91', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'CollaborativeDoc with ALPN routing', 'text', NULL, NULL, '{"sequence":29,"ID":"43f329da-cfb4-4764-b599-06f4b6272f91"}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.649446Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7aef40b2-14e1-4df0-a825-18603c55d198', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Offline-first with background sync', 'text', NULL, NULL, '{"sequence":30,"ID":"7aef40b2-14e1-4df0-a825-18603c55d198"}', 1773940562149, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.649831Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e148d7b7-c505-4201-83b7-36986a981a56', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Iroh P2P transport for Loro documents', 'text', NULL, NULL, '{"ID":"e148d7b7-c505-4201-83b7-36986a981a56","sequence":31}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.650212Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Dependency Injection [/]', 'text', NULL, NULL, '{"sequence":32,"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9"}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.650702Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b980e51f-0c91-4708-9a17-3d41284974b2', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'OperationDispatcher routing to providers', 'text', NULL, NULL, '{"sequence":33,"ID":"b980e51f-0c91-4708-9a17-3d41284974b2"}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.651096Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:97cc8506-47d2-44cb-bdca-8e9a507953a0', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'BackendEngine as main orchestration point', 'text', NULL, NULL, '{"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0","sequence":34}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.651477Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'ferrous-di based service composition', 'text', NULL, NULL, '{"sequence":35,"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8"}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.651847Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'SchemaRegistry with topological initialization', 'text', NULL, NULL, '{"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47","sequence":36}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.652221Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Query & Render Pipeline [/]', 'text', NULL, NULL, '{"sequence":37,"ID":"b489c622-6c87-4bf6-8d35-787eb732d670"}', 1773940562150, 1773940562172, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.652603Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1bbec456-7217-4477-a49c-0b8422e441e9', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)', 'text', NULL, NULL, '{"sequence":38,"ID":"1bbec456-7217-4477-a49c-0b8422e441e9"}', 1773940562150, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.652983Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2b1c341e-5da2-4207-a609-f4af6d7ceebd', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Automatic operation wiring (lineage analysis → widget binding)', 'text', NULL, NULL, '{"ID":"2b1c341e-5da2-4207-a609-f4af6d7ceebd","task_state":"DOING","sequence":39}', 1773940562150, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.653357Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2d44d7df-5d7d-4cfe-9061-459c7578e334', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'GQL (graph query) support via EAV schema', 'text', NULL, NULL, '{"ID":"2d44d7df-5d7d-4cfe-9061-459c7578e334","sequence":40,"task_state":"DOING"}', 1773940562150, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.653728Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:54ed1be5-765e-4884-87ab-02268e0208c7', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'PRQL compilation (PRQL → SQL + RenderSpec)', 'text', NULL, NULL, '{"sequence":41,"ID":"54ed1be5-765e-4884-87ab-02268e0208c7"}', 1773940562150, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.654121Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5384c1da-f058-4321-8401-929b3570c2a5', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'RenderSpec tree for declarative UI description', 'text', NULL, NULL, '{"sequence":42,"ID":"5384c1da-f058-4321-8401-929b3570c2a5"}', 1773940562150, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.654982Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Unified execute_query supporting PRQL/GQL/SQL', 'text', NULL, NULL, '{"ID":"fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","sequence":43,"task_state":"DONE"}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.655356Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'SQL direct execution support', 'text', NULL, NULL, '{"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","task_state":"DOING","sequence":44}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.655725Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Loro CRDT Integration [/]', 'text', NULL, NULL, '{"ID":"d9374dc3-05fc-40b2-896d-f88bb8a33c92","sequence":45}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.656104Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'LoroBackend implementing CoreOperations trait', 'text', NULL, NULL, '{"sequence":46,"ID":"b1dc3ad3-574b-472a-b74b-e3ea29a433e6"}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.656495Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'LoroDocumentStore for managing CRDT documents on disk', 'text', NULL, NULL, '{"ID":"ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","task_state":"DOING","sequence":47}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.656895Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:35652c3f-720c-4e20-ab90-5e25e1429733', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'LoroBlockOperations as OperationProvider routing writes through CRDT', 'text', NULL, NULL, '{"sequence":48,"ID":"35652c3f-720c-4e20-ab90-5e25e1429733"}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.657277Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cycle detection in move_block', 'text', NULL, NULL, '{"ID":"090731e3-38ae-4bf1-b5ec-dbb33eae4fb2","sequence":49}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.657659Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ddf208e4-9b73-422d-b8ab-4ec58b328907', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Loro-to-Turso materialization (CRDT → SQL cache → CDC)', 'text', NULL, NULL, '{"ID":"ddf208e4-9b73-422d-b8ab-4ec58b328907","sequence":50}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.658040Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Org-Mode Sync [/]', 'text', NULL, NULL, '{"ID":"9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","sequence":51}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.658407Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7bc5f362-0bf9-45a1-b2b7-6882585ed169', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'OrgRenderer as single path for producing org text', 'text', NULL, NULL, '{"ID":"7bc5f362-0bf9-45a1-b2b7-6882585ed169","sequence":52}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.658791Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8eab3453-25d2-4e7a-89f8-f9f79be939c9', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Document identity & aliases (UUID ↔ file path mapping)', 'text', NULL, NULL, '{"ID":"8eab3453-25d2-4e7a-89f8-f9f79be939c9","sequence":53}', 1773940562151, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.659169Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fc60da1b-6065-4d36-8551-5479ff145df0', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'OrgSyncController with echo suppression', 'text', NULL, NULL, '{"ID":"fc60da1b-6065-4d36-8551-5479ff145df0","sequence":54}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.659544Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6e5a1157-b477-45a1-892f-57807b4d969b', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Bidirectional sync (file changes ↔ block changes)', 'text', NULL, NULL, '{"ID":"6e5a1157-b477-45a1-892f-57807b4d969b","sequence":55}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.659931Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Org file parsing (headlines, properties, source blocks)', 'text', NULL, NULL, '{"sequence":56,"ID":"6e4dab75-cd13-4c5e-9168-bf266d11aa3f"}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.660323Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Flutter Frontend [/]', 'text', NULL, NULL, '{"ID":"bb3bc716-ca9a-438a-936d-03631e2ee929","sequence":57}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.660695Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'FFI bridge via flutter_rust_bridge', 'text', NULL, NULL, '{"ID":"b4753cd8-47ea-4f7d-bd00-e1ec563aa43f","sequence":58}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.661093Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3289bc82-f8a9-4cad-8545-ad1fee9dc282', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Navigation system (history, cursor, focus)', 'text', NULL, NULL, '{"ID":"3289bc82-f8a9-4cad-8545-ad1fee9dc282","sequence":59,"task_state":"DOING"}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.661487Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ebca0a24-f6f6-4c49-8a27-9d9973acf737', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Block editor (outliner interactions)', 'text', NULL, NULL, '{"ID":"ebca0a24-f6f6-4c49-8a27-9d9973acf737","sequence":60}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.661994Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Reactive UI updates from CDC change streams', 'text', NULL, NULL, '{"sequence":61,"ID":"eb7e34f8-19f5-48f5-a22d-8f62493bafdd"}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.662356Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7a0a4905-59c5-4277-8114-1e9ca9d425e3', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Three-column layout (sidebar, main, right panel)', 'text', NULL, NULL, '{"ID":"7a0a4905-59c5-4277-8114-1e9ca9d425e3","sequence":62}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.662736Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:19d7b512-e5e0-469c-917b-eb27d7a38bed', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Flutter desktop app shell', 'text', NULL, NULL, '{"ID":"19d7b512-e5e0-469c-917b-eb27d7a38bed","sequence":63}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.663146Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Petri-Net Task Ranking (WSJF) [/]', 'text', NULL, NULL, '{"ID":"afe4f75c-7948-4d4c-9724-4bfab7d47d88","sequence":64}', 1773940562152, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.663544Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Prototype blocks with =computed Rhai expressions', 'text', NULL, NULL, '{"sequence":65,"ID":"d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","task_state":"DOING"}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.663921Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2d399fd7-79d8-41f1-846b-31dabcec208a', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Verb dictionary (~30 German + English verbs → transition types)', 'text', NULL, NULL, '{"ID":"2d399fd7-79d8-41f1-846b-31dabcec208a","sequence":66}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.664311Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2385f4e3-25e1-4911-bf75-77cefd394206', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'rank_tasks() engine with tiebreak ordering', 'text', NULL, NULL, '{"ID":"2385f4e3-25e1-4911-bf75-77cefd394206","task_state":"DOING","sequence":67}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.664893Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cae619f2-26fe-464e-b67a-0a04f76543c9', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Block → Petri Net materialization (petri.rs)', 'text', NULL, NULL, '{"task_state":"DOING","sequence":68,"ID":"cae619f2-26fe-464e-b67a-0a04f76543c9"}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.665266Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eaee1c9b-5466-428f-8dbb-f4882ccdb066', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Self Descriptor (person block with is_self: true)', 'text', NULL, NULL, '{"ID":"eaee1c9b-5466-428f-8dbb-f4882ccdb066","task_state":"DOING","sequence":69}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.665667Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:023da362-ce5d-4a3b-827a-29e745d6f778', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'WSJF scoring (priority_weight × urgency_weight + position_weight)', 'text', NULL, NULL, '{"ID":"023da362-ce5d-4a3b-827a-29e745d6f778","sequence":70,"task_state":"DOING"}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.666056Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Task syntax parser (@, ?, >, [[links]])', 'text', NULL, NULL, '{"sequence":71,"ID":"46a8c75e-8ab8-4a5a-b4af-a1388f6a4812"}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.666436Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 2: First Integration (Todoist) [/]\nGoal: Prove hybrid architecture', 'text', NULL, NULL, '{"sequence":72,"ID":"29c0aa5f-d9ca-46f3-8601-6023f87cefbd"}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.666805Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Reconciliation [/]', 'text', NULL, NULL, '{"ID":"00fa0916-2681-4699-9554-44fcb8e2ea6a","sequence":73}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.667173Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:632af903-5459-4d44-921a-43145e20dc82', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Sync token management to prevent duplicate processing', 'text', NULL, NULL, '{"ID":"632af903-5459-4d44-921a-43145e20dc82","sequence":74}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.667557Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:78f9d6e3-42d4-4975-910d-3728e23410b1', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Conflict detection and resolution UI', 'text', NULL, NULL, '{"sequence":75,"ID":"78f9d6e3-42d4-4975-910d-3728e23410b1"}', 1773940562153, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.667947Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fa2854d1-2751-4a07-8f83-70c2f9c6c190', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Last-write-wins for concurrent edits', 'text', NULL, NULL, '{"ID":"fa2854d1-2751-4a07-8f83-70c2f9c6c190","sequence":76}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.668339Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Operation Queue & Offline Support [/]', 'text', NULL, NULL, '{"ID":"043ed925-6bf2-4db3-baf8-2277f1a5afaa","sequence":77}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.668711Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Offline operation queue with retry logic', 'text', NULL, NULL, '{"ID":"5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","sequence":78}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.669087Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Sync status indicators (synced, pending, conflict, error)', 'text', NULL, NULL, '{"ID":"7de8d37b-49ba-4ada-9b1e-df1c41c0db05","sequence":79}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.669463Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:302eb0c5-56fe-4980-8292-bae8a9a0450a', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Optimistic updates with ID mapping (internal ↔ external)', 'text', NULL, NULL, '{"sequence":80,"ID":"302eb0c5-56fe-4980-8292-bae8a9a0450a"}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.669832Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist-Specific Features [/]', 'text', NULL, NULL, '{"ID":"b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","sequence":81}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.670220Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a27cd79b-63bd-4704-b20f-f3b595838e89', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Bi-directional task completion sync', 'text', NULL, NULL, '{"ID":"a27cd79b-63bd-4704-b20f-f3b595838e89","sequence":82}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.670601Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist due dates → deadline penalty functions', 'text', NULL, NULL, '{"ID":"ab2868f6-ac6a-48de-b56f-ffa755f6cd22","sequence":83}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.670990Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f6e32a19-a659-47f7-b2dc-24142c6616f7', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '@person labels → delegation/waiting_for tracking', 'text', NULL, NULL, '{"ID":"f6e32a19-a659-47f7-b2dc-24142c6616f7","sequence":84}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.671353Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:19923c1b-89ab-42f3-97a2-d78e994a2e1c', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist priority → WSJF CoD weight mapping', 'text', NULL, NULL, '{"sequence":85,"ID":"19923c1b-89ab-42f3-97a2-d78e994a2e1c"}', 1773940562154, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.671729Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'MCP Client Bridge [/]', 'text', NULL, NULL, '{"sequence":86,"ID":"f37ab7bc-c89e-4b47-9317-3a9f7a440d2a"}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.672100Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4d30926a-54c4-40b4-978e-eeca2d273fd1', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Tool name normalization (kebab-case ↔ snake_case)', 'text', NULL, NULL, '{"ID":"4d30926a-54c4-40b4-978e-eeca2d273fd1","sequence":87}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.672468Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'McpOperationProvider converting MCP tool schemas → OperationDescriptors', 'text', NULL, NULL, '{"sequence":88,"ID":"c30b7e5a-4e9f-41e8-ab19-e803c93dc467"}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.672985Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:836bab0e-5ac1-4df1-9f40-4005320c406e', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'holon-mcp-client crate for connecting to external MCP servers', 'text', NULL, NULL, '{"ID":"836bab0e-5ac1-4df1-9f40-4005320c406e","sequence":89}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.673361Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ceb59dae-6090-41be-aff7-89de33ec600a', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'YAML sidecar for UI annotations (affected_fields, triggered_by, preconditions)', 'text', NULL, NULL, '{"ID":"ceb59dae-6090-41be-aff7-89de33ec600a","sequence":90}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.673730Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:419e493f-c2de-47c2-a612-787db669cd89', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'JSON Schema → TypeHint mapping', 'text', NULL, NULL, '{"ID":"419e493f-c2de-47c2-a612-787db669cd89","sequence":91}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.674129Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist API Integration [/]', 'text', NULL, NULL, '{"ID":"bdce9ec2-1508-47e9-891e-e12a7b228fcc","sequence":92}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.674539Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e9398514-1686-4fef-a44a-5fef1742d004', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'TodoistOperationProvider for operation routing', 'text', NULL, NULL, '{"sequence":93,"ID":"e9398514-1686-4fef-a44a-5fef1742d004"}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.674902Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9670e586-5cda-42a2-8071-efaf855fd5d4', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist REST API client', 'text', NULL, NULL, '{"ID":"9670e586-5cda-42a2-8071-efaf855fd5d4","sequence":94}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.675279Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f41aeaa5-fe1d-45a5-806d-1f815040a33d', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Todoist entity types (tasks, projects, sections, labels)', 'text', NULL, NULL, '{"sequence":95,"ID":"f41aeaa5-fe1d-45a5-806d-1f815040a33d"}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.675639Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'TodoistSyncProvider with incremental sync tokens', 'text', NULL, NULL, '{"sequence":96,"ID":"d041e942-f3a1-4b7d-80b8-7de6eb289ebe"}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.676025Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f3b43be1-5503-4b1a-a724-fc657b47e18c', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'TodoistTaskDataSource implementing DataSource<TodoistTask>', 'text', NULL, NULL, '{"sequence":97,"ID":"f3b43be1-5503-4b1a-a724-fc657b47e18c"}', 1773940562155, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.676420Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 3: Multiple Integrations [/]\nGoal: Validate type unification scales', 'text', NULL, NULL, '{"ID":"88810f15-a95b-4343-92e2-909c5113cc9c","sequence":98}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.676794Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Unified Item Types [/]', 'text', NULL, NULL, '{"ID":"9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","sequence":99}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.677178Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5b1e8251-be26-4099-b169-a330cc16f0a6', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Macro-generated serialization boilerplate', 'text', NULL, NULL, '{"ID":"5b1e8251-be26-4099-b169-a330cc16f0a6","sequence":100}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.677558Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5b49aefd-e14f-4151-bf9e-ccccae3545ec', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Trait-based protocol for common task interface', 'text', NULL, NULL, '{"ID":"5b49aefd-e14f-4151-bf9e-ccccae3545ec","sequence":101}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.677939Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Extension structs for system-specific features', 'text', NULL, NULL, '{"ID":"e6162a0a-e9ae-494e-b3f5-4cf98cb2f447","sequence":102}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.678290Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-System Features [/]', 'text', NULL, NULL, '{"sequence":103,"ID":"d6ab6d5f-68ae-404a-bcad-b5db61586634"}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.678657Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5403c088-a551-4ca6-8830-34e00d5e5820', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Context Bundles assembling related items from all sources', 'text', NULL, NULL, '{"ID":"5403c088-a551-4ca6-8830-34e00d5e5820","sequence":104}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.679028Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:091caad8-1689-472d-9130-e3c855c510a8', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Embedding third-party items anywhere in the graph', 'text', NULL, NULL, '{"ID":"091caad8-1689-472d-9130-e3c855c510a8","sequence":105}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.679394Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cfb257f0-1a9c-426c-ab24-940eb18853ea', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Unified search across all systems', 'text', NULL, NULL, '{"sequence":106,"ID":"cfb257f0-1a9c-426c-ab24-940eb18853ea"}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.679755Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:52a440c1-4099-4911-8d9d-e2d583dbdde7', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'P.A.R.A. project-based organization with auto-linking', 'text', NULL, NULL, '{"ID":"52a440c1-4099-4911-8d9d-e2d583dbdde7","sequence":107}', 1773940562156, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.680125Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Additional Integrations [/]', 'text', NULL, NULL, '{"ID":"34fa9276-cc30-4fcb-95b5-a97b5d708757","sequence":108}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.680503Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9240c0d7-d60a-46e0-8265-ceacfbf04d50', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Linear integration (cycles, projects)', 'text', NULL, NULL, '{"ID":"9240c0d7-d60a-46e0-8265-ceacfbf04d50","sequence":109}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.680868Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8ea813ff-b355-4165-b377-fbdef4d3d7d8', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Google Calendar integration (events as time tokens)', 'text', NULL, NULL, '{"ID":"8ea813ff-b355-4165-b377-fbdef4d3d7d8","sequence":110}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.681246Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Gmail integration (email threads, labels)', 'text', NULL, NULL, '{"ID":"ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29","sequence":111}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.681608Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f583e6d9-f67d-4997-a658-ed00149a34cc', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'JIRA integration (sprints, story points, epics)', 'text', NULL, NULL, '{"sequence":112,"ID":"f583e6d9-f67d-4997-a658-ed00149a34cc"}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.681988Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9fed69a3-9180-4eba-a778-fa93bc398064', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'GPUI Components', 'text', NULL, NULL, '{"ID":"9fed69a3-9180-4eba-a778-fa93bc398064","sequence":113}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.682390Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9f523ce8-5449-4a2f-81c8-8ee08399fc31', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'https://github.com/MeowLynxSea/yororen-ui', 'text', NULL, NULL, '{"sequence":114,"ID":"9f523ce8-5449-4a2f-81c8-8ee08399fc31"}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.682799Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fd965570-883d-48f7-82b0-92ba257b2597', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Pomodoro\nhttps://github.com/rubbieKelvin/bmo', 'text', NULL, NULL, '{"ID":"fd965570-883d-48f7-82b0-92ba257b2597","sequence":115}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.683188Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9657e201-4426-4091-891b-eb40e299d81d', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Diff viewer\nhttps://github.com/BlixtWallet/hunk', 'text', NULL, NULL, '{"ID":"9657e201-4426-4091-891b-eb40e299d81d","sequence":116}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.683567Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:61a47437-c394-42db-b195-3dabbd5d87ab', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Animation\nhttps://github.com/chi11321/gpui-animation', 'text', NULL, NULL, '{"ID":"61a47437-c394-42db-b195-3dabbd5d87ab","sequence":117}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.684097Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5841efc0-cfe6-4e69-9dbc-9f627693e59a', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Editor\nhttps://github.com/iamnbutler/gpui-editor', 'text', NULL, NULL, '{"ID":"5841efc0-cfe6-4e69-9dbc-9f627693e59a","sequence":118}', 1773940562157, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.684607Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'WebView\nhttps://github.com/longbridge/wef', 'text', NULL, NULL, '{"sequence":119,"ID":"482c5cbb-dd4f-4225-9329-ca9ca0beea4c"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.685005Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 4: AI Foundation [/]\nGoal: Infrastructure for AI features', 'text', NULL, NULL, '{"sequence":120,"ID":"7b960cd0-3478-412b-b96f-15822117ac14"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.685376Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Agent Embedding', 'text', NULL, NULL, '{"ID":"553f3545-4ec7-44e5-bccf-3d6443f22ecc","sequence":121}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.685750Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Via Terminal', 'text', NULL, NULL, '{"ID":"d4c1533f-3a67-4314-b430-0e24bd62ce34","sequence":122}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.686140Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Okena\nA fast, native terminal multiplexer built in Rust with GPUI\nhttps://github.com/contember/okena', 'text', NULL, NULL, '{"sequence":123,"ID":"6e2fd9a2-6f39-48d2-b323-935fc18a3f5e"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.686529Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'PMux\nhttps://github.com/zhoujinliang/pmux', 'text', NULL, NULL, '{"sequence":124,"ID":"c4b1ce62-0ad1-4c33-90fe-d7463f40800e"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.687061Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Slick\nhttps://github.com/tristanpoland/Slick', 'text', NULL, NULL, '{"sequence":125,"ID":"e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.687435Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d50a9a7a-0155-4778-ac99-5f83555a1952', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'https://github.com/zortax/gpui-terminal', 'text', NULL, NULL, '{"ID":"d50a9a7a-0155-4778-ac99-5f83555a1952","sequence":126}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.687809Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cf102b47-01db-427b-97b6-3c066d9dba24', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'https://github.com/Xuanwo/gpui-ghostty', 'text', NULL, NULL, '{"sequence":127,"ID":"cf102b47-01db-427b-97b6-3c066d9dba24"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.688184Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Via Chat', 'text', NULL, NULL, '{"ID":"1236a3b4-6e03-421a-a94b-fce9d7dc123c","sequence":128}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.688562Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'coop\nhttps://github.com/lumehq/coop?tab=readme-ov-file', 'text', NULL, NULL, '{"sequence":129,"ID":"f47a6df7-abfc-47b8-bdfe-f19eaf35b847"}', 1773940562158, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.688935Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:671593d9-a9c6-4716-860b-8410c8616539', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Embeddings & Search [/]', 'text', NULL, NULL, '{"ID":"671593d9-a9c6-4716-860b-8410c8616539","sequence":130}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.689305Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d58b8367-14eb-4895-9e56-ffa7ff716d59', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Local vector embeddings (sentence-transformers)', 'text', NULL, NULL, '{"sequence":131,"ID":"d58b8367-14eb-4895-9e56-ffa7ff716d59"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.689729Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Semantic search using local embeddings', 'text', NULL, NULL, '{"sequence":132,"ID":"5f3e7d1e-af67-4699-a591-fd9291bf0cdc"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.690100Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:96f4647c-8b74-4b08-8952-4f87820aed86', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Entity linking (manual first, then automatic)', 'text', NULL, NULL, '{"ID":"96f4647c-8b74-4b08-8952-4f87820aed86","sequence":133}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.690467Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0da39f39-6635-4f9b-a468-34310147bea9', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Tantivy full-text search integration', 'text', NULL, NULL, '{"ID":"0da39f39-6635-4f9b-a468-34310147bea9","sequence":134}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.690864Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Self Digital Twin [/]', 'text', NULL, NULL, '{"sequence":135,"ID":"439af07e-3237-420c-8bc0-c71aeb37c61a"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.691259Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Energy/focus/flow_depth dynamics', 'text', NULL, NULL, '{"ID":"5f3e8ef3-df52-4fb9-80c1-ccb81be40412","sequence":136}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.691632Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:30406a65-8e66-4589-b070-3a1b4db6e4e0', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Peripheral awareness modeling', 'text', NULL, NULL, '{"sequence":137,"ID":"30406a65-8e66-4589-b070-3a1b4db6e4e0"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.692003Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bed11feb-a634-4f8d-b930-f0021ec0512b', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Observable signals (window switches, typing cadence)', 'text', NULL, NULL, '{"sequence":138,"ID":"bed11feb-a634-4f8d-b930-f0021ec0512b"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.692373Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:11c9c8bb-b72e-4752-8b6c-846e45920418', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Mental slots tracking (materialized view of open transitions)', 'text', NULL, NULL, '{"sequence":139,"ID":"11c9c8bb-b72e-4752-8b6c-846e45920418"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.692737Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Logging & Training Data [/]', 'text', NULL, NULL, '{"sequence":140,"ID":"b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5"}', 1773940562159, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.693127Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a186c88f-6ca5-49e2-8a0d-19632cb689fc', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Conflict logging system (capture every conflict + resolution)', 'text', NULL, NULL, '{"ID":"a186c88f-6ca5-49e2-8a0d-19632cb689fc","sequence":141}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.693650Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f342692d-5414-4c48-89fe-ed8f9ccf2172', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Pattern logging for Guide to learn from', 'text', NULL, NULL, '{"sequence":142,"ID":"f342692d-5414-4c48-89fe-ed8f9ccf2172"}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.694026Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:30f04064-a58e-416d-b0d2-7533637effe8', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Behavioral logging for search ranking', 'text', NULL, NULL, '{"ID":"30f04064-a58e-416d-b0d2-7533637effe8","sequence":143}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.694412Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:84151cf1-696a-420f-b73c-4947b0a4437e', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Objective Function Engine [/]', 'text', NULL, NULL, '{"sequence":144,"ID":"84151cf1-696a-420f-b73c-4947b0a4437e"}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.694772Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Evaluate token attributes via PRQL → scalar score', 'text', NULL, NULL, '{"ID":"fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7","sequence":145}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.695133Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:480f2628-c49f-4940-9e26-572ea23f25a3', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Store weights as prototype block properties', 'text', NULL, NULL, '{"ID":"480f2628-c49f-4940-9e26-572ea23f25a3","sequence":146}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.695511Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Support multiple goal types (achievement, maintenance, process)', 'text', NULL, NULL, '{"ID":"e4e93198-6617-4c7c-b8f7-4b2d8188a77e","sequence":147}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.695872Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 5: AI Features [/]\nGoal: Three AI services operational', 'text', NULL, NULL, '{"sequence":148,"ID":"8b962d6c-0246-4119-8826-d517e2357f21"}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.696238Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'The Guide (Growth) [/]', 'text', NULL, NULL, '{"ID":"567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","sequence":149}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.696618Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:37c082de-d10a-4f11-82ad-5fb3316bb3e4', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Velocity and capacity analysis', 'text', NULL, NULL, '{"sequence":150,"ID":"37c082de-d10a-4f11-82ad-5fb3316bb3e4"}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.696989Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:52bedd69-85ec-448d-81b6-0099bd413149', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Stuck task identification (postponement tracking)', 'text', NULL, NULL, '{"sequence":151,"ID":"52bedd69-85ec-448d-81b6-0099bd413149"}', 1773940562160, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.697362Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2b5ec929-a22d-4d7f-8640-66495331a40d', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Shadow Work prompts for avoided tasks', 'text', NULL, NULL, '{"sequence":152,"ID":"2b5ec929-a22d-4d7f-8640-66495331a40d"}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.697783Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:dd9075a4-5c64-4d6b-9661-7937897337d3', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Growth tracking and visualization', 'text', NULL, NULL, '{"ID":"dd9075a4-5c64-4d6b-9661-7937897337d3","sequence":153}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.698172Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:15a61916-b0c1-4d24-9046-4e066a312401', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Pattern recognition across time', 'text', NULL, NULL, '{"ID":"15a61916-b0c1-4d24-9046-4e066a312401","sequence":154}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.698525Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Intelligent Conflict Reconciliation [/]', 'text', NULL, NULL, '{"ID":"8ae21b36-6f48-41f1-80d9-bb7ce43b4545","sequence":155}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.698882Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0db1be3e-ae11-4341-8aa8-b1d80e22963a', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'LLM-based resolution for low-confidence cases', 'text', NULL, NULL, '{"sequence":156,"ID":"0db1be3e-ae11-4341-8aa8-b1d80e22963a"}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.699304Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:314e7db7-fb5e-40b6-ac10-a589ff3c809d', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Rule-based conflict resolver', 'text', NULL, NULL, '{"ID":"314e7db7-fb5e-40b6-ac10-a589ff3c809d","sequence":157}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.699656Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:655e2f77-d02e-4347-aa5f-dcd03ac140eb', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Train classifier on logged conflicts', 'text', NULL, NULL, '{"ID":"655e2f77-d02e-4347-aa5f-dcd03ac140eb","sequence":158}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.700131Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3bbdc016-4f08-49e4-b550-ba3d09a03933', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Conflict resolution UI with reasoning display', 'text', NULL, NULL, '{"ID":"3bbdc016-4f08-49e4-b550-ba3d09a03933","sequence":159}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.700505Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'AI Trust Ladder [/]', 'text', NULL, NULL, '{"ID":"be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","sequence":160}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.700844Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8a72f072-cc14-4e5f-987c-72bd27d94ced', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Level 3 (Agentic) with permission prompts', 'text', NULL, NULL, '{"ID":"8a72f072-cc14-4e5f-987c-72bd27d94ced","sequence":161}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.701197Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c2289c19-1733-476e-9b50-43da1d70221f', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Level 4 (Autonomous) for power users', 'text', NULL, NULL, '{"sequence":162,"ID":"c2289c19-1733-476e-9b50-43da1d70221f"}', 1773940562161, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.701581Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Level 2 (Advisory) features', 'text', NULL, NULL, '{"ID":"c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","sequence":163}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.701951Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:84706843-7132-4c12-a2ae-32fb7109982c', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Per-feature trust tracking', 'text', NULL, NULL, '{"ID":"84706843-7132-4c12-a2ae-32fb7109982c","sequence":164}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.702304Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:66b47313-a556-4628-954e-1da7fb1d402d', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Trust level visualization UI', 'text', NULL, NULL, '{"ID":"66b47313-a556-4628-954e-1da7fb1d402d","sequence":165}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.702654Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Background Enrichment Agents [/]', 'text', NULL, NULL, '{"sequence":166,"ID":"d1e6541b-0c6b-4065-aea5-ad9057dc5bb5"}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.703047Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2618de83-3d90-4dc6-b586-98f95e351fb5', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Infer likely token types from context', 'text', NULL, NULL, '{"ID":"2618de83-3d90-4dc6-b586-98f95e351fb5","sequence":167}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.703395Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Suggest dependencies between siblings', 'text', NULL, NULL, '{"sequence":168,"ID":"edd212e6-16a9-4dfd-95f9-e2a2a3a55eec"}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.703745Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Suggest [[links]] for plain-text nouns (local LLM)', 'text', NULL, NULL, '{"sequence":169,"ID":"44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda"}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.704114Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2ff960fa-38a4-42dd-8eb0-77e15c89659e', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Classify tasks as question/delegation/action', 'text', NULL, NULL, '{"ID":"2ff960fa-38a4-42dd-8eb0-77e15c89659e","sequence":170}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.704495Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:864527d2-65d4-4716-a65e-73a868c7e63b', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Suggest via: routes for questions', 'text', NULL, NULL, '{"ID":"864527d2-65d4-4716-a65e-73a868c7e63b","sequence":171}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.704853Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'The Integrator (Wholeness) [/]', 'text', NULL, NULL, '{"ID":"8a4a658e-d773-4528-8c61-ff3e5e425f47","sequence":172}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.705216Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Smart linking suggestions', 'text', NULL, NULL, '{"ID":"2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","sequence":173}', 1773940562162, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.705579Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Context Bundle assembly for Flow mode', 'text', NULL, NULL, '{"ID":"4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6","sequence":174}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.706134Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7efa2454-274c-4304-8641-e3b8171c5b5a', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-system deduplication', 'text', NULL, NULL, '{"sequence":175,"ID":"7efa2454-274c-4304-8641-e3b8171c5b5a"}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.706495Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:311aa51c-88af-446f-8cb6-b791b9740665', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Related item discovery', 'text', NULL, NULL, '{"ID":"311aa51c-88af-446f-8cb6-b791b9740665","sequence":176}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.706861Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9b6b2563-21b8-4286-9fac-dbdddc1a79be', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Automatic entity linking via embeddings', 'text', NULL, NULL, '{"sequence":177,"ID":"9b6b2563-21b8-4286-9fac-dbdddc1a79be"}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.707239Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'The Watcher (Awareness) [/]', 'text', NULL, NULL, '{"ID":"d385afbe-5bc9-4341-b879-6d14b8d763bc","sequence":178}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.707656Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Risk and deadline tracking', 'text', NULL, NULL, '{"sequence":179,"ID":"244abb7d-ef0f-4768-9e4e-b4bd7f3eec23"}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.708026Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f9a2e27c-218f-402a-b405-b6b14b498bcf', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Capacity analysis across all systems', 'text', NULL, NULL, '{"sequence":180,"ID":"f9a2e27c-218f-402a-b405-b6b14b498bcf"}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.708387Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-system monitoring and alerts', 'text', NULL, NULL, '{"sequence":181,"ID":"92d9dee2-3c16-4d14-9d54-1a93313ee1f4"}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.708742Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e6c28ce7-c659-49e7-874b-334f05852cc4', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Daily/weekly synthesis for Orient mode', 'text', NULL, NULL, '{"sequence":182,"ID":"e6c28ce7-c659-49e7-874b-334f05852cc4"}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.709107Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Dependency chain analysis', 'text', NULL, NULL, '{"ID":"1ffa7eb6-174a-4bed-85d2-9c47d9d55519","sequence":183}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.709473Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 6: Flow Optimization [/]\nGoal: Users achieve flow states regularly', 'text', NULL, NULL, '{"ID":"c74fcc72-883d-4788-911a-0632f6145e4d","sequence":184}', 1773940562163, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.709883Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f908d928-db6f-495e-a941-22fcdfdba73a', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Self DT Work Rhythms [/]', 'text', NULL, NULL, '{"ID":"f908d928-db6f-495e-a941-22fcdfdba73a","sequence":185}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.710231Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0570c0bf-84b4-4734-b6f3-25242a12a154', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Emergent break suggestions from energy/focus dynamics', 'text', NULL, NULL, '{"sequence":186,"ID":"0570c0bf-84b4-4734-b6f3-25242a12a154"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.710605Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9d85cad6-1e74-499a-8d8e-899c5553c3d6', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Flow depth tracking with peripheral awareness alerts', 'text', NULL, NULL, '{"sequence":187,"ID":"9d85cad6-1e74-499a-8d8e-899c5553c3d6"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.710963Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:adc7803b-9318-4ca5-877b-83f213445aba', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Quick task suggestions during breaks (2-minute rule)', 'text', NULL, NULL, '{"ID":"adc7803b-9318-4ca5-877b-83f213445aba","sequence":188}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.711360Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Three Modes [/]', 'text', NULL, NULL, '{"sequence":189,"ID":"b5771daa-0208-43fe-a890-ef1fcebf5f2f"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.711718Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:be15792f-21f3-476f-8b5f-e2e6b478b864', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Orient mode (Watcher Dashboard, daily/weekly review)', 'text', NULL, NULL, '{"sequence":190,"ID":"be15792f-21f3-476f-8b5f-e2e6b478b864"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.712116Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Flow mode (single task focus, context on demand)', 'text', NULL, NULL, '{"sequence":191,"ID":"c68e8d5a-3f4b-4e8c-a887-2341e9b98bde"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.712493Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Capture mode (global hotkey, quick input overlay)', 'text', NULL, NULL, '{"ID":"b1b2db9a-fc0d-4f51-98ae-9c5ab056a963","sequence":192}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.712882Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a3e31c87-d10b-432e-987c-0371e730f753', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Review Workflows [/]', 'text', NULL, NULL, '{"sequence":193,"ID":"a3e31c87-d10b-432e-987c-0371e730f753"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.713255Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4c020c67-1726-46d8-92e3-b9e0dbc90b62', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Daily orientation ("What does today look like?")', 'text', NULL, NULL, '{"ID":"4c020c67-1726-46d8-92e3-b9e0dbc90b62","sequence":194}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.713613Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0906f769-52eb-47a2-917a-f9b57b7e80d1', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Inbox zero workflow', 'text', NULL, NULL, '{"sequence":195,"ID":"0906f769-52eb-47a2-917a-f9b57b7e80d1"}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.713979Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Weekly review (comprehensive synthesis)', 'text', NULL, NULL, '{"ID":"091e7648-5314-4b4d-8e9c-bd7e0b8efc6f","sequence":196}', 1773940562164, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.714345Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:240acff4-cf06-445e-99ee-42040da1bb84', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Context Bundles in Flow [/]', 'text', NULL, NULL, '{"sequence":197,"ID":"240acff4-cf06-445e-99ee-42040da1bb84"}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.714729Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:90702048-5baf-4732-96fb-ddae16824257', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Hide distractions, show progress', 'text', NULL, NULL, '{"sequence":198,"ID":"90702048-5baf-4732-96fb-ddae16824257"}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.715097Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Slide-in context panel from edge', 'text', NULL, NULL, '{"sequence":199,"ID":"e4aeb8f0-4c63-48f6-b745-92a89cfd4130"}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.715445Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3907168e-eaf8-48ee-8ccc-6dfef069371e', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Assemble all related items for focused task', 'text', NULL, NULL, '{"ID":"3907168e-eaf8-48ee-8ccc-6dfef069371e","sequence":200}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.715852Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e233124d-8711-4dd4-8153-c884f889bc07', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Progressive Concealment [/]', 'text', NULL, NULL, '{"ID":"e233124d-8711-4dd4-8153-c884f889bc07","sequence":201}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.716234Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:70485255-a2be-4356-bb9e-967270878b7e', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Peripheral element dimming during sustained typing', 'text', NULL, NULL, '{"ID":"70485255-a2be-4356-bb9e-967270878b7e","sequence":202}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.716749Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Focused block emphasis, surrounding content fades', 'text', NULL, NULL, '{"ID":"ea7f8d72-f963-4a51-ab4f-d10f981eafcc","sequence":203}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.717124Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:30a71e2f-f070-4745-947d-c443a86a7149', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Automatic visibility restore on cursor movement', 'text', NULL, NULL, '{"sequence":204,"ID":"30a71e2f-f070-4745-947d-c443a86a7149"}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.717500Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Phase 7: Team Features [/]\nGoal: Teams leverage individual excellence', 'text', NULL, NULL, '{"sequence":205,"ID":"4c647dfe-0639-4064-8ab6-491d57c7e367"}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.718017Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Delegation System [/]', 'text', NULL, NULL, '{"sequence":206,"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede"}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.718384Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '@[[Person]]: syntax for delegation sub-nets', 'text', NULL, NULL, '{"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264","sequence":207}', 1773940562165, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.718757Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fbbce845-023e-438b-963e-471833c51505', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Waiting-for tracking (automatic from delegation patterns)', 'text', NULL, NULL, '{"ID":"fbbce845-023e-438b-963e-471833c51505","sequence":208}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.719124Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Delegation status sync with external systems', 'text', NULL, NULL, '{"sequence":209,"ID":"25e19c99-63c2-4edb-8fb1-deb1daf4baf0"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.719500Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', '@anyone: team pool transitions', 'text', NULL, NULL, '{"ID":"938f03b8-6129-4eda-9c5f-31a76ad8b8dc","sequence":210}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.719873Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Sharing & Collaboration [/]', 'text', NULL, NULL, '{"sequence":211,"ID":"5bdf3ba6-f617-4bc1-93c2-15d84d925e01"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.720242Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:88b467b1-5a46-4b64-acb3-fcf9f377030e', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Collaborative editing', 'text', NULL, NULL, '{"ID":"88b467b1-5a46-4b64-acb3-fcf9f377030e","sequence":212}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.720611Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Shared views and dashboards', 'text', NULL, NULL, '{"sequence":213,"ID":"f3ce62cd-5817-4a7c-81f6-7a7077aff7da"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.720963Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:135c74b1-8341-4719-b5d1-492eb26e2189', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Read-only sharing for documentation', 'text', NULL, NULL, '{"sequence":214,"ID":"135c74b1-8341-4719-b5d1-492eb26e2189"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.721330Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Competition analysis', 'text', NULL, NULL, '{"sequence":215,"ID":"e0f90f1e-5468-4229-9b6d-438b31f09ed6"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.721684Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ceb203d0-0b59-4aa0-a840-2e4763234112', 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'https://github.com/3xpyth0n/ideon\nOrganize repositories, notes, links and more on a shared infinite canvas.', 'text', NULL, NULL, '{"sequence":216,"ID":"ceb203d0-0b59-4aa0-a840-2e4763234112"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.722060Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-Cutting Concerns [/]', 'text', NULL, NULL, '{"sequence":217,"ID":"f407a7ec-c924-4a38-96e0-7e73472e7353"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.722421Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Privacy & Security [/]', 'text', NULL, NULL, '{"sequence":218,"ID":"ad1d8307-134f-4a34-b58e-07d6195b2466"}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T17:16:02.723428Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:717db234-61eb-41ef-a8bf-b67e870f9aa6', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Plugin sandboxing (WASM)', 'text', NULL, NULL, '{"ID":"717db234-61eb-41ef-a8bf-b67e870f9aa6","sequence":219}', 1773940562166, 1773940562173, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.723823Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:75604518-b736-4653-a2a3-941215e798c7', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Self-hosted LLM option (Ollama/vLLM)', 'text', NULL, NULL, '{"ID":"75604518-b736-4653-a2a3-941215e798c7","sequence":220}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.724209Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bfaedc82-3bc7-4b16-8314-273721ea997f', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Optional cloud LLM with explicit consent', 'text', NULL, NULL, '{"ID":"bfaedc82-3bc7-4b16-8314-273721ea997f","sequence":221}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.724613Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4b96f182-61e5-4f0e-861d-1a7d2413abe7', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Local-first by default (all data on device)', 'text', NULL, NULL, '{"sequence":222,"ID":"4b96f182-61e5-4f0e-861d-1a7d2413abe7"}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.724996Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eac105ca-efda-4976-9856-6c39a9b1502e', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Petri-Net Advanced [/]', 'text', NULL, NULL, '{"sequence":223,"ID":"eac105ca-efda-4976-9856-6c39a9b1502e"}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.725355Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'SOP extraction from repeated interaction patterns', 'text', NULL, NULL, '{"sequence":224,"ID":"0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59"}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.725724Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:143d071e-2b90-4f93-98d3-7aa5d3a14933', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Delegation sub-nets (waiting_for pattern)', 'text', NULL, NULL, '{"sequence":225,"ID":"143d071e-2b90-4f93-98d3-7aa5d3a14933"}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.726101Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc499de0-f953-4f41-b795-0864b366d8ab', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Token type hierarchy with mixins', 'text', NULL, NULL, '{"ID":"cc499de0-f953-4f41-b795-0864b366d8ab","sequence":226}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.726498Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Projections as views on flat net (Kanban, SOP, pipeline)', 'text', NULL, NULL, '{"sequence":227,"ID":"bd99d866-66ed-4474-8a4d-7ac1c1b08fbb"}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.726868Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4041eb2e-23a6-4fea-9a69-0c152a6311e8', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Question/Information tokens with confidence tracking', 'text', NULL, NULL, '{"sequence":228,"ID":"4041eb2e-23a6-4fea-9a69-0c152a6311e8"}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.727242Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e1027d2-4c0f-4975-ba59-c3c601d1f661', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Simulation engine (fork marking, compare scenarios)', 'text', NULL, NULL, '{"ID":"1e1027d2-4c0f-4975-ba59-c3c601d1f661","sequence":229}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.727620Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a80f6d58-c876-48f5-8bfe-69390a8f9bde', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Browser plugin for web app Digital Twins', 'text', NULL, NULL, '{"ID":"a80f6d58-c876-48f5-8bfe-69390a8f9bde","sequence":230}', 1773940562167, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.728009Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:723a51a9-3861-429c-bb10-f73c01f8463d', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'PRQL Automation [/]', 'text', NULL, NULL, '{"sequence":231,"ID":"723a51a9-3861-429c-bb10-f73c01f8463d"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.728366Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Cross-system status propagation rules', 'text', NULL, NULL, '{"sequence":232,"ID":"e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.728740Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c1338a15-080b-4dba-bbdc-87b6b8467f28', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Auto-tag blocks based on content analysis', 'text', NULL, NULL, '{"sequence":233,"ID":"c1338a15-080b-4dba-bbdc-87b6b8467f28"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.729293Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5707965a-6578-443c-aeff-bf40170edea9', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'PRQL-based automation rules (query → action)', 'text', NULL, NULL, '{"ID":"5707965a-6578-443c-aeff-bf40170edea9","sequence":234}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.729664Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Platform Support [/]', 'text', NULL, NULL, '{"ID":"8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","sequence":235}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.730031Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Android mobile', 'text', NULL, NULL, '{"ID":"4c4ff372-c3b9-44e6-9d46-33b7a4e7882e","sequence":236}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.730398Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'WASM compatibility (MaybeSendSync trait)', 'text', NULL, NULL, '{"sequence":237,"ID":"e5b9db2d-f39a-439d-99f8-b4e7c4ff6857"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.730765Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Windows desktop', 'text', NULL, NULL, '{"sequence":238,"ID":"d61290d4-e1f6-41e7-89e0-a7ed7a6662db"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.731143Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e729eef-3fff-43cb-8d13-499a8a8d4203', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'iOS mobile', 'text', NULL, NULL, '{"sequence":239,"ID":"1e729eef-3fff-43cb-8d13-499a8a8d4203"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.731528Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:500b7aae-5c3b-4dd5-a3c8-373fe746990b', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Linux desktop', 'text', NULL, NULL, '{"sequence":240,"ID":"500b7aae-5c3b-4dd5-a3c8-373fe746990b"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.731909Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a79ab251-4685-4728-b98b-0a652774f06c', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'macOS desktop (Flutter)', 'text', NULL, NULL, '{"sequence":241,"ID":"a79ab251-4685-4728-b98b-0a652774f06c"}', 1773940562168, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.732301Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'UI/UX Design System [/]', 'text', NULL, NULL, '{"sequence":242,"ID":"ac137431-daf6-4741-9808-6dc71c13e7c6"}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.732678Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a85de368-9546-446d-ad61-17b72c7dbc3e', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Which-Key navigation system (Space → mnemonic keys)', 'text', NULL, NULL, '{"sequence":243,"ID":"a85de368-9546-446d-ad61-17b72c7dbc3e"}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.733059Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Micro-interactions (checkbox animation, smooth reorder)', 'text', NULL, NULL, '{"ID":"1cea6bd3-680f-46c3-bdbc-5989da5ed7d9","sequence":244}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.733424Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Light and dark themes', 'text', NULL, NULL, '{"sequence":245,"ID":"d1fbee2c-3a11-4adc-a3db-fd93f5b117e3"}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.733795Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:beeec959-ba87-4c57-9531-c1d7f24d2b2c', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Color palette (warm, professional, calm technology)', 'text', NULL, NULL, '{"sequence":246,"ID":"beeec959-ba87-4c57-9531-c1d7f24d2b2c"}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.734192Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d36014da-518a-4da5-b360-218d027ee104', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Typography system (Inter + JetBrains Mono)', 'text', NULL, NULL, '{"ID":"d36014da-518a-4da5-b360-218d027ee104","sequence":247}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.734572Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'LogSeq replacement', 'text', NULL, NULL, '{"ID":"01806047-9cf8-42fe-8391-6d608bfade9e","sequence":248}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.734940Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Editing experience', 'text', NULL, NULL, '{"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","sequence":249}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.735307Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'GitHub Flavored Markdown parser & renderer for GPUI\nhttps://github.com/joris-gallot/gpui-gfm', 'text', NULL, NULL, '{"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2","sequence":250}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.735675Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Desktop Markdown viewer built with Rust and GPUI\nhttps://github.com/chunghha/markdown_viewer', 'text', NULL, NULL, '{"sequence":251,"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8"}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.736059Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f7730a68-6268-4e65-ac93-3fdf79e92133', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Markdown Editor and Viewer\nhttps://github.com/kumarUjjawal/aster', 'text', NULL, NULL, '{"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133","sequence":252}', 1773940562169, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.736421Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8594ab7c-5f36-44cf-8f92-248b31508441', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'PDF Viewer & Annotator', 'text', NULL, NULL, '{"ID":"8594ab7c-5f36-44cf-8f92-248b31508441","sequence":253}', 1773940562170, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.736774Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Combining gpui and hayro for a little application that render pdfs\nhttps://github.com/vincenthz/gpui-hayro?tab=readme-ov-file', 'text', NULL, NULL, '{"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c","sequence":254}', 1773940562170, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.737146Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b95a19a6-5448-42f0-af06-177e95e27f49', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Libera Reader\nModern, performance-oriented desktop e-book reader built with Rust and GPUI.\nhttps://github.com/RikaKit2/libera-reader', 'text', NULL, NULL, '{"ID":"b95a19a6-5448-42f0-af06-177e95e27f49","sequence":255}', 1773940562170, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.737536Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Monica: On-screen anotation software\nhttps://github.com/tasuren/monica', 'text', NULL, NULL, '{"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad","sequence":256}', 1773940562170, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.737896Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:419b2df8-0121-4532-8dcd-21f04df806d8', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'Graph vis', 'text', NULL, NULL, '{"sequence":257,"ID":"419b2df8-0121-4532-8dcd-21f04df806d8"}', 1773940562170, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T17:16:02.738252Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f520a9ff-71bf-4a72-8777-9864bad7c535', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', 'doc:9b2816ba-ae3a-4deb-8add-41369f88c7d5', 'https://github.com/jerlendds/gpug', 'text', NULL, NULL, '{"sequence":258,"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535"}', 1773940562170, 1773940562174, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [actor_tx_commit] 2026-03-19T17:16:02.738611Z
COMMIT;

-- Wait 743ms
-- [actor_exec] 2026-03-19T17:16:03.482003Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_query] 2026-03-19T17:16:03.490409Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_3b8f070830f6b4d1';

-- [actor_exec] 2026-03-19T17:16:03.490662Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.491428Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.492158Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_3b8f070830f6b4d1';

-- [actor_exec] 2026-03-19T17:16:03.492330Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.492946Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.493553Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_3b8f070830f6b4d1';

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:03.500911Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.501646Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_3b8f070830f6b4d1 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:root-layout' OR parent_id = 'block:root-layout';

-- Wait 5ms
-- [actor_exec] 2026-03-19T17:16:03.507249Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_exec] 2026-03-19T17:16:03.515506Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.516327Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T17:16:03.517078Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.517813Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:03.525160Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.525969Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_441ba8cd9ee4ed5d';

-- [actor_exec] 2026-03-19T17:16:03.526276Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.527052Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_441ba8cd9ee4ed5d';

-- [actor_exec] 2026-03-19T17:16:03.527231Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.527834Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_441ba8cd9ee4ed5d';

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:03.535348Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.536211Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_441ba8cd9ee4ed5d AS SELECT _v2.*, json_extract(_v2."properties", '$.sequence') AS "sequence", json_extract(_v2."properties", '$.collapse_to') AS "collapse_to", json_extract(_v2."properties", '$.ideal_width') AS "ideal_width", json_extract(_v2."properties", '$.column_priority') AS "priority" FROM block AS _v0 JOIN block AS _v2 ON _v2.parent_id = _v0.id WHERE _v0."id" = 'block:root-layout' AND _v2."content_type" = 'text';

-- Wait 99ms
-- [actor_exec] 2026-03-19T17:16:03.635952Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_query] 2026-03-19T17:16:03.644311Z
SELECT * FROM watch_view_441ba8cd9ee4ed5d;

-- [actor_exec] 2026-03-19T17:16:03.644597Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.645376Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.646131Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.646892Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.647560Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.648261Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.648911Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.649515Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.650106Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.650680Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.651264Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_exec] 2026-03-19T17:16:03.659286Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.660084Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:03.660864Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.661640Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_64c720ee4172de97';

-- [actor_query] 2026-03-19T17:16:03.661894Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T17:16:03.662081Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_108228dcd523dde5';

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:03.669759Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.670643Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_64c720ee4172de97';

-- [actor_exec] 2026-03-19T17:16:03.670832Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.671446Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_64c720ee4172de97';

-- [actor_exec] 2026-03-19T17:16:03.671694Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.672340Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_64c720ee4172de97 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c' OR parent_id = 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c';

-- Wait 13ms
-- [actor_exec] 2026-03-19T17:16:03.685633Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.686475Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T17:16:03.686673Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T17:16:03.687205Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.687829Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T17:16:03.688057Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:03.695727Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.696497Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_15d1b245264ba81d AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa' OR parent_id = 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa';

-- Wait 5ms
-- [actor_exec] 2026-03-19T17:16:03.702012Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.702682Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_108228dcd523dde5';

-- [actor_query] 2026-03-19T17:16:03.702898Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T17:16:03.703454Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.704123Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_108228dcd523dde5';

-- Wait 8ms
-- [actor_query] 2026-03-19T17:16:03.712124Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_query] 2026-03-19T17:16:03.712465Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_a41eaf3ca30d73c2';

-- [actor_exec] 2026-03-19T17:16:03.712672Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.713479Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_108228dcd523dde5 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c' OR parent_id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c';

-- Wait 6ms
-- [actor_query] 2026-03-19T17:16:03.719492Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_4348389a5df1b560';

-- [actor_exec] 2026-03-19T17:16:03.719683Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_query] 2026-03-19T17:16:03.727826Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T17:16:03.728074Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T17:16:03.728624Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.729343Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T17:16:03.729582Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_exec] 2026-03-19T17:16:03.729733Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.730355Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_a41eaf3ca30d73c2 AS SELECT * FROM document WHERE name <> '' AND name <> 'index' AND name <> '__default__';

-- Wait 14ms
-- [actor_exec] 2026-03-19T17:16:03.745077Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.745870Z
SELECT * FROM watch_view_a41eaf3ca30d73c2;

-- [actor_query] 2026-03-19T17:16:03.746025Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_4348389a5df1b560';

-- [actor_query] 2026-03-19T17:16:03.746231Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_c76e152ae78174ad';

-- [actor_exec] 2026-03-19T17:16:03.746424Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:03.747180Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_4348389a5df1b560';

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:03.754921Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:03.755723Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_4348389a5df1b560 AS WITH RECURSIVE _vl2 AS (SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth, CAST(_v1.id AS TEXT) AS visited FROM block AS _v1 UNION ALL SELECT _fk.id, _vl2.source_id, _vl2.depth + 1, _vl2.visited || ',' || CAST(_fk.id AS TEXT) FROM _vl2 JOIN block _fk ON _fk.parent_id = _vl2.node_id WHERE _vl2.depth < 20 AND ',' || _vl2.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%') SELECT _v3.*, json_extract(_v3."properties", '$.sequence') AS "sequence" FROM focus_roots AS _v0 JOIN block AS _v1 ON _v1."id" = _v0."root_id" JOIN _vl2 ON _vl2.source_id = _v1.id JOIN block AS _v3 ON _v3.id = _vl2.node_id WHERE _v0."region" = 'main' AND _v3."content_type" <> 'source' AND _vl2.depth >= 0 AND _vl2.depth <= 20;

-- Wait 834ms
-- [actor_exec] 2026-03-19T17:16:04.590430Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T17:16:04.598369Z
SELECT * FROM watch_view_4348389a5df1b560;

-- [actor_query] 2026-03-19T17:16:04.598500Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_c76e152ae78174ad';

-- [actor_exec] 2026-03-19T17:16:04.598716Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T17:16:04.599430Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_c76e152ae78174ad';

-- [actor_exec] 2026-03-19T17:16:04.599706Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T17:16:04.600404Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_c76e152ae78174ad AS WITH children AS (SELECT * FROM block WHERE parent_id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c' AND content_type <> 'source') SELECT * FROM children;

-- Wait 31ms
-- [actor_exec] 2026-03-19T17:16:04.632137Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T17:16:04.640049Z
SELECT * FROM watch_view_c76e152ae78174ad;

-- [actor_exec] 2026-03-19T17:16:04.640261Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.641012Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.641757Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_exec] 2026-03-19T17:16:04.649792Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.650572Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.651217Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.658856Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.659640Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.660293Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.660883Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.661513Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.662112Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.662695Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.663263Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.663828Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.664389Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.664943Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.665490Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.673346Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.674085Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.674788Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.682562Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.683416Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.684044Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.684639Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.691960Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.692645Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.693405Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.701209Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.702011Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.702610Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.703208Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.703793Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.704370Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.705062Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.705678Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.706297Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.706888Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.707452Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.708024Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.715809Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.716512Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.717096Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.724738Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.725489Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.726077Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.726637Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.734304Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.734984Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.735545Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.736098Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.743766Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.744480Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.745100Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.745691Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.746302Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.746868Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.747434Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.747984Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.748534Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.749089Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.749704Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.750294Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.750873Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.758471Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.759164Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.759725Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.760273Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.767888Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.768554Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.769104Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.769668Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.777412Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.778089Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.778682Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.779258Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.786840Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.787502Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.788067Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.788648Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.789246Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.789792Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.790372Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.791051Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.791665Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.792251Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.792828Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.793419Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.794019Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.801553Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.802196Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.802755Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.803308Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.810880Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.811582Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.812170Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.812732Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.820468Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.821185Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.821768Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.822323Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.830034Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.830737Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.831342Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.831939Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.832511Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.833082Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.833645Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.834207Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.834763Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.835324Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.835920Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.836487Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.837049Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.844811Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.845481Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.846052Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.846640Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.854301Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.854970Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.855545Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.856091Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.863728Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.864399Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.864981Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.865543Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.873175Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.873843Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.874386Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.874931Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.875469Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.876006Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.876536Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.877070Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.877601Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.878181Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 1ms
-- [actor_exec] 2026-03-19T17:16:04.879441Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.887242Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.887875Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.888445Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.889009Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.889575Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.897067Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.897769Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.898335Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.898909Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.906627Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.907416Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.908205Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_exec] 2026-03-19T17:16:04.916336Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.917153Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.917821Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.918392Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.918944Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.919593Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.920243Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.920835Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.921404Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.921959Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.922567Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.923141Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.931134Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.931964Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.932581Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.940208Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.940859Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.941443Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.942036Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.949669Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.950382Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.951020Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.951652Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.959258Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.959937Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.960559Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.961125Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.961690Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 2ms
-- [actor_exec] 2026-03-19T17:16:04.964401Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.965250Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.965901Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.966471Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.974333Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.974999Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.975551Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.976121Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.983871Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.984602Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.985204Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.985792Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:04.993399Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.994070Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.994674Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:04.995268Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:05.003050Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.003755Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.004315Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.004894Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.005451Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.006076Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.006634Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.007174Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.007722Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.008251Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.008780Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.009307Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.009832Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.010393Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:05.018016Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.018702Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.019265Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.019804Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T17:16:05.027381Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.028071Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.028831Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T17:16:05.029400Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 38684ms
-- [actor_exec] 2026-03-19T17:16:43.714366Z
UPDATE block SET properties = json_set(COALESCE(properties, '{}'), '$.task_state', 'DONE') WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';

-- Wait 48ms
-- [actor_query] 2026-03-19T17:16:43.763235Z
SELECT parent_id FROM block WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';

-- Wait 4ms
-- [actor_query] 2026-03-19T17:16:43.767426Z
SELECT parent_id FROM block WHERE id = 'block:661368d9-e4bd-4722-b5c2-40f32006c643';

-- Wait 3ms
-- [actor_query] 2026-03-19T17:16:43.770791Z
SELECT parent_id FROM block WHERE id = 'block:599b60af-960d-4c9c-b222-d3d9de95c513';

-- Wait 2ms
-- [actor_exec] 2026-03-19T17:16:43.773698Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES (?, ?, ?, ?, ?, ?, ?, ?,;

-- Wait 101ms
-- [actor_query] 2026-03-19T17:16:43.875084Z
UPDATE operation SET status = $new_status WHERE status = $old_status;

-- Wait 94ms
-- [actor_tx_begin] 2026-03-19T17:16:43.969999Z
BEGIN TRANSACTION (0 stmts);

-- [actor_tx_commit] 2026-03-19T17:16:43.970352Z
COMMIT;

-- [actor_query] 2026-03-19T17:16:43.970497Z
INSERT INTO operation (operation, inverse, status, created_at, display_name, entity_name, op_name)
                          VALUES ($operation, $inverse, $status, $created_at, $display_name, $entity_;

-- Wait 2ms
-- [actor_query] 2026-03-19T17:16:43.972708Z
SELECT last_insert_rowid() as id;

-- Wait 1ms
-- [actor_exec] 2026-03-19T17:16:43.973897Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 5ms
-- [actor_query] 2026-03-19T17:16:43.979361Z
SELECT COUNT(*) as count FROM operation;
