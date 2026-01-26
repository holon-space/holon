-- Extracted from: /tmp/holon-gpui.log
-- Statements: 481
-- Time range: 2026-03-19T15:38:10.506888Z .. 2026-03-19T15:39:12.789274Z

-- !SET_CHANGE_CALLBACK 2026-03-19T15:38:10.506888Z

-- Wait 9ms
-- [actor_ddl] 2026-03-19T15:38:10.516366Z
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

-- Wait 7ms
-- [actor_ddl] 2026-03-19T15:38:10.524060Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block(parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.525045Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block(document_id);

-- [actor_ddl] 2026-03-19T15:38:10.525212Z
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

-- [actor_ddl] 2026-03-19T15:38:10.525442Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document(parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.525561Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document(name);

-- [actor_ddl] 2026-03-19T15:38:10.525675Z
CREATE TABLE IF NOT EXISTS directory (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    depth INTEGER NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T15:38:10.525848Z
CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory(parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.525973Z
CREATE TABLE IF NOT EXISTS file (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    document_id TEXT,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T15:38:10.526139Z
CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file(parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.526243Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file(document_id);

-- [actor_ddl] 2026-03-19T15:38:10.526399Z
CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

-- [actor_ddl] 2026-03-19T15:38:10.527062Z
CREATE INDEX IF NOT EXISTS idx_navigation_history_region
ON navigation_history(region);

-- [actor_ddl] 2026-03-19T15:38:10.527204Z
CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- [actor_ddl] 2026-03-19T15:38:10.527345Z
DROP VIEW IF EXISTS focus_roots;

-- Wait 4ms
-- [actor_ddl] 2026-03-19T15:38:10.531624Z
DROP VIEW IF EXISTS current_focus;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T15:38:10.532841Z
CREATE MATERIALIZED VIEW current_focus AS
SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Wait 11ms
-- [actor_ddl] 2026-03-19T15:38:10.544227Z
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

-- Wait 122ms
-- [actor_query] 2026-03-19T15:38:10.666915Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:10.668756Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- [actor_query] 2026-03-19T15:38:10.668910Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- [actor_ddl] 2026-03-19T15:38:10.669113Z
CREATE TABLE IF NOT EXISTS sync_states (
    provider_name TEXT PRIMARY KEY NOT NULL,
    sync_token TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T15:38:10.669344Z
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

-- [actor_ddl] 2026-03-19T15:38:10.669535Z
CREATE INDEX IF NOT EXISTS idx_operation_entity_name
ON operation(entity_name);

-- [actor_ddl] 2026-03-19T15:38:10.669666Z
CREATE INDEX IF NOT EXISTS idx_operation_created_at
ON operation(created_at);

-- [actor_ddl] 2026-03-19T15:38:10.669893Z
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

-- Wait 2ms
-- [actor_ddl] 2026-03-19T15:38:10.672817Z
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

-- [actor_ddl] 2026-03-19T15:38:10.673252Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document (parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.673384Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document (name);

-- [actor_query] 2026-03-19T15:38:10.673829Z
INSERT OR IGNORE INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_ddl] 2026-03-19T15:38:10.674386Z
CREATE TABLE IF NOT EXISTS directory (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  depth INTEGER NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T15:38:10.674544Z
CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory (parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.674669Z
CREATE TABLE IF NOT EXISTS file (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  document_id TEXT,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T15:38:10.674839Z
CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file (parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.674950Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file (document_id);

-- [actor_ddl] 2026-03-19T15:38:10.675277Z
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

-- [actor_ddl] 2026-03-19T15:38:10.675466Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block (parent_id);

-- [actor_ddl] 2026-03-19T15:38:10.675570Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block (document_id);

-- [actor_ddl] 2026-03-19T15:38:10.675777Z
CREATE TABLE IF NOT EXISTS sync_states (
  provider_name TEXT PRIMARY KEY NOT NULL,
  sync_token TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T15:38:10.676245Z
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

-- [actor_ddl] 2026-03-19T15:38:10.676528Z
CREATE INDEX IF NOT EXISTS idx_events_loro_pending
ON events(created_at)
WHERE processed_by_loro = 0 AND origin != 'loro' AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T15:38:10.676720Z
CREATE INDEX IF NOT EXISTS idx_events_org_pending
ON events(created_at)
WHERE processed_by_org = 0 AND origin != 'org' AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T15:38:10.676892Z
CREATE INDEX IF NOT EXISTS idx_events_cache_pending
ON events(created_at)
WHERE processed_by_cache = 0 AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T15:38:10.677030Z
CREATE INDEX IF NOT EXISTS idx_events_aggregate
ON events(aggregate_type, aggregate_id, created_at);

-- [actor_ddl] 2026-03-19T15:38:10.677152Z
CREATE INDEX IF NOT EXISTS idx_events_command
ON events(command_id)
WHERE command_id IS NOT NULL;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:10.678199Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T15:38:10.678665Z
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

-- [actor_query] 2026-03-19T15:38:10.678888Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T15:38:10.679192Z
CREATE INDEX IF NOT EXISTS idx_operation_created_at ON operation (created_at);

-- [actor_query] 2026-03-19T15:38:10.679284Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T15:38:10.679829Z
CREATE INDEX IF NOT EXISTS idx_operation_entity_name ON operation (entity_name);

-- [actor_ddl] 2026-03-19T15:38:10.679955Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_b271926fc3f569a8 AS SELECT * FROM document;

-- Wait 11ms
-- [actor_query] 2026-03-19T15:38:10.691597Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_query] 2026-03-19T15:38:10.692029Z
SELECT * FROM watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T15:38:10.692326Z
SELECT * FROM watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T15:38:10.692692Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_e2453b3c0b29a253';

-- Wait 11ms
-- [actor_query] 2026-03-19T15:38:10.704362Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_e2453b3c0b29a253';

-- [actor_query] 2026-03-19T15:38:10.704849Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_e2453b3c0b29a253';

-- [actor_ddl] 2026-03-19T15:38:10.705468Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_e2453b3c0b29a253 AS SELECT id, parent_id, source_language FROM block WHERE content_type = 'source' AND source_language IN ('holon_prql', 'holon_gql', 'holon_sql');

-- Wait 27ms
-- [transaction_stmt] 2026-03-19T15:38:10.732736Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD988Q2QQ31GEEYYXCF', 'directory.created', 'directory', 'Projects', 'org', 'confirmed', '{"change_type":"created","data":{"id":"Projects","name":"Projects","parent_id":"null","depth":1}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.733567Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9KDZHPWKDHEPVNWWE', 'directory.created', 'directory', '.jj', 'org', 'confirmed', '{"data":{"id":".jj","name":".jj","parent_id":"null","depth":1},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.734080Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD99RXAAQMJCS4QJKRW', 'directory.created', 'directory', '.jj/working_copy', 'org', 'confirmed', '{"data":{"id":".jj/working_copy","name":"working_copy","parent_id":".jj","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.734571Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9B3VH1YZV3Q8SJ0WW', 'directory.created', 'directory', '.jj/repo', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo","name":"repo","parent_id":".jj","depth":2}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.735098Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9BZHTMFWSAWGY4X6F', 'directory.created', 'directory', '.jj/repo/op_store', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_store","name":"op_store","parent_id":".jj/repo","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.735576Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9NEK6NDN3P9GCCNFR', 'directory.created', 'directory', '.jj/repo/op_store/operations', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_store/operations","name":"operations","parent_id":".jj/repo/op_store","depth":4}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.736058Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9CSJN2PECDDPXTMZM', 'directory.created', 'directory', '.jj/repo/op_store/views', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_store/views","name":"views","parent_id":".jj/repo/op_store","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.736537Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9MCJX8SGR9F5DX7YA', 'directory.created', 'directory', '.jj/repo/op_heads', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_heads","name":"op_heads","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.737012Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9Y0S523MJ61CFRRWN', 'directory.created', 'directory', '.jj/repo/op_heads/heads', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_heads/heads","name":"heads","parent_id":".jj/repo/op_heads","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.737496Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD96QQ8A17PAZ9QV4ZS', 'directory.created', 'directory', '.jj/repo/index', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/index","name":"index","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.737974Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD94M4C1DVBSDJ59NFR', 'directory.created', 'directory', '.jj/repo/index/op_links', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/op_links","name":"op_links","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.739581Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9H03CANMQYJQBW3AF', 'directory.created', 'directory', '.jj/repo/index/operations', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/operations","name":"operations","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.740059Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9XKV9S9R9NPCAS2CG', 'directory.created', 'directory', '.jj/repo/index/changed_paths', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/changed_paths","name":"changed_paths","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.740543Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9YR4C2Z4GRD9M4EAA', 'directory.created', 'directory', '.jj/repo/index/segments', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/segments","name":"segments","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.741021Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD98TNK0DNG1BC0RJ1K', 'directory.created', 'directory', '.jj/repo/submodule_store', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/submodule_store","name":"submodule_store","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.741501Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD91HMVNCYR0CEF2KJ0', 'directory.created', 'directory', '.jj/repo/store', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/store","name":"store","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.741982Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9PF2FJTYH39WE22VW', 'directory.created', 'directory', '.jj/repo/store/extra', 'org', 'confirmed', '{"data":{"id":".jj/repo/store/extra","name":"extra","parent_id":".jj/repo/store","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.742474Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9HX8VMZ3FE757QW6G', 'directory.created', 'directory', '.jj/repo/store/extra/heads', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/store/extra/heads","name":"heads","parent_id":".jj/repo/store/extra","depth":5}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.742962Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD948XW2Q70C5WKDT4J', 'directory.created', 'directory', '.git', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git","name":".git","parent_id":"null","depth":1}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.743498Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9GNEF87J326RZ9P83', 'directory.created', 'directory', '.git/objects', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects","name":"objects","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.743981Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9GMZHPAPDWPKVT93V', 'directory.created', 'directory', '.git/objects/61', 'org', 'confirmed', '{"data":{"id":".git/objects/61","name":"61","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.745635Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9XGDJ7P1TZRZT9VH7', 'directory.created', 'directory', '.git/objects/0d', 'org', 'confirmed', '{"data":{"id":".git/objects/0d","name":"0d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.748706Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9ENWB9K4GMA06WE95', 'directory.created', 'directory', '.git/objects/95', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/95","name":"95","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.749190Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD969M6MKFVV4KEG2G1', 'directory.created', 'directory', '.git/objects/59', 'org', 'confirmed', '{"data":{"id":".git/objects/59","name":"59","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T15:38:10.752120Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9DVH9NHDE48APNTQZ', 'directory.created', 'directory', '.git/objects/92', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/92","name":"92","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.752601Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9MS8WHVS29FSCDK57', 'directory.created', 'directory', '.git/objects/0c', 'org', 'confirmed', '{"data":{"id":".git/objects/0c","name":"0c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.754195Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD93670YFZG5CBFJDSF', 'directory.created', 'directory', '.git/objects/66', 'org', 'confirmed', '{"data":{"id":".git/objects/66","name":"66","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.754677Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD98VRFVYB7YNG82GPW', 'directory.created', 'directory', '.git/objects/3e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3e","name":"3e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.756170Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9CQ9B6NQ75HJ4BKR1', 'directory.created', 'directory', '.git/objects/50', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/50","name":"50","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.757826Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9EMVGHWH6X3EM26PW', 'directory.created', 'directory', '.git/objects/3b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3b","name":"3b","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.758288Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9BJ9RJRA9JQT04T8D', 'directory.created', 'directory', '.git/objects/6f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/6f","name":"6f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.758753Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9Q816DGQAF8NRSHH4', 'directory.created', 'directory', '.git/objects/03', 'org', 'confirmed', '{"data":{"id":".git/objects/03","name":"03","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.760074Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9MYKQETA2FV502BHY', 'directory.created', 'directory', '.git/objects/9b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/9b","name":"9b","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.760537Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9GP59TJXDPM06CPDH', 'directory.created', 'directory', '.git/objects/9e', 'org', 'confirmed', '{"data":{"id":".git/objects/9e","name":"9e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.760998Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9P87SYZTGGSK5GSRC', 'directory.created', 'directory', '.git/objects/04', 'org', 'confirmed', '{"data":{"id":".git/objects/04","name":"04","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.761505Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9R45TTRV6VN55PEYV', 'directory.created', 'directory', '.git/objects/32', 'org', 'confirmed', '{"data":{"id":".git/objects/32","name":"32","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.764560Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9441AG0Q0HTF9TD3W', 'directory.created', 'directory', '.git/objects/35', 'org', 'confirmed', '{"data":{"id":".git/objects/35","name":"35","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.765022Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD95K9B09SD465VBTEB', 'directory.created', 'directory', '.git/objects/69', 'org', 'confirmed', '{"data":{"id":".git/objects/69","name":"69","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.765482Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9YH6QY5NE976P9RGD', 'directory.created', 'directory', '.git/objects/3c', 'org', 'confirmed', '{"data":{"id":".git/objects/3c","name":"3c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.765944Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9EP42G7FB0JTR6AFH', 'directory.created', 'directory', '.git/objects/56', 'org', 'confirmed', '{"data":{"id":".git/objects/56","name":"56","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.766409Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9CCSC2EGKEQEAJN9N', 'directory.created', 'directory', '.git/objects/51', 'org', 'confirmed', '{"data":{"id":".git/objects/51","name":"51","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.767765Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD983T090Y06AJVK993', 'directory.created', 'directory', '.git/objects/3d', 'org', 'confirmed', '{"data":{"id":".git/objects/3d","name":"3d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.769115Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9BX9CJQGK65GRR2YK', 'directory.created', 'directory', '.git/objects/58', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/58","name":"58","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.773083Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD910CZ8EVSP5W63N09', 'directory.created', 'directory', '.git/objects/67', 'org', 'confirmed', '{"data":{"id":".git/objects/67","name":"67","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.774520Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9EHFZRA48ADS67WD4', 'directory.created', 'directory', '.git/objects/93', 'org', 'confirmed', '{"data":{"id":".git/objects/93","name":"93","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.774983Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9CKQWD7K1RDDENF6E', 'directory.created', 'directory', '.git/objects/94', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/94","name":"94","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.778091Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9MV4R5W2W0Q7D59FB', 'directory.created', 'directory', '.git/objects/60', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/60","name":"60","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.778560Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9R3FC0QR7YH22RHC4', 'directory.created', 'directory', '.git/objects/34', 'org', 'confirmed', '{"data":{"id":".git/objects/34","name":"34","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 4ms
-- [transaction_stmt] 2026-03-19T15:38:10.782580Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9NZ0Q2RRBW40VK4RT', 'directory.created', 'directory', '.git/objects/5a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5a","name":"5a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.784002Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9NP7FAC0A478173S1', 'directory.created', 'directory', '.git/objects/5f', 'org', 'confirmed', '{"data":{"id":".git/objects/5f","name":"5f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.784450Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9GX7G2P03Y0KNG82H', 'directory.created', 'directory', '.git/objects/33', 'org', 'confirmed', '{"data":{"id":".git/objects/33","name":"33","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.784926Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9R1GWARXPPNVA75B0', 'directory.created', 'directory', '.git/objects/05', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/05","name":"05","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T15:38:10.787923Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD90H1FNKMAQZYVXVVQ', 'directory.created', 'directory', '.git/objects/9c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/9c","name":"9c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.788370Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9SNQ35EPPT712TPB3', 'directory.created', 'directory', '.git/objects/02', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/02","name":"02","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.788807Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9QVQ1FARDAD2NHG4S', 'directory.created', 'directory', '.git/objects/a4', 'org', 'confirmed', '{"data":{"id":".git/objects/a4","name":"a4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.789250Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9H6S3SGBDV8ZPN7E4', 'directory.created', 'directory', '.git/objects/b5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/b5","name":"b5","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.789711Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9J5JFVHZ7XWKF9C52', 'directory.created', 'directory', '.git/objects/b2', 'org', 'confirmed', '{"data":{"id":".git/objects/b2","name":"b2","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.790149Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD99B0F200P3NDA5JA0', 'directory.created', 'directory', '.git/objects/d9', 'org', 'confirmed', '{"data":{"id":".git/objects/d9","name":"d9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.790597Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9SB13TMF6264NKGHE', 'directory.created', 'directory', '.git/objects/ac', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ac","name":"ac","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.791034Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9C793QFJWBG3WQHPS', 'directory.created', 'directory', '.git/objects/ad', 'org', 'confirmed', '{"data":{"id":".git/objects/ad","name":"ad","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.791477Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD99VFFJNEG3S1CCE2M', 'directory.created', 'directory', '.git/objects/bb', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/bb","name":"bb","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.791916Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD96MJE5Q98NJXBD2P8', 'directory.created', 'directory', '.git/objects/d7', 'org', 'confirmed', '{"data":{"id":".git/objects/d7","name":"d7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.792352Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9XDV0B0ZZHN9PJBCT', 'directory.created', 'directory', '.git/objects/d0', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d0","name":"d0","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.792799Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9RQB8HNB0HTEN2577', 'directory.created', 'directory', '.git/objects/be', 'org', 'confirmed', '{"data":{"id":".git/objects/be","name":"be","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.793246Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD987JGQVGFGM9E4NMF', 'directory.created', 'directory', '.git/objects/b3', 'org', 'confirmed', '{"data":{"id":".git/objects/b3","name":"b3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.793695Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9FHJ46N85SS6XKBJ8', 'directory.created', 'directory', '.git/objects/df', 'org', 'confirmed', '{"data":{"id":".git/objects/df","name":"df","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.796767Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD93K11664SPMG14R7N', 'directory.created', 'directory', '.git/objects/a5', 'org', 'confirmed', '{"data":{"id":".git/objects/a5","name":"a5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.797266Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9A0CT7BXCV0E6G9PH', 'directory.created', 'directory', '.git/objects/bd', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/bd","name":"bd","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.797711Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9Z9GVFDQ97QGY5Q6C', 'directory.created', 'directory', '.git/objects/d1', 'org', 'confirmed', '{"data":{"id":".git/objects/d1","name":"d1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.798147Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9RYFDCDJ5BZ9RSEAN', 'directory.created', 'directory', '.git/objects/d6', 'org', 'confirmed', '{"data":{"id":".git/objects/d6","name":"d6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.798586Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9W88RCX8Y18NNYXQE', 'directory.created', 'directory', '.git/objects/bc', 'org', 'confirmed', '{"data":{"id":".git/objects/bc","name":"bc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.799028Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9P5MBHMYHA0QX3PYX', 'directory.created', 'directory', '.git/objects/ae', 'org', 'confirmed', '{"data":{"id":".git/objects/ae","name":"ae","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.799506Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9RCBVS8NET26CV9N0', 'directory.created', 'directory', '.git/objects/d8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d8","name":"d8","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.802626Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD91BD2SPSV1Y49V4Y0', 'directory.created', 'directory', '.git/objects/ab', 'org', 'confirmed', '{"data":{"id":".git/objects/ab","name":"ab","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.804125Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9F7NR4KEADXHB33HA', 'directory.created', 'directory', '.git/objects/e5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e5","name":"e5","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.804612Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9XZEKRNT3YF5154DX', 'directory.created', 'directory', '.git/objects/e2', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e2","name":"e2","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.807845Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9TMC5V0DW6PRM5RQ4', 'directory.created', 'directory', '.git/objects/f4', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f4","name":"f4","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.808462Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD910ANHFKYWF3GEJ75', 'directory.created', 'directory', '.git/objects/f3', 'org', 'confirmed', '{"data":{"id":".git/objects/f3","name":"f3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.811606Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9J248Z2JK8EVQSBBY', 'directory.created', 'directory', '.git/objects/c7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c7","name":"c7","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.812171Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9TR50V78YJHF1STC9', 'directory.created', 'directory', '.git/objects/ee', 'org', 'confirmed', '{"data":{"id":".git/objects/ee","name":"ee","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.812634Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9CKKP0C8QVJDCB1JB', 'directory.created', 'directory', '.git/objects/c9', 'org', 'confirmed', '{"data":{"id":".git/objects/c9","name":"c9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.813072Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9WR2G95A0RY5DEHG9', 'directory.created', 'directory', '.git/objects/fd', 'org', 'confirmed', '{"data":{"id":".git/objects/fd","name":"fd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.813509Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9YNG5PJSAJEQH7NGR', 'directory.created', 'directory', '.git/objects/f2', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f2","name":"f2","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.814002Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9G7S8VAXGZWN8HM33', 'directory.created', 'directory', '.git/objects/f5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f5","name":"f5","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.815414Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9Q2QWED8X4K02BHPG', 'directory.created', 'directory', '.git/objects/cf', 'org', 'confirmed', '{"data":{"id":".git/objects/cf","name":"cf","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.815864Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9XS75CS84K20M2J68', 'directory.created', 'directory', '.git/objects/ca', 'org', 'confirmed', '{"data":{"id":".git/objects/ca","name":"ca","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.816295Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9TG785DETSJPK660A', 'directory.created', 'directory', '.git/objects/fe', 'org', 'confirmed', '{"data":{"id":".git/objects/fe","name":"fe","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.816724Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9X2ACKD0W117GDA16', 'directory.created', 'directory', '.git/objects/c8', 'org', 'confirmed', '{"data":{"id":".git/objects/c8","name":"c8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.817146Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD93RVEN2JKKRH10YY5', 'directory.created', 'directory', '.git/objects/fb', 'org', 'confirmed', '{"data":{"id":".git/objects/fb","name":"fb","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.817587Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9YKD6F2DHB3DJKPYF', 'directory.created', 'directory', '.git/objects/ed', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ed","name":"ed","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.818994Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9HPMGWKGBKXWTF26F', 'directory.created', 'directory', '.git/objects/c1', 'org', 'confirmed', '{"data":{"id":".git/objects/c1","name":"c1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.819418Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD90D9BJQ00QRBP7QAC', 'directory.created', 'directory', '.git/objects/c6', 'org', 'confirmed', '{"data":{"id":".git/objects/c6","name":"c6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.819838Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD955JPTAFHDJNT22WK', 'directory.created', 'directory', '.git/objects/ec', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ec","name":"ec","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.820262Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD91TF3JQRQ34GJHC67', 'directory.created', 'directory', '.git/objects/4e', 'org', 'confirmed', '{"data":{"id":".git/objects/4e","name":"4e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.820705Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9F9EM8AD50N6J9E8N', 'directory.created', 'directory', '.git/objects/18', 'org', 'confirmed', '{"data":{"id":".git/objects/18","name":"18","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.821137Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9SNAJNVB3N77YDCT5', 'directory.created', 'directory', '.git/objects/27', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/27","name":"27","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.821563Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9GQZ6FVZSRPSZKJTV', 'directory.created', 'directory', '.git/objects/4b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4b","name":"4b","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.822904Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9FGAT2R7MXG4KVJEN', 'directory.created', 'directory', '.git/objects/pack', 'org', 'confirmed', '{"data":{"id":".git/objects/pack","name":"pack","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.823359Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9T35R77PEJCPTC9HZ', 'directory.created', 'directory', '.git/objects/11', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/11","name":"11","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.823827Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9DCB2C1NZZ19WGWZA', 'directory.created', 'directory', '.git/objects/7d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/7d","name":"7d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.825235Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9XG1J7B0M2TXJGZ19', 'directory.created', 'directory', '.git/objects/7c', 'org', 'confirmed', '{"data":{"id":".git/objects/7c","name":"7c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.825674Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9KGPM3EKY0358715P', 'directory.created', 'directory', '.git/objects/16', 'org', 'confirmed', '{"data":{"id":".git/objects/16","name":"16","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.827098Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD918DWZ076JTWW0H90', 'directory.created', 'directory', '.git/objects/45', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/45","name":"45","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.828432Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9P6PZ3WY943KF78GN', 'directory.created', 'directory', '.git/objects/1f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1f","name":"1f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.829989Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD976MA8SH1FF1S45HT', 'directory.created', 'directory', '.git/objects/73', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/73","name":"73","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.831316Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD938EXXDSEWQYWVR2F', 'directory.created', 'directory', '.git/objects/87', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/87","name":"87","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.832631Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9W4ZN97KN9SKJK6TE', 'directory.created', 'directory', '.git/objects/80', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/80","name":"80","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.833950Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9WKAD4R5HX9NJJAF4', 'directory.created', 'directory', '.git/objects/74', 'org', 'confirmed', '{"data":{"id":".git/objects/74","name":"74","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.835451Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9T2N8ZNEEE76EP0AW', 'directory.created', 'directory', '.git/objects/1a', 'org', 'confirmed', '{"data":{"id":".git/objects/1a","name":"1a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.835882Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD94R80QC82YB9GE17E', 'directory.created', 'directory', '.git/objects/28', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/28","name":"28","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.836304Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDARA8KQV89SSYHP93C', 'directory.created', 'directory', '.git/objects/17', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/17","name":"17","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.836739Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA1SSX2XC6GSTVW24J', 'directory.created', 'directory', '.git/objects/7b', 'org', 'confirmed', '{"data":{"id":".git/objects/7b","name":"7b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.837169Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAG874MYPAR2S0N315', 'directory.created', 'directory', '.git/objects/8f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8f","name":"8f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.838709Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAV5Z50H0R93TKFR16', 'directory.created', 'directory', '.git/objects/7e', 'org', 'confirmed', '{"data":{"id":".git/objects/7e","name":"7e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.839164Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAZ7WBDK8NGRG0J1VA', 'directory.created', 'directory', '.git/objects/10', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/10","name":"10","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.839646Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAGXC3CN11MR8XJB5M', 'directory.created', 'directory', '.git/objects/19', 'org', 'confirmed', '{"data":{"id":".git/objects/19","name":"19","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.840101Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDARA3KZBHENP61G3YK', 'directory.created', 'directory', '.git/objects/4c', 'org', 'confirmed', '{"data":{"id":".git/objects/4c","name":"4c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.841349Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDANJDW31XM8DE97AQC', 'directory.created', 'directory', '.git/objects/26', 'org', 'confirmed', '{"data":{"id":".git/objects/26","name":"26","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.841774Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDABQVXZX14D3ER2HG6', 'directory.created', 'directory', '.git/objects/4d', 'org', 'confirmed', '{"data":{"id":".git/objects/4d","name":"4d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.842202Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAQ4AZDM13GYBGEBSZ', 'directory.created', 'directory', '.git/objects/75', 'org', 'confirmed', '{"data":{"id":".git/objects/75","name":"75","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.842638Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA43AMG7G21M2Q8S0F', 'directory.created', 'directory', '.git/objects/81', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/81","name":"81","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.843066Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAX1P29KB2FFSSY0CC', 'directory.created', 'directory', '.git/objects/86', 'org', 'confirmed', '{"data":{"id":".git/objects/86","name":"86","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.843507Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAPCG3ZTVCYSHPJXTE', 'directory.created', 'directory', '.git/objects/72', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/72","name":"72","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.843939Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDASPHE709DSXFTWDFY', 'directory.created', 'directory', '.git/objects/44', 'org', 'confirmed', '{"data":{"id":".git/objects/44","name":"44","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.844377Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA0SZV24TCER4WR0SD', 'directory.created', 'directory', '.git/objects/2a', 'org', 'confirmed', '{"data":{"id":".git/objects/2a","name":"2a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.845983Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAVXSZ71R1XSXMRAQJ', 'directory.created', 'directory', '.git/objects/2f', 'org', 'confirmed', '{"data":{"id":".git/objects/2f","name":"2f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.846436Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDABANBNJVZGA71D5Y4', 'directory.created', 'directory', '.git/objects/43', 'org', 'confirmed', '{"data":{"id":".git/objects/43","name":"43","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.846898Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA0D6HRGV5VSPDRA4N', 'directory.created', 'directory', '.git/objects/88', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/88","name":"88","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.847335Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAYX415F5QZ42GBSNP', 'directory.created', 'directory', '.git/objects/9f', 'org', 'confirmed', '{"data":{"id":".git/objects/9f","name":"9f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.848645Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAM22BRY2MVKWV5P54', 'directory.created', 'directory', '.git/objects/07', 'org', 'confirmed', '{"data":{"id":".git/objects/07","name":"07","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.849089Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA77BAWZ18EV4HK2RZ', 'directory.created', 'directory', '.git/objects/38', 'org', 'confirmed', '{"data":{"id":".git/objects/38","name":"38","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.850744Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAF6SXV7669G7DT2Y8', 'directory.created', 'directory', '.git/objects/00', 'org', 'confirmed', '{"data":{"id":".git/objects/00","name":"00","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.851186Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDASW4E0E57YEMSBRJ5', 'directory.created', 'directory', '.git/objects/6e', 'org', 'confirmed', '{"data":{"id":".git/objects/6e","name":"6e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.851621Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAWA1P2AM9JKB9TX9Z', 'directory.created', 'directory', '.git/objects/9a', 'org', 'confirmed', '{"data":{"id":".git/objects/9a","name":"9a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.853060Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAGQZB18XBYC068D5R', 'directory.created', 'directory', '.git/objects/5c', 'org', 'confirmed', '{"data":{"id":".git/objects/5c","name":"5c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.854615Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA3F4D2CFSRHTFWG9H', 'directory.created', 'directory', '.git/objects/09', 'org', 'confirmed', '{"data":{"id":".git/objects/09","name":"09","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.855043Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAYJYG817JFED8TND0', 'directory.created', 'directory', '.git/objects/5d', 'org', 'confirmed', '{"data":{"id":".git/objects/5d","name":"5d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.855473Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAN0WXN4C001N6QV60', 'directory.created', 'directory', '.git/objects/info', 'org', 'confirmed', '{"data":{"id":".git/objects/info","name":"info","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.855919Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAHQFWJNZNTB68TNZ8', 'directory.created', 'directory', '.git/objects/91', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/91","name":"91","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.856350Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAEAFQFZQTX67RQE7K', 'directory.created', 'directory', '.git/objects/65', 'org', 'confirmed', '{"data":{"id":".git/objects/65","name":"65","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.856790Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA0Y3PKAYF81QVY3V6', 'directory.created', 'directory', '.git/objects/62', 'org', 'confirmed', '{"data":{"id":".git/objects/62","name":"62","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.857220Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA6TB36YWY0GYW74ZX', 'directory.created', 'directory', '.git/objects/96', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/96","name":"96","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.857664Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDADAFQ600SGH0JVMBY', 'directory.created', 'directory', '.git/objects/3a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3a","name":"3a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.858097Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDATC1Z5QPZ453RSYJ9', 'directory.created', 'directory', '.git/objects/54', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/54","name":"54","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.858528Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAPKJVR4WPTDT0E1GV', 'directory.created', 'directory', '.git/objects/98', 'org', 'confirmed', '{"data":{"id":".git/objects/98","name":"98","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.858962Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA9HJ8X73ATMTD39V5', 'directory.created', 'directory', '.git/objects/53', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/53","name":"53","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.859403Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAMZBWSSS4A8E1ZFNT', 'directory.created', 'directory', '.git/objects/3f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3f","name":"3f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.859887Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAKQVXXR4G0VRAQXM2', 'directory.created', 'directory', '.git/objects/30', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/30","name":"30","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.860331Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAZ01M0P60K4BNMQ60', 'directory.created', 'directory', '.git/objects/5e', 'org', 'confirmed', '{"data":{"id":".git/objects/5e","name":"5e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.860766Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDACAZ412G71BYY2M91', 'directory.created', 'directory', '.git/objects/5b', 'org', 'confirmed', '{"data":{"id":".git/objects/5b","name":"5b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.861197Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA2QEED1DZZBK863P2', 'directory.created', 'directory', '.git/objects/37', 'org', 'confirmed', '{"data":{"id":".git/objects/37","name":"37","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.861632Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAEQMRD9P6WJGTP5ZX', 'directory.created', 'directory', '.git/objects/08', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/08","name":"08","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.862082Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA7171AS8K543N0FPH', 'directory.created', 'directory', '.git/objects/6d', 'org', 'confirmed', '{"data":{"id":".git/objects/6d","name":"6d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.862530Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAV4TSWRWYKGZEG1YT', 'directory.created', 'directory', '.git/objects/01', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/01","name":"01","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.862980Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAQ4WCAA759K52MY1D', 'directory.created', 'directory', '.git/objects/06', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/06","name":"06","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.863439Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA270J75ZDE60PNWM7', 'directory.created', 'directory', '.git/objects/6c', 'org', 'confirmed', '{"data":{"id":".git/objects/6c","name":"6c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.863892Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAVSTBTD1HM8441QYQ', 'directory.created', 'directory', '.git/objects/39', 'org', 'confirmed', '{"data":{"id":".git/objects/39","name":"39","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.864338Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAYR03ZYNNTA5M5REP', 'directory.created', 'directory', '.git/objects/99', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/99","name":"99","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.864791Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA3DE1BV24KHH5N6VR', 'directory.created', 'directory', '.git/objects/52', 'org', 'confirmed', '{"data":{"id":".git/objects/52","name":"52","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.865241Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAFHZD0SG27V1QFQA8', 'directory.created', 'directory', '.git/objects/55', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/55","name":"55","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.865683Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDANHY9JKRK2YAQQ850', 'directory.created', 'directory', '.git/objects/97', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/97","name":"97","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.866154Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA730SZWHK4Q64ZDJF', 'directory.created', 'directory', '.git/objects/0a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/0a","name":"0a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.866605Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAG67HT6Z6NBF7MRAQ', 'directory.created', 'directory', '.git/objects/90', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/90","name":"90","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.867093Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAGXKKDHN5STZH9GAH', 'directory.created', 'directory', '.git/objects/bf', 'org', 'confirmed', '{"data":{"id":".git/objects/bf","name":"bf","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.867548Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA6MRCB4S0NZVN30FH', 'directory.created', 'directory', '.git/objects/d3', 'org', 'confirmed', '{"data":{"id":".git/objects/d3","name":"d3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.868002Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAGXCFRSG1D5Z2SYT3', 'directory.created', 'directory', '.git/objects/d4', 'org', 'confirmed', '{"data":{"id":".git/objects/d4","name":"d4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.868469Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA7WFNJBDNDHJ2M67R', 'directory.created', 'directory', '.git/objects/ba', 'org', 'confirmed', '{"data":{"id":".git/objects/ba","name":"ba","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.868935Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAS5EA1NWQG2C2FK81', 'directory.created', 'directory', '.git/objects/a0', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a0","name":"a0","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.869421Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA0Y1J3A6P4CQ4VH4N', 'directory.created', 'directory', '.git/objects/a7', 'org', 'confirmed', '{"data":{"id":".git/objects/a7","name":"a7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.869885Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAJC30YZ4VV3YJVJZP', 'directory.created', 'directory', '.git/objects/b8', 'org', 'confirmed', '{"data":{"id":".git/objects/b8","name":"b8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.870360Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAG7ME17W12F0RCXAE', 'directory.created', 'directory', '.git/objects/b1', 'org', 'confirmed', '{"data":{"id":".git/objects/b1","name":"b1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.870841Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAV56T2451TQ5R5NVW', 'directory.created', 'directory', '.git/objects/dd', 'org', 'confirmed', '{"data":{"id":".git/objects/dd","name":"dd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.871344Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAV3202KSR75N21ETK', 'directory.created', 'directory', '.git/objects/dc', 'org', 'confirmed', '{"data":{"id":".git/objects/dc","name":"dc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.872867Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA7HMYYP06SR3AHJ07', 'directory.created', 'directory', '.git/objects/b6', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/b6","name":"b6","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.873344Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDARNFKAED9RQRNGY0M', 'directory.created', 'directory', '.git/objects/a9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a9","name":"a9","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.873802Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA0JSB9PKG75W3QNRB', 'directory.created', 'directory', '.git/objects/d5', 'org', 'confirmed', '{"data":{"id":".git/objects/d5","name":"d5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.874264Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAB2VTNFE0XB41WJKX', 'directory.created', 'directory', '.git/objects/d2', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d2","name":"d2","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.874847Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA4QNCWCXKWEXX4SZG', 'directory.created', 'directory', '.git/objects/aa', 'org', 'confirmed', '{"data":{"id":".git/objects/aa","name":"aa","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.875381Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAFEQCV07C1523MFAN', 'directory.created', 'directory', '.git/objects/af', 'org', 'confirmed', '{"data":{"id":".git/objects/af","name":"af","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.875931Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA734NR68RNEXDF92B', 'directory.created', 'directory', '.git/objects/b7', 'org', 'confirmed', '{"data":{"id":".git/objects/b7","name":"b7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.876429Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA3X7TW8B6XF680A9B', 'directory.created', 'directory', '.git/objects/db', 'org', 'confirmed', '{"data":{"id":".git/objects/db","name":"db","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.876913Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA4MV4N4T32WA4AAYT', 'directory.created', 'directory', '.git/objects/a8', 'org', 'confirmed', '{"data":{"id":".git/objects/a8","name":"a8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.877401Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAA4D0HCF6NMM922G2', 'directory.created', 'directory', '.git/objects/de', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/de","name":"de","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.877885Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAZPF9XG2A3SZ393MK', 'directory.created', 'directory', '.git/objects/b0', 'org', 'confirmed', '{"data":{"id":".git/objects/b0","name":"b0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.878361Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAM7C9Z4105FAQN5QC', 'directory.created', 'directory', '.git/objects/b9', 'org', 'confirmed', '{"data":{"id":".git/objects/b9","name":"b9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.878831Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAYFJERNZZAZNGVYM5', 'directory.created', 'directory', '.git/objects/a1', 'org', 'confirmed', '{"data":{"id":".git/objects/a1","name":"a1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.879321Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA987PK0GHGYQG9RRK', 'directory.created', 'directory', '.git/objects/ef', 'org', 'confirmed', '{"data":{"id":".git/objects/ef","name":"ef","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.879812Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAGTHQS01D0DCAPY7T', 'directory.created', 'directory', '.git/objects/c3', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c3","name":"c3","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.880293Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAQWBX00TSBX2FC27A', 'directory.created', 'directory', '.git/objects/c4', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c4","name":"c4","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.881758Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAEXCJQQW2VDPFXPT6', 'directory.created', 'directory', '.git/objects/ea', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ea","name":"ea","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.882248Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAWFNR3BYBC1TP46BZ', 'directory.created', 'directory', '.git/objects/e1', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e1","name":"e1","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.882746Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDADXRPHGQ5N3GZS8NQ', 'directory.created', 'directory', '.git/objects/cd', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/cd","name":"cd","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.883235Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAG1J0KXM0E9Y0A6EC', 'directory.created', 'directory', '.git/objects/cc', 'org', 'confirmed', '{"data":{"id":".git/objects/cc","name":"cc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.883715Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA9GX1WWXYZ476ECQ9', 'directory.created', 'directory', '.git/objects/e6', 'org', 'confirmed', '{"data":{"id":".git/objects/e6","name":"e6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.884194Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDABVPKTXV3J1SBA25R', 'directory.created', 'directory', '.git/objects/f9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f9","name":"f9","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T15:38:10.886998Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAJJ7978F3ZKFY16T3', 'directory.created', 'directory', '.git/objects/f0', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f0","name":"f0","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.888576Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAJQJHHWGV3704HASR', 'directory.created', 'directory', '.git/objects/f7', 'org', 'confirmed', '{"data":{"id":".git/objects/f7","name":"f7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.889102Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA7R4EATX2F3YF6M8D', 'directory.created', 'directory', '.git/objects/e8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e8","name":"e8","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.890601Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAYZ79V6T8G3XVJZP2', 'directory.created', 'directory', '.git/objects/fa', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/fa","name":"fa","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.891085Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA6FR41XJW84AEZVJ0', 'directory.created', 'directory', '.git/objects/ff', 'org', 'confirmed', '{"data":{"id":".git/objects/ff","name":"ff","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.891577Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAWMF7TDN75PNRY4H7', 'directory.created', 'directory', '.git/objects/c5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c5","name":"c5","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.892040Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAQQM5BAQMKR886YJA', 'directory.created', 'directory', '.git/objects/f6', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f6","name":"f6","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.892508Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAA3CT40YN7GEXAKGK', 'directory.created', 'directory', '.git/objects/e9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e9","name":"e9","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.892965Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA4MCM4Y2DNCZCDG62', 'directory.created', 'directory', '.git/objects/f1', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f1","name":"f1","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.894365Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA153QXBXYRVRYKQSG', 'directory.created', 'directory', '.git/objects/e7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e7","name":"e7","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.894839Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDABTTBBRAF6K88V1ZD', 'directory.created', 'directory', '.git/objects/cb', 'org', 'confirmed', '{"data":{"id":".git/objects/cb","name":"cb","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.895305Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAVF4NXSK5VAC3AMQJ', 'directory.created', 'directory', '.git/objects/f8', 'org', 'confirmed', '{"data":{"id":".git/objects/f8","name":"f8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.895770Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAG0D74C24VF50P89C', 'directory.created', 'directory', '.git/objects/ce', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ce","name":"ce","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.897239Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA92DNCM2QKZETRNHY', 'directory.created', 'directory', '.git/objects/e0', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e0","name":"e0","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.897717Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAWKHF24J5J6BKHZXZ', 'directory.created', 'directory', '.git/objects/46', 'org', 'confirmed', '{"data":{"id":".git/objects/46","name":"46","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.898186Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAMM8BW13CJ3SCT323', 'directory.created', 'directory', '.git/objects/2c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/2c","name":"2c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.898691Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAR3ACKVXE85APJ7AT', 'directory.created', 'directory', '.git/objects/79', 'org', 'confirmed', '{"data":{"id":".git/objects/79","name":"79","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.899153Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAJVE6QBS6PZHG23KN', 'directory.created', 'directory', '.git/objects/2d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/2d","name":"2d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.899641Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAH16AVTBJZTE4BBA8', 'directory.created', 'directory', '.git/objects/41', 'org', 'confirmed', '{"data":{"id":".git/objects/41","name":"41","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.900104Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAE95NRKZ26752HJH0', 'directory.created', 'directory', '.git/objects/1b', 'org', 'confirmed', '{"data":{"id":".git/objects/1b","name":"1b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.900571Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDASXT8C8AV2A79WEGC', 'directory.created', 'directory', '.git/objects/77', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/77","name":"77","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.901038Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDATY3TDYXAXASCSAE0', 'directory.created', 'directory', '.git/objects/48', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/48","name":"48","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.901502Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAC9QF6P31B2715YS4', 'directory.created', 'directory', '.git/objects/1e', 'org', 'confirmed', '{"data":{"id":".git/objects/1e","name":"1e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.901974Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAPF4YEENYZFBTPT8N', 'directory.created', 'directory', '.git/objects/84', 'org', 'confirmed', '{"data":{"id":".git/objects/84","name":"84","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.903483Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAH9JZ7CVXGNAFANDW', 'directory.created', 'directory', '.git/objects/4a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4a","name":"4a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.903949Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAJXPQ8FVJ8FFFCE28', 'directory.created', 'directory', '.git/objects/24', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/24","name":"24","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.904433Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA7RDMAJ8SBY5R0VP5', 'directory.created', 'directory', '.git/objects/23', 'org', 'confirmed', '{"data":{"id":".git/objects/23","name":"23","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.904896Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDADJX0Z4MJY0CQRHWD', 'directory.created', 'directory', '.git/objects/4f', 'org', 'confirmed', '{"data":{"id":".git/objects/4f","name":"4f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.905371Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAM58QRJVQAD510B2G', 'directory.created', 'directory', '.git/objects/8d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8d","name":"8d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.905852Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAWEKP7T4747AYCM24', 'directory.created', 'directory', '.git/objects/15', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/15","name":"15","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T15:38:10.908565Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAEFHFHMGH4FVPHASW', 'directory.created', 'directory', '.git/objects/12', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/12","name":"12","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.909037Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAQC8EQPEDDD0B61EP', 'directory.created', 'directory', '.git/objects/85', 'org', 'confirmed', '{"data":{"id":".git/objects/85","name":"85","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.909536Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAHNARHRV28GSBCXKY', 'directory.created', 'directory', '.git/objects/1d', 'org', 'confirmed', '{"data":{"id":".git/objects/1d","name":"1d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.909988Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAFT612C8QGVSR26XE', 'directory.created', 'directory', '.git/objects/71', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/71","name":"71","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.910445Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAXPBB3AWABPZGRNRB', 'directory.created', 'directory', '.git/objects/76', 'org', 'confirmed', '{"data":{"id":".git/objects/76","name":"76","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.910900Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAJVJK3PJFFMBZY9ZA', 'directory.created', 'directory', '.git/objects/1c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1c","name":"1c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.912504Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAZFK88453F9KD8KVB', 'directory.created', 'directory', '.git/objects/82', 'org', 'confirmed', '{"data":{"id":".git/objects/82","name":"82","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.912970Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAA10DH4PCPENXXNAD', 'directory.created', 'directory', '.git/objects/49', 'org', 'confirmed', '{"data":{"id":".git/objects/49","name":"49","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.913451Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA24H8GGQTEW4SSSR4', 'directory.created', 'directory', '.git/objects/40', 'org', 'confirmed', '{"data":{"id":".git/objects/40","name":"40","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.913927Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDADK52RH2BQ5A32CZV', 'directory.created', 'directory', '.git/objects/2e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/2e","name":"2e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.914383Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA4X6RPV30ZGP6FZSZ', 'directory.created', 'directory', '.git/objects/2b', 'org', 'confirmed', '{"data":{"id":".git/objects/2b","name":"2b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.914841Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA03N1WHAFCXA83FSG', 'directory.created', 'directory', '.git/objects/47', 'org', 'confirmed', '{"data":{"id":".git/objects/47","name":"47","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.915305Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAV9WQ71XQFBHTGN25', 'directory.created', 'directory', '.git/objects/78', 'org', 'confirmed', '{"data":{"id":".git/objects/78","name":"78","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.916888Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAD9W4YW8Q0GSN4PG2', 'directory.created', 'directory', '.git/objects/8b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8b","name":"8b","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.917364Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDANNKJQDC29VGMYRW3', 'directory.created', 'directory', '.git/objects/13', 'org', 'confirmed', '{"data":{"id":".git/objects/13","name":"13","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.918842Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAFYPTSZKYMB13G16W', 'directory.created', 'directory', '.git/objects/7a', 'org', 'confirmed', '{"data":{"id":".git/objects/7a","name":"7a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.920456Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAST5NFP2ZZZ1RR83J', 'directory.created', 'directory', '.git/objects/14', 'org', 'confirmed', '{"data":{"id":".git/objects/14","name":"14","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.920909Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA17SSM80V2MP8CQDR', 'directory.created', 'directory', '.git/objects/8e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8e","name":"8e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.921410Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDASBT0R2HQP0GECY72', 'directory.created', 'directory', '.git/objects/22', 'org', 'confirmed', '{"data":{"id":".git/objects/22","name":"22","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.921857Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAZRYBB7GV9VD48WFA', 'directory.created', 'directory', '.git/objects/25', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/25","name":"25","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.922319Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA4AA3DY29TT9HN71S', 'directory.created', 'directory', '.git/rr-cache', 'org', 'confirmed', '{"data":{"id":".git/rr-cache","name":"rr-cache","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.922922Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA8VWY4JDH2E0FHFYD', 'directory.created', 'directory', '.git/info', 'org', 'confirmed', '{"data":{"id":".git/info","name":"info","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.923497Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDASHEJE789MX4E2Z8R', 'directory.created', 'directory', '.git/logs', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/logs","name":"logs","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.924000Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAQMQZN1PRGHJA572G', 'directory.created', 'directory', '.git/logs/refs', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/logs/refs","name":"refs","parent_id":".git/logs","depth":3}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.924485Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAAAVYC5F1QPG7KZP9', 'directory.created', 'directory', '.git/logs/refs/heads', 'org', 'confirmed', '{"data":{"id":".git/logs/refs/heads","name":"heads","parent_id":".git/logs/refs","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.924994Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAWRXN18DARJEXBTQJ', 'directory.created', 'directory', '.git/hooks', 'org', 'confirmed', '{"data":{"id":".git/hooks","name":"hooks","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 3ms
-- [transaction_stmt] 2026-03-19T15:38:10.928028Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA6G1Q1PXYVNGY4V1M', 'directory.created', 'directory', '.git/refs', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/refs","name":"refs","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.928515Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDACJPW3YFR6WEVF01W', 'directory.created', 'directory', '.git/refs/heads', 'org', 'confirmed', '{"data":{"id":".git/refs/heads","name":"heads","parent_id":".git/refs","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.928987Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDARQY5MXRZB3FMWE5X', 'directory.created', 'directory', '.git/refs/tags', 'org', 'confirmed', '{"data":{"id":".git/refs/tags","name":"tags","parent_id":".git/refs","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.930341Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDA8D51T4T8DFDG54WT', 'directory.created', 'directory', '.git/refs/jj', 'org', 'confirmed', '{"data":{"id":".git/refs/jj","name":"jj","parent_id":".git/refs","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.931690Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGDAZ82YRVE35ZFDSJ3W', 'directory.created', 'directory', '.git/refs/jj/keep', 'org', 'confirmed', '{"data":{"id":".git/refs/jj/keep","name":"keep","parent_id":".git/refs/jj","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690730, NULL, NULL);

-- Wait 24ms
-- [actor_query] 2026-03-19T15:38:10.956278Z
SELECT * FROM watch_view_e2453b3c0b29a253;

-- [actor_query] 2026-03-19T15:38:10.956725Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_block';

-- [transaction_stmt] 2026-03-19T15:38:10.957104Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD94R4W45352ABARRN3', 'file.created', 'file', 'file:index.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:index.org","name":"index.org","parent_id":"null","content_hash":"2c45843e5c445c10c43f30dc4aaf59018fe6696700adf391a4347650b1977af2","document_id":null}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.957702Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD98HK70WNWWQK14X7M', 'file.created', 'file', 'file:__default__.org', 'org', 'confirmed', '{"data":{"id":"file:__default__.org","name":"__default__.org","parent_id":"null","content_hash":"9fd72b98d2fdcc99b3a0b4132dd515fa62233e6482c4ae90d39f429f40826f78","document_id":null},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T15:38:10.959324Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9NZFB9886Z0NFVCSW', 'file.created', 'file', 'file:ClaudeCode.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:ClaudeCode.org","name":"ClaudeCode.org","parent_id":"null","content_hash":"e57d79f0cf908c2c3b5a4ef5e5c8f4a5044c05dd4c05fa94ab2f2ae845336566","document_id":null}}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- [transaction_stmt] 2026-03-19T15:38:10.959852Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3BYGD9EQY9J7RGR9SNSBSH', 'file.created', 'file', 'file:Projects/Holon.org', 'org', 'confirmed', '{"data":{"id":"file:Projects/Holon.org","name":"Holon.org","parent_id":"Projects","content_hash":"b42533dcc01eb91e5e075876075c2768286edbad397affec99ec19eacb1e7154","document_id":null},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773934690729, NULL, NULL);

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:10.961242Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T15:38:10.961652Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T15:38:10.961998Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_d77ac41ba85c1706';

-- [actor_ddl] 2026-03-19T15:38:10.962581Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_d77ac41ba85c1706 AS SELECT id, content FROM block WHERE content_type = 'source' AND source_language = 'holon_entity_profile_yaml';

-- Wait 5ms
-- [actor_query] 2026-03-19T15:38:10.968414Z
SELECT * FROM watch_view_d77ac41ba85c1706;

-- Wait 23ms
-- [actor_ddl] 2026-03-19T15:38:10.991800Z
CREATE TABLE IF NOT EXISTS nodes (id INTEGER PRIMARY KEY AUTOINCREMENT);

-- [actor_ddl] 2026-03-19T15:38:10.992032Z
CREATE TABLE IF NOT EXISTS edges (id INTEGER PRIMARY KEY AUTOINCREMENT, source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, type TEXT NOT NULL);

-- [actor_ddl] 2026-03-19T15:38:10.992225Z
CREATE TABLE IF NOT EXISTS property_keys (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT UNIQUE NOT NULL);

-- [actor_ddl] 2026-03-19T15:38:10.992370Z
CREATE TABLE IF NOT EXISTS node_labels (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, label TEXT NOT NULL, PRIMARY KEY (node_id, label));

-- [actor_ddl] 2026-03-19T15:38:10.992571Z
CREATE TABLE IF NOT EXISTS node_props_int (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.992806Z
CREATE TABLE IF NOT EXISTS node_props_text (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.993001Z
CREATE TABLE IF NOT EXISTS node_props_real (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value REAL NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.993193Z
CREATE TABLE IF NOT EXISTS node_props_bool (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.993379Z
CREATE TABLE IF NOT EXISTS node_props_json (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.993605Z
CREATE TABLE IF NOT EXISTS edge_props_int (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.993803Z
CREATE TABLE IF NOT EXISTS edge_props_text (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.993991Z
CREATE TABLE IF NOT EXISTS edge_props_real (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value REAL NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.994184Z
CREATE TABLE IF NOT EXISTS edge_props_bool (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.994363Z
CREATE TABLE IF NOT EXISTS edge_props_json (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T15:38:10.994541Z
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);

-- [actor_ddl] 2026-03-19T15:38:10.994696Z
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);

-- [actor_ddl] 2026-03-19T15:38:10.994833Z
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(type);

-- [actor_ddl] 2026-03-19T15:38:10.994971Z
CREATE INDEX IF NOT EXISTS idx_node_labels_label ON node_labels(label, node_id);

-- [actor_ddl] 2026-03-19T15:38:10.995110Z
CREATE INDEX IF NOT EXISTS idx_property_keys_key ON property_keys(key);

-- [actor_ddl] 2026-03-19T15:38:10.995235Z
CREATE INDEX IF NOT EXISTS idx_node_props_int_key_value ON node_props_int(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T15:38:10.995386Z
CREATE INDEX IF NOT EXISTS idx_node_props_text_key_value ON node_props_text(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T15:38:10.995533Z
CREATE INDEX IF NOT EXISTS idx_node_props_real_key_value ON node_props_real(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T15:38:10.995675Z
CREATE INDEX IF NOT EXISTS idx_node_props_bool_key_value ON node_props_bool(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T15:38:10.995817Z
CREATE INDEX IF NOT EXISTS idx_node_props_json_key_value ON node_props_json(key_id, node_id);

-- [actor_ddl] 2026-03-19T15:38:10.995954Z
CREATE INDEX IF NOT EXISTS idx_edge_props_int_key_value ON edge_props_int(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T15:38:10.996084Z
CREATE INDEX IF NOT EXISTS idx_edge_props_text_key_value ON edge_props_text(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T15:38:10.996209Z
CREATE INDEX IF NOT EXISTS idx_edge_props_real_key_value ON edge_props_real(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T15:38:10.996394Z
CREATE INDEX IF NOT EXISTS idx_edge_props_bool_key_value ON edge_props_bool(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T15:38:10.996597Z
CREATE INDEX IF NOT EXISTS idx_edge_props_json_key_value ON edge_props_json(key_id, edge_id);

-- Wait 57ms
-- [actor_query] 2026-03-19T15:38:11.053636Z
SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%';

-- [actor_ddl] 2026-03-19T15:38:11.054368Z
DROP VIEW IF EXISTS watch_view_1570347602dda3f9;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.056063Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_1570347602dda3f9';

-- [actor_ddl] 2026-03-19T15:38:11.056629Z
DROP VIEW IF EXISTS watch_view_dd27958f4ec0f8e7;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.058269Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_dd27958f4ec0f8e7';

-- [actor_ddl] 2026-03-19T15:38:11.058765Z
DROP VIEW IF EXISTS watch_view_3b8f070830f6b4d1;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.059928Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_3b8f070830f6b4d1';

-- [actor_ddl] 2026-03-19T15:38:11.060430Z
DROP VIEW IF EXISTS watch_view_441ba8cd9ee4ed5d;

-- Wait 2ms
-- [actor_query] 2026-03-19T15:38:11.062971Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_441ba8cd9ee4ed5d';

-- [actor_ddl] 2026-03-19T15:38:11.063458Z
DROP VIEW IF EXISTS watch_view_64c720ee4172de97;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.064516Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_64c720ee4172de97';

-- [actor_ddl] 2026-03-19T15:38:11.064961Z
DROP VIEW IF EXISTS watch_view_15d1b245264ba81d;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.066211Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_15d1b245264ba81d';

-- [actor_ddl] 2026-03-19T15:38:11.066644Z
DROP VIEW IF EXISTS watch_view_108228dcd523dde5;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.067770Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_108228dcd523dde5';

-- [actor_ddl] 2026-03-19T15:38:11.068233Z
DROP VIEW IF EXISTS watch_view_4348389a5df1b560;

-- Wait 5ms
-- [actor_query] 2026-03-19T15:38:11.073945Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_4348389a5df1b560';

-- [actor_ddl] 2026-03-19T15:38:11.074393Z
DROP VIEW IF EXISTS watch_view_a41eaf3ca30d73c2;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.075447Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_a41eaf3ca30d73c2';

-- [actor_ddl] 2026-03-19T15:38:11.075836Z
DROP VIEW IF EXISTS watch_view_c76e152ae78174ad;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.076867Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_c76e152ae78174ad';

-- [actor_ddl] 2026-03-19T15:38:11.077264Z
DROP VIEW IF EXISTS watch_view_5e4c31e8664a1ce3;

-- [actor_query] 2026-03-19T15:38:11.078241Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_5e4c31e8664a1ce3';

-- [actor_ddl] 2026-03-19T15:38:11.078636Z
DROP VIEW IF EXISTS watch_view_226d0677b6b77cbb;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.079737Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_226d0677b6b77cbb';

-- [actor_ddl] 2026-03-19T15:38:11.080111Z
DROP VIEW IF EXISTS watch_view_bb3bb45b22aca539;

-- [actor_query] 2026-03-19T15:38:11.081033Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_bb3bb45b22aca539';

-- [actor_ddl] 2026-03-19T15:38:11.081424Z
DROP VIEW IF EXISTS watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T15:38:11.082310Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T15:38:11.082657Z
DROP VIEW IF EXISTS watch_view_e2453b3c0b29a253;

-- [actor_query] 2026-03-19T15:38:11.083611Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_e2453b3c0b29a253';

-- [actor_ddl] 2026-03-19T15:38:11.083930Z
DROP VIEW IF EXISTS watch_view_d77ac41ba85c1706;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.085037Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T15:38:11.085390Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_1570347602dda3f9';

-- [actor_ddl] 2026-03-19T15:38:11.085648Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_1570347602dda3f9 AS SELECT id, parent_id, content, content_type, source_language, block._change_origin AS _change_origin FROM block;

-- Wait 30ms
-- [actor_query] 2026-03-19T15:38:11.115981Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_dd27958f4ec0f8e7';

-- [actor_ddl] 2026-03-19T15:38:11.116345Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_dd27958f4ec0f8e7 AS SELECT id, content, block._change_origin AS _change_origin FROM block WHERE content_type = 'text';

-- Wait 26ms
-- [actor_query] 2026-03-19T15:38:11.143117Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_block';

-- [actor_query] 2026-03-19T15:38:11.143532Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_directory';

-- [actor_query] 2026-03-19T15:38:11.143849Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_file';

-- [actor_query] 2026-03-19T15:38:11.144227Z
SELECT id FROM block WHERE id = 'block:root-layout';

-- [actor_query] 2026-03-19T15:38:11.144467Z
SELECT document_id FROM block WHERE id = 'block:root-layout' AND document_id != 'doc:__default__';

-- [actor_exec] 2026-03-19T15:38:11.144731Z
DELETE FROM block WHERE document_id = 'doc:__default__';

-- [actor_exec] 2026-03-19T15:38:11.145038Z
DELETE FROM document WHERE id = 'doc:__default__';

-- Wait 2ms
-- [actor_query] 2026-03-19T15:38:11.147161Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_3b8f070830f6b4d1';

-- [actor_query] 2026-03-19T15:38:11.147443Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_3b8f070830f6b4d1';

-- [actor_query] 2026-03-19T15:38:11.147701Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_3b8f070830f6b4d1';

-- [actor_ddl] 2026-03-19T15:38:11.148118Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_3b8f070830f6b4d1 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:root-layout' OR parent_id = 'block:root-layout';

-- Wait 9ms
-- [actor_query] 2026-03-19T15:38:11.157609Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.158665Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:11.160382Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_441ba8cd9ee4ed5d';

-- [actor_query] 2026-03-19T15:38:11.160695Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_441ba8cd9ee4ed5d';

-- [actor_query] 2026-03-19T15:38:11.160971Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_441ba8cd9ee4ed5d';

-- [actor_ddl] 2026-03-19T15:38:11.161486Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_441ba8cd9ee4ed5d AS SELECT _v2.*, json_extract(_v2."properties", '$.sequence') AS "sequence", json_extract(_v2."properties", '$.collapse_to') AS "collapse_to", json_extract(_v2."properties", '$.ideal_width') AS "ideal_width", json_extract(_v2."properties", '$.column_priority') AS "priority" FROM block AS _v0 JOIN block AS _v2 ON _v2.parent_id = _v0.id WHERE _v0."id" = 'block:root-layout' AND _v2."content_type" = 'text';

-- Wait 145ms
-- [actor_query] 2026-03-19T15:38:11.307213Z
SELECT * FROM watch_view_441ba8cd9ee4ed5d;

-- Wait 17ms
-- [actor_query] 2026-03-19T15:38:11.324685Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_64c720ee4172de97';

-- [actor_query] 2026-03-19T15:38:11.325155Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T15:38:11.325426Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_108228dcd523dde5';

-- [actor_query] 2026-03-19T15:38:11.325677Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_64c720ee4172de97';

-- [actor_query] 2026-03-19T15:38:11.325958Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_64c720ee4172de97';

-- [actor_ddl] 2026-03-19T15:38:11.326442Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_64c720ee4172de97 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c' OR parent_id = 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c';

-- Wait 8ms
-- [actor_query] 2026-03-19T15:38:11.334944Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T15:38:11.335251Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_query] 2026-03-19T15:38:11.336126Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T15:38:11.336524Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_ddl] 2026-03-19T15:38:11.336959Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_15d1b245264ba81d AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa' OR parent_id = 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa';

-- Wait 8ms
-- [actor_query] 2026-03-19T15:38:11.345455Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_108228dcd523dde5';

-- [actor_query] 2026-03-19T15:38:11.345775Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_query] 2026-03-19T15:38:11.346639Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_108228dcd523dde5';

-- [actor_query] 2026-03-19T15:38:11.347031Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_ddl] 2026-03-19T15:38:11.347481Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_108228dcd523dde5 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c' OR parent_id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c';

-- Wait 8ms
-- [actor_query] 2026-03-19T15:38:11.356131Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_4348389a5df1b560';

-- [actor_query] 2026-03-19T15:38:11.356528Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_4348389a5df1b560';

-- [actor_query] 2026-03-19T15:38:11.356785Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_query] 2026-03-19T15:38:11.357596Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_4348389a5df1b560';

-- [actor_query] 2026-03-19T15:38:11.357973Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_ddl] 2026-03-19T15:38:11.358708Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_4348389a5df1b560 AS WITH RECURSIVE _vl2 AS (SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth, CAST(_v1.id AS TEXT) AS visited FROM block AS _v1 UNION ALL SELECT _fk.id, _vl2.source_id, _vl2.depth + 1, _vl2.visited || ',' || CAST(_fk.id AS TEXT) FROM _vl2 JOIN block _fk ON _fk.parent_id = _vl2.node_id WHERE _vl2.depth < 20 AND ',' || _vl2.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%') SELECT _v3.*, json_extract(_v3."properties", '$.sequence') AS "sequence" FROM focus_roots AS _v0 JOIN block AS _v1 ON _v1."id" = _v0."root_id" JOIN _vl2 ON _vl2.source_id = _v1.id JOIN block AS _v3 ON _v3.id = _vl2.node_id WHERE _v0."region" = 'main' AND _v3."content_type" <> 'source' AND _vl2.depth >= 0 AND _vl2.depth <= 20;

-- Wait 1315ms
-- [actor_query] 2026-03-19T15:38:12.674114Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T15:38:12.674591Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_c76e152ae78174ad';

-- [actor_query] 2026-03-19T15:38:12.674880Z
SELECT * FROM watch_view_4348389a5df1b560;

-- [actor_query] 2026-03-19T15:38:12.675269Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T15:38:12.675582Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_a41eaf3ca30d73c2';

-- [actor_ddl] 2026-03-19T15:38:12.676092Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_a41eaf3ca30d73c2 AS SELECT * FROM document WHERE name <> '' AND name <> 'index' AND name <> '__default__';

-- Wait 11ms
-- [actor_query] 2026-03-19T15:38:12.687215Z
SELECT * FROM watch_view_a41eaf3ca30d73c2;

-- [actor_query] 2026-03-19T15:38:12.687481Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_c76e152ae78174ad';

-- [actor_query] 2026-03-19T15:38:12.687883Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_c76e152ae78174ad';

-- [actor_ddl] 2026-03-19T15:38:12.688418Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_c76e152ae78174ad AS WITH children AS (SELECT * FROM block WHERE parent_id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c' AND content_type <> 'source') SELECT * FROM children;

-- Wait 51ms
-- [actor_query] 2026-03-19T15:38:12.740131Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_5e4c31e8664a1ce3';

-- [actor_query] 2026-03-19T15:38:12.740518Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_226d0677b6b77cbb';

-- [actor_query] 2026-03-19T15:38:12.740815Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_bb3bb45b22aca539';

-- [actor_query] 2026-03-19T15:38:12.741094Z
SELECT * FROM watch_view_c76e152ae78174ad;

-- [actor_query] 2026-03-19T15:38:12.741385Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_5e4c31e8664a1ce3';

-- [actor_query] 2026-03-19T15:38:12.741696Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_5e4c31e8664a1ce3';

-- [actor_ddl] 2026-03-19T15:38:12.742219Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_5e4c31e8664a1ce3 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cc-projects' OR parent_id = 'block:cc-projects';

-- Wait 8ms
-- [actor_query] 2026-03-19T15:38:12.750706Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_226d0677b6b77cbb';

-- [actor_query] 2026-03-19T15:38:12.751085Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_query] 2026-03-19T15:38:12.751959Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_226d0677b6b77cbb';

-- [actor_query] 2026-03-19T15:38:12.752364Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T15:38:12.753830Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_226d0677b6b77cbb AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cc-sessions' OR parent_id = 'block:cc-sessions';

-- Wait 8ms
-- [actor_query] 2026-03-19T15:38:12.762193Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_bb3bb45b22aca539';

-- [actor_query] 2026-03-19T15:38:12.762521Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_query] 2026-03-19T15:38:12.763462Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_bb3bb45b22aca539';

-- [actor_query] 2026-03-19T15:38:12.763877Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T15:38:12.765188Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_bb3bb45b22aca539 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cc-tasks' OR parent_id = 'block:cc-tasks';

-- Wait 8ms
-- [actor_query] 2026-03-19T15:38:12.773617Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_query] 2026-03-19T15:38:12.774566Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 2ms
-- [actor_query] 2026-03-19T15:38:12.776900Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_f790ad84acba28d';

-- [actor_query] 2026-03-19T15:38:12.777248Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_f790ad84acba28d';

-- [actor_query] 2026-03-19T15:38:12.777567Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_f790ad84acba28d';

-- Wait 11ms
-- [actor_query] 2026-03-19T15:38:12.788636Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_2ed15df44eed46a0';

-- Wait 10ms
-- [actor_query] 2026-03-19T15:38:12.798984Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_461f6dd248aa8888';

-- Wait 29983ms
-- [actor_query] 2026-03-19T15:38:42.782697Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_2ed15df44eed46a0';

-- [actor_query] 2026-03-19T15:38:42.783356Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_2ed15df44eed46a0';

-- Wait 1586ms
-- [actor_exec] 2026-03-19T15:38:44.370097Z
UPDATE block SET properties = json_set(COALESCE(properties, '{}'), '$.task_state', 'TODO') WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';

-- Wait 12ms
-- [actor_query] 2026-03-19T15:38:44.382663Z
SELECT parent_id FROM block WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';

-- [actor_query] 2026-03-19T15:38:44.383085Z
SELECT parent_id FROM block WHERE id = 'block:661368d9-e4bd-4722-b5c2-40f32006c643';

-- [actor_query] 2026-03-19T15:38:44.383347Z
SELECT parent_id FROM block WHERE id = 'block:599b60af-960d-4c9c-b222-d3d9de95c513';

-- [actor_exec] 2026-03-19T15:38:44.383608Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES (?, ?, ?, ?, ?, ?, ?, ?,;

-- Wait 12ms
-- [actor_query] 2026-03-19T15:38:44.396506Z
UPDATE operation SET status = $new_status WHERE status = $old_status;

-- [actor_exec] 2026-03-19T15:38:44.396959Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 1ms
-- [actor_query] 2026-03-19T15:38:44.398951Z
INSERT INTO operation (operation, inverse, status, created_at, display_name, entity_name, op_name)
                          VALUES ($operation, $inverse, $status, $created_at, $display_name, $entity_;

-- [actor_query] 2026-03-19T15:38:44.399372Z
SELECT last_insert_rowid() as id;

-- [actor_query] 2026-03-19T15:38:44.399848Z
SELECT COUNT(*) as count FROM operation;

-- Wait 28386ms
-- [actor_query] 2026-03-19T15:39:12.786680Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_461f6dd248aa8888';

-- Wait 2ms
-- [actor_query] 2026-03-19T15:39:12.789274Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_461f6dd248aa8888';

