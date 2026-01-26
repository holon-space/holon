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

