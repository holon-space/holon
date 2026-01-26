-- Extracted from: /tmp/holon-gpui-fresh.log
-- Statements: 1573
-- Time range: 2026-03-19T16:50:23.735103Z .. 2026-03-19T16:51:31.898087Z

-- !SET_CHANGE_CALLBACK 2026-03-19T16:50:23.735103Z

-- Wait 8ms
-- [actor_ddl] 2026-03-19T16:50:23.743283Z
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
-- [actor_ddl] 2026-03-19T16:50:23.758230Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block(parent_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T16:50:23.759633Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block(document_id);

-- [actor_ddl] 2026-03-19T16:50:23.760042Z
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

-- [actor_ddl] 2026-03-19T16:50:23.760516Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document(parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.760871Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document(name);

-- [actor_ddl] 2026-03-19T16:50:23.761200Z
CREATE TABLE IF NOT EXISTS directory (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    depth INTEGER NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T16:50:23.761701Z
CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory(parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.762097Z
CREATE TABLE IF NOT EXISTS file (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    document_id TEXT,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T16:50:23.762547Z
CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file(parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.762913Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file(document_id);

-- [actor_ddl] 2026-03-19T16:50:23.763356Z
CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

-- Wait 2ms
-- [actor_ddl] 2026-03-19T16:50:23.765545Z
CREATE INDEX IF NOT EXISTS idx_navigation_history_region
ON navigation_history(region);

-- [actor_ddl] 2026-03-19T16:50:23.765946Z
CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- [actor_ddl] 2026-03-19T16:50:23.766395Z
DROP VIEW IF EXISTS focus_roots;

-- [actor_ddl] 2026-03-19T16:50:23.766918Z
DROP VIEW IF EXISTS current_focus;

-- [actor_ddl] 2026-03-19T16:50:23.766986Z
CREATE MATERIALIZED VIEW current_focus AS
SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Wait 4ms
-- [actor_ddl] 2026-03-19T16:50:23.771973Z
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

-- Wait 6ms
-- [actor_query] 2026-03-19T16:50:23.778830Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- Wait 1ms
-- [actor_query] 2026-03-19T16:50:23.780455Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- [actor_query] 2026-03-19T16:50:23.780842Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ($region, NULL);

-- [actor_ddl] 2026-03-19T16:50:23.781129Z
CREATE TABLE IF NOT EXISTS sync_states (
    provider_name TEXT PRIMARY KEY NOT NULL,
    sync_token TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T16:50:23.781698Z
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

-- [actor_ddl] 2026-03-19T16:50:23.782196Z
CREATE INDEX IF NOT EXISTS idx_operation_entity_name
ON operation(entity_name);

-- [actor_ddl] 2026-03-19T16:50:23.782631Z
CREATE INDEX IF NOT EXISTS idx_operation_created_at
ON operation(created_at);

-- [actor_ddl] 2026-03-19T16:50:23.783155Z
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
-- [actor_ddl] 2026-03-19T16:50:23.819848Z
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

-- [actor_ddl] 2026-03-19T16:50:23.820236Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document (parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.820344Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document (name);

-- [actor_query] 2026-03-19T16:50:23.820683Z
INSERT OR IGNORE INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_ddl] 2026-03-19T16:50:23.821307Z
CREATE TABLE IF NOT EXISTS directory (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  depth INTEGER NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T16:50:23.821454Z
CREATE INDEX IF NOT EXISTS idx_directory_parent_id ON directory (parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.821566Z
CREATE TABLE IF NOT EXISTS file (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  document_id TEXT,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T16:50:23.821691Z
CREATE INDEX IF NOT EXISTS idx_file_parent_id ON file (parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.821774Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file (document_id);

-- [actor_ddl] 2026-03-19T16:50:23.822052Z
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

-- [actor_ddl] 2026-03-19T16:50:23.822259Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block (parent_id);

-- [actor_ddl] 2026-03-19T16:50:23.822454Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block (document_id);

-- [actor_ddl] 2026-03-19T16:50:23.822633Z
CREATE TABLE IF NOT EXISTS sync_states (
  provider_name TEXT PRIMARY KEY NOT NULL,
  sync_token TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T16:50:23.822934Z
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

-- [actor_ddl] 2026-03-19T16:50:23.823823Z
CREATE INDEX IF NOT EXISTS idx_events_loro_pending
ON events(created_at)
WHERE processed_by_loro = 0 AND origin != 'loro' AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T16:50:23.824540Z
CREATE INDEX IF NOT EXISTS idx_events_org_pending
ON events(created_at)
WHERE processed_by_org = 0 AND origin != 'org' AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T16:50:23.825059Z
CREATE INDEX IF NOT EXISTS idx_events_cache_pending
ON events(created_at)
WHERE processed_by_cache = 0 AND status = 'confirmed';

-- [actor_ddl] 2026-03-19T16:50:23.825680Z
CREATE INDEX IF NOT EXISTS idx_events_aggregate
ON events(aggregate_type, aggregate_id, created_at);

-- [actor_ddl] 2026-03-19T16:50:23.826173Z
CREATE INDEX IF NOT EXISTS idx_events_command
ON events(command_id)
WHERE command_id IS NOT NULL;

-- Wait 1ms
-- [actor_ddl] 2026-03-19T16:50:23.827438Z
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

-- [actor_query] 2026-03-19T16:50:23.827697Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T16:50:23.827898Z
CREATE INDEX IF NOT EXISTS idx_operation_created_at ON operation (created_at);

-- [actor_ddl] 2026-03-19T16:50:23.828017Z
CREATE INDEX IF NOT EXISTS idx_operation_entity_name ON operation (entity_name);

-- [actor_query] 2026-03-19T16:50:23.828134Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_query] 2026-03-19T16:50:23.828334Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_query] 2026-03-19T16:50:23.828595Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_query] 2026-03-19T16:50:23.828732Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T16:50:23.828869Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_b271926fc3f569a8 AS SELECT * FROM document;

-- Wait 7ms
-- [actor_query] 2026-03-19T16:50:23.835971Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T16:50:23.836259Z
DROP TABLE IF EXISTS __turso_internal_dbsp_state_v1_watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T16:50:23.836340Z
SELECT * FROM watch_view_b271926fc3f569a8;

-- [actor_ddl] 2026-03-19T16:50:23.836551Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_b271926fc3f569a8 AS SELECT * FROM document;

-- [actor_query] 2026-03-19T16:50:23.836676Z
SELECT * FROM watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T16:50:23.836865Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_e2453b3c0b29a253';

-- [actor_query] 2026-03-19T16:50:23.837024Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_e2453b3c0b29a253';

-- [actor_query] 2026-03-19T16:50:23.837210Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_e2453b3c0b29a253';

-- [actor_query] 2026-03-19T16:50:23.837401Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_block';

-- [actor_ddl] 2026-03-19T16:50:23.837509Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_e2453b3c0b29a253 AS SELECT id, parent_id, source_language FROM block WHERE content_type = 'source' AND source_language IN ('holon_prql', 'holon_gql', 'holon_sql');

-- Wait 15ms
-- [actor_ddl] 2026-03-19T16:50:23.853452Z
CREATE MATERIALIZED VIEW events_view_block AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'block';

-- Wait 10ms
-- [actor_query] 2026-03-19T16:50:23.864263Z
SELECT * FROM watch_view_e2453b3c0b29a253;

-- [actor_query] 2026-03-19T16:50:23.864528Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T16:50:23.864702Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T16:50:23.864842Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_d77ac41ba85c1706';

-- [actor_ddl] 2026-03-19T16:50:23.865071Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_d77ac41ba85c1706 AS SELECT id, content FROM block WHERE content_type = 'source' AND source_language = 'holon_entity_profile_yaml';

-- Wait 2ms
-- [actor_query] 2026-03-19T16:50:23.868008Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_query] 2026-03-19T16:50:23.868465Z
SELECT * FROM watch_view_d77ac41ba85c1706;

-- [actor_ddl] 2026-03-19T16:50:23.868922Z
CREATE TABLE IF NOT EXISTS nodes (id INTEGER PRIMARY KEY AUTOINCREMENT);

-- [actor_ddl] 2026-03-19T16:50:23.869527Z
CREATE TABLE IF NOT EXISTS edges (id INTEGER PRIMARY KEY AUTOINCREMENT, source_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, target_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, type TEXT NOT NULL);

-- [actor_ddl] 2026-03-19T16:50:23.870074Z
CREATE TABLE IF NOT EXISTS property_keys (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT UNIQUE NOT NULL);

-- Wait 4ms
-- [actor_ddl] 2026-03-19T16:50:23.875043Z
CREATE TABLE IF NOT EXISTS node_labels (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, label TEXT NOT NULL, PRIMARY KEY (node_id, label));

-- [actor_ddl] 2026-03-19T16:50:23.876008Z
CREATE TABLE IF NOT EXISTS node_props_int (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.876939Z
CREATE TABLE IF NOT EXISTS node_props_text (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.877889Z
CREATE TABLE IF NOT EXISTS node_props_real (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value REAL NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.878661Z
CREATE TABLE IF NOT EXISTS node_props_bool (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.879313Z
CREATE TABLE IF NOT EXISTS node_props_json (node_id INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (node_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.879914Z
CREATE TABLE IF NOT EXISTS edge_props_int (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.880515Z
CREATE TABLE IF NOT EXISTS edge_props_text (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.881125Z
CREATE TABLE IF NOT EXISTS edge_props_real (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value REAL NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [actor_ddl] 2026-03-19T16:50:23.881739Z
CREATE TABLE IF NOT EXISTS edge_props_bool (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value INTEGER NOT NULL, PRIMARY KEY (edge_id, key_id));

-- [transaction_stmt] 2026-03-19T16:50:23.882556Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0735MB8DFKM21QF6WZ', 'directory.created', 'directory', 'Projects', 'org', 'confirmed', '{"change_type":"created","data":{"id":"Projects","name":"Projects","parent_id":"null","depth":1}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.883014Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07S295BM318CMEMH3V', 'directory.created', 'directory', '.jj', 'org', 'confirmed', '{"data":{"id":".jj","name":".jj","parent_id":"null","depth":1},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.883203Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07HS9RXY5XE27ZWHB6', 'directory.created', 'directory', '.jj/working_copy', 'org', 'confirmed', '{"data":{"id":".jj/working_copy","name":"working_copy","parent_id":".jj","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.883402Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CPQHCVNC29TVRK7W', 'directory.created', 'directory', '.jj/repo', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo","name":"repo","parent_id":".jj","depth":2}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.883574Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0706SBRHVN7YGPQCSF', 'directory.created', 'directory', '.jj/repo/op_store', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_store","name":"op_store","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.883745Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07XT3QVH8C02RHXQ5Z', 'directory.created', 'directory', '.jj/repo/op_store/operations', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_store/operations","name":"operations","parent_id":".jj/repo/op_store","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.883939Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07P8RZ59SM1ZERAWB0', 'directory.created', 'directory', '.jj/repo/op_store/views', 'org', 'confirmed', '{"data":{"id":".jj/repo/op_store/views","name":"views","parent_id":".jj/repo/op_store","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.884175Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R078JV0SZWE74M49TGQ', 'directory.created', 'directory', '.jj/repo/op_heads', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_heads","name":"op_heads","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.884383Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CHD9TWEV7WCJRX6P', 'directory.created', 'directory', '.jj/repo/op_heads/heads', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/op_heads/heads","name":"heads","parent_id":".jj/repo/op_heads","depth":4}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.884578Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CJMEMDYC6R9M5ADQ', 'directory.created', 'directory', '.jj/repo/index', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/index","name":"index","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.884766Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R077PTAGKPYN3Q15EN4', 'directory.created', 'directory', '.jj/repo/index/op_links', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/op_links","name":"op_links","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.884951Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RAM3WJC7G0Z2JDQA', 'directory.created', 'directory', '.jj/repo/index/operations', 'org', 'confirmed', '{"data":{"id":".jj/repo/index/operations","name":"operations","parent_id":".jj/repo/index","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.885133Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07H57G3YGJVZYR9D83', 'directory.created', 'directory', '.jj/repo/index/changed_paths', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/index/changed_paths","name":"changed_paths","parent_id":".jj/repo/index","depth":4}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.885313Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074RH625FM369ADMFF', 'directory.created', 'directory', '.jj/repo/index/segments', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/index/segments","name":"segments","parent_id":".jj/repo/index","depth":4}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.885491Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TACBV9TZKKR4ZZDX', 'directory.created', 'directory', '.jj/repo/submodule_store', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/submodule_store","name":"submodule_store","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.885671Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07YE15EAH6KSXFVE7G', 'directory.created', 'directory', '.jj/repo/store', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/store","name":"store","parent_id":".jj/repo","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.885876Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07XBJX5H52DMEH9JZ0', 'directory.created', 'directory', '.jj/repo/store/extra', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/store/extra","name":"extra","parent_id":".jj/repo/store","depth":4}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.886062Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R075B0E4B71R5K9NDHS', 'directory.created', 'directory', '.jj/repo/store/extra/heads', 'org', 'confirmed', '{"change_type":"created","data":{"id":".jj/repo/store/extra/heads","name":"heads","parent_id":".jj/repo/store/extra","depth":5}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.886242Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07C9BFK4PT4Y7DPZ4Y', 'directory.created', 'directory', '.git', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git","name":".git","parent_id":"null","depth":1}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.886420Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R077MJ8NVKJWYEAAYM9', 'directory.created', 'directory', '.git/objects', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects","name":"objects","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.886599Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R075WWSYDYNNSANZSNN', 'directory.created', 'directory', '.git/objects/61', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/61","name":"61","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.886779Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074GB0Y08B412DANP4', 'directory.created', 'directory', '.git/objects/0d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/0d","name":"0d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.886959Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07N4R88BS63B8MH2FE', 'directory.created', 'directory', '.git/objects/95', 'org', 'confirmed', '{"data":{"id":".git/objects/95","name":"95","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.887141Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07SM7S2V4G0XSKQNFP', 'directory.created', 'directory', '.git/objects/59', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/59","name":"59","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.887322Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ZT090TB21QJ65F97', 'directory.created', 'directory', '.git/objects/92', 'org', 'confirmed', '{"data":{"id":".git/objects/92","name":"92","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.887504Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074ZPCFR6J9SFW4YCT', 'directory.created', 'directory', '.git/objects/0c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/0c","name":"0c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.887687Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07QK7W45ZJA7K6EHFZ', 'directory.created', 'directory', '.git/objects/66', 'org', 'confirmed', '{"data":{"id":".git/objects/66","name":"66","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.887871Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071ZTSR39QP3G14G11', 'directory.created', 'directory', '.git/objects/3e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3e","name":"3e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.888062Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R078YQSF1GF6R9V6Y40', 'directory.created', 'directory', '.git/objects/50', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/50","name":"50","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.888248Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07T28SKSKFKYW5P5GN', 'directory.created', 'directory', '.git/objects/3b', 'org', 'confirmed', '{"data":{"id":".git/objects/3b","name":"3b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.888436Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074XK3JTF7CA1PFKQ2', 'directory.created', 'directory', '.git/objects/6f', 'org', 'confirmed', '{"data":{"id":".git/objects/6f","name":"6f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.888623Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07MD1N1WKAD4BF3H0D', 'directory.created', 'directory', '.git/objects/03', 'org', 'confirmed', '{"data":{"id":".git/objects/03","name":"03","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.888820Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ZM6G3RFYD854A3QZ', 'directory.created', 'directory', '.git/objects/9b', 'org', 'confirmed', '{"data":{"id":".git/objects/9b","name":"9b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.889011Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07YH07YDC3F3G75Z1F', 'directory.created', 'directory', '.git/objects/9e', 'org', 'confirmed', '{"data":{"id":".git/objects/9e","name":"9e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.889198Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0705YT48V8PWEC64CM', 'directory.created', 'directory', '.git/objects/04', 'org', 'confirmed', '{"data":{"id":".git/objects/04","name":"04","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.889387Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CW93FRN7PRAR8N7Q', 'directory.created', 'directory', '.git/objects/32', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/32","name":"32","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.889576Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07107826MHM1VPGHM3', 'directory.created', 'directory', '.git/objects/35', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/35","name":"35","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.889766Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RMP6Y4VVBRFZJKYC', 'directory.created', 'directory', '.git/objects/69', 'org', 'confirmed', '{"data":{"id":".git/objects/69","name":"69","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.889959Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R076BSN2G55083FVNC4', 'directory.created', 'directory', '.git/objects/3c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3c","name":"3c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.890150Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0793FDXKK9A1GC5Q1Q', 'directory.created', 'directory', '.git/objects/56', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/56","name":"56","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.890342Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07HCMG94MVJSX5XB09', 'directory.created', 'directory', '.git/objects/51', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/51","name":"51","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.890535Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07XDZ6J18XXY9AZCWZ', 'directory.created', 'directory', '.git/objects/3d', 'org', 'confirmed', '{"data":{"id":".git/objects/3d","name":"3d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.890730Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R073Y4Y2W3DAAHRXW2Q', 'directory.created', 'directory', '.git/objects/58', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/58","name":"58","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.890924Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0707FEGB4KFPGDMG2J', 'directory.created', 'directory', '.git/objects/67', 'org', 'confirmed', '{"data":{"id":".git/objects/67","name":"67","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.891127Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PJDH706ZSMYW14VW', 'directory.created', 'directory', '.git/objects/93', 'org', 'confirmed', '{"data":{"id":".git/objects/93","name":"93","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.891324Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07H5X93178V7VQPRRS', 'directory.created', 'directory', '.git/objects/94', 'org', 'confirmed', '{"data":{"id":".git/objects/94","name":"94","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.891523Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07FTKQR4Q2P8VVW13B', 'directory.created', 'directory', '.git/objects/60', 'org', 'confirmed', '{"data":{"id":".git/objects/60","name":"60","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.891721Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07GPS7H7T9JEQKG759', 'directory.created', 'directory', '.git/objects/34', 'org', 'confirmed', '{"data":{"id":".git/objects/34","name":"34","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.891929Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RMX839Y1TXDS3CNZ', 'directory.created', 'directory', '.git/objects/5a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5a","name":"5a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.892128Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07KRTQY9G84RY5RYHF', 'directory.created', 'directory', '.git/objects/5f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5f","name":"5f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.892326Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PDC7K9V1JHJQ6VWE', 'directory.created', 'directory', '.git/objects/33', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/33","name":"33","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.892524Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R078BT4PQMAEAYCD19Z', 'directory.created', 'directory', '.git/objects/05', 'org', 'confirmed', '{"data":{"id":".git/objects/05","name":"05","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.892722Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07X72RRXXQKYE192H4', 'directory.created', 'directory', '.git/objects/9c', 'org', 'confirmed', '{"data":{"id":".git/objects/9c","name":"9c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.892927Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071SRKC9N46C3S20SK', 'directory.created', 'directory', '.git/objects/02', 'org', 'confirmed', '{"data":{"id":".git/objects/02","name":"02","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.893129Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07MPBGM2QRC0VE3PEG', 'directory.created', 'directory', '.git/objects/a4', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a4","name":"a4","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.893331Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RP14N5BW5BNCC10D', 'directory.created', 'directory', '.git/objects/b5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/b5","name":"b5","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.893533Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TP53EZN4B75T1S8G', 'directory.created', 'directory', '.git/objects/b2', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/b2","name":"b2","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.893737Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07AM9B5SV6CEH7WJWA', 'directory.created', 'directory', '.git/objects/d9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d9","name":"d9","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.893946Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TM9YKXYMWCWBNYTZ', 'directory.created', 'directory', '.git/objects/ac', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ac","name":"ac","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.894154Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07BKWEGPSSX9797G5D', 'directory.created', 'directory', '.git/objects/ad', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ad","name":"ad","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.894363Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R078BBT9Y1A372K5BE9', 'directory.created', 'directory', '.git/objects/bb', 'org', 'confirmed', '{"data":{"id":".git/objects/bb","name":"bb","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.894570Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07R3FPPEPC0FM69NB1', 'directory.created', 'directory', '.git/objects/d7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d7","name":"d7","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.894777Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0764HWBY3ERT30HC90', 'directory.created', 'directory', '.git/objects/d0', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d0","name":"d0","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.894985Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R072EQ6NW17KKS98MYT', 'directory.created', 'directory', '.git/objects/be', 'org', 'confirmed', '{"data":{"id":".git/objects/be","name":"be","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.895203Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07P18EPXXTRW2P32TC', 'directory.created', 'directory', '.git/objects/b3', 'org', 'confirmed', '{"data":{"id":".git/objects/b3","name":"b3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.895411Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TMW8004EY64K7Y99', 'directory.created', 'directory', '.git/objects/df', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/df","name":"df","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.895622Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PBSSNQ0844HK06VY', 'directory.created', 'directory', '.git/objects/a5', 'org', 'confirmed', '{"data":{"id":".git/objects/a5","name":"a5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.895831Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07BNVTGTDAGS40H5FH', 'directory.created', 'directory', '.git/objects/bd', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/bd","name":"bd","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.896041Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07Q4TFVR2P5223X8HC', 'directory.created', 'directory', '.git/objects/d1', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d1","name":"d1","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.896250Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07HM1X3K4Z56KG6F2Z', 'directory.created', 'directory', '.git/objects/d6', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d6","name":"d6","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.896462Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07GRQSNTQM5PQDCR7B', 'directory.created', 'directory', '.git/objects/bc', 'org', 'confirmed', '{"data":{"id":".git/objects/bc","name":"bc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.896681Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07489G2X4YR1R2QE25', 'directory.created', 'directory', '.git/objects/ae', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ae","name":"ae","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.896895Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RMQ1MND9F0X1NAYD', 'directory.created', 'directory', '.git/objects/d8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d8","name":"d8","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.897109Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RCS0EQ3FGEJ1Z6A7', 'directory.created', 'directory', '.git/objects/ab', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ab","name":"ab","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.897323Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PV6RJFF1Y7WJGJQ8', 'directory.created', 'directory', '.git/objects/e5', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e5","name":"e5","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.897538Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074VFQBR5QRJGMQ1RE', 'directory.created', 'directory', '.git/objects/e2', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e2","name":"e2","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.897758Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07JF77GWQGT44PZ0ZZ', 'directory.created', 'directory', '.git/objects/f4', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f4","name":"f4","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.897979Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07FA523R8B07HK0QYJ', 'directory.created', 'directory', '.git/objects/f3', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/f3","name":"f3","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.898197Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07B3FGZ1RF01X4WXR6', 'directory.created', 'directory', '.git/objects/c7', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c7","name":"c7","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.898416Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0775FCFXKMHATEVVPW', 'directory.created', 'directory', '.git/objects/ee', 'org', 'confirmed', '{"data":{"id":".git/objects/ee","name":"ee","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.898649Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07SZKXACGERB1WZXJT', 'directory.created', 'directory', '.git/objects/c9', 'org', 'confirmed', '{"data":{"id":".git/objects/c9","name":"c9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.898866Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07XQBQ4E4X7C9B9EKB', 'directory.created', 'directory', '.git/objects/fd', 'org', 'confirmed', '{"data":{"id":".git/objects/fd","name":"fd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.899082Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TYVAYZDMDTHBQTM7', 'directory.created', 'directory', '.git/objects/f2', 'org', 'confirmed', '{"data":{"id":".git/objects/f2","name":"f2","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.899303Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CE8MQP9W40PSSH9B', 'directory.created', 'directory', '.git/objects/f5', 'org', 'confirmed', '{"data":{"id":".git/objects/f5","name":"f5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.899522Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R079J4B6QXMMWZA7FTH', 'directory.created', 'directory', '.git/objects/cf', 'org', 'confirmed', '{"data":{"id":".git/objects/cf","name":"cf","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.899746Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07Q6488Z81Z9GX98E2', 'directory.created', 'directory', '.git/objects/ca', 'org', 'confirmed', '{"data":{"id":".git/objects/ca","name":"ca","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.899973Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07N7H0420R01YN7T44', 'directory.created', 'directory', '.git/objects/fe', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/fe","name":"fe","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.900201Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ZE2BV9QJPX0QV3M8', 'directory.created', 'directory', '.git/objects/c8', 'org', 'confirmed', '{"data":{"id":".git/objects/c8","name":"c8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.900424Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0783YWRR5SX5D7J5TJ', 'directory.created', 'directory', '.git/objects/fb', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/fb","name":"fb","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.900647Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07VPK4NKD2F68TRZ2M', 'directory.created', 'directory', '.git/objects/ed', 'org', 'confirmed', '{"data":{"id":".git/objects/ed","name":"ed","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.900873Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R070EGFGEE2SJRE4JMR', 'directory.created', 'directory', '.git/objects/c1', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/c1","name":"c1","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.901100Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07D6HSDZ9PN7YG6RJH', 'directory.created', 'directory', '.git/objects/c6', 'org', 'confirmed', '{"data":{"id":".git/objects/c6","name":"c6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.901326Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07DZY55A9WXXK5ZV47', 'directory.created', 'directory', '.git/objects/ec', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ec","name":"ec","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.901560Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NSF57T6AJTJBSN0J', 'directory.created', 'directory', '.git/objects/4e', 'org', 'confirmed', '{"data":{"id":".git/objects/4e","name":"4e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.901790Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CNFQ4RK4K62B54FN', 'directory.created', 'directory', '.git/objects/18', 'org', 'confirmed', '{"data":{"id":".git/objects/18","name":"18","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.902021Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0700E5ZVRPFGSMNVDP', 'directory.created', 'directory', '.git/objects/27', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/27","name":"27","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.902261Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07K7CSR9AMABWQTRPX', 'directory.created', 'directory', '.git/objects/4b', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4b","name":"4b","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.902490Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07WJRECSR4E0427A1J', 'directory.created', 'directory', '.git/objects/pack', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/pack","name":"pack","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.902718Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RW9AAGKTX2ECC5TE', 'directory.created', 'directory', '.git/objects/11', 'org', 'confirmed', '{"data":{"id":".git/objects/11","name":"11","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.902948Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07439YBJQR598SB6PE', 'directory.created', 'directory', '.git/objects/7d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/7d","name":"7d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.903180Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07C7K7MBVGBHQE77XR', 'directory.created', 'directory', '.git/objects/7c', 'org', 'confirmed', '{"data":{"id":".git/objects/7c","name":"7c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.903412Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R077VJ9GH0E5XM6D6MQ', 'directory.created', 'directory', '.git/objects/16', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/16","name":"16","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.903642Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07FZV52NP930BEM0PN', 'directory.created', 'directory', '.git/objects/45', 'org', 'confirmed', '{"data":{"id":".git/objects/45","name":"45","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.903875Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RY6NP62M6EGJ07F2', 'directory.created', 'directory', '.git/objects/1f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1f","name":"1f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.904430Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07THPFPGJZE24B0A7Y', 'directory.created', 'directory', '.git/objects/73', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/73","name":"73","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.904661Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074TPKH11ANVFQ8V0X', 'directory.created', 'directory', '.git/objects/87', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/87","name":"87","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.904892Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R079W263HF3WAZM4C6E', 'directory.created', 'directory', '.git/objects/80', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/80","name":"80","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.905124Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ED8RPXGX8Z55W57B', 'directory.created', 'directory', '.git/objects/74', 'org', 'confirmed', '{"data":{"id":".git/objects/74","name":"74","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.905359Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07K5XF7KRHW8907VNM', 'directory.created', 'directory', '.git/objects/1a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1a","name":"1a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.905591Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07J563EM5W47733Q7C', 'directory.created', 'directory', '.git/objects/28', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/28","name":"28","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.905822Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RH58NFPPPKF0MWV2', 'directory.created', 'directory', '.git/objects/17', 'org', 'confirmed', '{"data":{"id":".git/objects/17","name":"17","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.906056Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R075HSFHHK44AYZ4JFJ', 'directory.created', 'directory', '.git/objects/7b', 'org', 'confirmed', '{"data":{"id":".git/objects/7b","name":"7b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.906301Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ZFDA36DVAXCZZZH5', 'directory.created', 'directory', '.git/objects/8f', 'org', 'confirmed', '{"data":{"id":".git/objects/8f","name":"8f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.906535Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074R1014ZHYR6E9B5V', 'directory.created', 'directory', '.git/objects/7e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/7e","name":"7e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.906770Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CKB87R50W8NYR7TZ', 'directory.created', 'directory', '.git/objects/10', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/10","name":"10","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.907006Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R078604CDZBFPF2QG9X', 'directory.created', 'directory', '.git/objects/19', 'org', 'confirmed', '{"data":{"id":".git/objects/19","name":"19","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.907241Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07N5WSTJN9X1JWXVTE', 'directory.created', 'directory', '.git/objects/4c', 'org', 'confirmed', '{"data":{"id":".git/objects/4c","name":"4c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.907478Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07KFCACE3NWTB0KWQC', 'directory.created', 'directory', '.git/objects/26', 'org', 'confirmed', '{"data":{"id":".git/objects/26","name":"26","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.907716Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071Q38K417M86VJ7EV', 'directory.created', 'directory', '.git/objects/4d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4d","name":"4d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.907960Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NHSBNR5B1SF23WA2', 'directory.created', 'directory', '.git/objects/75', 'org', 'confirmed', '{"data":{"id":".git/objects/75","name":"75","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.908199Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R076WYPN3XNM1WZV5EQ', 'directory.created', 'directory', '.git/objects/81', 'org', 'confirmed', '{"data":{"id":".git/objects/81","name":"81","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.908439Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07014XP2TBPQ6B9S44', 'directory.created', 'directory', '.git/objects/86', 'org', 'confirmed', '{"data":{"id":".git/objects/86","name":"86","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.908679Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07Z0FZD2PZKPHHP3Q6', 'directory.created', 'directory', '.git/objects/72', 'org', 'confirmed', '{"data":{"id":".git/objects/72","name":"72","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.908922Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07VEYHJJ3CFXV94FYX', 'directory.created', 'directory', '.git/objects/44', 'org', 'confirmed', '{"data":{"id":".git/objects/44","name":"44","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.909468Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NDVWC6MJ52JGZBRW', 'directory.created', 'directory', '.git/objects/2a', 'org', 'confirmed', '{"data":{"id":".git/objects/2a","name":"2a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.909704Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07038KFVP1CXY0GY8N', 'directory.created', 'directory', '.git/objects/2f', 'org', 'confirmed', '{"data":{"id":".git/objects/2f","name":"2f","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.909939Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07BD1NHX8YXABKE1SG', 'directory.created', 'directory', '.git/objects/43', 'org', 'confirmed', '{"data":{"id":".git/objects/43","name":"43","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.910175Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07B3AMPEM0470F00TS', 'directory.created', 'directory', '.git/objects/88', 'org', 'confirmed', '{"data":{"id":".git/objects/88","name":"88","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.910420Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07EJE43G0J904TJNBM', 'directory.created', 'directory', '.git/objects/9f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/9f","name":"9f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.910659Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0737QQPYAZ4FAXDHNB', 'directory.created', 'directory', '.git/objects/07', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/07","name":"07","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.910894Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07BE64T2JVNAQMGDSA', 'directory.created', 'directory', '.git/objects/38', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/38","name":"38","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.911129Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07V0MJQE4VPWYV8R0V', 'directory.created', 'directory', '.git/objects/00', 'org', 'confirmed', '{"data":{"id":".git/objects/00","name":"00","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.911363Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07F2W7H22TKHQ30ZTY', 'directory.created', 'directory', '.git/objects/6e', 'org', 'confirmed', '{"data":{"id":".git/objects/6e","name":"6e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.911599Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NV7SWGTPD9K76F4N', 'directory.created', 'directory', '.git/objects/9a', 'org', 'confirmed', '{"data":{"id":".git/objects/9a","name":"9a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.911835Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RJ0YFG99R925YZ4Q', 'directory.created', 'directory', '.git/objects/5c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5c","name":"5c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.912072Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TYH7ARX7RQAZG0G4', 'directory.created', 'directory', '.git/objects/09', 'org', 'confirmed', '{"data":{"id":".git/objects/09","name":"09","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.912310Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07C962P8V30MHK62QV', 'directory.created', 'directory', '.git/objects/5d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/5d","name":"5d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.912551Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PB3V33CMCG1D1JQ9', 'directory.created', 'directory', '.git/objects/info', 'org', 'confirmed', '{"data":{"id":".git/objects/info","name":"info","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.912785Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0794H0WQCZJRSF5CYY', 'directory.created', 'directory', '.git/objects/91', 'org', 'confirmed', '{"data":{"id":".git/objects/91","name":"91","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.913029Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07KJM2W4ZQDNTRF2X7', 'directory.created', 'directory', '.git/objects/65', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/65","name":"65","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.913273Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TPGS2NSFC980VHAT', 'directory.created', 'directory', '.git/objects/62', 'org', 'confirmed', '{"data":{"id":".git/objects/62","name":"62","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.913516Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07MZTDENM8185BG0M4', 'directory.created', 'directory', '.git/objects/96', 'org', 'confirmed', '{"data":{"id":".git/objects/96","name":"96","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.913761Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0702MVZJN36JZQ4RME', 'directory.created', 'directory', '.git/objects/3a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3a","name":"3a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.914009Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07XWWH7NZPFQBBW76P', 'directory.created', 'directory', '.git/objects/54', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/54","name":"54","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.914260Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ZDZ74RD1DTKVE0T9', 'directory.created', 'directory', '.git/objects/98', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/98","name":"98","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.914501Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CETN5DJ38S20YMK2', 'directory.created', 'directory', '.git/objects/53', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/53","name":"53","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.914743Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07VM389EXXVSXGB505', 'directory.created', 'directory', '.git/objects/3f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/3f","name":"3f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.914989Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07BFBT72F3NBNDYNTS', 'directory.created', 'directory', '.git/objects/30', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/30","name":"30","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.915232Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0721XCRV0H9844EH8X', 'directory.created', 'directory', '.git/objects/5e', 'org', 'confirmed', '{"data":{"id":".git/objects/5e","name":"5e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.915479Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07F57T35TZW3PDMSBR', 'directory.created', 'directory', '.git/objects/5b', 'org', 'confirmed', '{"data":{"id":".git/objects/5b","name":"5b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.915726Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07AF67KQCVEYC3CVM7', 'directory.created', 'directory', '.git/objects/37', 'org', 'confirmed', '{"data":{"id":".git/objects/37","name":"37","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.915976Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07VGHQY4XZM0ZQ6ZVB', 'directory.created', 'directory', '.git/objects/08', 'org', 'confirmed', '{"data":{"id":".git/objects/08","name":"08","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.916223Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07R6D2133HHMKR2B4X', 'directory.created', 'directory', '.git/objects/6d', 'org', 'confirmed', '{"data":{"id":".git/objects/6d","name":"6d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.916469Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TXFFNWGRYK5PRWD1', 'directory.created', 'directory', '.git/objects/01', 'org', 'confirmed', '{"data":{"id":".git/objects/01","name":"01","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.916719Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R075Y9C3CBCPFCNHAYQ', 'directory.created', 'directory', '.git/objects/06', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/06","name":"06","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.916971Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07S1F1PE1APMHQVKB1', 'directory.created', 'directory', '.git/objects/6c', 'org', 'confirmed', '{"data":{"id":".git/objects/6c","name":"6c","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.917223Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PJVAYHCXG98189KM', 'directory.created', 'directory', '.git/objects/39', 'org', 'confirmed', '{"data":{"id":".git/objects/39","name":"39","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.917842Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PF18C0QHDBD9DCTR', 'directory.created', 'directory', '.git/objects/99', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/99","name":"99","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.918093Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R076YN0B360KFP5KS53', 'directory.created', 'directory', '.git/objects/52', 'org', 'confirmed', '{"data":{"id":".git/objects/52","name":"52","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.918341Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R070MQ52ZEBWHQWVHPR', 'directory.created', 'directory', '.git/objects/55', 'org', 'confirmed', '{"data":{"id":".git/objects/55","name":"55","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.918602Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07STEAZ6M3TVTNWB0M', 'directory.created', 'directory', '.git/objects/97', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/97","name":"97","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.918847Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074GGXEZ4X39WD96QS', 'directory.created', 'directory', '.git/objects/0a', 'org', 'confirmed', '{"data":{"id":".git/objects/0a","name":"0a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.919093Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07DR4MNFDRY4G8KPT7', 'directory.created', 'directory', '.git/objects/90', 'org', 'confirmed', '{"data":{"id":".git/objects/90","name":"90","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.919339Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CQBGGFY7PC9JMFZB', 'directory.created', 'directory', '.git/objects/bf', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/bf","name":"bf","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.919587Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R077WK0BKBEV26MY1YG', 'directory.created', 'directory', '.git/objects/d3', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/d3","name":"d3","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.919836Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07JVPFR1G0AYHYAEY0', 'directory.created', 'directory', '.git/objects/d4', 'org', 'confirmed', '{"data":{"id":".git/objects/d4","name":"d4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.920083Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0773022KEXBX82WE8S', 'directory.created', 'directory', '.git/objects/ba', 'org', 'confirmed', '{"data":{"id":".git/objects/ba","name":"ba","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.920336Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074SHMWXDCS27WSF6P', 'directory.created', 'directory', '.git/objects/a0', 'org', 'confirmed', '{"data":{"id":".git/objects/a0","name":"a0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.920587Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07C0K3A9FNZMGY32HC', 'directory.created', 'directory', '.git/objects/a7', 'org', 'confirmed', '{"data":{"id":".git/objects/a7","name":"a7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.920840Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0726CF75CQZF1RKWDE', 'directory.created', 'directory', '.git/objects/b8', 'org', 'confirmed', '{"data":{"id":".git/objects/b8","name":"b8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.921094Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07T085KV2WZ2WR0CZF', 'directory.created', 'directory', '.git/objects/b1', 'org', 'confirmed', '{"data":{"id":".git/objects/b1","name":"b1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.921347Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R073ERN1RAJ3GEKS2KB', 'directory.created', 'directory', '.git/objects/dd', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/dd","name":"dd","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.921604Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07KTSRXHMHW3BG0DF9', 'directory.created', 'directory', '.git/objects/dc', 'org', 'confirmed', '{"data":{"id":".git/objects/dc","name":"dc","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.921858Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07JH7C6FDNEK0MGQ4V', 'directory.created', 'directory', '.git/objects/b6', 'org', 'confirmed', '{"data":{"id":".git/objects/b6","name":"b6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.922111Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0755X8JRT4ETM6GK22', 'directory.created', 'directory', '.git/objects/a9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a9","name":"a9","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.922367Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07FKTD7XPRDDR9AY5R', 'directory.created', 'directory', '.git/objects/d5', 'org', 'confirmed', '{"data":{"id":".git/objects/d5","name":"d5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.922632Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R076EMQCHZ1JP3B5CKW', 'directory.created', 'directory', '.git/objects/d2', 'org', 'confirmed', '{"data":{"id":".git/objects/d2","name":"d2","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.922885Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ESKH0R48R2BG8M96', 'directory.created', 'directory', '.git/objects/aa', 'org', 'confirmed', '{"data":{"id":".git/objects/aa","name":"aa","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.923138Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074AYK0BZHNSRSTBB5', 'directory.created', 'directory', '.git/objects/af', 'org', 'confirmed', '{"data":{"id":".git/objects/af","name":"af","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.923393Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07WNVRES7PWMZKS68Q', 'directory.created', 'directory', '.git/objects/b7', 'org', 'confirmed', '{"data":{"id":".git/objects/b7","name":"b7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.923649Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07EQW4790D4KET9ME1', 'directory.created', 'directory', '.git/objects/db', 'org', 'confirmed', '{"data":{"id":".git/objects/db","name":"db","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.923911Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07CM8CMBXDBF8JMM05', 'directory.created', 'directory', '.git/objects/a8', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a8","name":"a8","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.924169Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07GXJH5BB5N0JF0C47', 'directory.created', 'directory', '.git/objects/de', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/de","name":"de","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.924427Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07R60QZR9DQFD0RP0G', 'directory.created', 'directory', '.git/objects/b0', 'org', 'confirmed', '{"data":{"id":".git/objects/b0","name":"b0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.924685Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07K8BTAGGXCCYRBBBZ', 'directory.created', 'directory', '.git/objects/b9', 'org', 'confirmed', '{"data":{"id":".git/objects/b9","name":"b9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.924945Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07S08N0DVFEW3K66VT', 'directory.created', 'directory', '.git/objects/a1', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/a1","name":"a1","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.925587Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07N9F9ZK503TR9Q59A', 'directory.created', 'directory', '.git/objects/ef', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/ef","name":"ef","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.925851Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PC5DCNQ15JBYHG8F', 'directory.created', 'directory', '.git/objects/c3', 'org', 'confirmed', '{"data":{"id":".git/objects/c3","name":"c3","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.926111Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R076MTP2TY8DAWBFSNR', 'directory.created', 'directory', '.git/objects/c4', 'org', 'confirmed', '{"data":{"id":".git/objects/c4","name":"c4","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.926380Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R075417G94MSKDMQGPR', 'directory.created', 'directory', '.git/objects/ea', 'org', 'confirmed', '{"data":{"id":".git/objects/ea","name":"ea","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.926641Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07MJB7WRV59G70MDJB', 'directory.created', 'directory', '.git/objects/e1', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e1","name":"e1","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.926908Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07WAGAE445WVZT8MS2', 'directory.created', 'directory', '.git/objects/cd', 'org', 'confirmed', '{"data":{"id":".git/objects/cd","name":"cd","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.927182Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RMWXTSF9PYCGPDQE', 'directory.created', 'directory', '.git/objects/cc', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/cc","name":"cc","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.927442Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0744BDXRHNBSGB977N', 'directory.created', 'directory', '.git/objects/e6', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e6","name":"e6","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.927711Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07EC9S2SERNY78SZ56', 'directory.created', 'directory', '.git/objects/f9', 'org', 'confirmed', '{"data":{"id":".git/objects/f9","name":"f9","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.927981Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0746BT04HXXSFZY0RJ', 'directory.created', 'directory', '.git/objects/f0', 'org', 'confirmed', '{"data":{"id":".git/objects/f0","name":"f0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.928245Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07A2J3TPAXFQNX4J19', 'directory.created', 'directory', '.git/objects/f7', 'org', 'confirmed', '{"data":{"id":".git/objects/f7","name":"f7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.928916Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07MS6YCVCYENT08C4P', 'directory.created', 'directory', '.git/objects/e8', 'org', 'confirmed', '{"data":{"id":".git/objects/e8","name":"e8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.929179Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07J48J736KX0DTRK8V', 'directory.created', 'directory', '.git/objects/fa', 'org', 'confirmed', '{"data":{"id":".git/objects/fa","name":"fa","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.929441Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07SHC6S4SCW63KV6TY', 'directory.created', 'directory', '.git/objects/ff', 'org', 'confirmed', '{"data":{"id":".git/objects/ff","name":"ff","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.929704Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07DJYKJJ2FFWB1XT1P', 'directory.created', 'directory', '.git/objects/c5', 'org', 'confirmed', '{"data":{"id":".git/objects/c5","name":"c5","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.929972Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07GGMV4YQQ89B76QTR', 'directory.created', 'directory', '.git/objects/f6', 'org', 'confirmed', '{"data":{"id":".git/objects/f6","name":"f6","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.930238Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07A39D6VAQ7XNVC6MP', 'directory.created', 'directory', '.git/objects/e9', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/e9","name":"e9","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.930898Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07HZ53VVVMAQRYZQ96', 'directory.created', 'directory', '.git/objects/f1', 'org', 'confirmed', '{"data":{"id":".git/objects/f1","name":"f1","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.931183Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07RBE978G79HJA3HN4', 'directory.created', 'directory', '.git/objects/e7', 'org', 'confirmed', '{"data":{"id":".git/objects/e7","name":"e7","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.931451Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07MVVBRB6RTCDZCDG2', 'directory.created', 'directory', '.git/objects/cb', 'org', 'confirmed', '{"data":{"id":".git/objects/cb","name":"cb","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.932139Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NM9DS7CJ7PP45JYP', 'directory.created', 'directory', '.git/objects/f8', 'org', 'confirmed', '{"data":{"id":".git/objects/f8","name":"f8","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.932853Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R072Z08MA03W7BAR829', 'directory.created', 'directory', '.git/objects/ce', 'org', 'confirmed', '{"data":{"id":".git/objects/ce","name":"ce","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.933140Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R079MEQP5E7752X52C3', 'directory.created', 'directory', '.git/objects/e0', 'org', 'confirmed', '{"data":{"id":".git/objects/e0","name":"e0","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.933400Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0708BTTQ3XCR45VMYA', 'directory.created', 'directory', '.git/objects/46', 'org', 'confirmed', '{"data":{"id":".git/objects/46","name":"46","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.933672Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07HXMCB79BWC7PPCR0', 'directory.created', 'directory', '.git/objects/2c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/2c","name":"2c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.933949Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NMKB0KAG3RPX340S', 'directory.created', 'directory', '.git/objects/79', 'org', 'confirmed', '{"data":{"id":".git/objects/79","name":"79","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.934206Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07595517KW4YRP9FS9', 'directory.created', 'directory', '.git/objects/2d', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/2d","name":"2d","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.934474Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0727EBADRADKV4W1EE', 'directory.created', 'directory', '.git/objects/41', 'org', 'confirmed', '{"data":{"id":".git/objects/41","name":"41","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.934748Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07HCFB42EWGDHVCVC0', 'directory.created', 'directory', '.git/objects/1b', 'org', 'confirmed', '{"data":{"id":".git/objects/1b","name":"1b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.935028Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R072RGQBA0EVPWTH315', 'directory.created', 'directory', '.git/objects/77', 'org', 'confirmed', '{"data":{"id":".git/objects/77","name":"77","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.935304Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07XGB7Y58J5N3SBYM6', 'directory.created', 'directory', '.git/objects/48', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/48","name":"48","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.935567Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07WJH34D96CCB3VHBB', 'directory.created', 'directory', '.git/objects/1e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1e","name":"1e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.935843Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07567Z7DNNANWQ9WNE', 'directory.created', 'directory', '.git/objects/84', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/84","name":"84","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.936126Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07Y0N35H7KH09PME17', 'directory.created', 'directory', '.git/objects/4a', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4a","name":"4a","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.936394Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07EAQYW9Y958FTFZKB', 'directory.created', 'directory', '.git/objects/24', 'org', 'confirmed', '{"data":{"id":".git/objects/24","name":"24","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.936670Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071DEB5WJHST5N8CTM', 'directory.created', 'directory', '.git/objects/23', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/23","name":"23","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.936948Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07PQMSCD5DMSNNVMCG', 'directory.created', 'directory', '.git/objects/4f', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/4f","name":"4f","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.937235Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0786Q2D1QKV411312P', 'directory.created', 'directory', '.git/objects/8d', 'org', 'confirmed', '{"data":{"id":".git/objects/8d","name":"8d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.937512Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R076WE8MKH5SSPE6SCW', 'directory.created', 'directory', '.git/objects/15', 'org', 'confirmed', '{"data":{"id":".git/objects/15","name":"15","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.937785Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07GJ8B2CKZ54DF9YTT', 'directory.created', 'directory', '.git/objects/12', 'org', 'confirmed', '{"data":{"id":".git/objects/12","name":"12","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.938055Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0717RCF98CFQZQ47XH', 'directory.created', 'directory', '.git/objects/85', 'org', 'confirmed', '{"data":{"id":".git/objects/85","name":"85","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.938341Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R073S71AFH5TW0AWDPR', 'directory.created', 'directory', '.git/objects/1d', 'org', 'confirmed', '{"data":{"id":".git/objects/1d","name":"1d","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.938621Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0798PCBB833GFHSPP9', 'directory.created', 'directory', '.git/objects/71', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/71","name":"71","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.938990Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07YBKPT3KM05KD74WQ', 'directory.created', 'directory', '.git/objects/76', 'org', 'confirmed', '{"data":{"id":".git/objects/76","name":"76","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.939263Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071XJF45NMJ8PT2F60', 'directory.created', 'directory', '.git/objects/1c', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/1c","name":"1c","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.939522Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07ZPKYH9E1G81J7RJK', 'directory.created', 'directory', '.git/objects/82', 'org', 'confirmed', '{"data":{"id":".git/objects/82","name":"82","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.939772Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R072F0BKJBHN8B7K3SW', 'directory.created', 'directory', '.git/objects/49', 'org', 'confirmed', '{"data":{"id":".git/objects/49","name":"49","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.940456Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071WT9MP203CVQ6F6K', 'directory.created', 'directory', '.git/objects/40', 'org', 'confirmed', '{"data":{"id":".git/objects/40","name":"40","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.940707Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R079N4CDYAVCE122PXV', 'directory.created', 'directory', '.git/objects/2e', 'org', 'confirmed', '{"data":{"id":".git/objects/2e","name":"2e","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.940956Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0732KVET494TG7VB17', 'directory.created', 'directory', '.git/objects/2b', 'org', 'confirmed', '{"data":{"id":".git/objects/2b","name":"2b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.941208Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TVJFJZJGPXJ82MNN', 'directory.created', 'directory', '.git/objects/47', 'org', 'confirmed', '{"data":{"id":".git/objects/47","name":"47","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.941457Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07K00CCCY5HCR4CYW9', 'directory.created', 'directory', '.git/objects/78', 'org', 'confirmed', '{"data":{"id":".git/objects/78","name":"78","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.942182Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R074ZYNNQZZM0YJQZZ1', 'directory.created', 'directory', '.git/objects/8b', 'org', 'confirmed', '{"data":{"id":".git/objects/8b","name":"8b","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.942429Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07SGKMWNYD2095TP4T', 'directory.created', 'directory', '.git/objects/13', 'org', 'confirmed', '{"data":{"id":".git/objects/13","name":"13","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.942685Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NS0T5Z46ZV8P77Z5', 'directory.created', 'directory', '.git/objects/7a', 'org', 'confirmed', '{"data":{"id":".git/objects/7a","name":"7a","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.942935Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07BTZA6ABDKABRVWJB', 'directory.created', 'directory', '.git/objects/14', 'org', 'confirmed', '{"data":{"id":".git/objects/14","name":"14","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.943205Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07B3WZ0MCRHH90RS0Y', 'directory.created', 'directory', '.git/objects/8e', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/8e","name":"8e","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.943452Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07DH11WS0GP6EHR13Y', 'directory.created', 'directory', '.git/objects/22', 'org', 'confirmed', '{"data":{"id":".git/objects/22","name":"22","parent_id":".git/objects","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.944157Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07GPNAJ014J3KJP5XN', 'directory.created', 'directory', '.git/objects/25', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/objects/25","name":"25","parent_id":".git/objects","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.944409Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R071V3MRNMYVR7SJ363', 'directory.created', 'directory', '.git/rr-cache', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/rr-cache","name":"rr-cache","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.945198Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07KSE5WRBX2DREASS5', 'directory.created', 'directory', '.git/info', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/info","name":"info","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.945448Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07TR7VH5GFZKVZDKMV', 'directory.created', 'directory', '.git/logs', 'org', 'confirmed', '{"data":{"id":".git/logs","name":"logs","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.945711Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0756GY30N1FPS9A467', 'directory.created', 'directory', '.git/logs/refs', 'org', 'confirmed', '{"data":{"id":".git/logs/refs","name":"refs","parent_id":".git/logs","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.945973Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07JN0BFCCPAE0GQ4N6', 'directory.created', 'directory', '.git/logs/refs/heads', 'org', 'confirmed', '{"data":{"id":".git/logs/refs/heads","name":"heads","parent_id":".git/logs/refs","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.946228Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07NGGA2P53AF4T8XFS', 'directory.created', 'directory', '.git/hooks', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/hooks","name":"hooks","parent_id":".git","depth":2}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.946493Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07184NCVNVHQ587082', 'directory.created', 'directory', '.git/refs', 'org', 'confirmed', '{"data":{"id":".git/refs","name":"refs","parent_id":".git","depth":2},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.946753Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07T0066D831V3NVWBQ', 'directory.created', 'directory', '.git/refs/heads', 'org', 'confirmed', '{"data":{"id":".git/refs/heads","name":"heads","parent_id":".git/refs","depth":3},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.947000Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07FG7R5NA60P73H8VZ', 'directory.created', 'directory', '.git/refs/tags', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/refs/tags","name":"tags","parent_id":".git/refs","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.947260Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R072DVGQ0HTHMM30QS5', 'directory.created', 'directory', '.git/refs/jj', 'org', 'confirmed', '{"change_type":"created","data":{"id":".git/refs/jj","name":"jj","parent_id":".git/refs","depth":3}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.947515Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07H1E2PMPM9YE89W3E', 'directory.created', 'directory', '.git/refs/jj/keep', 'org', 'confirmed', '{"data":{"id":".git/refs/jj/keep","name":"keep","parent_id":".git/refs/jj","depth":4},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T16:50:23.949357Z
CREATE TABLE IF NOT EXISTS edge_props_json (edge_id INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE, key_id INTEGER NOT NULL REFERENCES property_keys(id), value TEXT NOT NULL, PRIMARY KEY (edge_id, key_id));

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:23.950445Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R0705S4S5AN3ZM9H8R2', 'file.created', 'file', 'file:index.org', 'org', 'confirmed', '{"data":{"id":"file:index.org","name":"index.org","parent_id":"null","content_hash":"2c45843e5c445c10c43f30dc4aaf59018fe6696700adf391a4347650b1977af2","document_id":null},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.950835Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R070DYD93DHB4TDRRVT', 'file.created', 'file', 'file:__default__.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:__default__.org","name":"__default__.org","parent_id":"null","content_hash":"9fd72b98d2fdcc99b3a0b4132dd515fa62233e6482c4ae90d39f429f40826f78","document_id":null}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.951151Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07X641TRAYKC8PQECP', 'file.created', 'file', 'file:ClaudeCode.org', 'org', 'confirmed', '{"change_type":"created","data":{"id":"file:ClaudeCode.org","name":"ClaudeCode.org","parent_id":"null","content_hash":"e57d79f0cf908c2c3b5a4ef5e5c8f4a5044c05dd4c05fa94ab2f2ae845336566","document_id":null}}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.951452Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R07KC5WR8HA6Z37D3W2', 'file.created', 'file', 'file:Projects/Holon.org', 'org', 'confirmed', '{"data":{"id":"file:Projects/Holon.org","name":"Holon.org","parent_id":"Projects","content_hash":"efcd75943d4648c09eb7ad183eec2fc19988228b2431d6b58f1c6ac024a16e67","document_id":null},"change_type":"created"}', '00000000000000000000008000000001', NULL, 1773939023879, NULL, NULL);

-- [actor_ddl] 2026-03-19T16:50:23.951983Z
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);

-- [actor_ddl] 2026-03-19T16:50:23.952640Z
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);

-- [transaction_stmt] 2026-03-19T16:50:23.953198Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content", "parent_id", "content_type", "updated_at", "id", "properties") VALUES (1773939023870, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'Holon Layout', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'text', 1773939023952, 'block:root-layout', '{"ID":"root-layout","sequence":0}');

-- [transaction_stmt] 2026-03-19T16:50:23.953413Z
INSERT OR REPLACE INTO block ("id", "content_type", "created_at", "source_language", "content", "parent_id", "document_id", "updated_at", "properties") VALUES ('block:root-layout::src::0', 'source', 1773939023870, 'holon_gql', 'MATCH (root:block)<-[:CHILD_OF]-(d:block)
WHERE root.id = ''block:root-layout'' AND d.content_type = ''text''
RETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority
ORDER BY d.properties.sequence
', 'block:root-layout', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 1773939023952, '{"sequence":1,"ID":"root-layout::src::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.953590Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "parent_id", "id", "created_at", "source_language", "document_id", "content", "properties") VALUES ('source', 1773939023952, 'block:root-layout', 'block:holon-app-layout::render::0', 1773939023870, 'render', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'columns(#{gap: 4, sort_key: col("sequence"), item_template: block_ref()})
', '{"sequence":2,"ID":"holon-app-layout::render::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.953750Z
INSERT OR REPLACE INTO block ("id", "content", "parent_id", "content_type", "created_at", "updated_at", "document_id", "properties") VALUES ('block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'Left Sidebar', 'block:root-layout', 'text', 1773939023920, 1773939023952, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', '{"ID":"e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","sequence":3,"collapse_to":"drawer"}');

-- [transaction_stmt] 2026-03-19T16:50:23.953899Z
INSERT OR REPLACE INTO block ("content", "updated_at", "id", "content_type", "source_language", "created_at", "document_id", "parent_id", "properties") VALUES ('list(#{sortkey: "name", item_template: selectable(row(icon("notebook"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})
', 1773939023952, 'block:block:left_sidebar::render::0', 'source', 'render', 1773939023920, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', '{"ID":"block:left_sidebar::render::0","sequence":4}');

-- [transaction_stmt] 2026-03-19T16:50:23.954059Z
INSERT OR REPLACE INTO block ("content", "source_language", "parent_id", "content_type", "document_id", "id", "updated_at", "created_at", "properties") VALUES ('from document
filter name != "" && name != "index" && name != "__default__"
', 'holon_prql', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'source', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'block:block:left_sidebar::src::0', 1773939023952, 1773939023920, '{"ID":"block:left_sidebar::src::0","sequence":5}');

-- [transaction_stmt] 2026-03-19T16:50:23.954212Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "created_at", "content", "parent_id", "id", "updated_at", "properties") VALUES ('doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'text', 1773939023920, 'All Documents', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 1773939023952, '{"sequence":6,"ID":"e8b05308-37ed-49a6-9c94-bccf9e3499bc"}');

-- [transaction_stmt] 2026-03-19T16:50:23.954359Z
INSERT OR REPLACE INTO block ("parent_id", "content", "document_id", "updated_at", "created_at", "content_type", "id", "properties") VALUES ('block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'Test', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 1773939023952, 1773939023920, 'text', 'block:66c6aae4-4829-4d54-b92f-6638fda03368', '{"ID":"66c6aae4-4829-4d54-b92f-6638fda03368","sequence":7}');

-- [transaction_stmt] 2026-03-19T16:50:23.954509Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "content", "parent_id", "id", "updated_at", "document_id", "properties") VALUES (1773939023920, 'text', 'Favorites', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 1773939023952, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', '{"sequence":8,"ID":"88862721-ed4f-43ba-9222-f84f17c6692e"}');

-- [transaction_stmt] 2026-03-19T16:50:23.954659Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "created_at", "updated_at", "parent_id", "content", "id", "properties") VALUES ('doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'text', 1773939023920, 1773939023952, 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'Recently Opened', 'block:a5d47f54-8632-412b-8844-7762121788b6', '{"ID":"a5d47f54-8632-412b-8844-7762121788b6","sequence":9}');

-- [transaction_stmt] 2026-03-19T16:50:23.954809Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content", "content_type", "id", "created_at", "updated_at", "properties") VALUES ('block:root-layout', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'Main Panel', 'text', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 1773939023921, 1773939023952, '{"sequence":10,"ID":"03ad3820-2c9d-42d1-85f4-8b5695df22fa"}');

-- [transaction_stmt] 2026-03-19T16:50:23.954966Z
INSERT OR REPLACE INTO block ("content", "id", "updated_at", "content_type", "source_language", "created_at", "parent_id", "document_id", "properties") VALUES ('MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block)
WHERE fr.region = ''main'' AND root.id = fr.root_id AND d.content_type <> ''source''
RETURN d, d.properties.sequence AS sequence
ORDER BY d.properties.sequence
', 'block:main::src::0', 1773939023952, 'source', 'holon_gql', 1773939023921, 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', '{"ID":"main::src::0","sequence":11}');

-- [transaction_stmt] 2026-03-19T16:50:23.955137Z
INSERT OR REPLACE INTO block ("content", "source_language", "parent_id", "document_id", "id", "content_type", "created_at", "updated_at", "properties") VALUES ('tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})
', 'render', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'block:main::render::0', 'source', 1773939023921, 1773939023952, '{"ID":"main::render::0","sequence":12}');

-- [transaction_stmt] 2026-03-19T16:50:23.955296Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "content", "content_type", "created_at", "document_id", "id", "properties") VALUES (1773939023952, 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'Graph View', 'text', 1773939023921, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', '{"sequence":13,"ID":"aaca22e0-1b52-479b-891e-c55dcfc308f4"}');

-- [transaction_stmt] 2026-03-19T16:50:23.955448Z
INSERT OR REPLACE INTO block ("id", "content_type", "updated_at", "created_at", "parent_id", "document_id", "content", "source_language", "properties") VALUES ('block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1', 'source', 1773939023952, 1773939023921, 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'list(#{item_template: row(text(col("content")))})
', 'render', '{"sequence":14,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1"}');

-- [transaction_stmt] 2026-03-19T16:50:23.955631Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "parent_id", "source_language", "id", "created_at", "content", "updated_at", "properties") VALUES ('doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'source', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'holon_gql', 'block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0', 1773939023921, 'MATCH (b:block) WHERE b.content_type = ''text'' RETURN b
', 1773939023952, '{"sequence":15,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.955793Z
INSERT OR REPLACE INTO block ("content_type", "content", "document_id", "parent_id", "updated_at", "id", "created_at", "properties") VALUES ('text', 'Right Sidebar', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'block:root-layout', 1773939023952, 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 1773939023921, '{"sequence":16,"collapse_to":"drawer","ID":"cf7e0570-0e50-46ae-8b33-8c4b4f82e79c"}');

-- [transaction_stmt] 2026-03-19T16:50:23.955948Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "source_language", "content", "document_id", "created_at", "id", "updated_at", "properties") VALUES ('block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'source', 'render', 'list(#{item_template: render_entity()})
', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 1773939023921, 'block:block:right_sidebar::render::0', 1773939023952, '{"ID":"block:right_sidebar::render::0","sequence":17}');

-- [transaction_stmt] 2026-03-19T16:50:23.956106Z
INSERT OR REPLACE INTO block ("source_language", "document_id", "created_at", "id", "updated_at", "parent_id", "content_type", "content", "properties") VALUES ('holon_prql', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 1773939023921, 'block:block:right_sidebar::src::0', 1773939023952, 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'source', 'from children
', '{"ID":"block:right_sidebar::src::0","sequence":18}');

-- [transaction_stmt] 2026-03-19T16:50:23.956268Z
INSERT OR REPLACE INTO block ("id", "created_at", "updated_at", "parent_id", "content", "content_type", "document_id", "properties") VALUES ('block:510a2669-402e-4d35-a161-4a2c259ed519', 1773939023921, 1773939023952, 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'Another pointer that gets shuffled around', 'text', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', '{"ID":"510a2669-402e-4d35-a161-4a2c259ed519","sequence":19}');

-- [transaction_stmt] 2026-03-19T16:50:23.956429Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content", "parent_id", "updated_at", "id", "content_type", "properties") VALUES (1773939023921, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'Context Panel is reactive again!', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 1773939023952, 'block:cffccf2a-7792-4b6d-a600-f8b31dc086b0', 'text', '{"ID":"cffccf2a-7792-4b6d-a600-f8b31dc086b0","sequence":20}');

-- [transaction_stmt] 2026-03-19T16:50:23.956594Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "created_at", "id", "parent_id", "content", "content_type", "properties") VALUES (1773939023952, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 1773939023921, 'block:4510fef8-f1c5-47b8-805b-8cd2c4905909', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'Quick Capture', 'text', '{"ID":"4510fef8-f1c5-47b8-805b-8cd2c4905909","sequence":21}');

-- [transaction_stmt] 2026-03-19T16:50:23.956749Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "updated_at", "content", "content_type", "id", "created_at", "properties") VALUES ('doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 1773939023952, 'Block Profiles', 'text', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 1773939023922, '{"ID":"0c5c95a1-5202-427f-b714-86bec42fae89","sequence":22}');

-- [transaction_stmt] 2026-03-19T16:50:23.956903Z
INSERT OR REPLACE INTO block ("content", "id", "created_at", "document_id", "content_type", "source_language", "updated_at", "parent_id", "properties") VALUES ('entity_name: block
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
', 'block:block:blocks-profile::src::0', 1773939023922, 'doc:382956e9-dae4-45ad-a3bc-aebacc068ee1', 'source', 'holon_entity_profile_yaml', 1773939023952, 'block:0c5c95a1-5202-427f-b714-86bec42fae89', '{"sequence":23,"ID":"block:blocks-profile::src::0"}');

-- Wait 9ms
-- [actor_ddl] 2026-03-19T16:50:23.966786Z
CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(type);

-- [actor_ddl] 2026-03-19T16:50:23.967619Z
CREATE INDEX IF NOT EXISTS idx_node_labels_label ON node_labels(label, node_id);

-- [transaction_stmt] 2026-03-19T16:50:23.968265Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GF0AJCPGGD0G8E32P', 'block.created', 'block', 'block:root-layout', 'sql', 'confirmed', '{"data":{"created_at":1773939023870,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content":"Holon Layout","parent_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content_type":"text","updated_at":1773939023952,"id":"block:root-layout","properties":{"sequence":0,"ID":"root-layout"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.968651Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GXQCNJRSYMNMJWQND', 'block.created', 'block', 'block:root-layout::src::0', 'sql', 'confirmed', '{"data":{"id":"block:root-layout::src::0","content_type":"source","created_at":1773939023870,"source_language":"holon_gql","content":"MATCH (root:block)<-[:CHILD_OF]-(d:block)\\nWHERE root.id = ''block:root-layout'' AND d.content_type = ''text''\\nRETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority\\nORDER BY d.properties.sequence\\n","parent_id":"block:root-layout","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","updated_at":1773939023952,"properties":{"sequence":1,"ID":"root-layout::src::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.968981Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GVZA3G8DGJKC4517Q', 'block.created', 'block', 'block:holon-app-layout::render::0', 'sql', 'confirmed', '{"data":{"content_type":"source","updated_at":1773939023952,"parent_id":"block:root-layout","id":"block:holon-app-layout::render::0","created_at":1773939023870,"source_language":"render","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content":"columns(#{gap: 4, sort_key: col(\\"sequence\\"), item_template: block_ref()})\\n","properties":{"sequence":2,"ID":"holon-app-layout::render::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.969296Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GVYMSHTY8Z9BCSFJE', 'block.created', 'block', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'sql', 'confirmed', '{"data":{"id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","content":"Left Sidebar","parent_id":"block:root-layout","content_type":"text","created_at":1773939023920,"updated_at":1773939023952,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","properties":{"sequence":3,"collapse_to":"drawer","ID":"e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.969601Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GE62QX3ZZT8C6WMY6', 'block.created', 'block', 'block:block:left_sidebar::render::0', 'sql', 'confirmed', '{"data":{"content":"list(#{sortkey: \\"name\\", item_template: selectable(row(icon(\\"notebook\\"), spacer(6), text(col(\\"name\\"))), #{action: navigation_focus(#{region: \\"main\\", block_id: col(\\"id\\")})})})\\n","updated_at":1773939023952,"id":"block:block:left_sidebar::render::0","content_type":"source","source_language":"render","created_at":1773939023920,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","properties":{"sequence":4,"ID":"block:left_sidebar::render::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.969922Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GT3N94CJCRG2YVGDE', 'block.created', 'block', 'block:block:left_sidebar::src::0', 'sql', 'confirmed', '{"data":{"content":"from document\\nfilter name != \\"\\" && name != \\"index\\" && name != \\"__default__\\"\\n","source_language":"holon_prql","parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","content_type":"source","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","id":"block:block:left_sidebar::src::0","updated_at":1773939023952,"created_at":1773939023920,"properties":{"ID":"block:left_sidebar::src::0","sequence":5}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.970229Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GT1QM97WA8K99F6AA', 'block.created', 'block', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'sql', 'confirmed', '{"data":{"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content_type":"text","created_at":1773939023920,"content":"All Documents","parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","id":"block:e8b05308-37ed-49a6-9c94-bccf9e3499bc","updated_at":1773939023952,"properties":{"sequence":6,"ID":"e8b05308-37ed-49a6-9c94-bccf9e3499bc"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.971166Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GAREWH1TX10YHRT1N', 'block.created', 'block', 'block:66c6aae4-4829-4d54-b92f-6638fda03368', 'sql', 'confirmed', '{"data":{"parent_id":"block:e8b05308-37ed-49a6-9c94-bccf9e3499bc","content":"Test","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","updated_at":1773939023952,"created_at":1773939023920,"content_type":"text","id":"block:66c6aae4-4829-4d54-b92f-6638fda03368","properties":{"sequence":7,"ID":"66c6aae4-4829-4d54-b92f-6638fda03368"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.971465Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GNZ61SM90YMJCW313', 'block.created', 'block', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'sql', 'confirmed', '{"data":{"created_at":1773939023920,"content_type":"text","content":"Favorites","parent_id":"block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c","id":"block:88862721-ed4f-43ba-9222-f84f17c6692e","updated_at":1773939023952,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","properties":{"ID":"88862721-ed4f-43ba-9222-f84f17c6692e","sequence":8}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.971763Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GTEK7AJ3YDK377YH2', 'block.created', 'block', 'block:a5d47f54-8632-412b-8844-7762121788b6', 'sql', 'confirmed', '{"data":{"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content_type":"text","created_at":1773939023920,"updated_at":1773939023952,"parent_id":"block:88862721-ed4f-43ba-9222-f84f17c6692e","content":"Recently Opened","id":"block:a5d47f54-8632-412b-8844-7762121788b6","properties":{"sequence":9,"ID":"a5d47f54-8632-412b-8844-7762121788b6"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.972062Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GAKV7JN8XX1SE329S', 'block.created', 'block', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'sql', 'confirmed', '{"data":{"parent_id":"block:root-layout","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content":"Main Panel","content_type":"text","id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","created_at":1773939023921,"updated_at":1773939023952,"properties":{"sequence":10,"ID":"03ad3820-2c9d-42d1-85f4-8b5695df22fa"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.972360Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GJ3DXA7FJ10A1ME07', 'block.created', 'block', 'block:main::src::0', 'sql', 'confirmed', '{"data":{"content":"MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block)\\nWHERE fr.region = ''main'' AND root.id = fr.root_id AND d.content_type <> ''source''\\nRETURN d, d.properties.sequence AS sequence\\nORDER BY d.properties.sequence\\n","id":"block:main::src::0","updated_at":1773939023952,"content_type":"source","source_language":"holon_gql","created_at":1773939023921,"parent_id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","properties":{"sequence":11,"ID":"main::src::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.972678Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GQTD1CY9M4KCYHQVK', 'block.created', 'block', 'block:main::render::0', 'sql', 'confirmed', '{"data":{"content":"tree(#{parent_id: col(\\"parent_id\\"), sortkey: col(\\"sequence\\"), item_template: render_entity()})\\n","source_language":"render","parent_id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","id":"block:main::render::0","content_type":"source","created_at":1773939023921,"updated_at":1773939023952,"properties":{"ID":"main::render::0","sequence":12}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.972980Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2G59RN11MJ9HYV73YF', 'block.created', 'block', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'sql', 'confirmed', '{"data":{"updated_at":1773939023952,"parent_id":"block:03ad3820-2c9d-42d1-85f4-8b5695df22fa","content":"Graph View","content_type":"text","created_at":1773939023921,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","id":"block:aaca22e0-1b52-479b-891e-c55dcfc308f4","properties":{"ID":"aaca22e0-1b52-479b-891e-c55dcfc308f4","sequence":13}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.973911Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2G47V1CP20XY378W6M', 'block.created', 'block', 'block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1', 'sql', 'confirmed', '{"data":{"id":"block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1","content_type":"source","updated_at":1773939023952,"created_at":1773939023921,"parent_id":"block:aaca22e0-1b52-479b-891e-c55dcfc308f4","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content":"list(#{item_template: row(text(col(\\"content\\")))})\\n","source_language":"render","properties":{"sequence":14,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.974216Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GW4RTM026TZC3M6ZE', 'block.created', 'block', 'block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content_type":"source","parent_id":"block:aaca22e0-1b52-479b-891e-c55dcfc308f4","source_language":"holon_gql","id":"block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0","created_at":1773939023921,"content":"MATCH (b:block) WHERE b.content_type = ''text'' RETURN b\\n","updated_at":1773939023952,"properties":{"sequence":15,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.974521Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2G05X3P2JZQKJQJDT8', 'block.created', 'block', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"Right Sidebar","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","parent_id":"block:root-layout","updated_at":1773939023952,"id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","created_at":1773939023921,"properties":{"ID":"cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","sequence":16,"collapse_to":"drawer"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.974836Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2G1R3V2S1NNVV0NTEV', 'block.created', 'block', 'block:block:right_sidebar::render::0', 'sql', 'confirmed', '{"data":{"parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","content_type":"source","source_language":"render","content":"list(#{item_template: render_entity()})\\n","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","created_at":1773939023921,"id":"block:block:right_sidebar::render::0","updated_at":1773939023952,"properties":{"sequence":17,"ID":"block:right_sidebar::render::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.975146Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GH6FZAWKT462QG2QH', 'block.created', 'block', 'block:block:right_sidebar::src::0', 'sql', 'confirmed', '{"data":{"source_language":"holon_prql","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","created_at":1773939023921,"id":"block:block:right_sidebar::src::0","updated_at":1773939023952,"parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","content_type":"source","content":"from children\\n","properties":{"sequence":18,"ID":"block:right_sidebar::src::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.976078Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GHJJ96H4Z06NCZWBT', 'block.created', 'block', 'block:510a2669-402e-4d35-a161-4a2c259ed519', 'sql', 'confirmed', '{"data":{"id":"block:510a2669-402e-4d35-a161-4a2c259ed519","created_at":1773939023921,"updated_at":1773939023952,"parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","content":"Another pointer that gets shuffled around","content_type":"text","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","properties":{"ID":"510a2669-402e-4d35-a161-4a2c259ed519","sequence":19}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.976384Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GREFXNHN94XV99HHA', 'block.created', 'block', 'block:cffccf2a-7792-4b6d-a600-f8b31dc086b0', 'sql', 'confirmed', '{"data":{"created_at":1773939023921,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content":"Context Panel is reactive again!","parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","updated_at":1773939023952,"id":"block:cffccf2a-7792-4b6d-a600-f8b31dc086b0","content_type":"text","properties":{"ID":"cffccf2a-7792-4b6d-a600-f8b31dc086b0","sequence":20}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.976687Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GVPGS1D4BPW2W0W3X', 'block.created', 'block', 'block:4510fef8-f1c5-47b8-805b-8cd2c4905909', 'sql', 'confirmed', '{"data":{"updated_at":1773939023952,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","created_at":1773939023921,"id":"block:4510fef8-f1c5-47b8-805b-8cd2c4905909","parent_id":"block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c","content":"Quick Capture","content_type":"text","properties":{"ID":"4510fef8-f1c5-47b8-805b-8cd2c4905909","sequence":21}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.976996Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2GR4VNZTY49QYFDMZ4', 'block.created', 'block', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 'sql', 'confirmed', '{"data":{"parent_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","updated_at":1773939023952,"content":"Block Profiles","content_type":"text","id":"block:0c5c95a1-5202-427f-b714-86bec42fae89","created_at":1773939023922,"properties":{"sequence":22,"ID":"0c5c95a1-5202-427f-b714-86bec42fae89"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.977902Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R2G5QM185QHEBR80Q94', 'block.created', 'block', 'block:block:blocks-profile::src::0', 'sql', 'confirmed', '{"data":{"content":"entity_name: block\\n\\ncomputed:\\n  is_task: ''= task_state != ()''\\n  is_source: ''= content_type == \\"source\\"''\\n  has_query_source: ''= query_source(id) != ()''\\n\\ndefault:\\n  render: ''row(icon(\\"orgmode\\"), spacer(8), editable_text(col(\\"content\\")))''\\n\\nvariants:\\n  - name: query_block\\n    condition: ''= has_query_source''\\n    render: ''block_ref()''\\n  - name: task\\n    condition: ''= is_task''\\n    render: ''row(state_toggle(col(\\"task_state\\")), spacer(8), editable_text(col(\\"content\\")))''\\n  - name: source\\n    condition: ''= is_source''\\n    render: ''source_editor(#{language: col(\\"source_language\\"), content: col(\\"content\\")})''\\n","id":"block:block:blocks-profile::src::0","created_at":1773939023922,"document_id":"doc:382956e9-dae4-45ad-a3bc-aebacc068ee1","content_type":"source","source_language":"holon_entity_profile_yaml","updated_at":1773939023952,"parent_id":"block:0c5c95a1-5202-427f-b714-86bec42fae89","properties":{"sequence":23,"ID":"block:blocks-profile::src::0"}}}', NULL, NULL, 1773939023952, NULL, NULL);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T16:50:23.979821Z
CREATE INDEX IF NOT EXISTS idx_property_keys_key ON property_keys(key);

-- [actor_ddl] 2026-03-19T16:50:23.980597Z
CREATE INDEX IF NOT EXISTS idx_node_props_int_key_value ON node_props_int(key_id, value, node_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T16:50:23.982021Z
CREATE INDEX IF NOT EXISTS idx_node_props_text_key_value ON node_props_text(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T16:50:23.982626Z
CREATE INDEX IF NOT EXISTS idx_node_props_real_key_value ON node_props_real(key_id, value, node_id);

-- [actor_ddl] 2026-03-19T16:50:23.983311Z
CREATE INDEX IF NOT EXISTS idx_node_props_bool_key_value ON node_props_bool(key_id, value, node_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-19T16:50:23.984779Z
CREATE INDEX IF NOT EXISTS idx_node_props_json_key_value ON node_props_json(key_id, node_id);

-- [actor_ddl] 2026-03-19T16:50:23.985417Z
CREATE INDEX IF NOT EXISTS idx_edge_props_int_key_value ON edge_props_int(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T16:50:23.986278Z
CREATE INDEX IF NOT EXISTS idx_edge_props_text_key_value ON edge_props_text(key_id, value, edge_id);

-- [actor_query] 2026-03-19T16:50:23.986850Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_ddl] 2026-03-19T16:50:23.987205Z
CREATE INDEX IF NOT EXISTS idx_edge_props_real_key_value ON edge_props_real(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T16:50:23.987779Z
CREATE INDEX IF NOT EXISTS idx_edge_props_bool_key_value ON edge_props_bool(key_id, value, edge_id);

-- [actor_ddl] 2026-03-19T16:50:23.988391Z
CREATE INDEX IF NOT EXISTS idx_edge_props_json_key_value ON edge_props_json(key_id, edge_id);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:23.990317Z
INSERT OR REPLACE INTO block ("created_at", "id", "updated_at", "parent_id", "document_id", "content", "content_type", "properties") VALUES (1773939023988, 'block:default-layout-root', 1773939023989, 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'Holon Layout', 'text', '{"sequence":0,"ID":"default-layout-root"}');

-- [transaction_stmt] 2026-03-19T16:50:23.990523Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "updated_at", "content_type", "content", "parent_id", "id", "source_language", "properties") VALUES ('doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023988, 1773939023989, 'source', 'columns(#{gap: 4, item_template: block_ref()})
', 'block:default-layout-root', 'block:default-layout-root::render::0', 'render', '{"sequence":1,"ID":"default-layout-root::render::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.990704Z
INSERT OR REPLACE INTO block ("document_id", "content", "updated_at", "parent_id", "id", "created_at", "content_type", "source_language", "properties") VALUES ('doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'from children
filter content_type != "source"
derive {
  seq = s"json_extract(properties, ''$.\"column-order\"'')" ?? 999999,
  collapse_to = s"json_extract(properties, ''$.\"collapse-to\"'')",
  ideal_width = s"json_extract(properties, ''$.\"ideal-width\"'')",
  priority = s"json_extract(properties, ''$.\"column-priority\"'')"
}
sort seq
', 1773939023989, 'block:default-layout-root', 'block:default-layout-root::src::0', 1773939023988, 'source', 'holon_prql', '{"sequence":2,"ID":"default-layout-root::src::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.990922Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "created_at", "content_type", "id", "parent_id", "content", "properties") VALUES (1773939023989, 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023988, 'text', 'block:default-left-sidebar', 'block:default-layout-root', 'Left Sidebar', '{"sequence":3,"ID":"default-left-sidebar"}');

-- [transaction_stmt] 2026-03-19T16:50:23.991089Z
INSERT OR REPLACE INTO block ("document_id", "id", "created_at", "updated_at", "source_language", "content", "content_type", "parent_id", "properties") VALUES ('doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'block:default-left-sidebar::render::0', 1773939023988, 1773939023989, 'render', 'list(#{sortkey: "name", item_template: clickable(row(icon("folder"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})
', 'source', 'block:default-left-sidebar', '{"sequence":4,"ID":"default-left-sidebar::render::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.991270Z
INSERT OR REPLACE INTO block ("updated_at", "id", "parent_id", "content_type", "document_id", "content", "source_language", "created_at", "properties") VALUES (1773939023989, 'block:default-left-sidebar::src::0', 'block:default-left-sidebar', 'source', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'from document
filter name != ""
', 'holon_prql', 1773939023988, '{"sequence":5,"ID":"default-left-sidebar::src::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.991437Z
INSERT OR REPLACE INTO block ("content", "content_type", "updated_at", "created_at", "id", "parent_id", "document_id", "properties") VALUES ('Main Panel', 'text', 1773939023989, 1773939023988, 'block:default-main-panel', 'block:default-layout-root', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', '{"ID":"default-main-panel","sequence":6}');

-- [transaction_stmt] 2026-03-19T16:50:23.991598Z
INSERT OR REPLACE INTO block ("content", "created_at", "parent_id", "content_type", "document_id", "updated_at", "id", "source_language", "properties") VALUES ('MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d
', 1773939023988, 'block:default-main-panel', 'source', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023989, 'block:default-main-panel::src::0', 'holon_gql', '{"sequence":7,"ID":"default-main-panel::src::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.991777Z
INSERT OR REPLACE INTO block ("content", "content_type", "parent_id", "updated_at", "id", "source_language", "document_id", "created_at", "properties") VALUES ('tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})
', 'source', 'block:default-main-panel', 1773939023989, 'block:default-main-panel::render::0', 'render', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023988, '{"ID":"default-main-panel::render::0","sequence":8}');

-- [transaction_stmt] 2026-03-19T16:50:23.991952Z
INSERT OR REPLACE INTO block ("parent_id", "content", "id", "document_id", "content_type", "created_at", "updated_at", "properties") VALUES ('block:default-layout-root', 'Right Sidebar', 'block:default-right-sidebar', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'text', 1773939023988, 1773939023989, '{"ID":"default-right-sidebar","sequence":9}');

-- [transaction_stmt] 2026-03-19T16:50:23.992119Z
INSERT OR REPLACE INTO block ("content", "updated_at", "content_type", "id", "parent_id", "document_id", "created_at", "source_language", "properties") VALUES ('list(#{item_template: render_entity()})
', 1773939023989, 'source', 'block:default-right-sidebar::render::0', 'block:default-right-sidebar', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023988, 'render', '{"sequence":10,"ID":"default-right-sidebar::render::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.992293Z
INSERT OR REPLACE INTO block ("content", "document_id", "created_at", "updated_at", "source_language", "id", "parent_id", "content_type", "properties") VALUES ('from children
', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023988, 1773939023989, 'holon_prql', 'block:default-right-sidebar::src::0', 'block:default-right-sidebar', 'source', '{"sequence":11,"ID":"default-right-sidebar::src::0"}');

-- [transaction_stmt] 2026-03-19T16:50:23.992463Z
INSERT OR REPLACE INTO block ("content", "updated_at", "content_type", "parent_id", "id", "document_id", "created_at", "properties") VALUES ('Block Profiles', 1773939023989, 'text', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'block:default-block-profiles', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 1773939023988, '{"ID":"default-block-profiles","sequence":12}');

-- [transaction_stmt] 2026-03-19T16:50:23.992632Z
INSERT OR REPLACE INTO block ("updated_at", "content", "document_id", "content_type", "created_at", "parent_id", "id", "source_language", "properties") VALUES (1773939023989, 'entity_name: block
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
', 'doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97', 'source', 1773939023988, 'block:default-block-profiles', 'block:default-block-profiles::src::0', 'holon_entity_profile_yaml', '{"ID":"default-block-profiles::src::0","sequence":13}');

-- Wait 6ms
-- [transaction_stmt] 2026-03-19T16:50:23.999077Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PGTGHTTA46T3P9AF8', 'block.created', 'block', 'block:default-layout-root', 'sql', 'confirmed', '{"data":{"created_at":1773939023988,"id":"block:default-layout-root","updated_at":1773939023989,"parent_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","content":"Holon Layout","content_type":"text","properties":{"ID":"default-layout-root","sequence":0}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:23.999490Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P5E5BQCD8XQXYFDJX', 'block.created', 'block', 'block:default-layout-root::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","created_at":1773939023988,"updated_at":1773939023989,"content_type":"source","content":"columns(#{gap: 4, item_template: block_ref()})\\n","parent_id":"block:default-layout-root","id":"block:default-layout-root::render::0","source_language":"render","properties":{"sequence":1,"ID":"default-layout-root::render::0"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.000503Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PD2PW8BWFE5A8DDVJ', 'block.created', 'block', 'block:default-layout-root::src::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","content":"from children\\nfilter content_type != \\"source\\"\\nderive {\\n  seq = s\\"json_extract(properties, ''$.\\\\\\"column-order\\\\\\"'')\\" ?? 999999,\\n  collapse_to = s\\"json_extract(properties, ''$.\\\\\\"collapse-to\\\\\\"'')\\",\\n  ideal_width = s\\"json_extract(properties, ''$.\\\\\\"ideal-width\\\\\\"'')\\",\\n  priority = s\\"json_extract(properties, ''$.\\\\\\"column-priority\\\\\\"'')\\"\\n}\\nsort seq\\n","updated_at":1773939023989,"parent_id":"block:default-layout-root","id":"block:default-layout-root::src::0","created_at":1773939023988,"content_type":"source","source_language":"holon_prql","properties":{"sequence":2,"ID":"default-layout-root::src::0"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.000863Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P8X7Q6PHG7875C8MC', 'block.created', 'block', 'block:default-left-sidebar', 'sql', 'confirmed', '{"data":{"updated_at":1773939023989,"document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","created_at":1773939023988,"content_type":"text","id":"block:default-left-sidebar","parent_id":"block:default-layout-root","content":"Left Sidebar","properties":{"sequence":3,"ID":"default-left-sidebar"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.001188Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P1ZT9S04BJKSJ4KZ6', 'block.created', 'block', 'block:default-left-sidebar::render::0', 'sql', 'confirmed', '{"data":{"document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","id":"block:default-left-sidebar::render::0","created_at":1773939023988,"updated_at":1773939023989,"source_language":"render","content":"list(#{sortkey: \\"name\\", item_template: clickable(row(icon(\\"folder\\"), spacer(6), text(col(\\"name\\"))), #{action: navigation_focus(#{region: \\"main\\", block_id: col(\\"id\\")})})})\\n","content_type":"source","parent_id":"block:default-left-sidebar","properties":{"sequence":4,"ID":"default-left-sidebar::render::0"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.002231Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PPTQHJSCVDCCQBZBX', 'block.created', 'block', 'block:default-left-sidebar::src::0', 'sql', 'confirmed', '{"data":{"updated_at":1773939023989,"id":"block:default-left-sidebar::src::0","parent_id":"block:default-left-sidebar","content_type":"source","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","content":"from document\\nfilter name != \\"\\"\\n","source_language":"holon_prql","created_at":1773939023988,"properties":{"ID":"default-left-sidebar::src::0","sequence":5}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.002571Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P2771P5JK26ASS1AW', 'block.created', 'block', 'block:default-main-panel', 'sql', 'confirmed', '{"data":{"content":"Main Panel","content_type":"text","updated_at":1773939023989,"created_at":1773939023988,"id":"block:default-main-panel","parent_id":"block:default-layout-root","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","properties":{"sequence":6,"ID":"default-main-panel"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.003519Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PZQ0ZZ0P6KZE9EC22', 'block.created', 'block', 'block:default-main-panel::src::0', 'sql', 'confirmed', '{"data":{"content":"MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d\\n","created_at":1773939023988,"parent_id":"block:default-main-panel","content_type":"source","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","updated_at":1773939023989,"id":"block:default-main-panel::src::0","source_language":"holon_gql","properties":{"ID":"default-main-panel::src::0","sequence":7}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.003841Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P8096X6WXNMQF168R', 'block.created', 'block', 'block:default-main-panel::render::0', 'sql', 'confirmed', '{"data":{"content":"tree(#{parent_id: col(\\"parent_id\\"), sortkey: col(\\"sequence\\"), item_template: render_entity()})\\n","content_type":"source","parent_id":"block:default-main-panel","updated_at":1773939023989,"id":"block:default-main-panel::render::0","source_language":"render","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","created_at":1773939023988,"properties":{"sequence":8,"ID":"default-main-panel::render::0"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.004769Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PQZ4CYP46FAW1Y92P', 'block.created', 'block', 'block:default-right-sidebar', 'sql', 'confirmed', '{"data":{"parent_id":"block:default-layout-root","content":"Right Sidebar","id":"block:default-right-sidebar","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","content_type":"text","created_at":1773939023988,"updated_at":1773939023989,"properties":{"ID":"default-right-sidebar","sequence":9}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.005653Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P0P4A7MJZPM13E0ZH', 'block.created', 'block', 'block:default-right-sidebar::render::0', 'sql', 'confirmed', '{"data":{"content":"list(#{item_template: render_entity()})\\n","updated_at":1773939023989,"content_type":"source","id":"block:default-right-sidebar::render::0","parent_id":"block:default-right-sidebar","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","created_at":1773939023988,"source_language":"render","properties":{"ID":"default-right-sidebar::render::0","sequence":10}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.006622Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PTDGBHZHEHZN6CVCJ', 'block.created', 'block', 'block:default-right-sidebar::src::0', 'sql', 'confirmed', '{"data":{"content":"from children\\n","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","created_at":1773939023988,"updated_at":1773939023989,"source_language":"holon_prql","id":"block:default-right-sidebar::src::0","parent_id":"block:default-right-sidebar","content_type":"source","properties":{"sequence":11,"ID":"default-right-sidebar::src::0"}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.007622Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3P2NX2J8EB44D236GD', 'block.created', 'block', 'block:default-block-profiles', 'sql', 'confirmed', '{"data":{"content":"Block Profiles","updated_at":1773939023989,"content_type":"text","parent_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","id":"block:default-block-profiles","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","created_at":1773939023988,"properties":{"ID":"default-block-profiles","sequence":12}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.007948Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R3PAMS96MZ6QYASMYSR', 'block.created', 'block', 'block:default-block-profiles::src::0', 'sql', 'confirmed', '{"data":{"updated_at":1773939023989,"content":"entity_name: block\\n\\ncomputed:\\n  is_task: ''= task_state != ()''\\n  is_source: ''= content_type == \\"source\\"''\\n  has_query_source: ''= query_source(id) != ()''\\n  todo_states: ''= if document_id != () { let d = document(document_id); if d != () { d.todo_keywords } else { () } } else { () }''\\n\\ndefault:\\n  render: ''row(icon(\\"orgmode\\"), spacer(8), editable_text(col(\\"content\\")))''\\n\\nvariants:\\n  - name: query_block\\n    condition: ''= has_query_source''\\n    render: ''block_ref()''\\n  - name: task\\n    condition: ''= is_task''\\n    render: ''row(state_toggle(col(\\"task_state\\"), #{states: col(\\"todo_states\\")}), spacer(8), editable_text(col(\\"content\\")))''\\n  - name: source\\n    condition: ''= is_source''\\n    render: ''source_editor(#{language: col(\\"source_language\\"), content: col(\\"content\\")})''\\n","document_id":"doc:c9f9bca6-db24-434d-9efc-eddbc11ebc97","content_type":"source","created_at":1773939023988,"parent_id":"block:default-block-profiles","id":"block:default-block-profiles::src::0","source_language":"holon_entity_profile_yaml","properties":{"ID":"default-block-profiles::src::0","sequence":13}}}', NULL, NULL, 1773939023990, NULL, NULL);

-- Wait 4ms
-- [actor_query] 2026-03-19T16:50:24.012294Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T16:50:24.015115Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content_type", "id", "content", "parent_id", "document_id", "properties") VALUES (1773939024014, 1773939024012, 'text', 'block:cc-history-root', 'Claude Code History', 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'doc:693b1d91-536e-431d-8292-82f69032a1d4', '{"ID":"cc-history-root","sequence":0}');

-- [transaction_stmt] 2026-03-19T16:50:24.015362Z
INSERT OR REPLACE INTO block ("document_id", "id", "content_type", "content", "updated_at", "parent_id", "created_at", "properties") VALUES ('doc:693b1d91-536e-431d-8292-82f69032a1d4', 'block:cc-projects', 'text', 'Projects', 1773939024014, 'block:cc-history-root', 1773939024013, '{"ID":"cc-projects","sequence":1}');

-- [transaction_stmt] 2026-03-19T16:50:24.015541Z
INSERT OR REPLACE INTO block ("source_language", "content_type", "content", "id", "updated_at", "created_at", "document_id", "parent_id", "properties") VALUES ('holon_prql', 'source', 'from cc_project
select {id, original_path, session_count, last_activity}
sort {-last_activity}
', 'block:block:cc-projects::src::0', 1773939024014, 1773939024013, 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'block:cc-projects', '{"ID":"block:cc-projects::src::0","sequence":2}');

-- [transaction_stmt] 2026-03-19T16:50:24.015725Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "source_language", "id", "content", "content_type", "parent_id", "document_id", "properties") VALUES (1773939024013, 1773939024014, 'render', 'block:block:cc-projects::render::0', 'list(#{item_template: row(text(col("original_path")), spacer(16), text(col("session_count")), spacer(8), text(col("last_activity")))})
', 'source', 'block:cc-projects', 'doc:693b1d91-536e-431d-8292-82f69032a1d4', '{"sequence":3,"ID":"block:cc-projects::render::0"}');

-- [transaction_stmt] 2026-03-19T16:50:24.015911Z
INSERT OR REPLACE INTO block ("id", "document_id", "updated_at", "created_at", "content", "parent_id", "content_type", "properties") VALUES ('block:cc-sessions', 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 1773939024014, 1773939024013, 'Recent Sessions', 'block:cc-history-root', 'text', '{"ID":"cc-sessions","sequence":4}');

-- [transaction_stmt] 2026-03-19T16:50:24.016081Z
INSERT OR REPLACE INTO block ("content", "document_id", "content_type", "id", "source_language", "created_at", "updated_at", "parent_id", "properties") VALUES ('from cc_session
filter message_count > 0
select {id, first_prompt, message_count, model, modified, git_branch}
sort {-modified}
take 30
', 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'source', 'block:block:cc-sessions::src::0', 'holon_prql', 1773939024013, 1773939024014, 'block:cc-sessions', '{"ID":"block:cc-sessions::src::0","sequence":5}');

-- [transaction_stmt] 2026-03-19T16:50:24.016265Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "document_id", "content", "id", "updated_at", "source_language", "parent_id", "properties") VALUES ('source', 1773939024013, 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'list(#{item_template: row(text(col("first_prompt")), spacer(16), text(col("message_count")), spacer(8), text(col("modified")))})
', 'block:block:cc-sessions::render::0', 1773939024014, 'render', 'block:cc-sessions', '{"sequence":6,"ID":"block:cc-sessions::render::0"}');

-- [transaction_stmt] 2026-03-19T16:50:24.016449Z
INSERT OR REPLACE INTO block ("content_type", "id", "created_at", "document_id", "parent_id", "content", "updated_at", "properties") VALUES ('text', 'block:cc-tasks', 1773939024013, 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'block:cc-history-root', 'Tasks', 1773939024014, '{"ID":"cc-tasks","sequence":7}');

-- [transaction_stmt] 2026-03-19T16:50:24.016618Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "document_id", "id", "source_language", "parent_id", "content", "content_type", "properties") VALUES (1773939024013, 1773939024014, 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'block:block:cc-tasks::src::0', 'holon_prql', 'block:cc-tasks', 'from cc_task
filter status == "in_progress"
select {id, subject, status, created_at}
sort {-created_at}
', 'source', '{"ID":"block:cc-tasks::src::0","sequence":8}');

-- [transaction_stmt] 2026-03-19T16:50:24.016799Z
INSERT OR REPLACE INTO block ("id", "created_at", "document_id", "content", "parent_id", "content_type", "updated_at", "source_language", "properties") VALUES ('block:block:cc-tasks::render::0', 1773939024013, 'doc:693b1d91-536e-431d-8292-82f69032a1d4', 'list(#{item_template: row(text(col("status")), spacer(8), text(col("subject")))})
', 'block:cc-tasks', 'source', 1773939024014, 'render', '{"ID":"block:cc-tasks::render::0","sequence":9}');

-- Wait 5ms
-- [transaction_stmt] 2026-03-19T16:50:24.021944Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4E99QMDAS2X4617MD4', 'block.created', 'block', 'block:cc-history-root', 'sql', 'confirmed', '{"data":{"updated_at":1773939024014,"created_at":1773939024012,"content_type":"text","id":"block:cc-history-root","content":"Claude Code History","parent_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","properties":{"sequence":0,"ID":"cc-history-root"}}}', NULL, NULL, 1773939024014, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.022306Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4EN0NQK6GF4HBPFDWM', 'block.created', 'block', 'block:cc-projects', 'sql', 'confirmed', '{"data":{"document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","id":"block:cc-projects","content_type":"text","content":"Projects","updated_at":1773939024014,"parent_id":"block:cc-history-root","created_at":1773939024013,"properties":{"ID":"cc-projects","sequence":1}}}', NULL, NULL, 1773939024014, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.022645Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4EPP6CCNSMN42R5HSX', 'block.created', 'block', 'block:block:cc-projects::src::0', 'sql', 'confirmed', '{"data":{"source_language":"holon_prql","content_type":"source","content":"from cc_project\\nselect {id, original_path, session_count, last_activity}\\nsort {-last_activity}\\n","id":"block:block:cc-projects::src::0","updated_at":1773939024014,"created_at":1773939024013,"document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","parent_id":"block:cc-projects","properties":{"ID":"block:cc-projects::src::0","sequence":2}}}', NULL, NULL, 1773939024014, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.022971Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4E7TW52E6D537ZY1WK', 'block.created', 'block', 'block:block:cc-projects::render::0', 'sql', 'confirmed', '{"data":{"created_at":1773939024013,"updated_at":1773939024014,"source_language":"render","id":"block:block:cc-projects::render::0","content":"list(#{item_template: row(text(col(\\"original_path\\")), spacer(16), text(col(\\"session_count\\")), spacer(8), text(col(\\"last_activity\\")))})\\n","content_type":"source","parent_id":"block:cc-projects","document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","properties":{"sequence":3,"ID":"block:cc-projects::render::0"}}}', NULL, NULL, 1773939024014, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.024356Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4ES8VET7F3AHZ3BZ9W', 'block.created', 'block', 'block:cc-sessions', 'sql', 'confirmed', '{"data":{"id":"block:cc-sessions","document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","updated_at":1773939024014,"created_at":1773939024013,"content":"Recent Sessions","parent_id":"block:cc-history-root","content_type":"text","properties":{"sequence":4,"ID":"cc-sessions"}}}', NULL, NULL, 1773939024014, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.024633Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4F6RPZ6FNR1WCHM3P9', 'block.created', 'block', 'block:block:cc-sessions::src::0', 'sql', 'confirmed', '{"data":{"content":"from cc_session\\nfilter message_count > 0\\nselect {id, first_prompt, message_count, model, modified, git_branch}\\nsort {-modified}\\ntake 30\\n","document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","content_type":"source","id":"block:block:cc-sessions::src::0","source_language":"holon_prql","created_at":1773939024013,"updated_at":1773939024014,"parent_id":"block:cc-sessions","properties":{"sequence":5,"ID":"block:cc-sessions::src::0"}}}', NULL, NULL, 1773939024015, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.024907Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4FTMD41K42XN7SR9HX', 'block.created', 'block', 'block:block:cc-sessions::render::0', 'sql', 'confirmed', '{"data":{"content_type":"source","created_at":1773939024013,"document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","content":"list(#{item_template: row(text(col(\\"first_prompt\\")), spacer(16), text(col(\\"message_count\\")), spacer(8), text(col(\\"modified\\")))})\\n","id":"block:block:cc-sessions::render::0","updated_at":1773939024014,"source_language":"render","parent_id":"block:cc-sessions","properties":{"sequence":6,"ID":"block:cc-sessions::render::0"}}}', NULL, NULL, 1773939024015, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.025187Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4FFQ3KKE7G1GRK6MHK', 'block.created', 'block', 'block:cc-tasks', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:cc-tasks","created_at":1773939024013,"document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","parent_id":"block:cc-history-root","content":"Tasks","updated_at":1773939024014,"properties":{"ID":"cc-tasks","sequence":7}}}', NULL, NULL, 1773939024015, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.025461Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4FP90Z9EDXX4QCZD0S', 'block.created', 'block', 'block:block:cc-tasks::src::0', 'sql', 'confirmed', '{"data":{"created_at":1773939024013,"updated_at":1773939024014,"document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","id":"block:block:cc-tasks::src::0","source_language":"holon_prql","parent_id":"block:cc-tasks","content":"from cc_task\\nfilter status == \\"in_progress\\"\\nselect {id, subject, status, created_at}\\nsort {-created_at}\\n","content_type":"source","properties":{"sequence":8,"ID":"block:cc-tasks::src::0"}}}', NULL, NULL, 1773939024015, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.026342Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R4FZNHZ9KP64S8B0W52', 'block.created', 'block', 'block:block:cc-tasks::render::0', 'sql', 'confirmed', '{"data":{"id":"block:block:cc-tasks::render::0","created_at":1773939024013,"document_id":"doc:693b1d91-536e-431d-8292-82f69032a1d4","content":"list(#{item_template: row(text(col(\\"status\\")), spacer(8), text(col(\\"subject\\")))})\\n","parent_id":"block:cc-tasks","content_type":"source","updated_at":1773939024014,"source_language":"render","properties":{"sequence":9,"ID":"block:cc-tasks::render::0"}}}', NULL, NULL, 1773939024015, NULL, NULL);

-- Wait 3ms
-- [actor_query] 2026-03-19T16:50:24.030115Z
SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%';

-- [actor_ddl] 2026-03-19T16:50:24.030377Z
DROP VIEW IF EXISTS watch_view_b271926fc3f569a8;

-- [actor_query] 2026-03-19T16:50:24.031067Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_query] 2026-03-19T16:50:24.031301Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_b271926fc3f569a8';

-- [actor_ddl] 2026-03-19T16:50:24.031546Z
DROP VIEW IF EXISTS watch_view_e2453b3c0b29a253;

-- [actor_query] 2026-03-19T16:50:24.032204Z
SELECT * FROM document WHERE id = $id LIMIT 1;

-- [actor_query] 2026-03-19T16:50:24.032344Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_e2453b3c0b29a253';

-- [actor_ddl] 2026-03-19T16:50:24.032555Z
DROP VIEW IF EXISTS watch_view_d77ac41ba85c1706;

-- [actor_query] 2026-03-19T16:50:24.033123Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ($id, $parent_id, $name, $sort_key, $properties, $created_at, $updated_at);

-- [actor_query] 2026-03-19T16:50:24.033317Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_d77ac41ba85c1706';

-- [actor_query] 2026-03-19T16:50:24.033543Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_1570347602dda3f9';

-- [actor_ddl] 2026-03-19T16:50:24.033746Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_1570347602dda3f9 AS SELECT id, parent_id, content, content_type, source_language, block._change_origin AS _change_origin FROM block;

-- Wait 8ms
-- [actor_query] 2026-03-19T16:50:24.041754Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_dd27958f4ec0f8e7';

-- [actor_ddl] 2026-03-19T16:50:24.041953Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_dd27958f4ec0f8e7 AS SELECT id, content, block._change_origin AS _change_origin FROM block WHERE content_type = 'text';

-- Wait 4ms
-- [actor_query] 2026-03-19T16:50:24.046907Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_block';

-- [actor_query] 2026-03-19T16:50:24.047166Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_directory';

-- [actor_ddl] 2026-03-19T16:50:24.047350Z
CREATE MATERIALIZED VIEW events_view_directory AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'directory';

-- Wait 38ms
-- [actor_query] 2026-03-19T16:50:24.085855Z
SELECT name FROM sqlite_master WHERE type='view' AND name='events_view_file';

-- [actor_ddl] 2026-03-19T16:50:24.086206Z
CREATE MATERIALIZED VIEW events_view_file AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'file';

-- Wait 11ms
-- [transaction_stmt] 2026-03-19T16:50:24.097730Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "created_at", "document_id", "id", "parent_id", "content", "properties") VALUES (1773939024086, 'text', 1773939024037, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 1: Core Outliner', '{"ID":"599b60af-960d-4c9c-b222-d3d9de95c513","sequence":0}');

-- [transaction_stmt] 2026-03-19T16:50:24.098002Z
INSERT OR REPLACE INTO block ("parent_id", "content", "content_type", "id", "created_at", "document_id", "updated_at", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'MCP Server Frontend [/]', 'text', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 1773939024037, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, '{"task_state":"DOING","ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb","sequence":1}');

-- [transaction_stmt] 2026-03-19T16:50:24.098222Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "id", "content", "created_at", "content_type", "updated_at", "properties") VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:db59d038-8a47-43e9-9502-0472b493a6b9', 'Context parameter support ($context_id, $context_parent_id)', 1773939024038, 'text', 1773939024086, '{"ID":"db59d038-8a47-43e9-9502-0472b493a6b9","sequence":2}');

-- [transaction_stmt] 2026-03-19T16:50:24.098416Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "id", "updated_at", "content_type", "created_at", "content", "properties") VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:95ad6166-c03c-4417-a435-349e88b8e90a', 1773939024086, 'text', 1773939024038, 'MCP server (stdio + HTTP modes)', '{"ID":"95ad6166-c03c-4417-a435-349e88b8e90a","sequence":3}');

-- [transaction_stmt] 2026-03-19T16:50:24.098609Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "content", "document_id", "created_at", "id", "parent_id", "properties") VALUES ('text', 1773939024086, 'MCP tools for query execution and operations', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024038, 'block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', '{"sequence":4,"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b"}');

-- [transaction_stmt] 2026-03-19T16:50:24.098793Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "updated_at", "content", "content_type", "id", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773939024086, 'Block Operations [/]', 'text', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 1773939024038, '{"ID":"661368d9-e4bd-4722-b5c2-40f32006c643","sequence":5}');

-- [transaction_stmt] 2026-03-19T16:50:24.099010Z
INSERT OR REPLACE INTO block ("created_at", "id", "document_id", "updated_at", "content", "content_type", "parent_id", "properties") VALUES (1773939024038, 'block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'Block hierarchy (parent/child, indent/outdent)', 'text', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', '{"sequence":6,"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007"}');

-- [transaction_stmt] 2026-03-19T16:50:24.099207Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "parent_id", "content_type", "id", "content", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024038, 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'text', 'block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'Split block operation', 1773939024086, '{"sequence":7,"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34"}');

-- [transaction_stmt] 2026-03-19T16:50:24.099394Z
INSERT OR REPLACE INTO block ("document_id", "content", "created_at", "updated_at", "id", "parent_id", "content_type", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block CRUD (create, read, update, delete)', 1773939024038, 1773939024086, 'block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'text', '{"sequence":8,"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb"}');

-- [transaction_stmt] 2026-03-19T16:50:24.099575Z
INSERT OR REPLACE INTO block ("created_at", "id", "updated_at", "content_type", "parent_id", "content", "document_id", "properties") VALUES (1773939024038, 'block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 1773939024086, 'text', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'Block movement (move_up, move_down, move_block)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63","sequence":9}');

-- [transaction_stmt] 2026-03-19T16:50:24.099757Z
INSERT OR REPLACE INTO block ("document_id", "content", "created_at", "parent_id", "id", "updated_at", "content_type", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Undo/redo system (UndoStack + persistent OperationLogStore)', 1773939024038, 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'block:225edb45-f670-445a-9162-18c150210ee6', 1773939024086, 'text', '{"sequence":10,"task_state":"TODO","ID":"225edb45-f670-445a-9162-18c150210ee6"}');

-- [transaction_stmt] 2026-03-19T16:50:24.099948Z
INSERT OR REPLACE INTO block ("content", "id", "parent_id", "updated_at", "created_at", "content_type", "document_id", "properties") VALUES ('Storage & Data Layer [/]', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773939024086, 1773939024038, 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"444b24f6-d412-43c4-a14b-6e725b673cee","sequence":11}');

-- [transaction_stmt] 2026-03-19T16:50:24.100129Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "document_id", "content_type", "created_at", "content", "id", "properties") VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024039, 'Schema Module system with topological dependency ordering', 'block:c5007917-6723-49e2-95d4-c8bd3c7659ae', '{"sequence":12,"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae"}');

-- [transaction_stmt] 2026-03-19T16:50:24.100317Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "id", "content", "updated_at", "content_type", "parent_id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024039, 'block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'Fractional indexing for block ordering', 1773939024086, 'text', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"sequence":13,"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699"}');

-- [transaction_stmt] 2026-03-19T16:50:24.100500Z
INSERT OR REPLACE INTO block ("content_type", "id", "content", "updated_at", "document_id", "created_at", "parent_id", "properties") VALUES ('text', 'block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'Turso (embedded SQLite) backend with connection pooling', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024039, 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"sequence":14,"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57"}');

-- [transaction_stmt] 2026-03-19T16:50:24.100687Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "parent_id", "content_type", "created_at", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'QueryableCache wrapping DataSource with local caching', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'text', 1773939024039, 1773939024086, '{"sequence":15,"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280"}');

-- [transaction_stmt] 2026-03-19T16:50:24.100876Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "updated_at", "content", "content_type", "id", "document_id", "properties") VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 1773939024039, 1773939024086, 'Entity derive macro (#[derive(Entity)]) for schema generation', 'text', 'block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.101066Z
INSERT OR REPLACE INTO block ("parent_id", "content", "document_id", "updated_at", "content_type", "created_at", "id", "properties") VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 'CDC (Change Data Capture) streaming from storage to UI', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'text', 1773939024039, 'block:d318cae4-759d-487b-a909-81940223ecc1', '{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}');

-- [transaction_stmt] 2026-03-19T16:50:24.101251Z
INSERT OR REPLACE INTO block ("content_type", "id", "parent_id", "created_at", "updated_at", "content", "document_id", "properties") VALUES ('text', 'block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 1773939024039, 1773939024086, 'Command sourcing infrastructure (append-only operation log)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":18,"task_state":"DONE","ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222"}');

-- [transaction_stmt] 2026-03-19T16:50:24.101440Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "id", "parent_id", "content", "content_type", "document_id", "properties") VALUES (1773939024086, 1773939024039, 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'Procedural Macros [/]', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","sequence":19}');

-- [transaction_stmt] 2026-03-19T16:50:24.101636Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "updated_at", "content", "parent_id", "created_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024086, '#[operations_trait] macro for operation dispatch generation', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 1773939024039, 'block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', '{"sequence":20,"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4"}');

-- [transaction_stmt] 2026-03-19T16:50:24.101820Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "document_id", "content", "content_type", "created_at", "id", "properties") VALUES ('block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '#[triggered_by(...)] for operation availability', 'text', 1773939024039, 'block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', '{"sequence":21,"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44"}');

-- [transaction_stmt] 2026-03-19T16:50:24.102004Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "id", "updated_at", "parent_id", "content", "document_id", "properties") VALUES (1773939024039, 'text', 'block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 1773939024086, 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'Type inference for OperationDescriptor parameters', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":22,"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd"}');

-- [transaction_stmt] 2026-03-19T16:50:24.102197Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "document_id", "content_type", "created_at", "id", "content", "properties") VALUES ('block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024040, 'block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', '#[affects(...)] for field-level reactivity', '{"sequence":23,"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2"}');

-- [transaction_stmt] 2026-03-19T16:50:24.102387Z
INSERT OR REPLACE INTO block ("id", "parent_id", "document_id", "content_type", "content", "updated_at", "created_at", "properties") VALUES ('block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Performance [/]', 1773939024086, 1773939024040, '{"sequence":24,"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.102585Z
INSERT OR REPLACE INTO block ("id", "document_id", "content_type", "updated_at", "created_at", "parent_id", "content", "properties") VALUES ('block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024086, 1773939024040, 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'Virtual scrolling and lazy loading', '{"ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73","task_state":"DOING","sequence":25}');

-- [transaction_stmt] 2026-03-19T16:50:24.102815Z
INSERT OR REPLACE INTO block ("document_id", "content", "content_type", "parent_id", "created_at", "id", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Connection pooling for Turso', 'text', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 1773939024040, 'block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', 1773939024086, '{"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","task_state":"DOING","sequence":26}');

-- [transaction_stmt] 2026-03-19T16:50:24.103018Z
INSERT OR REPLACE INTO block ("content_type", "id", "created_at", "parent_id", "document_id", "updated_at", "content", "properties") VALUES ('text', 'block:e0567a06-5a62-4957-9457-c55a6661cee5', 1773939024040, 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'Full-text search indexing (Tantivy)', '{"ID":"e0567a06-5a62-4957-9457-c55a6661cee5","sequence":27}');

-- [transaction_stmt] 2026-03-19T16:50:24.103216Z
INSERT OR REPLACE INTO block ("id", "parent_id", "content_type", "content", "created_at", "document_id", "updated_at", "properties") VALUES ('block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 'Cross-Device Sync [/]', 1773939024040, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, '{"sequence":28,"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34"}');

-- [transaction_stmt] 2026-03-19T16:50:24.103412Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "id", "content_type", "created_at", "updated_at", "content", "properties") VALUES ('block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:43f329da-cfb4-4764-b599-06f4b6272f91', 'text', 1773939024040, 1773939024086, 'CollaborativeDoc with ALPN routing', '{"sequence":29,"ID":"43f329da-cfb4-4764-b599-06f4b6272f91"}');

-- [transaction_stmt] 2026-03-19T16:50:24.103606Z
INSERT OR REPLACE INTO block ("id", "created_at", "document_id", "content", "parent_id", "content_type", "updated_at", "properties") VALUES ('block:7aef40b2-14e1-4df0-a825-18603c55d198', 1773939024040, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Offline-first with background sync', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'text', 1773939024086, '{"ID":"7aef40b2-14e1-4df0-a825-18603c55d198","sequence":30}');

-- [transaction_stmt] 2026-03-19T16:50:24.103800Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "content", "content_type", "parent_id", "updated_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024040, 'Iroh P2P transport for Loro documents', 'text', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 1773939024086, 'block:e148d7b7-c505-4201-83b7-36986a981a56', '{"ID":"e148d7b7-c505-4201-83b7-36986a981a56","sequence":31}');

-- [transaction_stmt] 2026-03-19T16:50:24.103994Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "id", "parent_id", "content_type", "content", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024040, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 'Dependency Injection [/]', 1773939024086, '{"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9","sequence":32}');

-- [transaction_stmt] 2026-03-19T16:50:24.104199Z
INSERT OR REPLACE INTO block ("created_at", "id", "parent_id", "document_id", "content", "content_type", "updated_at", "properties") VALUES (1773939024040, 'block:b980e51f-0c91-4708-9a17-3d41284974b2', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'OperationDispatcher routing to providers', 'text', 1773939024086, '{"ID":"b980e51f-0c91-4708-9a17-3d41284974b2","sequence":33}');

-- [transaction_stmt] 2026-03-19T16:50:24.104394Z
INSERT OR REPLACE INTO block ("id", "document_id", "content_type", "created_at", "parent_id", "updated_at", "content", "properties") VALUES ('block:97cc8506-47d2-44cb-bdca-8e9a507953a0', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024041, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 1773939024086, 'BackendEngine as main orchestration point', '{"sequence":34,"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0"}');

-- [transaction_stmt] 2026-03-19T16:50:24.104605Z
INSERT OR REPLACE INTO block ("updated_at", "id", "content", "parent_id", "document_id", "created_at", "content_type", "properties") VALUES (1773939024086, 'block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'ferrous-di based service composition', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024041, 'text', '{"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8","sequence":35}');

-- [transaction_stmt] 2026-03-19T16:50:24.104802Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "updated_at", "parent_id", "content_type", "content", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024041, 1773939024086, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'text', 'SchemaRegistry with topological initialization', 'block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', '{"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47","sequence":36}');

-- [transaction_stmt] 2026-03-19T16:50:24.104999Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content_type", "id", "updated_at", "content", "created_at", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 1773939024086, 'Query & Render Pipeline [/]', 1773939024041, '{"ID":"b489c622-6c87-4bf6-8d35-787eb732d670","sequence":37}');

-- [transaction_stmt] 2026-03-19T16:50:24.105193Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "content_type", "id", "created_at", "content", "document_id", "properties") VALUES (1773939024086, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'text', 'block:1bbec456-7217-4477-a49c-0b8422e441e9', 1773939024041, 'Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":38,"ID":"1bbec456-7217-4477-a49c-0b8422e441e9"}');

-- [transaction_stmt] 2026-03-19T16:50:24.105394Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "id", "content", "created_at", "document_id", "updated_at", "properties") VALUES ('text', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:2b1c341e-5da2-4207-a609-f4af6d7ceebd', 'Automatic operation wiring (lineage analysis → widget binding)', 1773939024041, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, '{"sequence":39,"task_state":"DOING","ID":"2b1c341e-5da2-4207-a609-f4af6d7ceebd"}');

-- [transaction_stmt] 2026-03-19T16:50:24.105786Z
INSERT OR REPLACE INTO block ("content", "content_type", "created_at", "updated_at", "parent_id", "id", "document_id", "properties") VALUES ('GQL (graph query) support via EAV schema', 'text', 1773939024041, 1773939024086, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:2d44d7df-5d7d-4cfe-9061-459c7578e334', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"task_state":"DOING","ID":"2d44d7df-5d7d-4cfe-9061-459c7578e334","sequence":40}');

-- [transaction_stmt] 2026-03-19T16:50:24.105987Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "parent_id", "document_id", "id", "content", "created_at", "properties") VALUES ('text', 1773939024086, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:54ed1be5-765e-4884-87ab-02268e0208c7', 'PRQL compilation (PRQL → SQL + RenderSpec)', 1773939024041, '{"sequence":41,"ID":"54ed1be5-765e-4884-87ab-02268e0208c7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.106180Z
INSERT OR REPLACE INTO block ("updated_at", "id", "content_type", "parent_id", "content", "created_at", "document_id", "properties") VALUES (1773939024086, 'block:5384c1da-f058-4321-8401-929b3570c2a5', 'text', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'RenderSpec tree for declarative UI description', 1773939024041, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":42,"ID":"5384c1da-f058-4321-8401-929b3570c2a5"}');

-- [transaction_stmt] 2026-03-19T16:50:24.106373Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "document_id", "content", "content_type", "updated_at", "properties") VALUES ('block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc', 1773939024041, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Unified execute_query supporting PRQL/GQL/SQL', 'text', 1773939024086, '{"ID":"fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","sequence":43,"task_state":"DONE"}');

-- [transaction_stmt] 2026-03-19T16:50:24.106785Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content", "id", "created_at", "updated_at", "content_type", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'SQL direct execution support', 'block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 1773939024041, 1773939024086, 'text', '{"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","task_state":"DOING","sequence":44}');

-- [transaction_stmt] 2026-03-19T16:50:24.106976Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "content", "updated_at", "id", "document_id", "parent_id", "properties") VALUES ('text', 1773939024042, 'Loro CRDT Integration [/]', 1773939024086, 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', '{"sequence":45,"ID":"d9374dc3-05fc-40b2-896d-f88bb8a33c92"}');

-- [transaction_stmt] 2026-03-19T16:50:24.107150Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "document_id", "created_at", "id", "content", "updated_at", "properties") VALUES ('text', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024042, 'block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6', 'LoroBackend implementing CoreOperations trait', 1773939024086, '{"sequence":46,"ID":"b1dc3ad3-574b-472a-b74b-e3ea29a433e6"}');

-- [transaction_stmt] 2026-03-19T16:50:24.107538Z
INSERT OR REPLACE INTO block ("document_id", "content", "created_at", "id", "content_type", "parent_id", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LoroDocumentStore for managing CRDT documents on disk', 1773939024042, 'block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957', 'text', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 1773939024086, '{"task_state":"DOING","ID":"ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","sequence":47}');

-- [transaction_stmt] 2026-03-19T16:50:24.107721Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "parent_id", "updated_at", "content_type", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:35652c3f-720c-4e20-ab90-5e25e1429733', 'LoroBlockOperations as OperationProvider routing writes through CRDT', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 1773939024086, 'text', 1773939024042, '{"sequence":48,"ID":"35652c3f-720c-4e20-ab90-5e25e1429733"}');

-- [transaction_stmt] 2026-03-19T16:50:24.107931Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "content_type", "id", "content", "created_at", "document_id", "properties") VALUES (1773939024086, 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'text', 'block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2', 'Cycle detection in move_block', 1773939024042, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":49,"ID":"090731e3-38ae-4bf1-b5ec-dbb33eae4fb2"}');

-- [transaction_stmt] 2026-03-19T16:50:24.108099Z
INSERT OR REPLACE INTO block ("content", "created_at", "document_id", "content_type", "parent_id", "id", "updated_at", "properties") VALUES ('Loro-to-Turso materialization (CRDT → SQL cache → CDC)', 1773939024042, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'block:ddf208e4-9b73-422d-b8ab-4ec58b328907', 1773939024086, '{"sequence":50,"ID":"ddf208e4-9b73-422d-b8ab-4ec58b328907"}');

-- [transaction_stmt] 2026-03-19T16:50:24.108283Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "content", "parent_id", "content_type", "created_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'Org-Mode Sync [/]', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 1773939024042, 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', '{"sequence":51,"ID":"9af3a008-c1d7-422b-a1c8-e853f3ccb6fa"}');

-- [transaction_stmt] 2026-03-19T16:50:24.108462Z
INSERT OR REPLACE INTO block ("id", "document_id", "created_at", "parent_id", "content", "updated_at", "content_type", "properties") VALUES ('block:7bc5f362-0bf9-45a1-b2b7-6882585ed169', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024042, 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'OrgRenderer as single path for producing org text', 1773939024086, 'text', '{"sequence":52,"ID":"7bc5f362-0bf9-45a1-b2b7-6882585ed169"}');

-- [transaction_stmt] 2026-03-19T16:50:24.108641Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "created_at", "content", "updated_at", "content_type", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 1773939024042, 'Document identity & aliases (UUID ↔ file path mapping)', 1773939024086, 'text', 'block:8eab3453-25d2-4e7a-89f8-f9f79be939c9', '{"sequence":53,"ID":"8eab3453-25d2-4e7a-89f8-f9f79be939c9"}');

-- [transaction_stmt] 2026-03-19T16:50:24.108816Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "id", "content", "parent_id", "created_at", "content_type", "properties") VALUES (1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:fc60da1b-6065-4d36-8551-5479ff145df0', 'OrgSyncController with echo suppression', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 1773939024042, 'text', '{"sequence":54,"ID":"fc60da1b-6065-4d36-8551-5479ff145df0"}');

-- [transaction_stmt] 2026-03-19T16:50:24.108999Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "parent_id", "id", "document_id", "content_type", "content", "properties") VALUES (1773939024043, 1773939024086, 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'block:6e5a1157-b477-45a1-892f-57807b4d969b', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Bidirectional sync (file changes ↔ block changes)', '{"sequence":55,"ID":"6e5a1157-b477-45a1-892f-57807b4d969b"}');

-- [transaction_stmt] 2026-03-19T16:50:24.109179Z
INSERT OR REPLACE INTO block ("id", "parent_id", "created_at", "content", "updated_at", "document_id", "content_type", "properties") VALUES ('block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 1773939024043, 'Org file parsing (headlines, properties, source blocks)', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"sequence":56,"ID":"6e4dab75-cd13-4c5e-9168-bf266d11aa3f"}');

-- [transaction_stmt] 2026-03-19T16:50:24.109357Z
INSERT OR REPLACE INTO block ("id", "document_id", "created_at", "content_type", "updated_at", "parent_id", "content", "properties") VALUES ('block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024043, 'text', 1773939024086, 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'Flutter Frontend [/]', '{"ID":"bb3bc716-ca9a-438a-936d-03631e2ee929","sequence":57}');

-- [transaction_stmt] 2026-03-19T16:50:24.109538Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "document_id", "content_type", "content", "parent_id", "id", "properties") VALUES (1773939024086, 1773939024043, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'FFI bridge via flutter_rust_bridge', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f', '{"ID":"b4753cd8-47ea-4f7d-bd00-e1ec563aa43f","sequence":58}');

-- [transaction_stmt] 2026-03-19T16:50:24.109731Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "parent_id", "document_id", "content", "id", "content_type", "properties") VALUES (1773939024086, 1773939024043, 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Navigation system (history, cursor, focus)', 'block:3289bc82-f8a9-4cad-8545-ad1fee9dc282', 'text', '{"task_state":"DOING","sequence":59,"ID":"3289bc82-f8a9-4cad-8545-ad1fee9dc282"}');

-- [transaction_stmt] 2026-03-19T16:50:24.109915Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "document_id", "id", "content", "updated_at", "content_type", "properties") VALUES (1773939024043, 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:ebca0a24-f6f6-4c49-8a27-9d9973acf737', 'Block editor (outliner interactions)', 1773939024086, 'text', '{"sequence":60,"ID":"ebca0a24-f6f6-4c49-8a27-9d9973acf737"}');

-- [transaction_stmt] 2026-03-19T16:50:24.110102Z
INSERT OR REPLACE INTO block ("content_type", "content", "created_at", "updated_at", "parent_id", "document_id", "id", "properties") VALUES ('text', 'Reactive UI updates from CDC change streams', 1773939024043, 1773939024086, 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd', '{"sequence":61,"ID":"eb7e34f8-19f5-48f5-a22d-8f62493bafdd"}');

-- [transaction_stmt] 2026-03-19T16:50:24.110276Z
INSERT OR REPLACE INTO block ("content", "document_id", "content_type", "created_at", "updated_at", "id", "parent_id", "properties") VALUES ('Three-column layout (sidebar, main, right panel)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024043, 1773939024086, 'block:7a0a4905-59c5-4277-8114-1e9ca9d425e3', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', '{"ID":"7a0a4905-59c5-4277-8114-1e9ca9d425e3","sequence":62}');

-- [transaction_stmt] 2026-03-19T16:50:24.110453Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "created_at", "parent_id", "id", "content", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024043, 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'block:19d7b512-e5e0-469c-917b-eb27d7a38bed', 'Flutter desktop app shell', 1773939024086, '{"sequence":63,"ID":"19d7b512-e5e0-469c-917b-eb27d7a38bed"}');

-- [transaction_stmt] 2026-03-19T16:50:24.110677Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "id", "parent_id", "content_type", "content", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024043, 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 'Petri-Net Task Ranking (WSJF) [/]', 1773939024086, '{"ID":"afe4f75c-7948-4d4c-9724-4bfab7d47d88","sequence":64}');

-- [transaction_stmt] 2026-03-19T16:50:24.110886Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "created_at", "id", "document_id", "content", "content_type", "properties") VALUES ('block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 1773939024086, 1773939024043, 'block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Prototype blocks with =computed Rhai expressions', 'text', '{"task_state":"DOING","ID":"d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","sequence":65}');

-- [transaction_stmt] 2026-03-19T16:50:24.111060Z
INSERT OR REPLACE INTO block ("id", "content", "created_at", "content_type", "updated_at", "document_id", "parent_id", "properties") VALUES ('block:2d399fd7-79d8-41f1-846b-31dabcec208a', 'Verb dictionary (~30 German + English verbs → transition types)', 1773939024044, 'text', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', '{"ID":"2d399fd7-79d8-41f1-846b-31dabcec208a","sequence":66}');

-- [transaction_stmt] 2026-03-19T16:50:24.111242Z
INSERT OR REPLACE INTO block ("document_id", "content", "updated_at", "parent_id", "created_at", "content_type", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'rank_tasks() engine with tiebreak ordering', 1773939024086, 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 1773939024044, 'text', 'block:2385f4e3-25e1-4911-bf75-77cefd394206', '{"task_state":"DOING","sequence":67,"ID":"2385f4e3-25e1-4911-bf75-77cefd394206"}');

-- [transaction_stmt] 2026-03-19T16:50:24.111422Z
INSERT OR REPLACE INTO block ("id", "parent_id", "updated_at", "content", "document_id", "content_type", "created_at", "properties") VALUES ('block:cae619f2-26fe-464e-b67a-0a04f76543c9', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 1773939024086, 'Block → Petri Net materialization (petri.rs)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024044, '{"sequence":68,"task_state":"DOING","ID":"cae619f2-26fe-464e-b67a-0a04f76543c9"}');

-- [transaction_stmt] 2026-03-19T16:50:24.111606Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "id", "content_type", "content", "document_id", "created_at", "properties") VALUES ('block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 1773939024086, 'block:eaee1c9b-5466-428f-8dbb-f4882ccdb066', 'text', 'Self Descriptor (person block with is_self: true)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024044, '{"task_state":"DOING","ID":"eaee1c9b-5466-428f-8dbb-f4882ccdb066","sequence":69}');

-- [transaction_stmt] 2026-03-19T16:50:24.111785Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "updated_at", "document_id", "content_type", "content", "properties") VALUES ('block:023da362-ce5d-4a3b-827a-29e745d6f778', 1773939024044, 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'WSJF scoring (priority_weight × urgency_weight + position_weight)', '{"task_state":"DOING","ID":"023da362-ce5d-4a3b-827a-29e745d6f778","sequence":70}');

-- [transaction_stmt] 2026-03-19T16:50:24.111970Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "document_id", "created_at", "updated_at", "content", "id", "properties") VALUES ('block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024044, 1773939024086, 'Task syntax parser (@, ?, >, [[links]])', 'block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812', '{"ID":"46a8c75e-8ab8-4a5a-b4af-a1388f6a4812","sequence":71}');

-- [transaction_stmt] 2026-03-19T16:50:24.112159Z
INSERT OR REPLACE INTO block ("id", "content_type", "parent_id", "updated_at", "content", "created_at", "document_id", "properties") VALUES ('block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'Phase 2: First Integration (Todoist) [/]
Goal: Prove hybrid architecture', 1773939024044, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":72,"ID":"29c0aa5f-d9ca-46f3-8601-6023f87cefbd"}');

-- [transaction_stmt] 2026-03-19T16:50:24.112347Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "parent_id", "id", "document_id", "content", "updated_at", "properties") VALUES ('text', 1773939024044, 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Reconciliation [/]', 1773939024086, '{"ID":"00fa0916-2681-4699-9554-44fcb8e2ea6a","sequence":73}');

-- [transaction_stmt] 2026-03-19T16:50:24.112514Z
INSERT OR REPLACE INTO block ("id", "content", "document_id", "updated_at", "content_type", "parent_id", "created_at", "properties") VALUES ('block:632af903-5459-4d44-921a-43145e20dc82', 'Sync token management to prevent duplicate processing', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'text', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 1773939024044, '{"sequence":74,"ID":"632af903-5459-4d44-921a-43145e20dc82"}');

-- [transaction_stmt] 2026-03-19T16:50:24.112709Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "content", "created_at", "id", "document_id", "parent_id", "properties") VALUES (1773939024086, 'text', 'Conflict detection and resolution UI', 1773939024044, 'block:78f9d6e3-42d4-4975-910d-3728e23410b1', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', '{"sequence":75,"ID":"78f9d6e3-42d4-4975-910d-3728e23410b1"}');

-- [transaction_stmt] 2026-03-19T16:50:24.112904Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "parent_id", "content_type", "created_at", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:fa2854d1-2751-4a07-8f83-70c2f9c6c190', 'Last-write-wins for concurrent edits', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'text', 1773939024044, 1773939024086, '{"sequence":76,"ID":"fa2854d1-2751-4a07-8f83-70c2f9c6c190"}');

-- [transaction_stmt] 2026-03-19T16:50:24.113086Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "parent_id", "content", "created_at", "updated_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'Operation Queue & Offline Support [/]', 1773939024045, 1773939024086, 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', '{"sequence":77,"ID":"043ed925-6bf2-4db3-baf8-2277f1a5afaa"}');

-- [transaction_stmt] 2026-03-19T16:50:24.113307Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "created_at", "content_type", "parent_id", "id", "content", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 1773939024045, 'text', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3', 'Offline operation queue with retry logic', '{"ID":"5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","sequence":78}');

-- [transaction_stmt] 2026-03-19T16:50:24.113480Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "document_id", "parent_id", "updated_at", "id", "content", "properties") VALUES ('text', 1773939024045, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 1773939024086, 'block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05', 'Sync status indicators (synced, pending, conflict, error)', '{"ID":"7de8d37b-49ba-4ada-9b1e-df1c41c0db05","sequence":79}');

-- [transaction_stmt] 2026-03-19T16:50:24.113662Z
INSERT OR REPLACE INTO block ("id", "parent_id", "content", "document_id", "content_type", "updated_at", "created_at", "properties") VALUES ('block:302eb0c5-56fe-4980-8292-bae8a9a0450a', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'Optimistic updates with ID mapping (internal ↔ external)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024086, 1773939024045, '{"ID":"302eb0c5-56fe-4980-8292-bae8a9a0450a","sequence":80}');

-- [transaction_stmt] 2026-03-19T16:50:24.113839Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "id", "content_type", "document_id", "content", "created_at", "properties") VALUES ('block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 1773939024086, 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist-Specific Features [/]', 1773939024045, '{"sequence":81,"ID":"b1b2037e-b2e9-45db-8cb9-2ed783ede2ce"}');

-- [transaction_stmt] 2026-03-19T16:50:24.114026Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "id", "created_at", "content", "document_id", "content_type", "properties") VALUES ('block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 1773939024086, 'block:a27cd79b-63bd-4704-b20f-f3b595838e89', 1773939024045, 'Bi-directional task completion sync', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"sequence":82,"ID":"a27cd79b-63bd-4704-b20f-f3b595838e89"}');

-- [transaction_stmt] 2026-03-19T16:50:24.114216Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "created_at", "content", "updated_at", "content_type", "id", "properties") VALUES ('block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024045, 'Todoist due dates → deadline penalty functions', 1773939024086, 'text', 'block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22', '{"sequence":83,"ID":"ab2868f6-ac6a-48de-b56f-ffa755f6cd22"}');

-- [transaction_stmt] 2026-03-19T16:50:24.114699Z
INSERT OR REPLACE INTO block ("id", "content", "created_at", "parent_id", "document_id", "content_type", "updated_at", "properties") VALUES ('block:f6e32a19-a659-47f7-b2dc-24142c6616f7', '@person labels → delegation/waiting_for tracking', 1773939024045, 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024086, '{"sequence":84,"ID":"f6e32a19-a659-47f7-b2dc-24142c6616f7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.114886Z
INSERT OR REPLACE INTO block ("content", "updated_at", "created_at", "parent_id", "content_type", "document_id", "id", "properties") VALUES ('Todoist priority → WSJF CoD weight mapping', 1773939024086, 1773939024045, 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:19923c1b-89ab-42f3-97a2-d78e994a2e1c', '{"ID":"19923c1b-89ab-42f3-97a2-d78e994a2e1c","sequence":85}');

-- [transaction_stmt] 2026-03-19T16:50:24.115070Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "id", "content", "content_type", "document_id", "parent_id", "properties") VALUES (1773939024045, 1773939024086, 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'MCP Client Bridge [/]', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', '{"sequence":86,"ID":"f37ab7bc-c89e-4b47-9317-3a9f7a440d2a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.115253Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "document_id", "id", "updated_at", "content", "parent_id", "properties") VALUES ('text', 1773939024045, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:4d30926a-54c4-40b4-978e-eeca2d273fd1', 1773939024086, 'Tool name normalization (kebab-case ↔ snake_case)', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', '{"sequence":87,"ID":"4d30926a-54c4-40b4-978e-eeca2d273fd1"}');

-- [transaction_stmt] 2026-03-19T16:50:24.115437Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "content", "created_at", "parent_id", "content_type", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'McpOperationProvider converting MCP tool schemas → OperationDescriptors', 1773939024046, 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'text', 'block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467', '{"ID":"c30b7e5a-4e9f-41e8-ab19-e803c93dc467","sequence":88}');

-- [transaction_stmt] 2026-03-19T16:50:24.115629Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "content", "document_id", "id", "content_type", "parent_id", "properties") VALUES (1773939024046, 1773939024086, 'holon-mcp-client crate for connecting to external MCP servers', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:836bab0e-5ac1-4df1-9f40-4005320c406e', 'text', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', '{"ID":"836bab0e-5ac1-4df1-9f40-4005320c406e","sequence":89}');

-- [transaction_stmt] 2026-03-19T16:50:24.115824Z
INSERT OR REPLACE INTO block ("created_at", "id", "document_id", "content_type", "updated_at", "parent_id", "content", "properties") VALUES (1773939024046, 'block:ceb59dae-6090-41be-aff7-89de33ec600a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024086, 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'YAML sidecar for UI annotations (affected_fields, triggered_by, preconditions)', '{"ID":"ceb59dae-6090-41be-aff7-89de33ec600a","sequence":90}');

-- [transaction_stmt] 2026-03-19T16:50:24.116007Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "id", "content", "content_type", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'block:419e493f-c2de-47c2-a612-787db669cd89', 'JSON Schema → TypeHint mapping', 'text', 1773939024046, '{"sequence":91,"ID":"419e493f-c2de-47c2-a612-787db669cd89"}');

-- [transaction_stmt] 2026-03-19T16:50:24.116201Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "updated_at", "content", "parent_id", "id", "content_type", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024046, 1773939024086, 'Todoist API Integration [/]', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'text', '{"sequence":92,"ID":"bdce9ec2-1508-47e9-891e-e12a7b228fcc"}');

-- [transaction_stmt] 2026-03-19T16:50:24.116390Z
INSERT OR REPLACE INTO block ("content_type", "id", "updated_at", "parent_id", "content", "created_at", "document_id", "properties") VALUES ('text', 'block:e9398514-1686-4fef-a44a-5fef1742d004', 1773939024086, 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'TodoistOperationProvider for operation routing', 1773939024046, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"e9398514-1686-4fef-a44a-5fef1742d004","sequence":93}');

-- [transaction_stmt] 2026-03-19T16:50:24.116565Z
INSERT OR REPLACE INTO block ("parent_id", "content", "created_at", "updated_at", "content_type", "id", "document_id", "properties") VALUES ('block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'Todoist REST API client', 1773939024046, 1773939024086, 'text', 'block:9670e586-5cda-42a2-8071-efaf855fd5d4', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":94,"ID":"9670e586-5cda-42a2-8071-efaf855fd5d4"}');

-- [transaction_stmt] 2026-03-19T16:50:24.116761Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "content_type", "content", "id", "parent_id", "created_at", "properties") VALUES (1773939024086, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Todoist entity types (tasks, projects, sections, labels)', 'block:f41aeaa5-fe1d-45a5-806d-1f815040a33d', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 1773939024046, '{"ID":"f41aeaa5-fe1d-45a5-806d-1f815040a33d","sequence":95}');

-- [transaction_stmt] 2026-03-19T16:50:24.116947Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "content", "id", "content_type", "updated_at", "parent_id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024046, 'TodoistSyncProvider with incremental sync tokens', 'block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe', 'text', 1773939024086, 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', '{"sequence":96,"ID":"d041e942-f3a1-4b7d-80b8-7de6eb289ebe"}');

-- [transaction_stmt] 2026-03-19T16:50:24.117129Z
INSERT OR REPLACE INTO block ("content", "content_type", "id", "document_id", "updated_at", "parent_id", "created_at", "properties") VALUES ('TodoistTaskDataSource implementing DataSource<TodoistTask>', 'text', 'block:f3b43be1-5503-4b1a-a724-fc657b47e18c', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024086, 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 1773939024046, '{"sequence":97,"ID":"f3b43be1-5503-4b1a-a724-fc657b47e18c"}');

-- [transaction_stmt] 2026-03-19T16:50:24.117319Z
INSERT OR REPLACE INTO block ("content", "created_at", "document_id", "parent_id", "id", "content_type", "updated_at", "properties") VALUES ('Phase 3: Multiple Integrations [/]
Goal: Validate type unification scales', 1773939024046, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'text', 1773939024086, '{"ID":"88810f15-a95b-4343-92e2-909c5113cc9c","sequence":98}');

-- [transaction_stmt] 2026-03-19T16:50:24.117511Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "content", "updated_at", "created_at", "id", "parent_id", "properties") VALUES ('text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Unified Item Types [/]', 1773939024086, 1773939024047, 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', '{"sequence":99,"ID":"9ea38e3d-383e-4c27-9533-d53f1f8b1fb2"}');

-- [transaction_stmt] 2026-03-19T16:50:24.117706Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "content", "created_at", "id", "parent_id", "document_id", "properties") VALUES ('text', 1773939024086, 'Macro-generated serialization boilerplate', 1773939024047, 'block:5b1e8251-be26-4099-b169-a330cc16f0a6', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"5b1e8251-be26-4099-b169-a330cc16f0a6","sequence":100}');

-- [transaction_stmt] 2026-03-19T16:50:24.117912Z
INSERT OR REPLACE INTO block ("content_type", "content", "updated_at", "id", "created_at", "parent_id", "document_id", "properties") VALUES ('text', 'Trait-based protocol for common task interface', 1773939024087, 'block:5b49aefd-e14f-4151-bf9e-ccccae3545ec', 1773939024047, 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"5b49aefd-e14f-4151-bf9e-ccccae3545ec","sequence":101}');

-- [transaction_stmt] 2026-03-19T16:50:24.118113Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "updated_at", "content", "created_at", "id", "content_type", "properties") VALUES ('block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'Extension structs for system-specific features', 1773939024047, 'block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447', 'text', '{"sequence":102,"ID":"e6162a0a-e9ae-494e-b3f5-4cf98cb2f447"}');

-- [transaction_stmt] 2026-03-19T16:50:24.118303Z
INSERT OR REPLACE INTO block ("id", "document_id", "content", "content_type", "created_at", "parent_id", "updated_at", "properties") VALUES ('block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-System Features [/]', 'text', 1773939024047, 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 1773939024087, '{"ID":"d6ab6d5f-68ae-404a-bcad-b5db61586634","sequence":103}');

-- [transaction_stmt] 2026-03-19T16:50:24.118489Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "created_at", "content", "id", "parent_id", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024047, 'Context Bundles assembling related items from all sources', 'block:5403c088-a551-4ca6-8830-34e00d5e5820', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 1773939024087, '{"ID":"5403c088-a551-4ca6-8830-34e00d5e5820","sequence":104}');

-- [transaction_stmt] 2026-03-19T16:50:24.118694Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "updated_at", "content", "created_at", "document_id", "id", "properties") VALUES ('block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'text', 1773939024087, 'Embedding third-party items anywhere in the graph', 1773939024047, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:091caad8-1689-472d-9130-e3c855c510a8', '{"ID":"091caad8-1689-472d-9130-e3c855c510a8","sequence":105}');

-- [transaction_stmt] 2026-03-19T16:50:24.119580Z
INSERT OR REPLACE INTO block ("id", "content", "content_type", "created_at", "updated_at", "parent_id", "document_id", "properties") VALUES ('block:cfb257f0-1a9c-426c-ab24-940eb18853ea', 'Unified search across all systems', 'text', 1773939024047, 1773939024087, 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":106,"ID":"cfb257f0-1a9c-426c-ab24-940eb18853ea"}');

-- [transaction_stmt] 2026-03-19T16:50:24.119770Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "created_at", "id", "parent_id", "content_type", "content", "properties") VALUES (1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024047, 'block:52a440c1-4099-4911-8d9d-e2d583dbdde7', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'text', 'P.A.R.A. project-based organization with auto-linking', '{"sequence":107,"ID":"52a440c1-4099-4911-8d9d-e2d583dbdde7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.119952Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "content_type", "updated_at", "id", "content", "document_id", "properties") VALUES (1773939024047, 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'text', 1773939024087, 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'Additional Integrations [/]', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":108,"ID":"34fa9276-cc30-4fcb-95b5-a97b5d708757"}');

-- [transaction_stmt] 2026-03-19T16:50:24.120142Z
INSERT OR REPLACE INTO block ("content", "updated_at", "content_type", "parent_id", "created_at", "id", "document_id", "properties") VALUES ('Linear integration (cycles, projects)', 1773939024087, 'text', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 1773939024047, 'block:9240c0d7-d60a-46e0-8265-ceacfbf04d50', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"9240c0d7-d60a-46e0-8265-ceacfbf04d50","sequence":109}');

-- [transaction_stmt] 2026-03-19T16:50:24.120333Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "content_type", "document_id", "updated_at", "id", "content", "properties") VALUES (1773939024048, 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'block:8ea813ff-b355-4165-b377-fbdef4d3d7d8', 'Google Calendar integration (events as time tokens)', '{"sequence":110,"ID":"8ea813ff-b355-4165-b377-fbdef4d3d7d8"}');

-- [transaction_stmt] 2026-03-19T16:50:24.120513Z
INSERT OR REPLACE INTO block ("id", "content", "created_at", "parent_id", "content_type", "updated_at", "document_id", "properties") VALUES ('block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29', 'Gmail integration (email threads, labels)', 1773939024048, 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'text', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":111,"ID":"ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29"}');

-- [transaction_stmt] 2026-03-19T16:50:24.120708Z
INSERT OR REPLACE INTO block ("document_id", "id", "content_type", "created_at", "content", "parent_id", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:f583e6d9-f67d-4997-a658-ed00149a34cc', 'text', 1773939024048, 'JIRA integration (sprints, story points, epics)', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 1773939024087, '{"sequence":112,"ID":"f583e6d9-f67d-4997-a658-ed00149a34cc"}');

-- [transaction_stmt] 2026-03-19T16:50:24.120902Z
INSERT OR REPLACE INTO block ("id", "document_id", "content", "parent_id", "content_type", "updated_at", "created_at", "properties") VALUES ('block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'GPUI Components', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'text', 1773939024087, 1773939024048, '{"ID":"9fed69a3-9180-4eba-a778-fa93bc398064","sequence":113}');

-- [transaction_stmt] 2026-03-19T16:50:24.121098Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content", "content_type", "id", "parent_id", "updated_at", "properties") VALUES (1773939024048, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'https://github.com/MeowLynxSea/yororen-ui', 'text', 'block:9f523ce8-5449-4a2f-81c8-8ee08399fc31', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 1773939024087, '{"sequence":114,"ID":"9f523ce8-5449-4a2f-81c8-8ee08399fc31"}');

-- [transaction_stmt] 2026-03-19T16:50:24.121611Z
INSERT OR REPLACE INTO block ("id", "created_at", "document_id", "content", "content_type", "parent_id", "updated_at", "properties") VALUES ('block:fd965570-883d-48f7-82b0-92ba257b2597', 1773939024048, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Pomodoro
https://github.com/rubbieKelvin/bmo', 'text', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 1773939024087, '{"sequence":115,"ID":"fd965570-883d-48f7-82b0-92ba257b2597"}');

-- [transaction_stmt] 2026-03-19T16:50:24.121813Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "updated_at", "content_type", "content", "id", "document_id", "properties") VALUES (1773939024048, 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 1773939024087, 'text', 'Diff viewer
https://github.com/BlixtWallet/hunk', 'block:9657e201-4426-4091-891b-eb40e299d81d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":116,"ID":"9657e201-4426-4091-891b-eb40e299d81d"}');

-- [transaction_stmt] 2026-03-19T16:50:24.122357Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "content", "created_at", "id", "parent_id", "content_type", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'Animation
https://github.com/chi11321/gpui-animation', 1773939024048, 'block:61a47437-c394-42db-b195-3dabbd5d87ab', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'text', '{"ID":"61a47437-c394-42db-b195-3dabbd5d87ab","sequence":117}');

-- [transaction_stmt] 2026-03-19T16:50:24.122543Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "content_type", "content", "parent_id", "id", "created_at", "properties") VALUES (1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Editor
https://github.com/iamnbutler/gpui-editor', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'block:5841efc0-cfe6-4e69-9dbc-9f627693e59a', 1773939024048, '{"sequence":118,"ID":"5841efc0-cfe6-4e69-9dbc-9f627693e59a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.122747Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content", "updated_at", "parent_id", "content_type", "id", "properties") VALUES (1773939024048, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'WebView
https://github.com/longbridge/wef', 1773939024087, 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'text', 'block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c', '{"ID":"482c5cbb-dd4f-4225-9329-ca9ca0beea4c","sequence":119}');

-- [transaction_stmt] 2026-03-19T16:50:24.123295Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content_type", "id", "content", "updated_at", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'Phase 4: AI Foundation [/]
Goal: Infrastructure for AI features', 1773939024087, 1773939024049, '{"sequence":120,"ID":"7b960cd0-3478-412b-b96f-15822117ac14"}');

-- [transaction_stmt] 2026-03-19T16:50:24.123498Z
INSERT OR REPLACE INTO block ("content", "parent_id", "created_at", "document_id", "content_type", "updated_at", "id", "properties") VALUES ('Agent Embedding', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 1773939024049, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024087, 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', '{"ID":"553f3545-4ec7-44e5-bccf-3d6443f22ecc","sequence":121}');

-- [transaction_stmt] 2026-03-19T16:50:24.123692Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "id", "content", "document_id", "updated_at", "parent_id", "properties") VALUES (1773939024049, 'text', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'Via Terminal', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', '{"sequence":122,"ID":"d4c1533f-3a67-4314-b430-0e24bd62ce34"}');

-- [transaction_stmt] 2026-03-19T16:50:24.124579Z
INSERT OR REPLACE INTO block ("content", "document_id", "content_type", "id", "updated_at", "parent_id", "created_at", "properties") VALUES ('Okena
A fast, native terminal multiplexer built in Rust with GPUI
https://github.com/contember/okena', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e', 1773939024087, 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 1773939024049, '{"sequence":123,"ID":"6e2fd9a2-6f39-48d2-b323-935fc18a3f5e"}');

-- [transaction_stmt] 2026-03-19T16:50:24.125124Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "id", "content", "parent_id", "document_id", "created_at", "properties") VALUES (1773939024087, 'text', 'block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e', 'PMux
https://github.com/zhoujinliang/pmux', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024049, '{"ID":"c4b1ce62-0ad1-4c33-90fe-d7463f40800e","sequence":124}');

-- [transaction_stmt] 2026-03-19T16:50:24.125327Z
INSERT OR REPLACE INTO block ("created_at", "id", "parent_id", "updated_at", "content_type", "document_id", "content", "properties") VALUES (1773939024049, 'block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 1773939024087, 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Slick
https://github.com/tristanpoland/Slick', '{"sequence":125,"ID":"e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e"}');

-- [transaction_stmt] 2026-03-19T16:50:24.125880Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "parent_id", "content", "content_type", "id", "created_at", "properties") VALUES (1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'https://github.com/zortax/gpui-terminal', 'text', 'block:d50a9a7a-0155-4778-ac99-5f83555a1952', 1773939024049, '{"sequence":126,"ID":"d50a9a7a-0155-4778-ac99-5f83555a1952"}');

-- [transaction_stmt] 2026-03-19T16:50:24.126479Z
INSERT OR REPLACE INTO block ("content", "updated_at", "document_id", "parent_id", "content_type", "id", "created_at", "properties") VALUES ('https://github.com/Xuanwo/gpui-ghostty', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'text', 'block:cf102b47-01db-427b-97b6-3c066d9dba24', 1773939024049, '{"sequence":127,"ID":"cf102b47-01db-427b-97b6-3c066d9dba24"}');

-- [transaction_stmt] 2026-03-19T16:50:24.127012Z
INSERT OR REPLACE INTO block ("document_id", "content", "created_at", "updated_at", "id", "parent_id", "content_type", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Via Chat', 1773939024049, 1773939024087, 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'text', '{"sequence":128,"ID":"1236a3b4-6e03-421a-a94b-fce9d7dc123c"}');

-- [transaction_stmt] 2026-03-19T16:50:24.127563Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content_type", "created_at", "id", "content", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'text', 1773939024049, 'block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847', 'coop
https://github.com/lumehq/coop?tab=readme-ov-file', 1773939024087, '{"ID":"f47a6df7-abfc-47b8-bdfe-f19eaf35b847","sequence":129}');

-- [transaction_stmt] 2026-03-19T16:50:24.128155Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content", "id", "created_at", "content_type", "updated_at", "properties") VALUES ('block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Embeddings & Search [/]', 'block:671593d9-a9c6-4716-860b-8410c8616539', 1773939024049, 'text', 1773939024087, '{"ID":"671593d9-a9c6-4716-860b-8410c8616539","sequence":130}');

-- [transaction_stmt] 2026-03-19T16:50:24.128344Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "id", "document_id", "updated_at", "content", "parent_id", "properties") VALUES ('text', 1773939024050, 'block:d58b8367-14eb-4895-9e56-ffa7ff716d59', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'Local vector embeddings (sentence-transformers)', 'block:671593d9-a9c6-4716-860b-8410c8616539', '{"sequence":131,"ID":"d58b8367-14eb-4895-9e56-ffa7ff716d59"}');

-- [transaction_stmt] 2026-03-19T16:50:24.128525Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content", "id", "content_type", "updated_at", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'Semantic search using local embeddings', 'block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc', 'text', 1773939024087, 1773939024050, '{"sequence":132,"ID":"5f3e7d1e-af67-4699-a591-fd9291bf0cdc"}');

-- [transaction_stmt] 2026-03-19T16:50:24.128711Z
INSERT OR REPLACE INTO block ("content", "document_id", "parent_id", "id", "content_type", "updated_at", "created_at", "properties") VALUES ('Entity linking (manual first, then automatic)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'block:96f4647c-8b74-4b08-8952-4f87820aed86', 'text', 1773939024087, 1773939024050, '{"sequence":133,"ID":"96f4647c-8b74-4b08-8952-4f87820aed86"}');

-- [transaction_stmt] 2026-03-19T16:50:24.128902Z
INSERT OR REPLACE INTO block ("id", "parent_id", "document_id", "updated_at", "content_type", "created_at", "content", "properties") VALUES ('block:0da39f39-6635-4f9b-a468-34310147bea9', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'text', 1773939024050, 'Tantivy full-text search integration', '{"sequence":134,"ID":"0da39f39-6635-4f9b-a468-34310147bea9"}');

-- [transaction_stmt] 2026-03-19T16:50:24.129098Z
INSERT OR REPLACE INTO block ("created_at", "content", "content_type", "updated_at", "id", "parent_id", "document_id", "properties") VALUES (1773939024050, 'Self Digital Twin [/]', 'text', 1773939024087, 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":135,"ID":"439af07e-3237-420c-8bc0-c71aeb37c61a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.129309Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "content", "id", "document_id", "updated_at", "created_at", "properties") VALUES ('block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'text', 'Energy/focus/flow_depth dynamics', 'block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 1773939024050, '{"sequence":136,"ID":"5f3e8ef3-df52-4fb9-80c1-ccb81be40412"}');

-- [transaction_stmt] 2026-03-19T16:50:24.129486Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "updated_at", "content_type", "document_id", "content", "properties") VALUES ('block:30406a65-8e66-4589-b070-3a1b4db6e4e0', 1773939024050, 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 1773939024087, 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Peripheral awareness modeling', '{"sequence":137,"ID":"30406a65-8e66-4589-b070-3a1b4db6e4e0"}');

-- [transaction_stmt] 2026-03-19T16:50:24.129669Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "parent_id", "id", "content", "updated_at", "document_id", "properties") VALUES (1773939024050, 'text', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'block:bed11feb-a634-4f8d-b930-f0021ec0512b', 'Observable signals (window switches, typing cadence)', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"bed11feb-a634-4f8d-b930-f0021ec0512b","sequence":138}');

-- [transaction_stmt] 2026-03-19T16:50:24.129860Z
INSERT OR REPLACE INTO block ("content", "id", "content_type", "created_at", "updated_at", "document_id", "parent_id", "properties") VALUES ('Mental slots tracking (materialized view of open transitions)', 'block:11c9c8bb-b72e-4752-8b6c-846e45920418', 'text', 1773939024050, 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', '{"ID":"11c9c8bb-b72e-4752-8b6c-846e45920418","sequence":139}');

-- [transaction_stmt] 2026-03-19T16:50:24.130057Z
INSERT OR REPLACE INTO block ("id", "content", "document_id", "parent_id", "created_at", "content_type", "updated_at", "properties") VALUES ('block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'Logging & Training Data [/]', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 1773939024050, 'text', 1773939024087, '{"sequence":140,"ID":"b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5"}');

-- [transaction_stmt] 2026-03-19T16:50:24.130243Z
INSERT OR REPLACE INTO block ("id", "document_id", "updated_at", "created_at", "parent_id", "content", "content_type", "properties") VALUES ('block:a186c88f-6ca5-49e2-8a0d-19632cb689fc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 1773939024050, 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'Conflict logging system (capture every conflict + resolution)', 'text', '{"sequence":141,"ID":"a186c88f-6ca5-49e2-8a0d-19632cb689fc"}');

-- [transaction_stmt] 2026-03-19T16:50:24.130427Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "document_id", "content_type", "created_at", "content", "properties") VALUES (1773939024087, 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'block:f342692d-5414-4c48-89fe-ed8f9ccf2172', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024051, 'Pattern logging for Guide to learn from', '{"ID":"f342692d-5414-4c48-89fe-ed8f9ccf2172","sequence":142}');

-- [transaction_stmt] 2026-03-19T16:50:24.130627Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "created_at", "updated_at", "content", "id", "parent_id", "properties") VALUES ('text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024051, 1773939024087, 'Behavioral logging for search ranking', 'block:30f04064-a58e-416d-b0d2-7533637effe8', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', '{"sequence":143,"ID":"30f04064-a58e-416d-b0d2-7533637effe8"}');

-- [transaction_stmt] 2026-03-19T16:50:24.130814Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "id", "updated_at", "created_at", "document_id", "content", "properties") VALUES ('text', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 1773939024087, 1773939024051, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Objective Function Engine [/]', '{"ID":"84151cf1-696a-420f-b73c-4947b0a4437e","sequence":144}');

-- [transaction_stmt] 2026-03-19T16:50:24.131000Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "updated_at", "id", "parent_id", "content", "document_id", "properties") VALUES ('text', 1773939024051, 1773939024087, 'block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'Evaluate token attributes via PRQL → scalar score', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":145,"ID":"fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.131193Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "created_at", "id", "document_id", "parent_id", "content", "properties") VALUES (1773939024087, 'text', 1773939024051, 'block:480f2628-c49f-4940-9e26-572ea23f25a3', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'Store weights as prototype block properties', '{"sequence":146,"ID":"480f2628-c49f-4940-9e26-572ea23f25a3"}');

-- [transaction_stmt] 2026-03-19T16:50:24.131384Z
INSERT OR REPLACE INTO block ("id", "parent_id", "created_at", "updated_at", "content", "document_id", "content_type", "properties") VALUES ('block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 1773939024051, 1773939024087, 'Support multiple goal types (achievement, maintenance, process)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"sequence":147,"ID":"e4e93198-6617-4c7c-b8f7-4b2d8188a77e"}');

-- [transaction_stmt] 2026-03-19T16:50:24.131572Z
INSERT OR REPLACE INTO block ("content", "created_at", "parent_id", "id", "updated_at", "document_id", "content_type", "properties") VALUES ('Phase 5: AI Features [/]
Goal: Three AI services operational', 1773939024051, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"ID":"8b962d6c-0246-4119-8826-d517e2357f21","sequence":148}');

-- [transaction_stmt] 2026-03-19T16:50:24.131762Z
INSERT OR REPLACE INTO block ("updated_at", "content", "content_type", "id", "created_at", "parent_id", "document_id", "properties") VALUES (1773939024087, 'The Guide (Growth) [/]', 'text', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 1773939024051, 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","sequence":149}');

-- [transaction_stmt] 2026-03-19T16:50:24.131952Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "id", "content", "updated_at", "document_id", "content_type", "properties") VALUES (1773939024051, 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'block:37c082de-d10a-4f11-82ad-5fb3316bb3e4', 'Velocity and capacity analysis', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"ID":"37c082de-d10a-4f11-82ad-5fb3316bb3e4","sequence":150}');

-- [transaction_stmt] 2026-03-19T16:50:24.132156Z
INSERT OR REPLACE INTO block ("document_id", "id", "parent_id", "updated_at", "content_type", "content", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:52bedd69-85ec-448d-81b6-0099bd413149', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 1773939024087, 'text', 'Stuck task identification (postponement tracking)', 1773939024051, '{"sequence":151,"ID":"52bedd69-85ec-448d-81b6-0099bd413149"}');

-- [transaction_stmt] 2026-03-19T16:50:24.132345Z
INSERT OR REPLACE INTO block ("id", "document_id", "updated_at", "created_at", "content_type", "parent_id", "content", "properties") VALUES ('block:2b5ec929-a22d-4d7f-8640-66495331a40d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 1773939024051, 'text', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'Shadow Work prompts for avoided tasks', '{"sequence":152,"ID":"2b5ec929-a22d-4d7f-8640-66495331a40d"}');

-- [transaction_stmt] 2026-03-19T16:50:24.132547Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "created_at", "document_id", "id", "updated_at", "content", "properties") VALUES ('block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'text', 1773939024052, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:dd9075a4-5c64-4d6b-9661-7937897337d3', 1773939024087, 'Growth tracking and visualization', '{"sequence":153,"ID":"dd9075a4-5c64-4d6b-9661-7937897337d3"}');

-- [transaction_stmt] 2026-03-19T16:50:24.132737Z
INSERT OR REPLACE INTO block ("content", "content_type", "parent_id", "id", "created_at", "document_id", "updated_at", "properties") VALUES ('Pattern recognition across time', 'text', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'block:15a61916-b0c1-4d24-9046-4e066a312401', 1773939024052, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, '{"sequence":154,"ID":"15a61916-b0c1-4d24-9046-4e066a312401"}');

-- [transaction_stmt] 2026-03-19T16:50:24.132931Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content_type", "id", "updated_at", "parent_id", "content", "properties") VALUES (1773939024052, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 1773939024087, 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'Intelligent Conflict Reconciliation [/]', '{"ID":"8ae21b36-6f48-41f1-80d9-bb7ce43b4545","sequence":155}');

-- [transaction_stmt] 2026-03-19T16:50:24.133128Z
INSERT OR REPLACE INTO block ("content_type", "content", "updated_at", "created_at", "parent_id", "id", "document_id", "properties") VALUES ('text', 'LLM-based resolution for low-confidence cases', 1773939024087, 1773939024052, 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'block:0db1be3e-ae11-4341-8aa8-b1d80e22963a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":156,"ID":"0db1be3e-ae11-4341-8aa8-b1d80e22963a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.133321Z
INSERT OR REPLACE INTO block ("content_type", "parent_id", "id", "document_id", "content", "updated_at", "created_at", "properties") VALUES ('text', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'block:314e7db7-fb5e-40b6-ac10-a589ff3c809d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Rule-based conflict resolver', 1773939024087, 1773939024052, '{"sequence":157,"ID":"314e7db7-fb5e-40b6-ac10-a589ff3c809d"}');

-- [transaction_stmt] 2026-03-19T16:50:24.133512Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "created_at", "updated_at", "id", "content_type", "content", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 1773939024052, 1773939024087, 'block:655e2f77-d02e-4347-aa5f-dcd03ac140eb', 'text', 'Train classifier on logged conflicts', '{"sequence":158,"ID":"655e2f77-d02e-4347-aa5f-dcd03ac140eb"}');

-- [transaction_stmt] 2026-03-19T16:50:24.133706Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "updated_at", "created_at", "parent_id", "id", "content", "properties") VALUES ('text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 1773939024052, 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'block:3bbdc016-4f08-49e4-b550-ba3d09a03933', 'Conflict resolution UI with reasoning display', '{"sequence":159,"ID":"3bbdc016-4f08-49e4-b550-ba3d09a03933"}');

-- [transaction_stmt] 2026-03-19T16:50:24.133899Z
INSERT OR REPLACE INTO block ("document_id", "content", "parent_id", "updated_at", "content_type", "created_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'AI Trust Ladder [/]', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 1773939024087, 'text', 1773939024052, 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', '{"ID":"be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","sequence":160}');

-- [transaction_stmt] 2026-03-19T16:50:24.134088Z
INSERT OR REPLACE INTO block ("parent_id", "content", "updated_at", "id", "document_id", "content_type", "created_at", "properties") VALUES ('block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'Level 3 (Agentic) with permission prompts', 1773939024087, 'block:8a72f072-cc14-4e5f-987c-72bd27d94ced', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024052, '{"sequence":161,"ID":"8a72f072-cc14-4e5f-987c-72bd27d94ced"}');

-- [transaction_stmt] 2026-03-19T16:50:24.134282Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "updated_at", "document_id", "parent_id", "content", "id", "properties") VALUES ('text', 1773939024052, 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'Level 4 (Autonomous) for power users', 'block:c2289c19-1733-476e-9b50-43da1d70221f', '{"ID":"c2289c19-1733-476e-9b50-43da1d70221f","sequence":162}');

-- [transaction_stmt] 2026-03-19T16:50:24.134478Z
INSERT OR REPLACE INTO block ("id", "content", "parent_id", "document_id", "updated_at", "created_at", "content_type", "properties") VALUES ('block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0', 'Level 2 (Advisory) features', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 1773939024052, 'text', '{"ID":"c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","sequence":163}');

-- [transaction_stmt] 2026-03-19T16:50:24.134690Z
INSERT OR REPLACE INTO block ("document_id", "content", "id", "parent_id", "content_type", "created_at", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Per-feature trust tracking', 'block:84706843-7132-4c12-a2ae-32fb7109982c', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'text', 1773939024053, 1773939024087, '{"sequence":164,"ID":"84706843-7132-4c12-a2ae-32fb7109982c"}');

-- [transaction_stmt] 2026-03-19T16:50:24.134877Z
INSERT OR REPLACE INTO block ("content", "id", "content_type", "document_id", "updated_at", "created_at", "parent_id", "properties") VALUES ('Trust level visualization UI', 'block:66b47313-a556-4628-954e-1da7fb1d402d', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 1773939024053, 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', '{"sequence":165,"ID":"66b47313-a556-4628-954e-1da7fb1d402d"}');

-- [transaction_stmt] 2026-03-19T16:50:24.135078Z
INSERT OR REPLACE INTO block ("document_id", "content", "parent_id", "content_type", "id", "created_at", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Background Enrichment Agents [/]', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'text', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 1773939024053, 1773939024087, '{"sequence":166,"ID":"d1e6541b-0c6b-4065-aea5-ad9057dc5bb5"}');

-- [transaction_stmt] 2026-03-19T16:50:24.135263Z
INSERT OR REPLACE INTO block ("content", "updated_at", "document_id", "created_at", "parent_id", "content_type", "id", "properties") VALUES ('Infer likely token types from context', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024053, 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'text', 'block:2618de83-3d90-4dc6-b586-98f95e351fb5', '{"sequence":167,"ID":"2618de83-3d90-4dc6-b586-98f95e351fb5"}');

-- [transaction_stmt] 2026-03-19T16:50:24.135955Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "id", "content", "created_at", "content_type", "updated_at", "properties") VALUES ('block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec', 'Suggest dependencies between siblings', 1773939024053, 'text', 1773939024087, '{"ID":"edd212e6-16a9-4dfd-95f9-e2a2a3a55eec","sequence":168}');

-- [transaction_stmt] 2026-03-19T16:50:24.136155Z
INSERT OR REPLACE INTO block ("updated_at", "content", "created_at", "document_id", "parent_id", "id", "content_type", "properties") VALUES (1773939024087, 'Suggest [[links]] for plain-text nouns (local LLM)', 1773939024053, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda', 'text', '{"ID":"44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda","sequence":169}');

-- [transaction_stmt] 2026-03-19T16:50:24.136778Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "parent_id", "content_type", "id", "created_at", "content", "properties") VALUES (1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'text', 'block:2ff960fa-38a4-42dd-8eb0-77e15c89659e', 1773939024053, 'Classify tasks as question/delegation/action', '{"ID":"2ff960fa-38a4-42dd-8eb0-77e15c89659e","sequence":170}');

-- [transaction_stmt] 2026-03-19T16:50:24.136977Z
INSERT OR REPLACE INTO block ("updated_at", "content", "created_at", "id", "content_type", "parent_id", "document_id", "properties") VALUES (1773939024087, 'Suggest via: routes for questions', 1773939024053, 'block:864527d2-65d4-4716-a65e-73a868c7e63b', 'text', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"864527d2-65d4-4716-a65e-73a868c7e63b","sequence":171}');

-- [transaction_stmt] 2026-03-19T16:50:24.137168Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "content", "parent_id", "content_type", "id", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024053, 'The Integrator (Wholeness) [/]', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'text', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 1773939024087, '{"ID":"8a4a658e-d773-4528-8c61-ff3e5e425f47","sequence":172}');

-- [transaction_stmt] 2026-03-19T16:50:24.137846Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "created_at", "content", "content_type", "document_id", "properties") VALUES (1773939024087, 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a', 1773939024053, 'Smart linking suggestions', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","sequence":173}');

-- [transaction_stmt] 2026-03-19T16:50:24.138030Z
INSERT OR REPLACE INTO block ("content", "created_at", "parent_id", "document_id", "updated_at", "content_type", "id", "properties") VALUES ('Context Bundle assembly for Flow mode', 1773939024053, 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'text', 'block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6', '{"ID":"4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6","sequence":174}');

-- [transaction_stmt] 2026-03-19T16:50:24.138229Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "content_type", "id", "content", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'text', 'block:7efa2454-274c-4304-8641-e3b8171c5b5a', 'Cross-system deduplication', 1773939024054, '{"sequence":175,"ID":"7efa2454-274c-4304-8641-e3b8171c5b5a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.138443Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "created_at", "content", "updated_at", "parent_id", "id", "properties") VALUES ('text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024054, 'Related item discovery', 1773939024087, 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'block:311aa51c-88af-446f-8cb6-b791b9740665', '{"sequence":176,"ID":"311aa51c-88af-446f-8cb6-b791b9740665"}');

-- [transaction_stmt] 2026-03-19T16:50:24.138632Z
INSERT OR REPLACE INTO block ("parent_id", "content_type", "id", "content", "document_id", "created_at", "updated_at", "properties") VALUES ('block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'text', 'block:9b6b2563-21b8-4286-9fac-dbdddc1a79be', 'Automatic entity linking via embeddings', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024054, 1773939024087, '{"sequence":177,"ID":"9b6b2563-21b8-4286-9fac-dbdddc1a79be"}');

-- [transaction_stmt] 2026-03-19T16:50:24.138839Z
INSERT OR REPLACE INTO block ("created_at", "id", "parent_id", "content", "content_type", "updated_at", "document_id", "properties") VALUES (1773939024054, 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'The Watcher (Awareness) [/]', 'text', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"d385afbe-5bc9-4341-b879-6d14b8d763bc","sequence":178}');

-- [transaction_stmt] 2026-03-19T16:50:24.139038Z
INSERT OR REPLACE INTO block ("document_id", "created_at", "content", "id", "parent_id", "content_type", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024054, 'Risk and deadline tracking', 'block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'text', 1773939024087, '{"sequence":179,"ID":"244abb7d-ef0f-4768-9e4e-b4bd7f3eec23"}');

-- [transaction_stmt] 2026-03-19T16:50:24.139237Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "id", "updated_at", "content", "document_id", "parent_id", "properties") VALUES (1773939024054, 'text', 'block:f9a2e27c-218f-402a-b405-b6b14b498bcf', 1773939024087, 'Capacity analysis across all systems', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', '{"ID":"f9a2e27c-218f-402a-b405-b6b14b498bcf","sequence":180}');

-- [transaction_stmt] 2026-03-19T16:50:24.139445Z
INSERT OR REPLACE INTO block ("updated_at", "content", "document_id", "parent_id", "id", "content_type", "created_at", "properties") VALUES (1773939024087, 'Cross-system monitoring and alerts', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4', 'text', 1773939024054, '{"sequence":181,"ID":"92d9dee2-3c16-4d14-9d54-1a93313ee1f4"}');

-- [transaction_stmt] 2026-03-19T16:50:24.139641Z
INSERT OR REPLACE INTO block ("created_at", "parent_id", "updated_at", "id", "document_id", "content_type", "content", "properties") VALUES (1773939024054, 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 1773939024087, 'block:e6c28ce7-c659-49e7-874b-334f05852cc4', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Daily/weekly synthesis for Orient mode', '{"sequence":182,"ID":"e6c28ce7-c659-49e7-874b-334f05852cc4"}');

-- [transaction_stmt] 2026-03-19T16:50:24.139832Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "created_at", "id", "parent_id", "updated_at", "content", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024054, 'block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 1773939024087, 'Dependency chain analysis', '{"ID":"1ffa7eb6-174a-4bed-85d2-9c47d9d55519","sequence":183}');

-- [transaction_stmt] 2026-03-19T16:50:24.140031Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "parent_id", "id", "created_at", "content", "document_id", "properties") VALUES ('text', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 1773939024054, 'Phase 6: Flow Optimization [/]
Goal: Users achieve flow states regularly', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":184,"ID":"c74fcc72-883d-4788-911a-0632f6145e4d"}');

-- [transaction_stmt] 2026-03-19T16:50:24.140229Z
INSERT OR REPLACE INTO block ("updated_at", "id", "created_at", "parent_id", "document_id", "content", "content_type", "properties") VALUES (1773939024087, 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 1773939024054, 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Self DT Work Rhythms [/]', 'text', '{"sequence":185,"ID":"f908d928-db6f-495e-a941-22fcdfdba73a"}');

-- [transaction_stmt] 2026-03-19T16:50:24.140421Z
INSERT OR REPLACE INTO block ("id", "created_at", "content", "content_type", "document_id", "parent_id", "updated_at", "properties") VALUES ('block:0570c0bf-84b4-4734-b6f3-25242a12a154', 1773939024055, 'Emergent break suggestions from energy/focus dynamics', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 1773939024087, '{"sequence":186,"ID":"0570c0bf-84b4-4734-b6f3-25242a12a154"}');

-- [transaction_stmt] 2026-03-19T16:50:24.140615Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "document_id", "content", "id", "content_type", "created_at", "properties") VALUES (1773939024087, 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Flow depth tracking with peripheral awareness alerts', 'block:9d85cad6-1e74-499a-8d8e-899c5553c3d6', 'text', 1773939024055, '{"sequence":187,"ID":"9d85cad6-1e74-499a-8d8e-899c5553c3d6"}');

-- [transaction_stmt] 2026-03-19T16:50:24.140811Z
INSERT OR REPLACE INTO block ("document_id", "content", "id", "content_type", "created_at", "updated_at", "parent_id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Quick task suggestions during breaks (2-minute rule)', 'block:adc7803b-9318-4ca5-877b-83f213445aba', 'text', 1773939024055, 1773939024087, 'block:f908d928-db6f-495e-a941-22fcdfdba73a', '{"sequence":188,"ID":"adc7803b-9318-4ca5-877b-83f213445aba"}');

-- [transaction_stmt] 2026-03-19T16:50:24.141009Z
INSERT OR REPLACE INTO block ("id", "created_at", "document_id", "parent_id", "content", "content_type", "updated_at", "properties") VALUES ('block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 1773939024055, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'Three Modes [/]', 'text', 1773939024087, '{"sequence":189,"ID":"b5771daa-0208-43fe-a890-ef1fcebf5f2f"}');

-- [transaction_stmt] 2026-03-19T16:50:24.141208Z
INSERT OR REPLACE INTO block ("updated_at", "content", "document_id", "id", "parent_id", "created_at", "content_type", "properties") VALUES (1773939024087, 'Orient mode (Watcher Dashboard, daily/weekly review)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:be15792f-21f3-476f-8b5f-e2e6b478b864', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 1773939024055, 'text', '{"sequence":190,"ID":"be15792f-21f3-476f-8b5f-e2e6b478b864"}');

-- [transaction_stmt] 2026-03-19T16:50:24.141406Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "id", "parent_id", "document_id", "created_at", "content", "properties") VALUES (1773939024087, 'text', 'block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024055, 'Flow mode (single task focus, context on demand)', '{"sequence":191,"ID":"c68e8d5a-3f4b-4e8c-a887-2341e9b98bde"}');

-- [transaction_stmt] 2026-03-19T16:50:24.142115Z
INSERT OR REPLACE INTO block ("parent_id", "content", "document_id", "created_at", "updated_at", "content_type", "id", "properties") VALUES ('block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'Capture mode (global hotkey, quick input overlay)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024055, 1773939024087, 'text', 'block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963', '{"sequence":192,"ID":"b1b2db9a-fc0d-4f51-98ae-9c5ab056a963"}');

-- [transaction_stmt] 2026-03-19T16:50:24.142855Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "content", "id", "document_id", "content_type", "created_at", "properties") VALUES (1773939024087, 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'Review Workflows [/]', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024055, '{"sequence":193,"ID":"a3e31c87-d10b-432e-987c-0371e730f753"}');

-- [transaction_stmt] 2026-03-19T16:50:24.143053Z
INSERT OR REPLACE INTO block ("id", "created_at", "updated_at", "parent_id", "document_id", "content_type", "content", "properties") VALUES ('block:4c020c67-1726-46d8-92e3-b9e0dbc90b62', 1773939024055, 1773939024087, 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Daily orientation ("What does today look like?")', '{"sequence":194,"ID":"4c020c67-1726-46d8-92e3-b9e0dbc90b62"}');

-- [transaction_stmt] 2026-03-19T16:50:24.143250Z
INSERT OR REPLACE INTO block ("content_type", "updated_at", "document_id", "id", "content", "parent_id", "created_at", "properties") VALUES ('text', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:0906f769-52eb-47a2-917a-f9b57b7e80d1', 'Inbox zero workflow', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 1773939024055, '{"sequence":195,"ID":"0906f769-52eb-47a2-917a-f9b57b7e80d1"}');

-- [transaction_stmt] 2026-03-19T16:50:24.143966Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "updated_at", "content", "id", "parent_id", "document_id", "properties") VALUES ('text', 1773939024055, 1773939024087, 'Weekly review (comprehensive synthesis)', 'block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"091e7648-5314-4b4d-8e9c-bd7e0b8efc6f","sequence":196}');

-- [transaction_stmt] 2026-03-19T16:50:24.144171Z
INSERT OR REPLACE INTO block ("content", "id", "document_id", "created_at", "content_type", "updated_at", "parent_id", "properties") VALUES ('Context Bundles in Flow [/]', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024056, 'text', 1773939024087, 'block:c74fcc72-883d-4788-911a-0632f6145e4d', '{"ID":"240acff4-cf06-445e-99ee-42040da1bb84","sequence":197}');

-- [transaction_stmt] 2026-03-19T16:50:24.144897Z
INSERT OR REPLACE INTO block ("id", "parent_id", "updated_at", "content_type", "content", "created_at", "document_id", "properties") VALUES ('block:90702048-5baf-4732-96fb-ddae16824257', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 1773939024087, 'text', 'Hide distractions, show progress', 1773939024056, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"90702048-5baf-4732-96fb-ddae16824257","sequence":198}');

-- [transaction_stmt] 2026-03-19T16:50:24.145092Z
INSERT OR REPLACE INTO block ("updated_at", "document_id", "content_type", "id", "parent_id", "content", "created_at", "properties") VALUES (1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'Slide-in context panel from edge', 1773939024056, '{"ID":"e4aeb8f0-4c63-48f6-b745-92a89cfd4130","sequence":199}');

-- [transaction_stmt] 2026-03-19T16:50:24.145290Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "content_type", "content", "created_at", "updated_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'text', 'Assemble all related items for focused task', 1773939024056, 1773939024087, 'block:3907168e-eaf8-48ee-8ccc-6dfef069371e', '{"sequence":200,"ID":"3907168e-eaf8-48ee-8ccc-6dfef069371e"}');

-- [transaction_stmt] 2026-03-19T16:50:24.145479Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "content", "created_at", "id", "parent_id", "updated_at", "properties") VALUES ('text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Progressive Concealment [/]', 1773939024056, 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 1773939024087, '{"sequence":201,"ID":"e233124d-8711-4dd4-8153-c884f889bc07"}');

-- [transaction_stmt] 2026-03-19T16:50:24.145676Z
INSERT OR REPLACE INTO block ("content", "created_at", "id", "content_type", "updated_at", "document_id", "parent_id", "properties") VALUES ('Peripheral element dimming during sustained typing', 1773939024056, 'block:70485255-a2be-4356-bb9e-967270878b7e', 'text', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:e233124d-8711-4dd4-8153-c884f889bc07', '{"ID":"70485255-a2be-4356-bb9e-967270878b7e","sequence":202}');

-- [transaction_stmt] 2026-03-19T16:50:24.146387Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "created_at", "id", "content_type", "content", "updated_at", "properties") VALUES ('block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024056, 'block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc', 'text', 'Focused block emphasis, surrounding content fades', 1773939024087, '{"sequence":203,"ID":"ea7f8d72-f963-4a51-ab4f-d10f981eafcc"}');

-- [transaction_stmt] 2026-03-19T16:50:24.146593Z
INSERT OR REPLACE INTO block ("id", "updated_at", "document_id", "content", "content_type", "parent_id", "created_at", "properties") VALUES ('block:30a71e2f-f070-4745-947d-c443a86a7149', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Automatic visibility restore on cursor movement', 'text', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 1773939024056, '{"ID":"30a71e2f-f070-4745-947d-c443a86a7149","sequence":204}');

-- [transaction_stmt] 2026-03-19T16:50:24.146800Z
INSERT OR REPLACE INTO block ("content", "content_type", "created_at", "parent_id", "id", "updated_at", "document_id", "properties") VALUES ('Phase 7: Team Features [/]
Goal: Teams leverage individual excellence', 'text', 1773939024056, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":205,"ID":"4c647dfe-0639-4064-8ab6-491d57c7e367"}');

-- [transaction_stmt] 2026-03-19T16:50:24.147550Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "document_id", "content_type", "content", "updated_at", "properties") VALUES ('block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 1773939024056, 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Delegation System [/]', 1773939024087, '{"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede","sequence":206}');

-- [transaction_stmt] 2026-03-19T16:50:24.148243Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "document_id", "content", "created_at", "content_type", "properties") VALUES (1773939024087, 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '@[[Person]]: syntax for delegation sub-nets', 1773939024056, 'text', '{"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264","sequence":207}');

-- [transaction_stmt] 2026-03-19T16:50:24.148995Z
INSERT OR REPLACE INTO block ("content", "content_type", "parent_id", "document_id", "id", "created_at", "updated_at", "properties") VALUES ('Waiting-for tracking (automatic from delegation patterns)', 'text', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:fbbce845-023e-438b-963e-471833c51505', 1773939024057, 1773939024087, '{"ID":"fbbce845-023e-438b-963e-471833c51505","sequence":208}');

-- [transaction_stmt] 2026-03-19T16:50:24.149756Z
INSERT OR REPLACE INTO block ("id", "content_type", "updated_at", "parent_id", "document_id", "content", "created_at", "properties") VALUES ('block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0', 'text', 1773939024087, 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Delegation status sync with external systems', 1773939024057, '{"sequence":209,"ID":"25e19c99-63c2-4edb-8fb1-deb1daf4baf0"}');

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.151089Z
INSERT OR REPLACE INTO block ("updated_at", "content", "content_type", "created_at", "id", "parent_id", "document_id", "properties") VALUES (1773939024087, '@anyone: team pool transitions', 'text', 1773939024057, 'block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":210,"ID":"938f03b8-6129-4eda-9c5f-31a76ad8b8dc"}');

-- [transaction_stmt] 2026-03-19T16:50:24.151807Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "parent_id", "content", "content_type", "id", "document_id", "properties") VALUES (1773939024057, 1773939024087, 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'Sharing & Collaboration [/]', 'text', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":211,"ID":"5bdf3ba6-f617-4bc1-93c2-15d84d925e01"}');

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.153119Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "document_id", "content_type", "content", "updated_at", "properties") VALUES ('block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 1773939024057, 'block:88b467b1-5a46-4b64-acb3-fcf9f377030e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Collaborative editing', 1773939024087, '{"sequence":212,"ID":"88b467b1-5a46-4b64-acb3-fcf9f377030e"}');

-- [transaction_stmt] 2026-03-19T16:50:24.153905Z
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "content", "content_type", "id", "document_id", "created_at", "properties") VALUES ('block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 1773939024087, 'Shared views and dashboards', 'text', 'block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024057, '{"ID":"f3ce62cd-5817-4a7c-81f6-7a7077aff7da","sequence":213}');

-- [transaction_stmt] 2026-03-19T16:50:24.154089Z
INSERT OR REPLACE INTO block ("updated_at", "content", "created_at", "parent_id", "document_id", "id", "content_type", "properties") VALUES (1773939024087, 'Read-only sharing for documentation', 1773939024057, 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:135c74b1-8341-4719-b5d1-492eb26e2189', 'text', '{"sequence":214,"ID":"135c74b1-8341-4719-b5d1-492eb26e2189"}');

-- [transaction_stmt] 2026-03-19T16:50:24.154275Z
INSERT OR REPLACE INTO block ("content", "created_at", "parent_id", "document_id", "updated_at", "content_type", "id", "properties") VALUES ('Competition analysis', 1773939024057, 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'text', 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', '{"ID":"e0f90f1e-5468-4229-9b6d-438b31f09ed6","sequence":215}');

-- [transaction_stmt] 2026-03-19T16:50:24.154465Z
INSERT OR REPLACE INTO block ("created_at", "updated_at", "parent_id", "content_type", "document_id", "id", "content", "properties") VALUES (1773939024057, 1773939024087, 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:ceb203d0-0b59-4aa0-a840-2e4763234112', 'https://github.com/3xpyth0n/ideon
Organize repositories, notes, links and more on a shared infinite canvas.', '{"ID":"ceb203d0-0b59-4aa0-a840-2e4763234112","sequence":216}');

-- [transaction_stmt] 2026-03-19T16:50:24.154652Z
INSERT OR REPLACE INTO block ("content_type", "document_id", "content", "created_at", "id", "parent_id", "updated_at", "properties") VALUES ('text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-Cutting Concerns [/]', 1773939024057, 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, '{"ID":"f407a7ec-c924-4a38-96e0-7e73472e7353","sequence":217}');

-- [transaction_stmt] 2026-03-19T16:50:24.154841Z
INSERT OR REPLACE INTO block ("content_type", "id", "updated_at", "document_id", "parent_id", "created_at", "content", "properties") VALUES ('text', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773939024057, 'Privacy & Security [/]', '{"sequence":218,"ID":"ad1d8307-134f-4a34-b58e-07d6195b2466"}');

-- [transaction_stmt] 2026-03-19T16:50:24.155016Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "updated_at", "document_id", "content", "id", "content_type", "properties") VALUES ('block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773939024057, 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Plugin sandboxing (WASM)', 'block:717db234-61eb-41ef-a8bf-b67e870f9aa6', 'text', '{"ID":"717db234-61eb-41ef-a8bf-b67e870f9aa6","sequence":219}');

-- [transaction_stmt] 2026-03-19T16:50:24.155225Z
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "content", "updated_at", "content_type", "document_id", "properties") VALUES ('block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773939024058, 'block:75604518-b736-4653-a2a3-941215e798c7', 'Self-hosted LLM option (Ollama/vLLM)', 1773939024087, 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":220,"ID":"75604518-b736-4653-a2a3-941215e798c7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.155416Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content_type", "document_id", "id", "parent_id", "content", "properties") VALUES (1773939024087, 1773939024058, 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:bfaedc82-3bc7-4b16-8314-273721ea997f', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'Optional cloud LLM with explicit consent', '{"sequence":221,"ID":"bfaedc82-3bc7-4b16-8314-273721ea997f"}');

-- [transaction_stmt] 2026-03-19T16:50:24.155602Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "id", "document_id", "content", "parent_id", "updated_at", "properties") VALUES (1773939024058, 'text', 'block:4b96f182-61e5-4f0e-861d-1a7d2413abe7', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Local-first by default (all data on device)', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 1773939024087, '{"ID":"4b96f182-61e5-4f0e-861d-1a7d2413abe7","sequence":222}');

-- [transaction_stmt] 2026-03-19T16:50:24.155793Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "updated_at", "content_type", "content", "created_at", "id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773939024087, 'text', 'Petri-Net Advanced [/]', 1773939024058, 'block:eac105ca-efda-4976-9856-6c39a9b1502e', '{"ID":"eac105ca-efda-4976-9856-6c39a9b1502e","sequence":223}');

-- [transaction_stmt] 2026-03-19T16:50:24.155988Z
INSERT OR REPLACE INTO block ("content_type", "content", "updated_at", "created_at", "parent_id", "document_id", "id", "properties") VALUES ('text', 'SOP extraction from repeated interaction patterns', 1773939024087, 1773939024058, 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59', '{"sequence":224,"ID":"0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59"}');

-- [transaction_stmt] 2026-03-19T16:50:24.156176Z
INSERT OR REPLACE INTO block ("parent_id", "content", "document_id", "created_at", "updated_at", "content_type", "id", "properties") VALUES ('block:eac105ca-efda-4976-9856-6c39a9b1502e', 'Delegation sub-nets (waiting_for pattern)', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024058, 1773939024087, 'text', 'block:143d071e-2b90-4f93-98d3-7aa5d3a14933', '{"sequence":225,"ID":"143d071e-2b90-4f93-98d3-7aa5d3a14933"}');

-- [transaction_stmt] 2026-03-19T16:50:24.156376Z
INSERT OR REPLACE INTO block ("id", "document_id", "updated_at", "content_type", "content", "parent_id", "created_at", "properties") VALUES ('block:cc499de0-f953-4f41-b795-0864b366d8ab', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'text', 'Token type hierarchy with mixins', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 1773939024058, '{"ID":"cc499de0-f953-4f41-b795-0864b366d8ab","sequence":226}');

-- [transaction_stmt] 2026-03-19T16:50:24.156555Z
INSERT OR REPLACE INTO block ("id", "created_at", "document_id", "content_type", "parent_id", "content", "updated_at", "properties") VALUES ('block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb', 1773939024058, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'Projections as views on flat net (Kanban, SOP, pipeline)', 1773939024087, '{"sequence":227,"ID":"bd99d866-66ed-4474-8a4d-7ac1c1b08fbb"}');

-- [transaction_stmt] 2026-03-19T16:50:24.156748Z
INSERT OR REPLACE INTO block ("content", "content_type", "parent_id", "updated_at", "created_at", "id", "document_id", "properties") VALUES ('Question/Information tokens with confidence tracking', 'text', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 1773939024087, 1773939024058, 'block:4041eb2e-23a6-4fea-9a69-0c152a6311e8', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"4041eb2e-23a6-4fea-9a69-0c152a6311e8","sequence":228}');

-- [transaction_stmt] 2026-03-19T16:50:24.156950Z
INSERT OR REPLACE INTO block ("updated_at", "content_type", "document_id", "created_at", "content", "parent_id", "id", "properties") VALUES (1773939024087, 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024058, 'Simulation engine (fork marking, compare scenarios)', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'block:1e1027d2-4c0f-4975-ba59-c3c601d1f661', '{"sequence":229,"ID":"1e1027d2-4c0f-4975-ba59-c3c601d1f661"}');

-- [transaction_stmt] 2026-03-19T16:50:24.157124Z
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "updated_at", "document_id", "content_type", "content", "properties") VALUES ('block:a80f6d58-c876-48f5-8bfe-69390a8f9bde', 1773939024059, 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Browser plugin for web app Digital Twins', '{"sequence":230,"ID":"a80f6d58-c876-48f5-8bfe-69390a8f9bde"}');

-- [transaction_stmt] 2026-03-19T16:50:24.157317Z
INSERT OR REPLACE INTO block ("content", "updated_at", "parent_id", "created_at", "id", "document_id", "content_type", "properties") VALUES ('PRQL Automation [/]', 1773939024087, 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773939024059, 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"sequence":231,"ID":"723a51a9-3861-429c-bb10-f73c01f8463d"}');

-- [transaction_stmt] 2026-03-19T16:50:24.157498Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "id", "content_type", "document_id", "created_at", "content", "properties") VALUES (1773939024087, 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024059, 'Cross-system status propagation rules', '{"sequence":232,"ID":"e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7"}');

-- [transaction_stmt] 2026-03-19T16:50:24.157682Z
INSERT OR REPLACE INTO block ("id", "updated_at", "document_id", "content", "created_at", "content_type", "parent_id", "properties") VALUES ('block:c1338a15-080b-4dba-bbdc-87b6b8467f28', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Auto-tag blocks based on content analysis', 1773939024059, 'text', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', '{"sequence":233,"ID":"c1338a15-080b-4dba-bbdc-87b6b8467f28"}');

-- [transaction_stmt] 2026-03-19T16:50:24.157877Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "id", "content", "parent_id", "document_id", "updated_at", "properties") VALUES (1773939024059, 'text', 'block:5707965a-6578-443c-aeff-bf40170edea9', 'PRQL-based automation rules (query → action)', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, '{"ID":"5707965a-6578-443c-aeff-bf40170edea9","sequence":234}');

-- [transaction_stmt] 2026-03-19T16:50:24.158066Z
INSERT OR REPLACE INTO block ("id", "updated_at", "content", "document_id", "content_type", "parent_id", "created_at", "properties") VALUES ('block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 1773939024087, 'Platform Support [/]', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773939024059, '{"sequence":235,"ID":"8e2b4ddd-e428-4950-bc41-76ee8a0e27ce"}');

-- [transaction_stmt] 2026-03-19T16:50:24.158262Z
INSERT OR REPLACE INTO block ("document_id", "content", "updated_at", "created_at", "content_type", "id", "parent_id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Android mobile', 1773939024087, 1773939024059, 'text', 'block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', '{"ID":"4c4ff372-c3b9-44e6-9d46-33b7a4e7882e","sequence":236}');

-- [transaction_stmt] 2026-03-19T16:50:24.158452Z
INSERT OR REPLACE INTO block ("content", "parent_id", "document_id", "content_type", "updated_at", "created_at", "id", "properties") VALUES ('WASM compatibility (MaybeSendSync trait)', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024087, 1773939024059, 'block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857', '{"ID":"e5b9db2d-f39a-439d-99f8-b4e7c4ff6857","sequence":237}');

-- [transaction_stmt] 2026-03-19T16:50:24.158639Z
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "updated_at", "content", "content_type", "document_id", "properties") VALUES ('block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db', 1773939024059, 1773939024087, 'Windows desktop', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"sequence":238,"ID":"d61290d4-e1f6-41e7-89e0-a7ed7a6662db"}');

-- [transaction_stmt] 2026-03-19T16:50:24.158832Z
INSERT OR REPLACE INTO block ("document_id", "parent_id", "id", "content", "content_type", "updated_at", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'block:1e729eef-3fff-43cb-8d13-499a8a8d4203', 'iOS mobile', 'text', 1773939024087, 1773939024059, '{"sequence":239,"ID":"1e729eef-3fff-43cb-8d13-499a8a8d4203"}');

-- [transaction_stmt] 2026-03-19T16:50:24.159017Z
INSERT OR REPLACE INTO block ("created_at", "content", "updated_at", "id", "parent_id", "content_type", "document_id", "properties") VALUES (1773939024059, 'Linux desktop', 1773939024087, 'block:500b7aae-5c3b-4dd5-a3c8-373fe746990b', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"500b7aae-5c3b-4dd5-a3c8-373fe746990b","sequence":240}');

-- [transaction_stmt] 2026-03-19T16:50:24.159732Z
INSERT OR REPLACE INTO block ("content", "updated_at", "id", "parent_id", "created_at", "document_id", "content_type", "properties") VALUES ('macOS desktop (Flutter)', 1773939024087, 'block:a79ab251-4685-4728-b98b-0a652774f06c', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 1773939024060, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"ID":"a79ab251-4685-4728-b98b-0a652774f06c","sequence":241}');

-- [transaction_stmt] 2026-03-19T16:50:24.159924Z
INSERT OR REPLACE INTO block ("updated_at", "parent_id", "created_at", "document_id", "content", "id", "content_type", "properties") VALUES (1773939024087, 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 1773939024060, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'UI/UX Design System [/]', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'text', '{"sequence":242,"ID":"ac137431-daf6-4741-9808-6dc71c13e7c6"}');

-- [transaction_stmt] 2026-03-19T16:50:24.160670Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "id", "content", "content_type", "updated_at", "parent_id", "properties") VALUES (1773939024060, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:a85de368-9546-446d-ad61-17b72c7dbc3e', 'Which-Key navigation system (Space → mnemonic keys)', 'text', 1773939024087, 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', '{"sequence":243,"ID":"a85de368-9546-446d-ad61-17b72c7dbc3e"}');

-- [transaction_stmt] 2026-03-19T16:50:24.160875Z
INSERT OR REPLACE INTO block ("document_id", "id", "content_type", "content", "parent_id", "updated_at", "created_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9', 'text', 'Micro-interactions (checkbox animation, smooth reorder)', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 1773939024087, 1773939024060, '{"ID":"1cea6bd3-680f-46c3-bdbc-5989da5ed7d9","sequence":244}');

-- [transaction_stmt] 2026-03-19T16:50:24.161063Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "parent_id", "document_id", "id", "content", "content_type", "properties") VALUES (1773939024087, 1773939024060, 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3', 'Light and dark themes', 'text', '{"ID":"d1fbee2c-3a11-4adc-a3db-fd93f5b117e3","sequence":245}');

-- [transaction_stmt] 2026-03-19T16:50:24.161270Z
INSERT OR REPLACE INTO block ("document_id", "updated_at", "parent_id", "content_type", "id", "created_at", "content", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'text', 'block:beeec959-ba87-4c57-9531-c1d7f24d2b2c', 1773939024060, 'Color palette (warm, professional, calm technology)', '{"ID":"beeec959-ba87-4c57-9531-c1d7f24d2b2c","sequence":246}');

-- [transaction_stmt] 2026-03-19T16:50:24.161454Z
INSERT OR REPLACE INTO block ("created_at", "document_id", "content_type", "content", "updated_at", "id", "parent_id", "properties") VALUES (1773939024060, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 'Typography system (Inter + JetBrains Mono)', 1773939024087, 'block:d36014da-518a-4da5-b360-218d027ee104', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', '{"sequence":247,"ID":"d36014da-518a-4da5-b360-218d027ee104"}');

-- [transaction_stmt] 2026-03-19T16:50:24.161648Z
INSERT OR REPLACE INTO block ("content_type", "created_at", "id", "parent_id", "updated_at", "document_id", "content", "properties") VALUES ('text', 1773939024060, 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LogSeq replacement', '{"ID":"01806047-9cf8-42fe-8391-6d608bfade9e","sequence":248}');

-- [transaction_stmt] 2026-03-19T16:50:24.161834Z
INSERT OR REPLACE INTO block ("content", "created_at", "updated_at", "id", "document_id", "parent_id", "content_type", "properties") VALUES ('Editing experience', 1773939024060, 1773939024087, 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'text', '{"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","sequence":249}');

-- [transaction_stmt] 2026-03-19T16:50:24.162520Z
INSERT OR REPLACE INTO block ("document_id", "id", "content", "content_type", "parent_id", "created_at", "updated_at", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'GitHub Flavored Markdown parser & renderer for GPUI
https://github.com/joris-gallot/gpui-gfm', 'text', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 1773939024060, 1773939024087, '{"sequence":250,"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2"}');

-- [transaction_stmt] 2026-03-19T16:50:24.162721Z
INSERT OR REPLACE INTO block ("updated_at", "id", "created_at", "parent_id", "content_type", "document_id", "content", "properties") VALUES (1773939024087, 'block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', 1773939024060, 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Desktop Markdown viewer built with Rust and GPUI
https://github.com/chunghha/markdown_viewer', '{"sequence":251,"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8"}');

-- [transaction_stmt] 2026-03-19T16:50:24.162916Z
INSERT OR REPLACE INTO block ("parent_id", "document_id", "content", "content_type", "id", "created_at", "updated_at", "properties") VALUES ('block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Markdown Editor and Viewer
https://github.com/kumarUjjawal/aster', 'text', 'block:f7730a68-6268-4e65-ac93-3fdf79e92133', 1773939024061, 1773939024087, '{"sequence":252,"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133"}');

-- [transaction_stmt] 2026-03-19T16:50:24.163108Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "parent_id", "content_type", "id", "content", "document_id", "properties") VALUES (1773939024087, 1773939024061, 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'text', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'PDF Viewer & Annotator', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '{"ID":"8594ab7c-5f36-44cf-8f92-248b31508441","sequence":253}');

-- [transaction_stmt] 2026-03-19T16:50:24.163315Z
INSERT OR REPLACE INTO block ("updated_at", "id", "parent_id", "content_type", "document_id", "created_at", "content", "properties") VALUES (1773939024087, 'block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'text', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024061, 'Combining gpui and hayro for a little application that render pdfs
https://github.com/vincenthz/gpui-hayro?tab=readme-ov-file', '{"sequence":254,"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c"}');

-- [transaction_stmt] 2026-03-19T16:50:24.163527Z
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content", "parent_id", "id", "document_id", "content_type", "properties") VALUES (1773939024087, 1773939024061, 'Libera Reader
Modern, performance-oriented desktop e-book reader built with Rust and GPUI.
https://github.com/RikaKit2/libera-reader', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'block:b95a19a6-5448-42f0-af06-177e95e27f49', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"ID":"b95a19a6-5448-42f0-af06-177e95e27f49","sequence":255}');

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.164697Z
INSERT OR REPLACE INTO block ("document_id", "content_type", "updated_at", "id", "created_at", "content", "parent_id", "properties") VALUES ('doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', 1773939024087, 'block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 1773939024061, 'Monica: On-screen anotation software
https://github.com/tasuren/monica', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', '{"sequence":256,"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad"}');

-- [transaction_stmt] 2026-03-19T16:50:24.164909Z
INSERT OR REPLACE INTO block ("created_at", "content_type", "parent_id", "id", "document_id", "updated_at", "content", "properties") VALUES (1773939024061, 'text', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 1773939024087, 'Graph vis', '{"sequence":257,"ID":"419b2df8-0121-4532-8dcd-21f04df806d8"}');

-- [transaction_stmt] 2026-03-19T16:50:24.165096Z
INSERT OR REPLACE INTO block ("parent_id", "content", "updated_at", "id", "created_at", "document_id", "content_type", "properties") VALUES ('block:419b2df8-0121-4532-8dcd-21f04df806d8', 'https://github.com/jerlendds/gpug', 1773939024087, 'block:f520a9ff-71bf-4a72-8777-9864bad7c535', 1773939024061, 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'text', '{"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535","sequence":258}');

-- Wait 207ms
-- [actor_query] 2026-03-19T16:50:24.372138Z
SELECT id FROM block WHERE id = 'block:root-layout';

-- [actor_query] 2026-03-19T16:50:24.372482Z
SELECT document_id FROM block WHERE id = 'block:root-layout' AND document_id != 'doc:__default__';

-- [actor_exec] 2026-03-19T16:50:24.372679Z
DELETE FROM block WHERE document_id = 'doc:__default__';

-- [actor_exec] 2026-03-19T16:50:24.372848Z
DELETE FROM document WHERE id = 'doc:__default__';

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.373989Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6QKV89KHF2CSJKBPJW', 'block.created', 'block', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"content_type":"text","created_at":1773939024037,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Phase 1: Core Outliner","properties":{"ID":"599b60af-960d-4c9c-b222-d3d9de95c513","sequence":0}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.374417Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6QHNH02S6HRC8T2S8S', 'block.created', 'block', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'sql', 'confirmed', '{"data":{"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content":"MCP Server Frontend [/]","content_type":"text","id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","created_at":1773939024037,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"properties":{"ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb","task_state":"DOING","sequence":1}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.374765Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6QWCK42CJJH08V9EGG', 'block.created', 'block', 'block:db59d038-8a47-43e9-9502-0472b493a6b9', 'sql', 'confirmed', '{"data":{"parent_id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:db59d038-8a47-43e9-9502-0472b493a6b9","content":"Context parameter support ($context_id, $context_parent_id)","created_at":1773939024038,"content_type":"text","updated_at":1773939024086,"properties":{"ID":"db59d038-8a47-43e9-9502-0472b493a6b9","sequence":2}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.375114Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6QXMDBS549NXKJAKSW', 'block.created', 'block', 'block:95ad6166-c03c-4417-a435-349e88b8e90a', 'sql', 'confirmed', '{"data":{"parent_id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:95ad6166-c03c-4417-a435-349e88b8e90a","updated_at":1773939024086,"content_type":"text","created_at":1773939024038,"content":"MCP server (stdio + HTTP modes)","properties":{"sequence":3,"ID":"95ad6166-c03c-4417-a435-349e88b8e90a"}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.375430Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6Q0RJFHWP2E53AXQ6C', 'block.created', 'block', 'block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773939024086,"content":"MCP tools for query execution and operations","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024038,"id":"block:d365c9ef-c9aa-49ee-bd19-960c0e12669b","parent_id":"block:035cac65-27b7-4e1c-8a09-9af9d128dceb","properties":{"sequence":4,"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b"}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.375744Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6QM3PHTVMWQMPM46Q0', 'block.created', 'block', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","updated_at":1773939024086,"content":"Block Operations [/]","content_type":"text","id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","created_at":1773939024038,"properties":{"sequence":5,"ID":"661368d9-e4bd-4722-b5c2-40f32006c643"}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.376643Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6Q74GSVNTD1CJSB6CM', 'block.created', 'block', 'block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'sql', 'confirmed', '{"data":{"created_at":1773939024038,"id":"block:346e7a61-62a5-4813-8fd1-5deea67d9007","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content":"Block hierarchy (parent/child, indent/outdent)","content_type":"text","parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","properties":{"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007","sequence":6}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.376950Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6QMME8MS4Q619S2F9F', 'block.created', 'block', 'block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024038,"parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content_type":"text","id":"block:4fb5e908-31a0-47fb-8280-fe01cebada34","content":"Split block operation","updated_at":1773939024086,"properties":{"sequence":7,"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34"}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.377270Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6Q5D1BHZJY8NATBP98', 'block.created', 'block', 'block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Block CRUD (create, read, update, delete)","created_at":1773939024038,"updated_at":1773939024086,"id":"block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb","parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content_type":"text","properties":{"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb","sequence":8}}}', NULL, NULL, 1773939024087, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.377576Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RXGQHPA4Q6601WG41', 'block.created', 'block', 'block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 'sql', 'confirmed', '{"data":{"created_at":1773939024038,"id":"block:c3ad7889-3d40-4d07-88fb-adf569e50a63","updated_at":1773939024086,"content_type":"text","parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","content":"Block movement (move_up, move_down, move_block)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63","sequence":9}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.378479Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R0CNTRWSKW1ZJWVQH', 'block.created', 'block', 'block:225edb45-f670-445a-9162-18c150210ee6', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Undo/redo system (UndoStack + persistent OperationLogStore)","created_at":1773939024038,"parent_id":"block:661368d9-e4bd-4722-b5c2-40f32006c643","id":"block:225edb45-f670-445a-9162-18c150210ee6","updated_at":1773939024086,"content_type":"text","properties":{"task_state":"TODO","ID":"225edb45-f670-445a-9162-18c150210ee6","sequence":10}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.379340Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RM1GTT2X0CK475WG1', 'block.created', 'block', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'sql', 'confirmed', '{"data":{"content":"Storage & Data Layer [/]","id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","updated_at":1773939024086,"created_at":1773939024038,"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":11,"ID":"444b24f6-d412-43c4-a14b-6e725b673cee"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.379648Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RT4D3FPSQF01QAG5Z', 'block.created', 'block', 'block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 'sql', 'confirmed', '{"data":{"parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024039,"content":"Schema Module system with topological dependency ordering","id":"block:c5007917-6723-49e2-95d4-c8bd3c7659ae","properties":{"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae","sequence":12}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.379956Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RPC3V64HJSXVCQ8MT', 'block.created', 'block', 'block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024039,"id":"block:ecafcad8-15e9-4883-9f4a-79b9631b2699","content":"Fractional indexing for block ordering","updated_at":1773939024086,"content_type":"text","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","properties":{"sequence":13,"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.380853Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RQQ8EGYA2JF0S5X7Z', 'block.created', 'block', 'block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:1e0cf8f7-28e1-4748-a682-ce07be956b57","content":"Turso (embedded SQLite) backend with connection pooling","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024039,"parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","properties":{"sequence":14,"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.381159Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6REJEYRDQ59HH3DYAP', 'block.created', 'block', 'block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:eff0db85-3eb2-4c9b-ac02-3c2773193280","content":"QueryableCache wrapping DataSource with local caching","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","content_type":"text","created_at":1773939024039,"updated_at":1773939024086,"properties":{"sequence":15,"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.381482Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RZ8GNW7DVJ899PXXK', 'block.created', 'block', 'block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'sql', 'confirmed', '{"data":{"parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","created_at":1773939024039,"updated_at":1773939024086,"content":"Entity derive macro (#[derive(Entity)]) for schema generation","content_type":"text","id":"block:d4ae0e9f-d370-49e7-b777-bd8274305ad7","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.382323Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RD206T120VZG4JNDY', 'block.created', 'block', 'block:d318cae4-759d-487b-a909-81940223ecc1', 'sql', 'confirmed', '{"data":{"parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","content":"CDC (Change Data Capture) streaming from storage to UI","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content_type":"text","created_at":1773939024039,"id":"block:d318cae4-759d-487b-a909-81940223ecc1","properties":{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.382634Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RMZD489ND927T02G1', 'block.created', 'block', 'block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:d587e8d0-8e96-4b98-8a8f-f18f47e45222","parent_id":"block:444b24f6-d412-43c4-a14b-6e725b673cee","created_at":1773939024039,"updated_at":1773939024086,"content":"Command sourcing infrastructure (append-only operation log)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":18,"ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222","task_state":"DONE"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.383493Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RFYG9KZ8QZZ1M839J', 'block.created', 'block', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"created_at":1773939024039,"id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content":"Procedural Macros [/]","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":19,"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.384362Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R323DEQT693XVRGBQ', 'block.created', 'block', 'block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024086,"content":"#[operations_trait] macro for operation dispatch generation","parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","created_at":1773939024039,"id":"block:b90a254f-145b-4e0d-96ca-ad6139f13ce4","properties":{"sequence":20,"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.385282Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RCQB97CGGXYDAYBT9', 'block.created', 'block', 'block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', 'sql', 'confirmed', '{"data":{"parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"#[triggered_by(...)] for operation availability","content_type":"text","created_at":1773939024039,"id":"block:5657317c-dedf-4ae5-9db0-83bd3c92fc44","properties":{"sequence":21,"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.386207Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R5RDTA2GZWQ6J8Q2J', 'block.created', 'block', 'block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 'sql', 'confirmed', '{"data":{"created_at":1773939024039,"content_type":"text","id":"block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd","updated_at":1773939024086,"parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","content":"Type inference for OperationDescriptor parameters","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":22,"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.387173Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RWRP9R9ETX5G14D5B', 'block.created', 'block', 'block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', 'sql', 'confirmed', '{"data":{"parent_id":"block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024040,"id":"block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2","content":"#[affects(...)] for field-level reactivity","properties":{"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2","sequence":23}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.387487Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R7R1WHZCBN7A2CZVT', 'block.created', 'block', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'sql', 'confirmed', '{"data":{"id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Performance [/]","updated_at":1773939024086,"created_at":1773939024040,"properties":{"sequence":24,"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.387794Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RGE10BAPYVW4EEDPM', 'block.created', 'block', 'block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'sql', 'confirmed', '{"data":{"id":"block:6463c700-3e8b-42a7-ae49-ce13520f8c73","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024086,"created_at":1773939024040,"parent_id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","content":"Virtual scrolling and lazy loading","properties":{"task_state":"DOING","sequence":25,"ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.388112Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RB977XDE8AWFGXYY2', 'block.created', 'block', 'block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Connection pooling for Turso","content_type":"text","parent_id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","created_at":1773939024040,"id":"block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","updated_at":1773939024086,"properties":{"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","task_state":"DOING","sequence":26}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.388424Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RQCDSFZP0BNHZ7K0C', 'block.created', 'block', 'block:e0567a06-5a62-4957-9457-c55a6661cee5', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:e0567a06-5a62-4957-9457-c55a6661cee5","created_at":1773939024040,"parent_id":"block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content":"Full-text search indexing (Tantivy)","properties":{"sequence":27,"ID":"e0567a06-5a62-4957-9457-c55a6661cee5"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.388736Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RD7RT983135YPVEKT', 'block.created', 'block', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'sql', 'confirmed', '{"data":{"id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content_type":"text","content":"Cross-Device Sync [/]","created_at":1773939024040,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"properties":{"sequence":28,"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.389048Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RQ3X5JAB5XZCJG5PX', 'block.created', 'block', 'block:43f329da-cfb4-4764-b599-06f4b6272f91', 'sql', 'confirmed', '{"data":{"parent_id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:43f329da-cfb4-4764-b599-06f4b6272f91","content_type":"text","created_at":1773939024040,"updated_at":1773939024086,"content":"CollaborativeDoc with ALPN routing","properties":{"sequence":29,"ID":"43f329da-cfb4-4764-b599-06f4b6272f91"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.389359Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R5V1631K1CPAZMFQK', 'block.created', 'block', 'block:7aef40b2-14e1-4df0-a825-18603c55d198', 'sql', 'confirmed', '{"data":{"id":"block:7aef40b2-14e1-4df0-a825-18603c55d198","created_at":1773939024040,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Offline-first with background sync","parent_id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","content_type":"text","updated_at":1773939024086,"properties":{"ID":"7aef40b2-14e1-4df0-a825-18603c55d198","sequence":30}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.389670Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RS7PG1QASBN00PXWW', 'block.created', 'block', 'block:e148d7b7-c505-4201-83b7-36986a981a56', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024040,"content":"Iroh P2P transport for Loro documents","content_type":"text","parent_id":"block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","updated_at":1773939024086,"id":"block:e148d7b7-c505-4201-83b7-36986a981a56","properties":{"ID":"e148d7b7-c505-4201-83b7-36986a981a56","sequence":31}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.389960Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RBQND66QV6J38MAB4', 'block.created', 'block', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024040,"id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content_type":"text","content":"Dependency Injection [/]","updated_at":1773939024086,"properties":{"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9","sequence":32}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.390764Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R17A7HK12V0F66X8K', 'block.created', 'block', 'block:b980e51f-0c91-4708-9a17-3d41284974b2', 'sql', 'confirmed', '{"data":{"created_at":1773939024040,"id":"block:b980e51f-0c91-4708-9a17-3d41284974b2","parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"OperationDispatcher routing to providers","content_type":"text","updated_at":1773939024086,"properties":{"ID":"b980e51f-0c91-4708-9a17-3d41284974b2","sequence":33}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.391060Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RE96FF69YXGBFSTKQ', 'block.created', 'block', 'block:97cc8506-47d2-44cb-bdca-8e9a507953a0', 'sql', 'confirmed', '{"data":{"id":"block:97cc8506-47d2-44cb-bdca-8e9a507953a0","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024041,"parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","updated_at":1773939024086,"content":"BackendEngine as main orchestration point","properties":{"sequence":34,"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.392042Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RQGFQGP856ZYS936M', 'block.created', 'block', 'block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"id":"block:1c1f07b1-c801-47b2-8480-931cfb7930a8","content":"ferrous-di based service composition","parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024041,"content_type":"text","properties":{"sequence":35,"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.392354Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R9T2VRGWVYHEDJCTY', 'block.created', 'block', 'block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024041,"updated_at":1773939024086,"parent_id":"block:20e00c3a-2550-4791-a5e0-509d78137ce9","content_type":"text","content":"SchemaRegistry with topological initialization","id":"block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47","properties":{"sequence":36,"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.392647Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RPCNZCMT6K5Q71SMC', 'block.created', 'block', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'sql', 'confirmed', '{"data":{"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","updated_at":1773939024086,"content":"Query & Render Pipeline [/]","created_at":1773939024041,"properties":{"ID":"b489c622-6c87-4bf6-8d35-787eb732d670","sequence":37}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.392954Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RGF7QJ3ZC9DXRC83W', 'block.created', 'block', 'block:1bbec456-7217-4477-a49c-0b8422e441e9', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","content_type":"text","id":"block:1bbec456-7217-4477-a49c-0b8422e441e9","created_at":1773939024041,"content":"Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"1bbec456-7217-4477-a49c-0b8422e441e9","sequence":38}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.393783Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R8QT18R214E8VTHBB', 'block.created', 'block', 'block:2b1c341e-5da2-4207-a609-f4af6d7ceebd', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","id":"block:2b1c341e-5da2-4207-a609-f4af6d7ceebd","content":"Automatic operation wiring (lineage analysis → widget binding)","created_at":1773939024041,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"properties":{"sequence":39,"ID":"2b1c341e-5da2-4207-a609-f4af6d7ceebd","task_state":"DOING"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.394086Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R11VGEY7CF9PVC3VH', 'block.created', 'block', 'block:2d44d7df-5d7d-4cfe-9061-459c7578e334', 'sql', 'confirmed', '{"data":{"content":"GQL (graph query) support via EAV schema","content_type":"text","created_at":1773939024041,"updated_at":1773939024086,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","id":"block:2d44d7df-5d7d-4cfe-9061-459c7578e334","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"task_state":"DOING","sequence":40,"ID":"2d44d7df-5d7d-4cfe-9061-459c7578e334"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.394423Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RGDJ6PQQVB0QPF34P', 'block.created', 'block', 'block:54ed1be5-765e-4884-87ab-02268e0208c7', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773939024086,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:54ed1be5-765e-4884-87ab-02268e0208c7","content":"PRQL compilation (PRQL → SQL + RenderSpec)","created_at":1773939024041,"properties":{"sequence":41,"ID":"54ed1be5-765e-4884-87ab-02268e0208c7"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.394744Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RN2X1TTCCXFG8PYW1', 'block.created', 'block', 'block:5384c1da-f058-4321-8401-929b3570c2a5', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"id":"block:5384c1da-f058-4321-8401-929b3570c2a5","content_type":"text","parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","content":"RenderSpec tree for declarative UI description","created_at":1773939024041,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":42,"ID":"5384c1da-f058-4321-8401-929b3570c2a5"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.395549Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RFZJ6J9AZ8DQPTCRF', 'block.created', 'block', 'block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc', 'sql', 'confirmed', '{"data":{"id":"block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","created_at":1773939024041,"parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Unified execute_query supporting PRQL/GQL/SQL","content_type":"text","updated_at":1773939024086,"properties":{"task_state":"DONE","sequence":43,"ID":"fcf071b3-01f2-4d1d-882b-9f6a34c81bbc"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.395883Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R15PT5B8B6QZDHS0Y', 'block.created', 'block', 'block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:b489c622-6c87-4bf6-8d35-787eb732d670","content":"SQL direct execution support","id":"block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","created_at":1773939024041,"updated_at":1773939024086,"content_type":"text","properties":{"task_state":"DOING","sequence":44,"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.396186Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R0J3ZKZ72SCGR98W8', 'block.created', 'block', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024042,"content":"Loro CRDT Integration [/]","updated_at":1773939024086,"id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","properties":{"ID":"d9374dc3-05fc-40b2-896d-f88bb8a33c92","sequence":45}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.396504Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RY1WEP6RQ1RYYY9KX', 'block.created', 'block', 'block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024042,"id":"block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6","content":"LoroBackend implementing CoreOperations trait","updated_at":1773939024086,"properties":{"sequence":46,"ID":"b1dc3ad3-574b-472a-b74b-e3ea29a433e6"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.397371Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RWFWW7PH1NK9SQV7R', 'block.created', 'block', 'block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"LoroDocumentStore for managing CRDT documents on disk","created_at":1773939024042,"id":"block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","content_type":"text","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","updated_at":1773939024086,"properties":{"task_state":"DOING","ID":"ce2986c5-51a2-4d1e-9b0d-6ab9123cc957","sequence":47}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.398435Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RG5HTXTNM5FCD68EV', 'block.created', 'block', 'block:35652c3f-720c-4e20-ab90-5e25e1429733', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:35652c3f-720c-4e20-ab90-5e25e1429733","content":"LoroBlockOperations as OperationProvider routing writes through CRDT","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","updated_at":1773939024086,"content_type":"text","created_at":1773939024042,"properties":{"ID":"35652c3f-720c-4e20-ab90-5e25e1429733","sequence":48}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.399270Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R4KFSEPX9ZHK370PY', 'block.created', 'block', 'block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","content_type":"text","id":"block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2","content":"Cycle detection in move_block","created_at":1773939024042,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"090731e3-38ae-4bf1-b5ec-dbb33eae4fb2","sequence":49}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.399584Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R51RY87WMVF07188Z', 'block.created', 'block', 'block:ddf208e4-9b73-422d-b8ab-4ec58b328907', 'sql', 'confirmed', '{"data":{"content":"Loro-to-Turso materialization (CRDT → SQL cache → CDC)","created_at":1773939024042,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","parent_id":"block:d9374dc3-05fc-40b2-896d-f88bb8a33c92","id":"block:ddf208e4-9b73-422d-b8ab-4ec58b328907","updated_at":1773939024086,"properties":{"sequence":50,"ID":"ddf208e4-9b73-422d-b8ab-4ec58b328907"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.399906Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RQHMY098Q5036TZK4', 'block.created', 'block', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content":"Org-Mode Sync [/]","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content_type":"text","created_at":1773939024042,"id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","properties":{"sequence":51,"ID":"9af3a008-c1d7-422b-a1c8-e853f3ccb6fa"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.400748Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RVDV95N1JCNZVZJAV', 'block.created', 'block', 'block:7bc5f362-0bf9-45a1-b2b7-6882585ed169', 'sql', 'confirmed', '{"data":{"id":"block:7bc5f362-0bf9-45a1-b2b7-6882585ed169","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024042,"parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","content":"OrgRenderer as single path for producing org text","updated_at":1773939024086,"content_type":"text","properties":{"ID":"7bc5f362-0bf9-45a1-b2b7-6882585ed169","sequence":52}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.401597Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R1K3B03Q4WBP1F01C', 'block.created', 'block', 'block:8eab3453-25d2-4e7a-89f8-f9f79be939c9', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","created_at":1773939024042,"content":"Document identity & aliases (UUID ↔ file path mapping)","updated_at":1773939024086,"content_type":"text","id":"block:8eab3453-25d2-4e7a-89f8-f9f79be939c9","properties":{"ID":"8eab3453-25d2-4e7a-89f8-f9f79be939c9","sequence":53}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.403163Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R69BKD0H4HFN2N2S4', 'block.created', 'block', 'block:fc60da1b-6065-4d36-8551-5479ff145df0', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:fc60da1b-6065-4d36-8551-5479ff145df0","content":"OrgSyncController with echo suppression","parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","created_at":1773939024042,"content_type":"text","properties":{"ID":"fc60da1b-6065-4d36-8551-5479ff145df0","sequence":54}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.403962Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R8FPTKF4PDWB8W2BW', 'block.created', 'block', 'block:6e5a1157-b477-45a1-892f-57807b4d969b', 'sql', 'confirmed', '{"data":{"created_at":1773939024043,"updated_at":1773939024086,"parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","id":"block:6e5a1157-b477-45a1-892f-57807b4d969b","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Bidirectional sync (file changes ↔ block changes)","properties":{"ID":"6e5a1157-b477-45a1-892f-57807b4d969b","sequence":55}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.404849Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RK3SF9C3NCAF20TP3', 'block.created', 'block', 'block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f', 'sql', 'confirmed', '{"data":{"id":"block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f","parent_id":"block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa","created_at":1773939024043,"content":"Org file parsing (headlines, properties, source blocks)","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"ID":"6e4dab75-cd13-4c5e-9168-bf266d11aa3f","sequence":56}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.405833Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RFT1QRTPXASNWQS0M', 'block.created', 'block', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'sql', 'confirmed', '{"data":{"id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024043,"content_type":"text","updated_at":1773939024086,"parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content":"Flutter Frontend [/]","properties":{"ID":"bb3bc716-ca9a-438a-936d-03631e2ee929","sequence":57}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.406889Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RYTCRADCAVRXCCAD9', 'block.created', 'block', 'block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"created_at":1773939024043,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"FFI bridge via flutter_rust_bridge","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","id":"block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f","properties":{"sequence":58,"ID":"b4753cd8-47ea-4f7d-bd00-e1ec563aa43f"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.407198Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R10XX0603QWWQYSWA', 'block.created', 'block', 'block:3289bc82-f8a9-4cad-8545-ad1fee9dc282', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"created_at":1773939024043,"parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Navigation system (history, cursor, focus)","id":"block:3289bc82-f8a9-4cad-8545-ad1fee9dc282","content_type":"text","properties":{"ID":"3289bc82-f8a9-4cad-8545-ad1fee9dc282","task_state":"DOING","sequence":59}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.407509Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RGPRDNCCRR1A0MJB1', 'block.created', 'block', 'block:ebca0a24-f6f6-4c49-8a27-9d9973acf737', 'sql', 'confirmed', '{"data":{"created_at":1773939024043,"parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:ebca0a24-f6f6-4c49-8a27-9d9973acf737","content":"Block editor (outliner interactions)","updated_at":1773939024086,"content_type":"text","properties":{"sequence":60,"ID":"ebca0a24-f6f6-4c49-8a27-9d9973acf737"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.407815Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RBGAR3CXNCJGPS79E', 'block.created', 'block', 'block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"Reactive UI updates from CDC change streams","created_at":1773939024043,"updated_at":1773939024086,"parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd","properties":{"ID":"eb7e34f8-19f5-48f5-a22d-8f62493bafdd","sequence":61}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.408131Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RAH8Z204WDJE92Z98', 'block.created', 'block', 'block:7a0a4905-59c5-4277-8114-1e9ca9d425e3', 'sql', 'confirmed', '{"data":{"content":"Three-column layout (sidebar, main, right panel)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024043,"updated_at":1773939024086,"id":"block:7a0a4905-59c5-4277-8114-1e9ca9d425e3","parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","properties":{"sequence":62,"ID":"7a0a4905-59c5-4277-8114-1e9ca9d425e3"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.408440Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RDA0YGP4EZRTA34NZ', 'block.created', 'block', 'block:19d7b512-e5e0-469c-917b-eb27d7a38bed', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024043,"parent_id":"block:bb3bc716-ca9a-438a-936d-03631e2ee929","id":"block:19d7b512-e5e0-469c-917b-eb27d7a38bed","content":"Flutter desktop app shell","updated_at":1773939024086,"properties":{"ID":"19d7b512-e5e0-469c-917b-eb27d7a38bed","sequence":63}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.408744Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RETJXDC85RYRHCGDF', 'block.created', 'block', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024043,"id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","parent_id":"block:599b60af-960d-4c9c-b222-d3d9de95c513","content_type":"text","content":"Petri-Net Task Ranking (WSJF) [/]","updated_at":1773939024086,"properties":{"sequence":64,"ID":"afe4f75c-7948-4d4c-9724-4bfab7d47d88"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.409051Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6R4VP38TVN5787N5PX', 'block.created', 'block', 'block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7', 'sql', 'confirmed', '{"data":{"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","updated_at":1773939024086,"created_at":1773939024043,"id":"block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Prototype blocks with =computed Rhai expressions","content_type":"text","properties":{"task_state":"DOING","ID":"d81b05ee-70f9-4b19-b43e-40a93fd5e1b7","sequence":65}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.409376Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6RW653XY7ENH5TGK0N', 'block.created', 'block', 'block:2d399fd7-79d8-41f1-846b-31dabcec208a', 'sql', 'confirmed', '{"data":{"id":"block:2d399fd7-79d8-41f1-846b-31dabcec208a","content":"Verb dictionary (~30 German + English verbs → transition types)","created_at":1773939024044,"content_type":"text","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","properties":{"sequence":66,"ID":"2d399fd7-79d8-41f1-846b-31dabcec208a"}}}', NULL, NULL, 1773939024088, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.409689Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SCAME29BZCANZJ7RK', 'block.created', 'block', 'block:2385f4e3-25e1-4911-bf75-77cefd394206', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"rank_tasks() engine with tiebreak ordering","updated_at":1773939024086,"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","created_at":1773939024044,"content_type":"text","id":"block:2385f4e3-25e1-4911-bf75-77cefd394206","properties":{"task_state":"DOING","ID":"2385f4e3-25e1-4911-bf75-77cefd394206","sequence":67}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.410469Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SZPHCQJ14YHMQXG0X', 'block.created', 'block', 'block:cae619f2-26fe-464e-b67a-0a04f76543c9', 'sql', 'confirmed', '{"data":{"id":"block:cae619f2-26fe-464e-b67a-0a04f76543c9","parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","updated_at":1773939024086,"content":"Block → Petri Net materialization (petri.rs)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024044,"properties":{"sequence":68,"task_state":"DOING","ID":"cae619f2-26fe-464e-b67a-0a04f76543c9"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.410873Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SQ00ED8V1497VB5JR', 'block.created', 'block', 'block:eaee1c9b-5466-428f-8dbb-f4882ccdb066', 'sql', 'confirmed', '{"data":{"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","updated_at":1773939024086,"id":"block:eaee1c9b-5466-428f-8dbb-f4882ccdb066","content_type":"text","content":"Self Descriptor (person block with is_self: true)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024044,"properties":{"task_state":"DOING","sequence":69,"ID":"eaee1c9b-5466-428f-8dbb-f4882ccdb066"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.411247Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S9WF66BTN6WSV8KW5', 'block.created', 'block', 'block:023da362-ce5d-4a3b-827a-29e745d6f778', 'sql', 'confirmed', '{"data":{"id":"block:023da362-ce5d-4a3b-827a-29e745d6f778","created_at":1773939024044,"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"WSJF scoring (priority_weight × urgency_weight + position_weight)","properties":{"sequence":70,"ID":"023da362-ce5d-4a3b-827a-29e745d6f778","task_state":"DOING"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.411563Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S5RGPBZED2F0183AM', 'block.created', 'block', 'block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812', 'sql', 'confirmed', '{"data":{"parent_id":"block:afe4f75c-7948-4d4c-9724-4bfab7d47d88","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024044,"updated_at":1773939024086,"content":"Task syntax parser (@, ?, >, [[links]])","id":"block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812","properties":{"sequence":71,"ID":"46a8c75e-8ab8-4a5a-b4af-a1388f6a4812"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.411877Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S9N03RQFAP4JGEQCD', 'block.created', 'block', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'sql', 'confirmed', '{"data":{"id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","content_type":"text","parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content":"Phase 2: First Integration (Todoist) [/]\\nGoal: Prove hybrid architecture","created_at":1773939024044,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":72,"ID":"29c0aa5f-d9ca-46f3-8601-6023f87cefbd"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.412262Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SVKKV9CJMQ2J68V0X', 'block.created', 'block', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024044,"parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Reconciliation [/]","updated_at":1773939024086,"properties":{"ID":"00fa0916-2681-4699-9554-44fcb8e2ea6a","sequence":73}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.412580Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S40MYYPZVNQ7ZQKZK', 'block.created', 'block', 'block:632af903-5459-4d44-921a-43145e20dc82', 'sql', 'confirmed', '{"data":{"id":"block:632af903-5459-4d44-921a-43145e20dc82","content":"Sync token management to prevent duplicate processing","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content_type":"text","parent_id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","created_at":1773939024044,"properties":{"sequence":74,"ID":"632af903-5459-4d44-921a-43145e20dc82"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.412890Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S278SACNZ001SDTC6', 'block.created', 'block', 'block:78f9d6e3-42d4-4975-910d-3728e23410b1', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"content_type":"text","content":"Conflict detection and resolution UI","created_at":1773939024044,"id":"block:78f9d6e3-42d4-4975-910d-3728e23410b1","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","properties":{"sequence":75,"ID":"78f9d6e3-42d4-4975-910d-3728e23410b1"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.413665Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S9CGXE5JCKXYPDS7A', 'block.created', 'block', 'block:fa2854d1-2751-4a07-8f83-70c2f9c6c190', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:fa2854d1-2751-4a07-8f83-70c2f9c6c190","content":"Last-write-wins for concurrent edits","parent_id":"block:00fa0916-2681-4699-9554-44fcb8e2ea6a","content_type":"text","created_at":1773939024044,"updated_at":1773939024086,"properties":{"sequence":76,"ID":"fa2854d1-2751-4a07-8f83-70c2f9c6c190"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.413970Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SHMPQ4G87JCNA5V0S', 'block.created', 'block', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","content":"Operation Queue & Offline Support [/]","created_at":1773939024045,"updated_at":1773939024086,"id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","properties":{"sequence":77,"ID":"043ed925-6bf2-4db3-baf8-2277f1a5afaa"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.414271Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SBKNVA7A8RS632F4Y', 'block.created', 'block', 'block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"created_at":1773939024045,"content_type":"text","parent_id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","id":"block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","content":"Offline operation queue with retry logic","properties":{"ID":"5c1ce94f-fcf2-44d8-b94d-27cc91186ce3","sequence":78}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.414593Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SNGKECXHFE6CMDXRJ', 'block.created', 'block', 'block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024045,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","updated_at":1773939024086,"id":"block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05","content":"Sync status indicators (synced, pending, conflict, error)","properties":{"ID":"7de8d37b-49ba-4ada-9b1e-df1c41c0db05","sequence":79}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.415362Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SEDYZXE08WAH87MY0', 'block.created', 'block', 'block:302eb0c5-56fe-4980-8292-bae8a9a0450a', 'sql', 'confirmed', '{"data":{"id":"block:302eb0c5-56fe-4980-8292-bae8a9a0450a","parent_id":"block:043ed925-6bf2-4db3-baf8-2277f1a5afaa","content":"Optimistic updates with ID mapping (internal ↔ external)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024086,"created_at":1773939024045,"properties":{"sequence":80,"ID":"302eb0c5-56fe-4980-8292-bae8a9a0450a"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.415678Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SZB3C8X1DRTWKH8FY', 'block.created', 'block', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'sql', 'confirmed', '{"data":{"parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","updated_at":1773939024086,"id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Todoist-Specific Features [/]","created_at":1773939024045,"properties":{"ID":"b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","sequence":81}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.416447Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S55YKE9YHVECGNZ5G', 'block.created', 'block', 'block:a27cd79b-63bd-4704-b20f-f3b595838e89', 'sql', 'confirmed', '{"data":{"parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","updated_at":1773939024086,"id":"block:a27cd79b-63bd-4704-b20f-f3b595838e89","created_at":1773939024045,"content":"Bi-directional task completion sync","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"ID":"a27cd79b-63bd-4704-b20f-f3b595838e89","sequence":82}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.416753Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SXMFSF3D6BVDK3MSN', 'block.created', 'block', 'block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22', 'sql', 'confirmed', '{"data":{"parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024045,"content":"Todoist due dates → deadline penalty functions","updated_at":1773939024086,"content_type":"text","id":"block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22","properties":{"sequence":83,"ID":"ab2868f6-ac6a-48de-b56f-ffa755f6cd22"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.417566Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SE0R2TF765ZK7434D', 'block.created', 'block', 'block:f6e32a19-a659-47f7-b2dc-24142c6616f7', 'sql', 'confirmed', '{"data":{"id":"block:f6e32a19-a659-47f7-b2dc-24142c6616f7","content":"@person labels → delegation/waiting_for tracking","created_at":1773939024045,"parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024086,"properties":{"sequence":84,"ID":"f6e32a19-a659-47f7-b2dc-24142c6616f7"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.417881Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S4AC1VH18AJRBNDNF', 'block.created', 'block', 'block:19923c1b-89ab-42f3-97a2-d78e994a2e1c', 'sql', 'confirmed', '{"data":{"content":"Todoist priority → WSJF CoD weight mapping","updated_at":1773939024086,"created_at":1773939024045,"parent_id":"block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:19923c1b-89ab-42f3-97a2-d78e994a2e1c","properties":{"sequence":85,"ID":"19923c1b-89ab-42f3-97a2-d78e994a2e1c"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.418687Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S91CJHF70F6TKVJ9C', 'block.created', 'block', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'sql', 'confirmed', '{"data":{"created_at":1773939024045,"updated_at":1773939024086,"id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","content":"MCP Client Bridge [/]","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","properties":{"ID":"f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","sequence":86}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.419478Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S8K3MBFD1SRYFVPQE', 'block.created', 'block', 'block:4d30926a-54c4-40b4-978e-eeca2d273fd1', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024045,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:4d30926a-54c4-40b4-978e-eeca2d273fd1","updated_at":1773939024086,"content":"Tool name normalization (kebab-case ↔ snake_case)","parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","properties":{"sequence":87,"ID":"4d30926a-54c4-40b4-978e-eeca2d273fd1"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.420345Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SZ4BJVR09SJEBW506', 'block.created', 'block', 'block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"content":"McpOperationProvider converting MCP tool schemas → OperationDescriptors","created_at":1773939024046,"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","content_type":"text","id":"block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467","properties":{"ID":"c30b7e5a-4e9f-41e8-ab19-e803c93dc467","sequence":88}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.420685Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SPJ34X8Y5MMQ9RYEB', 'block.created', 'block', 'block:836bab0e-5ac1-4df1-9f40-4005320c406e', 'sql', 'confirmed', '{"data":{"created_at":1773939024046,"updated_at":1773939024086,"content":"holon-mcp-client crate for connecting to external MCP servers","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:836bab0e-5ac1-4df1-9f40-4005320c406e","content_type":"text","parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","properties":{"ID":"836bab0e-5ac1-4df1-9f40-4005320c406e","sequence":89}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.420999Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S66E0CE40X51NHT6N', 'block.created', 'block', 'block:ceb59dae-6090-41be-aff7-89de33ec600a', 'sql', 'confirmed', '{"data":{"created_at":1773939024046,"id":"block:ceb59dae-6090-41be-aff7-89de33ec600a","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024086,"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","content":"YAML sidecar for UI annotations (affected_fields, triggered_by, preconditions)","properties":{"ID":"ceb59dae-6090-41be-aff7-89de33ec600a","sequence":90}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.421324Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SVKNT2ESTS6WVED87', 'block.created', 'block', 'block:419e493f-c2de-47c2-a612-787db669cd89', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"parent_id":"block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","id":"block:419e493f-c2de-47c2-a612-787db669cd89","content":"JSON Schema → TypeHint mapping","content_type":"text","created_at":1773939024046,"properties":{"sequence":91,"ID":"419e493f-c2de-47c2-a612-787db669cd89"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.421636Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S17Q4ZCT7H8XC03P5', 'block.created', 'block', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024046,"updated_at":1773939024086,"content":"Todoist API Integration [/]","parent_id":"block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd","id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","content_type":"text","properties":{"sequence":92,"ID":"bdce9ec2-1508-47e9-891e-e12a7b228fcc"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.421944Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S9TY77NKSTH0KMYVG', 'block.created', 'block', 'block:e9398514-1686-4fef-a44a-5fef1742d004', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:e9398514-1686-4fef-a44a-5fef1742d004","updated_at":1773939024086,"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","content":"TodoistOperationProvider for operation routing","created_at":1773939024046,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":93,"ID":"e9398514-1686-4fef-a44a-5fef1742d004"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.422253Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S98PWRXXPD35NR33V', 'block.created', 'block', 'block:9670e586-5cda-42a2-8071-efaf855fd5d4', 'sql', 'confirmed', '{"data":{"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","content":"Todoist REST API client","created_at":1773939024046,"updated_at":1773939024086,"content_type":"text","id":"block:9670e586-5cda-42a2-8071-efaf855fd5d4","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"9670e586-5cda-42a2-8071-efaf855fd5d4","sequence":94}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.422574Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SZ4JE1YAX5PHXJMR4', 'block.created', 'block', 'block:f41aeaa5-fe1d-45a5-806d-1f815040a33d', 'sql', 'confirmed', '{"data":{"updated_at":1773939024086,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Todoist entity types (tasks, projects, sections, labels)","id":"block:f41aeaa5-fe1d-45a5-806d-1f815040a33d","parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","created_at":1773939024046,"properties":{"ID":"f41aeaa5-fe1d-45a5-806d-1f815040a33d","sequence":95}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.422912Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SJK7K6AD5AD2CYHEW', 'block.created', 'block', 'block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024046,"content":"TodoistSyncProvider with incremental sync tokens","id":"block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe","content_type":"text","updated_at":1773939024086,"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","properties":{"sequence":96,"ID":"d041e942-f3a1-4b7d-80b8-7de6eb289ebe"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.423226Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S7QJHRC8VM03YQ7MQ', 'block.created', 'block', 'block:f3b43be1-5503-4b1a-a724-fc657b47e18c', 'sql', 'confirmed', '{"data":{"content":"TodoistTaskDataSource implementing DataSource<TodoistTask>","content_type":"text","id":"block:f3b43be1-5503-4b1a-a724-fc657b47e18c","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024086,"parent_id":"block:bdce9ec2-1508-47e9-891e-e12a7b228fcc","created_at":1773939024046,"properties":{"sequence":97,"ID":"f3b43be1-5503-4b1a-a724-fc657b47e18c"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.423556Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S3295ZQBZ4KYATY10', 'block.created', 'block', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'sql', 'confirmed', '{"data":{"content":"Phase 3: Multiple Integrations [/]\\nGoal: Validate type unification scales","created_at":1773939024046,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","content_type":"text","updated_at":1773939024086,"properties":{"sequence":98,"ID":"88810f15-a95b-4343-92e2-909c5113cc9c"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.423884Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SQV95WE0A7G42P50V', 'block.created', 'block', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Unified Item Types [/]","updated_at":1773939024086,"created_at":1773939024047,"id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","parent_id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","properties":{"ID":"9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","sequence":99}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.424196Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S90NT263TA0J24WFT', 'block.created', 'block', 'block:5b1e8251-be26-4099-b169-a330cc16f0a6', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773939024086,"content":"Macro-generated serialization boilerplate","created_at":1773939024047,"id":"block:5b1e8251-be26-4099-b169-a330cc16f0a6","parent_id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":100,"ID":"5b1e8251-be26-4099-b169-a330cc16f0a6"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.424522Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SCDGX2JP1SZZRYYEV', 'block.created', 'block', 'block:5b49aefd-e14f-4151-bf9e-ccccae3545ec', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"Trait-based protocol for common task interface","updated_at":1773939024087,"id":"block:5b49aefd-e14f-4151-bf9e-ccccae3545ec","created_at":1773939024047,"parent_id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"5b49aefd-e14f-4151-bf9e-ccccae3545ec","sequence":101}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.424849Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SF77NYT50G55B42TE', 'block.created', 'block', 'block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447', 'sql', 'confirmed', '{"data":{"parent_id":"block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content":"Extension structs for system-specific features","created_at":1773939024047,"id":"block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447","content_type":"text","properties":{"sequence":102,"ID":"e6162a0a-e9ae-494e-b3f5-4cf98cb2f447"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.425166Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SGKMJQEMH4KCS7759', 'block.created', 'block', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'sql', 'confirmed', '{"data":{"id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Cross-System Features [/]","content_type":"text","created_at":1773939024047,"parent_id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","updated_at":1773939024087,"properties":{"ID":"d6ab6d5f-68ae-404a-bcad-b5db61586634","sequence":103}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.425481Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S0KHQHHX475F6N8N9', 'block.created', 'block', 'block:5403c088-a551-4ca6-8830-34e00d5e5820', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024047,"content":"Context Bundles assembling related items from all sources","id":"block:5403c088-a551-4ca6-8830-34e00d5e5820","parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","updated_at":1773939024087,"properties":{"sequence":104,"ID":"5403c088-a551-4ca6-8830-34e00d5e5820"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.426273Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S1FK3RWXJQXQ3EWFT', 'block.created', 'block', 'block:091caad8-1689-472d-9130-e3c855c510a8', 'sql', 'confirmed', '{"data":{"parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","content_type":"text","updated_at":1773939024087,"content":"Embedding third-party items anywhere in the graph","created_at":1773939024047,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:091caad8-1689-472d-9130-e3c855c510a8","properties":{"ID":"091caad8-1689-472d-9130-e3c855c510a8","sequence":105}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.426619Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6ST49KXR7DR5HV67NV', 'block.created', 'block', 'block:cfb257f0-1a9c-426c-ab24-940eb18853ea', 'sql', 'confirmed', '{"data":{"id":"block:cfb257f0-1a9c-426c-ab24-940eb18853ea","content":"Unified search across all systems","content_type":"text","created_at":1773939024047,"updated_at":1773939024087,"parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":106,"ID":"cfb257f0-1a9c-426c-ab24-940eb18853ea"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.426970Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SE4YMMTY71G1XH8FM', 'block.created', 'block', 'block:52a440c1-4099-4911-8d9d-e2d583dbdde7', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024047,"id":"block:52a440c1-4099-4911-8d9d-e2d583dbdde7","parent_id":"block:d6ab6d5f-68ae-404a-bcad-b5db61586634","content_type":"text","content":"P.A.R.A. project-based organization with auto-linking","properties":{"sequence":107,"ID":"52a440c1-4099-4911-8d9d-e2d583dbdde7"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.427300Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SWB9671R7RMWX0H0R', 'block.created', 'block', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'sql', 'confirmed', '{"data":{"created_at":1773939024047,"parent_id":"block:88810f15-a95b-4343-92e2-909c5113cc9c","content_type":"text","updated_at":1773939024087,"id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","content":"Additional Integrations [/]","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"34fa9276-cc30-4fcb-95b5-a97b5d708757","sequence":108}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.427663Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SGXDBFHYP506ZBKH1', 'block.created', 'block', 'block:9240c0d7-d60a-46e0-8265-ceacfbf04d50', 'sql', 'confirmed', '{"data":{"content":"Linear integration (cycles, projects)","updated_at":1773939024087,"content_type":"text","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","created_at":1773939024047,"id":"block:9240c0d7-d60a-46e0-8265-ceacfbf04d50","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":109,"ID":"9240c0d7-d60a-46e0-8265-ceacfbf04d50"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.428457Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SSAWK2TWHPG3G1WRZ', 'block.created', 'block', 'block:8ea813ff-b355-4165-b377-fbdef4d3d7d8', 'sql', 'confirmed', '{"data":{"created_at":1773939024048,"parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"id":"block:8ea813ff-b355-4165-b377-fbdef4d3d7d8","content":"Google Calendar integration (events as time tokens)","properties":{"sequence":110,"ID":"8ea813ff-b355-4165-b377-fbdef4d3d7d8"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.428793Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S577F2WTSSKG8EGRX', 'block.created', 'block', 'block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29', 'sql', 'confirmed', '{"data":{"id":"block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29","content":"Gmail integration (email threads, labels)","created_at":1773939024048,"parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","content_type":"text","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":111,"ID":"ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.429164Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SXNQCTYMH08ZTA7NE', 'block.created', 'block', 'block:f583e6d9-f67d-4997-a658-ed00149a34cc', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:f583e6d9-f67d-4997-a658-ed00149a34cc","content_type":"text","created_at":1773939024048,"content":"JIRA integration (sprints, story points, epics)","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","updated_at":1773939024087,"properties":{"sequence":112,"ID":"f583e6d9-f67d-4997-a658-ed00149a34cc"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.429502Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S3JZYPCJZGENQ2NRW', 'block.created', 'block', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'sql', 'confirmed', '{"data":{"id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"GPUI Components","parent_id":"block:34fa9276-cc30-4fcb-95b5-a97b5d708757","content_type":"text","updated_at":1773939024087,"created_at":1773939024048,"properties":{"sequence":113,"ID":"9fed69a3-9180-4eba-a778-fa93bc398064"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.429855Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SJB4TC87Z64ZT3D31', 'block.created', 'block', 'block:9f523ce8-5449-4a2f-81c8-8ee08399fc31', 'sql', 'confirmed', '{"data":{"created_at":1773939024048,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"https://github.com/MeowLynxSea/yororen-ui","content_type":"text","id":"block:9f523ce8-5449-4a2f-81c8-8ee08399fc31","parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","updated_at":1773939024087,"properties":{"sequence":114,"ID":"9f523ce8-5449-4a2f-81c8-8ee08399fc31"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.430200Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SMGAQ7094MYGM0VTB', 'block.created', 'block', 'block:fd965570-883d-48f7-82b0-92ba257b2597', 'sql', 'confirmed', '{"data":{"id":"block:fd965570-883d-48f7-82b0-92ba257b2597","created_at":1773939024048,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Pomodoro\\nhttps://github.com/rubbieKelvin/bmo","content_type":"text","parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","updated_at":1773939024087,"properties":{"ID":"fd965570-883d-48f7-82b0-92ba257b2597","sequence":115}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.430539Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SXNSF7Q82625KHDMB', 'block.created', 'block', 'block:9657e201-4426-4091-891b-eb40e299d81d', 'sql', 'confirmed', '{"data":{"created_at":1773939024048,"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","updated_at":1773939024087,"content_type":"text","content":"Diff viewer\\nhttps://github.com/BlixtWallet/hunk","id":"block:9657e201-4426-4091-891b-eb40e299d81d","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":116,"ID":"9657e201-4426-4091-891b-eb40e299d81d"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.430882Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SPGARYNYQ2SH581N7', 'block.created', 'block', 'block:61a47437-c394-42db-b195-3dabbd5d87ab', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content":"Animation\\nhttps://github.com/chi11321/gpui-animation","created_at":1773939024048,"id":"block:61a47437-c394-42db-b195-3dabbd5d87ab","parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","content_type":"text","properties":{"sequence":117,"ID":"61a47437-c394-42db-b195-3dabbd5d87ab"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.431234Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SRCPXCSBVF8BCFZ8R', 'block.created', 'block', 'block:5841efc0-cfe6-4e69-9dbc-9f627693e59a', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Editor\\nhttps://github.com/iamnbutler/gpui-editor","parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","id":"block:5841efc0-cfe6-4e69-9dbc-9f627693e59a","created_at":1773939024048,"properties":{"sequence":118,"ID":"5841efc0-cfe6-4e69-9dbc-9f627693e59a"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.432019Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SKKVNMFJEG74C999F', 'block.created', 'block', 'block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c', 'sql', 'confirmed', '{"data":{"created_at":1773939024048,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"WebView\\nhttps://github.com/longbridge/wef","updated_at":1773939024087,"parent_id":"block:9fed69a3-9180-4eba-a778-fa93bc398064","content_type":"text","id":"block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c","properties":{"ID":"482c5cbb-dd4f-4225-9329-ca9ca0beea4c","sequence":119}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.433460Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6STR1XEPSNZQ1VJD4X', 'block.created', 'block', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'sql', 'confirmed', '{"data":{"parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","id":"block:7b960cd0-3478-412b-b96f-15822117ac14","content":"Phase 4: AI Foundation [/]\\nGoal: Infrastructure for AI features","updated_at":1773939024087,"created_at":1773939024049,"properties":{"sequence":120,"ID":"7b960cd0-3478-412b-b96f-15822117ac14"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.434310Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S0T50EF1J1PNSPXR9', 'block.created', 'block', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'sql', 'confirmed', '{"data":{"content":"Agent Embedding","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","created_at":1773939024049,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024087,"id":"block:553f3545-4ec7-44e5-bccf-3d6443f22ecc","properties":{"ID":"553f3545-4ec7-44e5-bccf-3d6443f22ecc","sequence":121}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.434646Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S3HKAV0GMPZ9F37A8', 'block.created', 'block', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'sql', 'confirmed', '{"data":{"created_at":1773939024049,"content_type":"text","id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content":"Via Terminal","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"parent_id":"block:553f3545-4ec7-44e5-bccf-3d6443f22ecc","properties":{"sequence":122,"ID":"d4c1533f-3a67-4314-b430-0e24bd62ce34"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.435007Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S2BGJ3P1YV4PZA3YN', 'block.created', 'block', 'block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e', 'sql', 'confirmed', '{"data":{"content":"Okena\\nA fast, native terminal multiplexer built in Rust with GPUI\\nhttps://github.com/contember/okena","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","id":"block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e","updated_at":1773939024087,"parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","created_at":1773939024049,"properties":{"ID":"6e2fd9a2-6f39-48d2-b323-935fc18a3f5e","sequence":123}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.435376Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SGE4W6G752YSGG7BH', 'block.created', 'block', 'block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content_type":"text","id":"block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e","content":"PMux\\nhttps://github.com/zhoujinliang/pmux","parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024049,"properties":{"ID":"c4b1ce62-0ad1-4c33-90fe-d7463f40800e","sequence":124}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.435743Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6S2VMAZ0JDP75GJNM6', 'block.created', 'block', 'block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e', 'sql', 'confirmed', '{"data":{"created_at":1773939024049,"id":"block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e","parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","updated_at":1773939024087,"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Slick\\nhttps://github.com/tristanpoland/Slick","properties":{"sequence":125,"ID":"e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.436091Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6SFZRBZ4QV6MJ463A2', 'block.created', 'block', 'block:d50a9a7a-0155-4778-ac99-5f83555a1952', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content":"https://github.com/zortax/gpui-terminal","content_type":"text","id":"block:d50a9a7a-0155-4778-ac99-5f83555a1952","created_at":1773939024049,"properties":{"sequence":126,"ID":"d50a9a7a-0155-4778-ac99-5f83555a1952"}}}', NULL, NULL, 1773939024089, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.436437Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TBDAYQYQ3N6EK547V', 'block.created', 'block', 'block:cf102b47-01db-427b-97b6-3c066d9dba24', 'sql', 'confirmed', '{"data":{"content":"https://github.com/Xuanwo/gpui-ghostty","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:d4c1533f-3a67-4314-b430-0e24bd62ce34","content_type":"text","id":"block:cf102b47-01db-427b-97b6-3c066d9dba24","created_at":1773939024049,"properties":{"ID":"cf102b47-01db-427b-97b6-3c066d9dba24","sequence":127}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.436796Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TQKVHPYB2S0GGBAA2', 'block.created', 'block', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Via Chat","created_at":1773939024049,"updated_at":1773939024087,"id":"block:1236a3b4-6e03-421a-a94b-fce9d7dc123c","parent_id":"block:553f3545-4ec7-44e5-bccf-3d6443f22ecc","content_type":"text","properties":{"sequence":128,"ID":"1236a3b4-6e03-421a-a94b-fce9d7dc123c"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.437158Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TTVZQ7EZ3XSD309T4', 'block.created', 'block', 'block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:1236a3b4-6e03-421a-a94b-fce9d7dc123c","content_type":"text","created_at":1773939024049,"id":"block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847","content":"coop\\nhttps://github.com/lumehq/coop?tab=readme-ov-file","updated_at":1773939024087,"properties":{"ID":"f47a6df7-abfc-47b8-bdfe-f19eaf35b847","sequence":129}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.437507Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T47JF9EHKJCKAF45H', 'block.created', 'block', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'sql', 'confirmed', '{"data":{"parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Embeddings & Search [/]","id":"block:671593d9-a9c6-4716-860b-8410c8616539","created_at":1773939024049,"content_type":"text","updated_at":1773939024087,"properties":{"ID":"671593d9-a9c6-4716-860b-8410c8616539","sequence":130}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.437861Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TZSJS830PR7TKD8VW', 'block.created', 'block', 'block:d58b8367-14eb-4895-9e56-ffa7ff716d59', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024050,"id":"block:d58b8367-14eb-4895-9e56-ffa7ff716d59","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content":"Local vector embeddings (sentence-transformers)","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","properties":{"ID":"d58b8367-14eb-4895-9e56-ffa7ff716d59","sequence":131}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.438220Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T252D01WP4HTE9VTJ', 'block.created', 'block', 'block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","content":"Semantic search using local embeddings","id":"block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc","content_type":"text","updated_at":1773939024087,"created_at":1773939024050,"properties":{"sequence":132,"ID":"5f3e7d1e-af67-4699-a591-fd9291bf0cdc"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.438576Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T8H55HKYS1VT0CHA6', 'block.created', 'block', 'block:96f4647c-8b74-4b08-8952-4f87820aed86', 'sql', 'confirmed', '{"data":{"content":"Entity linking (manual first, then automatic)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","id":"block:96f4647c-8b74-4b08-8952-4f87820aed86","content_type":"text","updated_at":1773939024087,"created_at":1773939024050,"properties":{"sequence":133,"ID":"96f4647c-8b74-4b08-8952-4f87820aed86"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.438926Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TAFNV8T820K6G44P6', 'block.created', 'block', 'block:0da39f39-6635-4f9b-a468-34310147bea9', 'sql', 'confirmed', '{"data":{"id":"block:0da39f39-6635-4f9b-a468-34310147bea9","parent_id":"block:671593d9-a9c6-4716-860b-8410c8616539","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content_type":"text","created_at":1773939024050,"content":"Tantivy full-text search integration","properties":{"ID":"0da39f39-6635-4f9b-a468-34310147bea9","sequence":134}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.439277Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TXT48TX5V7XSPDMJK', 'block.created', 'block', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'sql', 'confirmed', '{"data":{"created_at":1773939024050,"content":"Self Digital Twin [/]","content_type":"text","updated_at":1773939024087,"id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":135,"ID":"439af07e-3237-420c-8bc0-c71aeb37c61a"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.439631Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TENGZNAMKB1DK6E5X', 'block.created', 'block', 'block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412', 'sql', 'confirmed', '{"data":{"parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","content_type":"text","content":"Energy/focus/flow_depth dynamics","id":"block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"created_at":1773939024050,"properties":{"ID":"5f3e8ef3-df52-4fb9-80c1-ccb81be40412","sequence":136}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.440393Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TZGPEAX154GZA83G9', 'block.created', 'block', 'block:30406a65-8e66-4589-b070-3a1b4db6e4e0', 'sql', 'confirmed', '{"data":{"id":"block:30406a65-8e66-4589-b070-3a1b4db6e4e0","created_at":1773939024050,"parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","updated_at":1773939024087,"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Peripheral awareness modeling","properties":{"ID":"30406a65-8e66-4589-b070-3a1b4db6e4e0","sequence":137}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.440738Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T8F5RS4XWK5MBTPR8', 'block.created', 'block', 'block:bed11feb-a634-4f8d-b930-f0021ec0512b', 'sql', 'confirmed', '{"data":{"created_at":1773939024050,"content_type":"text","parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","id":"block:bed11feb-a634-4f8d-b930-f0021ec0512b","content":"Observable signals (window switches, typing cadence)","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"bed11feb-a634-4f8d-b930-f0021ec0512b","sequence":138}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.441143Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T07TZ0RK4GDVNJ7SA', 'block.created', 'block', 'block:11c9c8bb-b72e-4752-8b6c-846e45920418', 'sql', 'confirmed', '{"data":{"content":"Mental slots tracking (materialized view of open transitions)","id":"block:11c9c8bb-b72e-4752-8b6c-846e45920418","content_type":"text","created_at":1773939024050,"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:439af07e-3237-420c-8bc0-c71aeb37c61a","properties":{"sequence":139,"ID":"11c9c8bb-b72e-4752-8b6c-846e45920418"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.441516Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TNCYC2DA20CGXNEXT', 'block.created', 'block', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'sql', 'confirmed', '{"data":{"id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","content":"Logging & Training Data [/]","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","created_at":1773939024050,"content_type":"text","updated_at":1773939024087,"properties":{"sequence":140,"ID":"b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.443396Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TEXQBN7HMZ54K3DM5', 'block.created', 'block', 'block:a186c88f-6ca5-49e2-8a0d-19632cb689fc', 'sql', 'confirmed', '{"data":{"id":"block:a186c88f-6ca5-49e2-8a0d-19632cb689fc","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"created_at":1773939024050,"parent_id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","content":"Conflict logging system (capture every conflict + resolution)","content_type":"text","properties":{"ID":"a186c88f-6ca5-49e2-8a0d-19632cb689fc","sequence":141}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.443721Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T8KXPWS8NS84RW6QV', 'block.created', 'block', 'block:f342692d-5414-4c48-89fe-ed8f9ccf2172', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","id":"block:f342692d-5414-4c48-89fe-ed8f9ccf2172","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024051,"content":"Pattern logging for Guide to learn from","properties":{"ID":"f342692d-5414-4c48-89fe-ed8f9ccf2172","sequence":142}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.444096Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TYPRD9B84XNBDD697', 'block.created', 'block', 'block:30f04064-a58e-416d-b0d2-7533637effe8', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024051,"updated_at":1773939024087,"content":"Behavioral logging for search ranking","id":"block:30f04064-a58e-416d-b0d2-7533637effe8","parent_id":"block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","properties":{"sequence":143,"ID":"30f04064-a58e-416d-b0d2-7533637effe8"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.445081Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TKHH6G32PFMT2BY4R', 'block.created', 'block', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:7b960cd0-3478-412b-b96f-15822117ac14","id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","updated_at":1773939024087,"created_at":1773939024051,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Objective Function Engine [/]","properties":{"sequence":144,"ID":"84151cf1-696a-420f-b73c-4947b0a4437e"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.445402Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TD904SBS7NG1S35QQ', 'block.created', 'block', 'block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024051,"updated_at":1773939024087,"id":"block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7","parent_id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","content":"Evaluate token attributes via PRQL → scalar score","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7","sequence":145}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.445762Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TFSAVAEW4H4KGWRQJ', 'block.created', 'block', 'block:480f2628-c49f-4940-9e26-572ea23f25a3', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content_type":"text","created_at":1773939024051,"id":"block:480f2628-c49f-4940-9e26-572ea23f25a3","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","content":"Store weights as prototype block properties","properties":{"ID":"480f2628-c49f-4940-9e26-572ea23f25a3","sequence":146}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.446511Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TEDEBQ3W0KG5P2C5G', 'block.created', 'block', 'block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e', 'sql', 'confirmed', '{"data":{"id":"block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e","parent_id":"block:84151cf1-696a-420f-b73c-4947b0a4437e","created_at":1773939024051,"updated_at":1773939024087,"content":"Support multiple goal types (achievement, maintenance, process)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"sequence":147,"ID":"e4e93198-6617-4c7c-b8f7-4b2d8188a77e"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.446853Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TH65TJGCTC3Z7R81K', 'block.created', 'block', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'sql', 'confirmed', '{"data":{"content":"Phase 5: AI Features [/]\\nGoal: Three AI services operational","created_at":1773939024051,"parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:8b962d6c-0246-4119-8826-d517e2357f21","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"sequence":148,"ID":"8b962d6c-0246-4119-8826-d517e2357f21"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.447224Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TQ3QAWQZC0MSS5DNC', 'block.created', 'block', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"The Guide (Growth) [/]","content_type":"text","id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","created_at":1773939024051,"parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","sequence":149}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.447547Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T7GMV4K8TESV96HKN', 'block.created', 'block', 'block:37c082de-d10a-4f11-82ad-5fb3316bb3e4', 'sql', 'confirmed', '{"data":{"created_at":1773939024051,"parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","id":"block:37c082de-d10a-4f11-82ad-5fb3316bb3e4","content":"Velocity and capacity analysis","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"sequence":150,"ID":"37c082de-d10a-4f11-82ad-5fb3316bb3e4"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.448335Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TZS7YBN7JK3ZPZQ47', 'block.created', 'block', 'block:52bedd69-85ec-448d-81b6-0099bd413149', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:52bedd69-85ec-448d-81b6-0099bd413149","parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","updated_at":1773939024087,"content_type":"text","content":"Stuck task identification (postponement tracking)","created_at":1773939024051,"properties":{"ID":"52bedd69-85ec-448d-81b6-0099bd413149","sequence":151}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.449080Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TPY62YD5K3X9RP4J0', 'block.created', 'block', 'block:2b5ec929-a22d-4d7f-8640-66495331a40d', 'sql', 'confirmed', '{"data":{"id":"block:2b5ec929-a22d-4d7f-8640-66495331a40d","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"created_at":1773939024051,"content_type":"text","parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","content":"Shadow Work prompts for avoided tasks","properties":{"sequence":152,"ID":"2b5ec929-a22d-4d7f-8640-66495331a40d"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.449849Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T61JQSH0545X1FH6H', 'block.created', 'block', 'block:dd9075a4-5c64-4d6b-9661-7937897337d3', 'sql', 'confirmed', '{"data":{"parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","content_type":"text","created_at":1773939024052,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:dd9075a4-5c64-4d6b-9661-7937897337d3","updated_at":1773939024087,"content":"Growth tracking and visualization","properties":{"ID":"dd9075a4-5c64-4d6b-9661-7937897337d3","sequence":153}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.450598Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TH6MWXYB31Q33SN7A', 'block.created', 'block', 'block:15a61916-b0c1-4d24-9046-4e066a312401', 'sql', 'confirmed', '{"data":{"content":"Pattern recognition across time","content_type":"text","parent_id":"block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","id":"block:15a61916-b0c1-4d24-9046-4e066a312401","created_at":1773939024052,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"properties":{"ID":"15a61916-b0c1-4d24-9046-4e066a312401","sequence":154}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.451373Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TYZ0YJMB2A6ZXSH1K', 'block.created', 'block', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'sql', 'confirmed', '{"data":{"created_at":1773939024052,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","updated_at":1773939024087,"parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content":"Intelligent Conflict Reconciliation [/]","properties":{"sequence":155,"ID":"8ae21b36-6f48-41f1-80d9-bb7ce43b4545"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.452443Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6THQJ7F072FBDHPXTG', 'block.created', 'block', 'block:0db1be3e-ae11-4341-8aa8-b1d80e22963a', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"LLM-based resolution for low-confidence cases","updated_at":1773939024087,"created_at":1773939024052,"parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","id":"block:0db1be3e-ae11-4341-8aa8-b1d80e22963a","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":156,"ID":"0db1be3e-ae11-4341-8aa8-b1d80e22963a"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.452784Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TS30F5JHB2VFP8A09', 'block.created', 'block', 'block:314e7db7-fb5e-40b6-ac10-a589ff3c809d', 'sql', 'confirmed', '{"data":{"content_type":"text","parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","id":"block:314e7db7-fb5e-40b6-ac10-a589ff3c809d","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Rule-based conflict resolver","updated_at":1773939024087,"created_at":1773939024052,"properties":{"sequence":157,"ID":"314e7db7-fb5e-40b6-ac10-a589ff3c809d"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.453149Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TZH4J0QXV31BV2BE2', 'block.created', 'block', 'block:655e2f77-d02e-4347-aa5f-dcd03ac140eb', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","created_at":1773939024052,"updated_at":1773939024087,"id":"block:655e2f77-d02e-4347-aa5f-dcd03ac140eb","content_type":"text","content":"Train classifier on logged conflicts","properties":{"ID":"655e2f77-d02e-4347-aa5f-dcd03ac140eb","sequence":158}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.453486Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TET6FT65GZ16W58W0', 'block.created', 'block', 'block:3bbdc016-4f08-49e4-b550-ba3d09a03933', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"created_at":1773939024052,"parent_id":"block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545","id":"block:3bbdc016-4f08-49e4-b550-ba3d09a03933","content":"Conflict resolution UI with reasoning display","properties":{"sequence":159,"ID":"3bbdc016-4f08-49e4-b550-ba3d09a03933"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.453825Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TEPJ08DEQHZC7RDEG', 'block.created', 'block', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"AI Trust Ladder [/]","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","updated_at":1773939024087,"content_type":"text","created_at":1773939024052,"id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","properties":{"sequence":160,"ID":"be9e6d6e-f995-4a27-bd5e-b2f70f12c93e"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.454159Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TTK649MPCHWJMS3AD', 'block.created', 'block', 'block:8a72f072-cc14-4e5f-987c-72bd27d94ced', 'sql', 'confirmed', '{"data":{"parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","content":"Level 3 (Agentic) with permission prompts","updated_at":1773939024087,"id":"block:8a72f072-cc14-4e5f-987c-72bd27d94ced","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024052,"properties":{"sequence":161,"ID":"8a72f072-cc14-4e5f-987c-72bd27d94ced"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.455226Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T96SP6G0CWQW4W6K9', 'block.created', 'block', 'block:c2289c19-1733-476e-9b50-43da1d70221f', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024052,"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","content":"Level 4 (Autonomous) for power users","id":"block:c2289c19-1733-476e-9b50-43da1d70221f","properties":{"sequence":162,"ID":"c2289c19-1733-476e-9b50-43da1d70221f"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.455582Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TBXKJPTKGJG4JCNM2', 'block.created', 'block', 'block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0', 'sql', 'confirmed', '{"data":{"id":"block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","content":"Level 2 (Advisory) features","parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"created_at":1773939024052,"content_type":"text","properties":{"ID":"c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","sequence":163}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.455929Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TW95BBC8995ZG0ETR', 'block.created', 'block', 'block:84706843-7132-4c12-a2ae-32fb7109982c', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Per-feature trust tracking","id":"block:84706843-7132-4c12-a2ae-32fb7109982c","parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","content_type":"text","created_at":1773939024053,"updated_at":1773939024087,"properties":{"sequence":164,"ID":"84706843-7132-4c12-a2ae-32fb7109982c"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.457360Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6THMHT3JXSAW7QF0CB', 'block.created', 'block', 'block:66b47313-a556-4628-954e-1da7fb1d402d', 'sql', 'confirmed', '{"data":{"content":"Trust level visualization UI","id":"block:66b47313-a556-4628-954e-1da7fb1d402d","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"created_at":1773939024053,"parent_id":"block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","properties":{"sequence":165,"ID":"66b47313-a556-4628-954e-1da7fb1d402d"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.457696Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T1PHTC5D881AS45XM', 'block.created', 'block', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Background Enrichment Agents [/]","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content_type":"text","id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","created_at":1773939024053,"updated_at":1773939024087,"properties":{"sequence":166,"ID":"d1e6541b-0c6b-4065-aea5-ad9057dc5bb5"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.458025Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TCBZRQJCTST50Z9HV', 'block.created', 'block', 'block:2618de83-3d90-4dc6-b586-98f95e351fb5', 'sql', 'confirmed', '{"data":{"content":"Infer likely token types from context","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024053,"parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","content_type":"text","id":"block:2618de83-3d90-4dc6-b586-98f95e351fb5","properties":{"sequence":167,"ID":"2618de83-3d90-4dc6-b586-98f95e351fb5"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.458775Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T1DJH3WWMS2TEK73C', 'block.created', 'block', 'block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec', 'sql', 'confirmed', '{"data":{"parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec","content":"Suggest dependencies between siblings","created_at":1773939024053,"content_type":"text","updated_at":1773939024087,"properties":{"ID":"edd212e6-16a9-4dfd-95f9-e2a2a3a55eec","sequence":168}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.459143Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TMP5PNCR8FWKRKW6W', 'block.created', 'block', 'block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"Suggest [[links]] for plain-text nouns (local LLM)","created_at":1773939024053,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","id":"block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda","content_type":"text","properties":{"sequence":169,"ID":"44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.459941Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TVHC25J4EZGGXWBAX', 'block.created', 'block', 'block:2ff960fa-38a4-42dd-8eb0-77e15c89659e', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","content_type":"text","id":"block:2ff960fa-38a4-42dd-8eb0-77e15c89659e","created_at":1773939024053,"content":"Classify tasks as question/delegation/action","properties":{"sequence":170,"ID":"2ff960fa-38a4-42dd-8eb0-77e15c89659e"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.460793Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T4KHBAMVJAPCVK68K', 'block.created', 'block', 'block:864527d2-65d4-4716-a65e-73a868c7e63b', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"Suggest via: routes for questions","created_at":1773939024053,"id":"block:864527d2-65d4-4716-a65e-73a868c7e63b","content_type":"text","parent_id":"block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":171,"ID":"864527d2-65d4-4716-a65e-73a868c7e63b"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.461127Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TJZEZKWH78XP2CRX4', 'block.created', 'block', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024053,"content":"The Integrator (Wholeness) [/]","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content_type":"text","id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","updated_at":1773939024087,"properties":{"sequence":172,"ID":"8a4a658e-d773-4528-8c61-ff3e5e425f47"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.461489Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T164NZH9AAGPGZHF1', 'block.created', 'block', 'block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","id":"block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","created_at":1773939024053,"content":"Smart linking suggestions","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","sequence":173}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.461818Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T2EW0TDN482NQP6GR', 'block.created', 'block', 'block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6', 'sql', 'confirmed', '{"data":{"content":"Context Bundle assembly for Flow mode","created_at":1773939024053,"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content_type":"text","id":"block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6","properties":{"sequence":174,"ID":"4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.462149Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TV0KJE6FVNGFJ9KJ5', 'block.created', 'block', 'block:7efa2454-274c-4304-8641-e3b8171c5b5a', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","content_type":"text","id":"block:7efa2454-274c-4304-8641-e3b8171c5b5a","content":"Cross-system deduplication","created_at":1773939024054,"properties":{"sequence":175,"ID":"7efa2454-274c-4304-8641-e3b8171c5b5a"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.462490Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T1EW11KQ40P60V2SG', 'block.created', 'block', 'block:311aa51c-88af-446f-8cb6-b791b9740665', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024054,"content":"Related item discovery","updated_at":1773939024087,"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","id":"block:311aa51c-88af-446f-8cb6-b791b9740665","properties":{"sequence":176,"ID":"311aa51c-88af-446f-8cb6-b791b9740665"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.462846Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TTT9BR4S36BFTZ72Y', 'block.created', 'block', 'block:9b6b2563-21b8-4286-9fac-dbdddc1a79be', 'sql', 'confirmed', '{"data":{"parent_id":"block:8a4a658e-d773-4528-8c61-ff3e5e425f47","content_type":"text","id":"block:9b6b2563-21b8-4286-9fac-dbdddc1a79be","content":"Automatic entity linking via embeddings","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024054,"updated_at":1773939024087,"properties":{"sequence":177,"ID":"9b6b2563-21b8-4286-9fac-dbdddc1a79be"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.463197Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T0219X1AV0CYD75KJ', 'block.created', 'block', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'sql', 'confirmed', '{"data":{"created_at":1773939024054,"id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","parent_id":"block:8b962d6c-0246-4119-8826-d517e2357f21","content":"The Watcher (Awareness) [/]","content_type":"text","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":178,"ID":"d385afbe-5bc9-4341-b879-6d14b8d763bc"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.463547Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TSHQGVY45WFK90AJX', 'block.created', 'block', 'block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024054,"content":"Risk and deadline tracking","id":"block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23","parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","content_type":"text","updated_at":1773939024087,"properties":{"sequence":179,"ID":"244abb7d-ef0f-4768-9e4e-b4bd7f3eec23"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.463902Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TWSJH08JJCCPTXF7Y', 'block.created', 'block', 'block:f9a2e27c-218f-402a-b405-b6b14b498bcf', 'sql', 'confirmed', '{"data":{"created_at":1773939024054,"content_type":"text","id":"block:f9a2e27c-218f-402a-b405-b6b14b498bcf","updated_at":1773939024087,"content":"Capacity analysis across all systems","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","properties":{"sequence":180,"ID":"f9a2e27c-218f-402a-b405-b6b14b498bcf"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.464241Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T1JMM1DAPZ36KDS4N', 'block.created', 'block', 'block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"Cross-system monitoring and alerts","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","id":"block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4","content_type":"text","created_at":1773939024054,"properties":{"sequence":181,"ID":"92d9dee2-3c16-4d14-9d54-1a93313ee1f4"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.464588Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TYYDN2JDCNJKK5NVH', 'block.created', 'block', 'block:e6c28ce7-c659-49e7-874b-334f05852cc4', 'sql', 'confirmed', '{"data":{"created_at":1773939024054,"parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","updated_at":1773939024087,"id":"block:e6c28ce7-c659-49e7-874b-334f05852cc4","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Daily/weekly synthesis for Orient mode","properties":{"sequence":182,"ID":"e6c28ce7-c659-49e7-874b-334f05852cc4"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.464928Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TW8R0E2GH3VV18N3A', 'block.created', 'block', 'block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024054,"id":"block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519","parent_id":"block:d385afbe-5bc9-4341-b879-6d14b8d763bc","updated_at":1773939024087,"content":"Dependency chain analysis","properties":{"ID":"1ffa7eb6-174a-4bed-85d2-9c47d9d55519","sequence":183}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.465276Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6THFXW575F2QVDMGXC', 'block.created', 'block', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773939024087,"parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","created_at":1773939024054,"content":"Phase 6: Flow Optimization [/]\\nGoal: Users achieve flow states regularly","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"c74fcc72-883d-4788-911a-0632f6145e4d","sequence":184}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.465610Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6TCZPN0FK8AS4NDFHY', 'block.created', 'block', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","created_at":1773939024054,"parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Self DT Work Rhythms [/]","content_type":"text","properties":{"sequence":185,"ID":"f908d928-db6f-495e-a941-22fcdfdba73a"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.465964Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6T2VD9HGHSEEQ74G3R', 'block.created', 'block', 'block:0570c0bf-84b4-4734-b6f3-25242a12a154', 'sql', 'confirmed', '{"data":{"id":"block:0570c0bf-84b4-4734-b6f3-25242a12a154","created_at":1773939024055,"content":"Emergent break suggestions from energy/focus dynamics","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","updated_at":1773939024087,"properties":{"sequence":186,"ID":"0570c0bf-84b4-4734-b6f3-25242a12a154"}}}', NULL, NULL, 1773939024090, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.466332Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VM7EK2D33GJX7JQXY', 'block.created', 'block', 'block:9d85cad6-1e74-499a-8d8e-899c5553c3d6', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Flow depth tracking with peripheral awareness alerts","id":"block:9d85cad6-1e74-499a-8d8e-899c5553c3d6","content_type":"text","created_at":1773939024055,"properties":{"ID":"9d85cad6-1e74-499a-8d8e-899c5553c3d6","sequence":187}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.466681Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V77KE63YPENXM1D8C', 'block.created', 'block', 'block:adc7803b-9318-4ca5-877b-83f213445aba', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Quick task suggestions during breaks (2-minute rule)","id":"block:adc7803b-9318-4ca5-877b-83f213445aba","content_type":"text","created_at":1773939024055,"updated_at":1773939024087,"parent_id":"block:f908d928-db6f-495e-a941-22fcdfdba73a","properties":{"sequence":188,"ID":"adc7803b-9318-4ca5-877b-83f213445aba"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.467019Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VFJCN2Y8TM7EY6G0N', 'block.created', 'block', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'sql', 'confirmed', '{"data":{"id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","created_at":1773939024055,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","content":"Three Modes [/]","content_type":"text","updated_at":1773939024087,"properties":{"ID":"b5771daa-0208-43fe-a890-ef1fcebf5f2f","sequence":189}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.467366Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V7Z149KX90K4QGC20', 'block.created', 'block', 'block:be15792f-21f3-476f-8b5f-e2e6b478b864', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"Orient mode (Watcher Dashboard, daily/weekly review)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:be15792f-21f3-476f-8b5f-e2e6b478b864","parent_id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","created_at":1773939024055,"content_type":"text","properties":{"sequence":190,"ID":"be15792f-21f3-476f-8b5f-e2e6b478b864"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.467708Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VE0JHV4WC94M9S2XE', 'block.created', 'block', 'block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content_type":"text","id":"block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde","parent_id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024055,"content":"Flow mode (single task focus, context on demand)","properties":{"sequence":191,"ID":"c68e8d5a-3f4b-4e8c-a887-2341e9b98bde"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.468064Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VVAXAT705TK0XVXQP', 'block.created', 'block', 'block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963', 'sql', 'confirmed', '{"data":{"parent_id":"block:b5771daa-0208-43fe-a890-ef1fcebf5f2f","content":"Capture mode (global hotkey, quick input overlay)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024055,"updated_at":1773939024087,"content_type":"text","id":"block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963","properties":{"sequence":192,"ID":"b1b2db9a-fc0d-4f51-98ae-9c5ab056a963"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.468422Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VNFPTXSW1SQJS922Z', 'block.created', 'block', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","content":"Review Workflows [/]","id":"block:a3e31c87-d10b-432e-987c-0371e730f753","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","created_at":1773939024055,"properties":{"sequence":193,"ID":"a3e31c87-d10b-432e-987c-0371e730f753"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.469195Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VZ5YDYW3PKSYQK3VX', 'block.created', 'block', 'block:4c020c67-1726-46d8-92e3-b9e0dbc90b62', 'sql', 'confirmed', '{"data":{"id":"block:4c020c67-1726-46d8-92e3-b9e0dbc90b62","created_at":1773939024055,"updated_at":1773939024087,"parent_id":"block:a3e31c87-d10b-432e-987c-0371e730f753","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Daily orientation (\\"What does today look like?\\")","properties":{"sequence":194,"ID":"4c020c67-1726-46d8-92e3-b9e0dbc90b62"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.469554Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V8MZYRPRRDBA5F0TH', 'block.created', 'block', 'block:0906f769-52eb-47a2-917a-f9b57b7e80d1', 'sql', 'confirmed', '{"data":{"content_type":"text","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:0906f769-52eb-47a2-917a-f9b57b7e80d1","content":"Inbox zero workflow","parent_id":"block:a3e31c87-d10b-432e-987c-0371e730f753","created_at":1773939024055,"properties":{"sequence":195,"ID":"0906f769-52eb-47a2-917a-f9b57b7e80d1"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.469931Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V5DGSE7BJKQEA3S81', 'block.created', 'block', 'block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024055,"updated_at":1773939024087,"content":"Weekly review (comprehensive synthesis)","id":"block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f","parent_id":"block:a3e31c87-d10b-432e-987c-0371e730f753","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":196,"ID":"091e7648-5314-4b4d-8e9c-bd7e0b8efc6f"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.470686Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V33XSK9TG4S0V6E59', 'block.created', 'block', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'sql', 'confirmed', '{"data":{"content":"Context Bundles in Flow [/]","id":"block:240acff4-cf06-445e-99ee-42040da1bb84","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024056,"content_type":"text","updated_at":1773939024087,"parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","properties":{"ID":"240acff4-cf06-445e-99ee-42040da1bb84","sequence":197}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.471044Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VEB1D5V8ABVTEM60W', 'block.created', 'block', 'block:90702048-5baf-4732-96fb-ddae16824257', 'sql', 'confirmed', '{"data":{"id":"block:90702048-5baf-4732-96fb-ddae16824257","parent_id":"block:240acff4-cf06-445e-99ee-42040da1bb84","updated_at":1773939024087,"content_type":"text","content":"Hide distractions, show progress","created_at":1773939024056,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"90702048-5baf-4732-96fb-ddae16824257","sequence":198}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.471422Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V6Z45AS4D0YKNSR0Y', 'block.created', 'block', 'block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","id":"block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130","parent_id":"block:240acff4-cf06-445e-99ee-42040da1bb84","content":"Slide-in context panel from edge","created_at":1773939024056,"properties":{"sequence":199,"ID":"e4aeb8f0-4c63-48f6-b745-92a89cfd4130"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.471790Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V29NRT31BTQ1ZND0T', 'block.created', 'block', 'block:3907168e-eaf8-48ee-8ccc-6dfef069371e', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:240acff4-cf06-445e-99ee-42040da1bb84","content_type":"text","content":"Assemble all related items for focused task","created_at":1773939024056,"updated_at":1773939024087,"id":"block:3907168e-eaf8-48ee-8ccc-6dfef069371e","properties":{"sequence":200,"ID":"3907168e-eaf8-48ee-8ccc-6dfef069371e"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.472559Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VKAECKNDTNBHH1CQ0', 'block.created', 'block', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Progressive Concealment [/]","created_at":1773939024056,"id":"block:e233124d-8711-4dd4-8153-c884f889bc07","parent_id":"block:c74fcc72-883d-4788-911a-0632f6145e4d","updated_at":1773939024087,"properties":{"sequence":201,"ID":"e233124d-8711-4dd4-8153-c884f889bc07"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.472920Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VXV4D4XNXS3WQ67H6', 'block.created', 'block', 'block:70485255-a2be-4356-bb9e-967270878b7e', 'sql', 'confirmed', '{"data":{"content":"Peripheral element dimming during sustained typing","created_at":1773939024056,"id":"block:70485255-a2be-4356-bb9e-967270878b7e","content_type":"text","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:e233124d-8711-4dd4-8153-c884f889bc07","properties":{"sequence":202,"ID":"70485255-a2be-4356-bb9e-967270878b7e"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.473290Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VZRJG68EBC0J81DGY', 'block.created', 'block', 'block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc', 'sql', 'confirmed', '{"data":{"parent_id":"block:e233124d-8711-4dd4-8153-c884f889bc07","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024056,"id":"block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc","content_type":"text","content":"Focused block emphasis, surrounding content fades","updated_at":1773939024087,"properties":{"sequence":203,"ID":"ea7f8d72-f963-4a51-ab4f-d10f981eafcc"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.473632Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V15E5ZH1Z19S9XEQR', 'block.created', 'block', 'block:30a71e2f-f070-4745-947d-c443a86a7149', 'sql', 'confirmed', '{"data":{"id":"block:30a71e2f-f070-4745-947d-c443a86a7149","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Automatic visibility restore on cursor movement","content_type":"text","parent_id":"block:e233124d-8711-4dd4-8153-c884f889bc07","created_at":1773939024056,"properties":{"ID":"30a71e2f-f070-4745-947d-c443a86a7149","sequence":204}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.473990Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VT1D3A2THXPZ2T874', 'block.created', 'block', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'sql', 'confirmed', '{"data":{"content":"Phase 7: Team Features [/]\\nGoal: Teams leverage individual excellence","content_type":"text","created_at":1773939024056,"parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":205,"ID":"4c647dfe-0639-4064-8ab6-491d57c7e367"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.474765Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V51A2TR5M1TK14WHR', 'block.created', 'block', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'sql', 'confirmed', '{"data":{"id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","created_at":1773939024056,"parent_id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Delegation System [/]","updated_at":1773939024087,"properties":{"sequence":206,"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.475493Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VTM8R96FBJ3VW1GJN', 'block.created', 'block', 'block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","id":"block:15c4b164-b29f-4fb0-b882-e6408f2e3264","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"@[[Person]]: syntax for delegation sub-nets","created_at":1773939024056,"content_type":"text","properties":{"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264","sequence":207}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.476282Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VX3SAQA2RS7Q7CZCJ', 'block.created', 'block', 'block:fbbce845-023e-438b-963e-471833c51505', 'sql', 'confirmed', '{"data":{"content":"Waiting-for tracking (automatic from delegation patterns)","content_type":"text","parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:fbbce845-023e-438b-963e-471833c51505","created_at":1773939024057,"updated_at":1773939024087,"properties":{"ID":"fbbce845-023e-438b-963e-471833c51505","sequence":208}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.476659Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V9FVYQCVXXKHFHHMH', 'block.created', 'block', 'block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0', 'sql', 'confirmed', '{"data":{"id":"block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0","content_type":"text","updated_at":1773939024087,"parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Delegation status sync with external systems","created_at":1773939024057,"properties":{"sequence":209,"ID":"25e19c99-63c2-4edb-8fb1-deb1daf4baf0"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.477530Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VE8SAT6GG26YXE3F4', 'block.created', 'block', 'block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"@anyone: team pool transitions","content_type":"text","created_at":1773939024057,"id":"block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc","parent_id":"block:8cf3b868-2970-4d45-93e5-8bca58e3bede","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":210,"ID":"938f03b8-6129-4eda-9c5f-31a76ad8b8dc"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.477913Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VE9MMEA693EJTWY8C', 'block.created', 'block', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'sql', 'confirmed', '{"data":{"created_at":1773939024057,"updated_at":1773939024087,"parent_id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","content":"Sharing & Collaboration [/]","content_type":"text","id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":211,"ID":"5bdf3ba6-f617-4bc1-93c2-15d84d925e01"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.478280Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VC36FAPHNSZVC7AZT', 'block.created', 'block', 'block:88b467b1-5a46-4b64-acb3-fcf9f377030e', 'sql', 'confirmed', '{"data":{"parent_id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","created_at":1773939024057,"id":"block:88b467b1-5a46-4b64-acb3-fcf9f377030e","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Collaborative editing","updated_at":1773939024087,"properties":{"sequence":212,"ID":"88b467b1-5a46-4b64-acb3-fcf9f377030e"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.478649Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V5S8PD5BNTBRF9390', 'block.created', 'block', 'block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da', 'sql', 'confirmed', '{"data":{"parent_id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","updated_at":1773939024087,"content":"Shared views and dashboards","content_type":"text","id":"block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024057,"properties":{"ID":"f3ce62cd-5817-4a7c-81f6-7a7077aff7da","sequence":213}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.479010Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VFK3P7TM9HQ3SSNEG', 'block.created', 'block', 'block:135c74b1-8341-4719-b5d1-492eb26e2189', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content":"Read-only sharing for documentation","created_at":1773939024057,"parent_id":"block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:135c74b1-8341-4719-b5d1-492eb26e2189","content_type":"text","properties":{"sequence":214,"ID":"135c74b1-8341-4719-b5d1-492eb26e2189"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T16:50:24.481366Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VQXFYEXPFBYFS61AM', 'block.created', 'block', 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'sql', 'confirmed', '{"data":{"content":"Competition analysis","created_at":1773939024057,"parent_id":"block:4c647dfe-0639-4064-8ab6-491d57c7e367","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content_type":"text","id":"block:e0f90f1e-5468-4229-9b6d-438b31f09ed6","properties":{"sequence":215,"ID":"e0f90f1e-5468-4229-9b6d-438b31f09ed6"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.481725Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VGKPJNNACAG6DR4NY', 'block.created', 'block', 'block:ceb203d0-0b59-4aa0-a840-2e4763234112', 'sql', 'confirmed', '{"data":{"created_at":1773939024057,"updated_at":1773939024087,"parent_id":"block:e0f90f1e-5468-4229-9b6d-438b31f09ed6","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:ceb203d0-0b59-4aa0-a840-2e4763234112","content":"https://github.com/3xpyth0n/ideon\\nOrganize repositories, notes, links and more on a shared infinite canvas.","properties":{"ID":"ceb203d0-0b59-4aa0-a840-2e4763234112","sequence":216}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.482073Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VWNYK18AR0A6F24ZS', 'block.created', 'block', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'sql', 'confirmed', '{"data":{"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Cross-Cutting Concerns [/]","created_at":1773939024057,"id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"properties":{"ID":"f407a7ec-c924-4a38-96e0-7e73472e7353","sequence":217}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.482456Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VM2FGYPMQ0MFQSNA2', 'block.created', 'block', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'sql', 'confirmed', '{"data":{"content_type":"text","id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","created_at":1773939024057,"content":"Privacy & Security [/]","properties":{"sequence":218,"ID":"ad1d8307-134f-4a34-b58e-07d6195b2466"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.482819Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V4NPQFHHQH835VPXB', 'block.created', 'block', 'block:717db234-61eb-41ef-a8bf-b67e870f9aa6', 'sql', 'confirmed', '{"data":{"parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","created_at":1773939024057,"updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Plugin sandboxing (WASM)","id":"block:717db234-61eb-41ef-a8bf-b67e870f9aa6","content_type":"text","properties":{"sequence":219,"ID":"717db234-61eb-41ef-a8bf-b67e870f9aa6"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.483168Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VB4161DGYPZ4XMSYX', 'block.created', 'block', 'block:75604518-b736-4653-a2a3-941215e798c7', 'sql', 'confirmed', '{"data":{"parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","created_at":1773939024058,"id":"block:75604518-b736-4653-a2a3-941215e798c7","content":"Self-hosted LLM option (Ollama/vLLM)","updated_at":1773939024087,"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":220,"ID":"75604518-b736-4653-a2a3-941215e798c7"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.483519Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VYWYB0668C8Z132GJ', 'block.created', 'block', 'block:bfaedc82-3bc7-4b16-8314-273721ea997f', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"created_at":1773939024058,"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:bfaedc82-3bc7-4b16-8314-273721ea997f","parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","content":"Optional cloud LLM with explicit consent","properties":{"sequence":221,"ID":"bfaedc82-3bc7-4b16-8314-273721ea997f"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.483891Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VW0J3CTZ1DK4Z1B5R', 'block.created', 'block', 'block:4b96f182-61e5-4f0e-861d-1a7d2413abe7', 'sql', 'confirmed', '{"data":{"created_at":1773939024058,"content_type":"text","id":"block:4b96f182-61e5-4f0e-861d-1a7d2413abe7","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Local-first by default (all data on device)","parent_id":"block:ad1d8307-134f-4a34-b58e-07d6195b2466","updated_at":1773939024087,"properties":{"sequence":222,"ID":"4b96f182-61e5-4f0e-861d-1a7d2413abe7"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.484256Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VW3GNKTX2RBN0N7D0', 'block.created', 'block', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","updated_at":1773939024087,"content_type":"text","content":"Petri-Net Advanced [/]","created_at":1773939024058,"id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","properties":{"sequence":223,"ID":"eac105ca-efda-4976-9856-6c39a9b1502e"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.484617Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V72E6FE20C0K6DMT1', 'block.created', 'block', 'block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59', 'sql', 'confirmed', '{"data":{"content_type":"text","content":"SOP extraction from repeated interaction patterns","updated_at":1773939024087,"created_at":1773939024058,"parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59","properties":{"sequence":224,"ID":"0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.484982Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VG9CSRX5TCN1RN4V0', 'block.created', 'block', 'block:143d071e-2b90-4f93-98d3-7aa5d3a14933', 'sql', 'confirmed', '{"data":{"parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","content":"Delegation sub-nets (waiting_for pattern)","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024058,"updated_at":1773939024087,"content_type":"text","id":"block:143d071e-2b90-4f93-98d3-7aa5d3a14933","properties":{"ID":"143d071e-2b90-4f93-98d3-7aa5d3a14933","sequence":225}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.485973Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V6VH23QBA599H9K27', 'block.created', 'block', 'block:cc499de0-f953-4f41-b795-0864b366d8ab', 'sql', 'confirmed', '{"data":{"id":"block:cc499de0-f953-4f41-b795-0864b366d8ab","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content_type":"text","content":"Token type hierarchy with mixins","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","created_at":1773939024058,"properties":{"sequence":226,"ID":"cc499de0-f953-4f41-b795-0864b366d8ab"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.486341Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VS696M3E6BY7T789Z', 'block.created', 'block', 'block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb', 'sql', 'confirmed', '{"data":{"id":"block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb","created_at":1773939024058,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","content":"Projections as views on flat net (Kanban, SOP, pipeline)","updated_at":1773939024087,"properties":{"sequence":227,"ID":"bd99d866-66ed-4474-8a4d-7ac1c1b08fbb"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.486737Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V6807MVR8R8NB3S2B', 'block.created', 'block', 'block:4041eb2e-23a6-4fea-9a69-0c152a6311e8', 'sql', 'confirmed', '{"data":{"content":"Question/Information tokens with confidence tracking","content_type":"text","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","updated_at":1773939024087,"created_at":1773939024058,"id":"block:4041eb2e-23a6-4fea-9a69-0c152a6311e8","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"4041eb2e-23a6-4fea-9a69-0c152a6311e8","sequence":228}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.487115Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VCMHA52RRFS57WHW8', 'block.created', 'block', 'block:1e1027d2-4c0f-4975-ba59-c3c601d1f661', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024058,"content":"Simulation engine (fork marking, compare scenarios)","parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","id":"block:1e1027d2-4c0f-4975-ba59-c3c601d1f661","properties":{"ID":"1e1027d2-4c0f-4975-ba59-c3c601d1f661","sequence":229}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.487473Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V2S9WAM5WWW9H0MP9', 'block.created', 'block', 'block:a80f6d58-c876-48f5-8bfe-69390a8f9bde', 'sql', 'confirmed', '{"data":{"id":"block:a80f6d58-c876-48f5-8bfe-69390a8f9bde","created_at":1773939024059,"parent_id":"block:eac105ca-efda-4976-9856-6c39a9b1502e","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Browser plugin for web app Digital Twins","properties":{"ID":"a80f6d58-c876-48f5-8bfe-69390a8f9bde","sequence":230}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.487839Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VGBWYKX2MZBCZYNFJ', 'block.created', 'block', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'sql', 'confirmed', '{"data":{"content":"PRQL Automation [/]","updated_at":1773939024087,"parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","created_at":1773939024059,"id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"ID":"723a51a9-3861-429c-bb10-f73c01f8463d","sequence":231}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.488216Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VD9ED3HZAKDSN0A84', 'block.created', 'block', 'block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","id":"block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024059,"content":"Cross-system status propagation rules","properties":{"sequence":232,"ID":"e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.488579Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VGRM9VM9NHPSHM97A', 'block.created', 'block', 'block:c1338a15-080b-4dba-bbdc-87b6b8467f28', 'sql', 'confirmed', '{"data":{"id":"block:c1338a15-080b-4dba-bbdc-87b6b8467f28","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Auto-tag blocks based on content analysis","created_at":1773939024059,"content_type":"text","parent_id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","properties":{"sequence":233,"ID":"c1338a15-080b-4dba-bbdc-87b6b8467f28"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.488960Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V37CZ8HADNEE9NT9Z', 'block.created', 'block', 'block:5707965a-6578-443c-aeff-bf40170edea9', 'sql', 'confirmed', '{"data":{"created_at":1773939024059,"content_type":"text","id":"block:5707965a-6578-443c-aeff-bf40170edea9","content":"PRQL-based automation rules (query → action)","parent_id":"block:723a51a9-3861-429c-bb10-f73c01f8463d","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"properties":{"sequence":234,"ID":"5707965a-6578-443c-aeff-bf40170edea9"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.489321Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VC8VH5BBFQWQZYJJC', 'block.created', 'block', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'sql', 'confirmed', '{"data":{"id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","updated_at":1773939024087,"content":"Platform Support [/]","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","created_at":1773939024059,"properties":{"sequence":235,"ID":"8e2b4ddd-e428-4950-bc41-76ee8a0e27ce"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.489693Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VVXGWZS5RHT25H8MH', 'block.created', 'block', 'block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Android mobile","updated_at":1773939024087,"created_at":1773939024059,"content_type":"text","id":"block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","properties":{"ID":"4c4ff372-c3b9-44e6-9d46-33b7a4e7882e","sequence":236}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.490049Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VE2K1N8MVNDBBK4R2', 'block.created', 'block', 'block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857', 'sql', 'confirmed', '{"data":{"content":"WASM compatibility (MaybeSendSync trait)","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024087,"created_at":1773939024059,"id":"block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857","properties":{"sequence":237,"ID":"e5b9db2d-f39a-439d-99f8-b4e7c4ff6857"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.490417Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V93HE6BYHERW45FRY', 'block.created', 'block', 'block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db', 'sql', 'confirmed', '{"data":{"parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","id":"block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db","created_at":1773939024059,"updated_at":1773939024087,"content":"Windows desktop","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"sequence":238,"ID":"d61290d4-e1f6-41e7-89e0-a7ed7a6662db"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.490778Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V2BCPSB5FWSFBQ7E2', 'block.created', 'block', 'block:1e729eef-3fff-43cb-8d13-499a8a8d4203', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","id":"block:1e729eef-3fff-43cb-8d13-499a8a8d4203","content":"iOS mobile","content_type":"text","updated_at":1773939024087,"created_at":1773939024059,"properties":{"sequence":239,"ID":"1e729eef-3fff-43cb-8d13-499a8a8d4203"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.491139Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V8EMXGVQS06THGB3A', 'block.created', 'block', 'block:500b7aae-5c3b-4dd5-a3c8-373fe746990b', 'sql', 'confirmed', '{"data":{"created_at":1773939024059,"content":"Linux desktop","updated_at":1773939024087,"id":"block:500b7aae-5c3b-4dd5-a3c8-373fe746990b","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"500b7aae-5c3b-4dd5-a3c8-373fe746990b","sequence":240}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.491920Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6V54EQC6PNF53KF7TY', 'block.created', 'block', 'block:a79ab251-4685-4728-b98b-0a652774f06c', 'sql', 'confirmed', '{"data":{"content":"macOS desktop (Flutter)","updated_at":1773939024087,"id":"block:a79ab251-4685-4728-b98b-0a652774f06c","parent_id":"block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce","created_at":1773939024060,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"ID":"a79ab251-4685-4728-b98b-0a652774f06c","sequence":241}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.492295Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VBD3JTA7XQ2EETKY5', 'block.created', 'block', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"parent_id":"block:f407a7ec-c924-4a38-96e0-7e73472e7353","created_at":1773939024060,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"UI/UX Design System [/]","id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","content_type":"text","properties":{"ID":"ac137431-daf6-4741-9808-6dc71c13e7c6","sequence":242}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.492655Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VAZZA25HM3W6V55M6', 'block.created', 'block', 'block:a85de368-9546-446d-ad61-17b72c7dbc3e', 'sql', 'confirmed', '{"data":{"created_at":1773939024060,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:a85de368-9546-446d-ad61-17b72c7dbc3e","content":"Which-Key navigation system (Space → mnemonic keys)","content_type":"text","updated_at":1773939024087,"parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","properties":{"sequence":243,"ID":"a85de368-9546-446d-ad61-17b72c7dbc3e"}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.493046Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6VEB1TJCX8DYVG1QYH', 'block.created', 'block', 'block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9","content_type":"text","content":"Micro-interactions (checkbox animation, smooth reorder)","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","updated_at":1773939024087,"created_at":1773939024060,"properties":{"ID":"1cea6bd3-680f-46c3-bdbc-5989da5ed7d9","sequence":244}}}', NULL, NULL, 1773939024091, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.493923Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WY0RQXNGXM4ZNGWWK', 'block.created', 'block', 'block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"created_at":1773939024060,"parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3","content":"Light and dark themes","content_type":"text","properties":{"sequence":245,"ID":"d1fbee2c-3a11-4adc-a3db-fd93f5b117e3"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.494273Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WTKH4SJSGX1Q2BARJ', 'block.created', 'block', 'block:beeec959-ba87-4c57-9531-c1d7f24d2b2c', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","content_type":"text","id":"block:beeec959-ba87-4c57-9531-c1d7f24d2b2c","created_at":1773939024060,"content":"Color palette (warm, professional, calm technology)","properties":{"sequence":246,"ID":"beeec959-ba87-4c57-9531-c1d7f24d2b2c"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.494613Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WPA1WW80G2C252FQT', 'block.created', 'block', 'block:d36014da-518a-4da5-b360-218d027ee104', 'sql', 'confirmed', '{"data":{"created_at":1773939024060,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","content":"Typography system (Inter + JetBrains Mono)","updated_at":1773939024087,"id":"block:d36014da-518a-4da5-b360-218d027ee104","parent_id":"block:ac137431-daf6-4741-9808-6dc71c13e7c6","properties":{"sequence":247,"ID":"d36014da-518a-4da5-b360-218d027ee104"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.494958Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WEQZDFDF61CH9YMFQ', 'block.created', 'block', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'sql', 'confirmed', '{"data":{"content_type":"text","created_at":1773939024060,"id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","parent_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"LogSeq replacement","properties":{"sequence":248,"ID":"01806047-9cf8-42fe-8391-6d608bfade9e"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.495301Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WVKVGDD9N7X0XHA8H', 'block.created', 'block', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'sql', 'confirmed', '{"data":{"content":"Editing experience","created_at":1773939024060,"updated_at":1773939024087,"id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","parent_id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","content_type":"text","properties":{"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","sequence":249}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- Wait 1ms
-- [transaction_stmt] 2026-03-19T16:50:24.496806Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6W427GQDSB0C9W666Z', 'block.created', 'block', 'block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","id":"block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2","content":"GitHub Flavored Markdown parser & renderer for GPUI\\nhttps://github.com/joris-gallot/gpui-gfm","content_type":"text","parent_id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","created_at":1773939024060,"updated_at":1773939024087,"properties":{"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2","sequence":250}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.497174Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WMB0PT7ZY1CFCRGM8', 'block.created', 'block', 'block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"id":"block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8","created_at":1773939024060,"parent_id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Desktop Markdown viewer built with Rust and GPUI\\nhttps://github.com/chunghha/markdown_viewer","properties":{"sequence":251,"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- Wait 2ms
-- [transaction_stmt] 2026-03-19T16:50:24.499613Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WNTBCAZ3YXNNF1MW4', 'block.created', 'block', 'block:f7730a68-6268-4e65-ac93-3fdf79e92133', 'sql', 'confirmed', '{"data":{"parent_id":"block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content":"Markdown Editor and Viewer\\nhttps://github.com/kumarUjjawal/aster","content_type":"text","id":"block:f7730a68-6268-4e65-ac93-3fdf79e92133","created_at":1773939024061,"updated_at":1773939024087,"properties":{"sequence":252,"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.499967Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6W82MVWYKW5450QT66', 'block.created', 'block', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"created_at":1773939024061,"parent_id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","content_type":"text","id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","content":"PDF Viewer & Annotator","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","properties":{"ID":"8594ab7c-5f36-44cf-8f92-248b31508441","sequence":253}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.500315Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6W6MXW7QPH010GAWBT', 'block.created', 'block', 'block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"id":"block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c","parent_id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","content_type":"text","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","created_at":1773939024061,"content":"Combining gpui and hayro for a little application that render pdfs\\nhttps://github.com/vincenthz/gpui-hayro?tab=readme-ov-file","properties":{"sequence":254,"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.500658Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6W4PX629657EW8JY1S', 'block.created', 'block', 'block:b95a19a6-5448-42f0-af06-177e95e27f49', 'sql', 'confirmed', '{"data":{"updated_at":1773939024087,"created_at":1773939024061,"content":"Libera Reader\\nModern, performance-oriented desktop e-book reader built with Rust and GPUI.\\nhttps://github.com/RikaKit2/libera-reader","parent_id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","id":"block:b95a19a6-5448-42f0-af06-177e95e27f49","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"sequence":255,"ID":"b95a19a6-5448-42f0-af06-177e95e27f49"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.501035Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WRC6N4H4CYD21ESVH', 'block.created', 'block', 'block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 'sql', 'confirmed', '{"data":{"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","updated_at":1773939024087,"id":"block:812924a9-0bc2-41a7-8820-1c60a40bd1ad","created_at":1773939024061,"content":"Monica: On-screen anotation software\\nhttps://github.com/tasuren/monica","parent_id":"block:8594ab7c-5f36-44cf-8f92-248b31508441","properties":{"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad","sequence":256}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.501802Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6W2N9NVT8MPTZZCY0Z', 'block.created', 'block', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', 'sql', 'confirmed', '{"data":{"created_at":1773939024061,"content_type":"text","parent_id":"block:01806047-9cf8-42fe-8391-6d608bfade9e","id":"block:419b2df8-0121-4532-8dcd-21f04df806d8","document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","updated_at":1773939024087,"content":"Graph vis","properties":{"sequence":257,"ID":"419b2df8-0121-4532-8dcd-21f04df806d8"}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- [transaction_stmt] 2026-03-19T16:50:24.502641Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES ('01KM3G2R6WAXG3MPFWKW069RK2', 'block.created', 'block', 'block:f520a9ff-71bf-4a72-8777-9864bad7c535', 'sql', 'confirmed', '{"data":{"parent_id":"block:419b2df8-0121-4532-8dcd-21f04df806d8","content":"https://github.com/jerlendds/gpug","updated_at":1773939024087,"id":"block:f520a9ff-71bf-4a72-8777-9864bad7c535","created_at":1773939024061,"document_id":"doc:b5af8c53-ac31-420d-be58-3fb35a999916","content_type":"text","properties":{"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535","sequence":258}}}', NULL, NULL, 1773939024092, NULL, NULL);

-- Wait 44ms
-- [actor_query] 2026-03-19T16:50:24.547260Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_3b8f070830f6b4d1';

-- [actor_query] 2026-03-19T16:50:24.547573Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_3b8f070830f6b4d1';

-- [actor_query] 2026-03-19T16:50:24.547808Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_3b8f070830f6b4d1';

-- [actor_ddl] 2026-03-19T16:50:24.548188Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_3b8f070830f6b4d1 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:root-layout' OR parent_id = 'block:root-layout';

-- Wait 12ms
-- [transaction_stmt] 2026-03-19T16:50:24.560368Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 1: Core Outliner', 'text', NULL, NULL, '{"sequence":0,"ID":"599b60af-960d-4c9c-b222-d3d9de95c513"}', 1773939024037, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.561143Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'MCP Server Frontend [/]', 'text', NULL, NULL, '{"task_state":"DOING","ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb","sequence":1}', 1773939024037, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.561608Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:db59d038-8a47-43e9-9502-0472b493a6b9', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Context parameter support ($context_id, $context_parent_id)', 'text', NULL, NULL, '{"ID":"db59d038-8a47-43e9-9502-0472b493a6b9","sequence":2}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.562045Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:95ad6166-c03c-4417-a435-349e88b8e90a', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'MCP server (stdio + HTTP modes)', 'text', NULL, NULL, '{"sequence":3,"ID":"95ad6166-c03c-4417-a435-349e88b8e90a"}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.562464Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'MCP tools for query execution and operations', 'text', NULL, NULL, '{"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b","sequence":4}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.562892Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:661368d9-e4bd-4722-b5c2-40f32006c643', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block Operations [/]', 'text', NULL, NULL, '{"ID":"661368d9-e4bd-4722-b5c2-40f32006c643","sequence":5}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.563302Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block hierarchy (parent/child, indent/outdent)', 'text', NULL, NULL, '{"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007","sequence":6}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.563718Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Split block operation', 'text', NULL, NULL, '{"sequence":7,"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34"}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.564127Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block CRUD (create, read, update, delete)', 'text', NULL, NULL, '{"sequence":8,"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb"}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.564540Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block movement (move_up, move_down, move_block)', 'text', NULL, NULL, '{"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63","sequence":9}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.565147Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:225edb45-f670-445a-9162-18c150210ee6', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Undo/redo system (UndoStack + persistent OperationLogStore)', 'text', NULL, NULL, '{"task_state":"TODO","ID":"225edb45-f670-445a-9162-18c150210ee6","sequence":10}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.565558Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Storage & Data Layer [/]', 'text', NULL, NULL, '{"ID":"444b24f6-d412-43c4-a14b-6e725b673cee","sequence":11}', 1773939024038, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.565967Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Schema Module system with topological dependency ordering', 'text', NULL, NULL, '{"sequence":12,"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.566382Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Fractional indexing for block ordering', 'text', NULL, NULL, '{"sequence":13,"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.566777Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Turso (embedded SQLite) backend with connection pooling', 'text', NULL, NULL, '{"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57","sequence":14}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.567223Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'QueryableCache wrapping DataSource with local caching', 'text', NULL, NULL, '{"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280","sequence":15}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.567763Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Entity derive macro (#[derive(Entity)]) for schema generation', 'text', NULL, NULL, '{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.568185Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d318cae4-759d-487b-a909-81940223ecc1', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'CDC (Change Data Capture) streaming from storage to UI', 'text', NULL, NULL, '{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.568577Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Command sourcing infrastructure (append-only operation log)', 'text', NULL, NULL, '{"task_state":"DONE","sequence":18,"ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.568937Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Procedural Macros [/]', 'text', NULL, NULL, '{"sequence":19,"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.569288Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '#[operations_trait] macro for operation dispatch generation', 'text', NULL, NULL, '{"sequence":20,"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.569641Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '#[triggered_by(...)] for operation availability', 'text', NULL, NULL, '{"sequence":21,"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44"}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.569990Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Type inference for OperationDescriptor parameters', 'text', NULL, NULL, '{"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd","sequence":22}', 1773939024039, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.570333Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '#[affects(...)] for field-level reactivity', 'text', NULL, NULL, '{"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2","sequence":23}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.570671Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Performance [/]', 'text', NULL, NULL, '{"sequence":24,"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a"}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.571014Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Virtual scrolling and lazy loading', 'text', NULL, NULL, '{"ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73","task_state":"DOING","sequence":25}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.571361Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Connection pooling for Turso', 'text', NULL, NULL, '{"sequence":26,"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","task_state":"DOING"}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.571700Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e0567a06-5a62-4957-9457-c55a6661cee5', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Full-text search indexing (Tantivy)', 'text', NULL, NULL, '{"ID":"e0567a06-5a62-4957-9457-c55a6661cee5","sequence":27}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.572033Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-Device Sync [/]', 'text', NULL, NULL, '{"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","sequence":28}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.572371Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:43f329da-cfb4-4764-b599-06f4b6272f91', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'CollaborativeDoc with ALPN routing', 'text', NULL, NULL, '{"ID":"43f329da-cfb4-4764-b599-06f4b6272f91","sequence":29}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.572733Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7aef40b2-14e1-4df0-a825-18603c55d198', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Offline-first with background sync', 'text', NULL, NULL, '{"ID":"7aef40b2-14e1-4df0-a825-18603c55d198","sequence":30}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.573075Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e148d7b7-c505-4201-83b7-36986a981a56', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Iroh P2P transport for Loro documents', 'text', NULL, NULL, '{"sequence":31,"ID":"e148d7b7-c505-4201-83b7-36986a981a56"}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.573408Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Dependency Injection [/]', 'text', NULL, NULL, '{"sequence":32,"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9"}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.573782Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b980e51f-0c91-4708-9a17-3d41284974b2', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'OperationDispatcher routing to providers', 'text', NULL, NULL, '{"ID":"b980e51f-0c91-4708-9a17-3d41284974b2","sequence":33}', 1773939024040, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.574127Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:97cc8506-47d2-44cb-bdca-8e9a507953a0', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'BackendEngine as main orchestration point', 'text', NULL, NULL, '{"sequence":34,"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.574489Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'ferrous-di based service composition', 'text', NULL, NULL, '{"sequence":35,"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.574860Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'SchemaRegistry with topological initialization', 'text', NULL, NULL, '{"sequence":36,"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.575215Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Query & Render Pipeline [/]', 'text', NULL, NULL, '{"sequence":37,"ID":"b489c622-6c87-4bf6-8d35-787eb732d670"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.575735Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1bbec456-7217-4477-a49c-0b8422e441e9', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)', 'text', NULL, NULL, '{"ID":"1bbec456-7217-4477-a49c-0b8422e441e9","sequence":38}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.576097Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2b1c341e-5da2-4207-a609-f4af6d7ceebd', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Automatic operation wiring (lineage analysis → widget binding)', 'text', NULL, NULL, '{"sequence":39,"ID":"2b1c341e-5da2-4207-a609-f4af6d7ceebd","task_state":"DOING"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.576455Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2d44d7df-5d7d-4cfe-9061-459c7578e334', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'GQL (graph query) support via EAV schema', 'text', NULL, NULL, '{"sequence":40,"ID":"2d44d7df-5d7d-4cfe-9061-459c7578e334","task_state":"DOING"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.576824Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:54ed1be5-765e-4884-87ab-02268e0208c7', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'PRQL compilation (PRQL → SQL + RenderSpec)', 'text', NULL, NULL, '{"sequence":41,"ID":"54ed1be5-765e-4884-87ab-02268e0208c7"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.577327Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5384c1da-f058-4321-8401-929b3570c2a5', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'RenderSpec tree for declarative UI description', 'text', NULL, NULL, '{"ID":"5384c1da-f058-4321-8401-929b3570c2a5","sequence":42}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.578313Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fcf071b3-01f2-4d1d-882b-9f6a34c81bbc', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Unified execute_query supporting PRQL/GQL/SQL', 'text', NULL, NULL, '{"task_state":"DONE","ID":"fcf071b3-01f2-4d1d-882b-9f6a34c81bbc","sequence":43}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.578710Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'SQL direct execution support', 'text', NULL, NULL, '{"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","sequence":44,"task_state":"DOING"}', 1773939024041, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.579095Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Loro CRDT Integration [/]', 'text', NULL, NULL, '{"ID":"d9374dc3-05fc-40b2-896d-f88bb8a33c92","sequence":45}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.579471Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b1dc3ad3-574b-472a-b74b-e3ea29a433e6', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LoroBackend implementing CoreOperations trait', 'text', NULL, NULL, '{"ID":"b1dc3ad3-574b-472a-b74b-e3ea29a433e6","sequence":46}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.579986Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ce2986c5-51a2-4d1e-9b0d-6ab9123cc957', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LoroDocumentStore for managing CRDT documents on disk', 'text', NULL, NULL, '{"task_state":"DOING","sequence":47,"ID":"ce2986c5-51a2-4d1e-9b0d-6ab9123cc957"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.580390Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:35652c3f-720c-4e20-ab90-5e25e1429733', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LoroBlockOperations as OperationProvider routing writes through CRDT', 'text', NULL, NULL, '{"sequence":48,"ID":"35652c3f-720c-4e20-ab90-5e25e1429733"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.580770Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:090731e3-38ae-4bf1-b5ec-dbb33eae4fb2', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cycle detection in move_block', 'text', NULL, NULL, '{"ID":"090731e3-38ae-4bf1-b5ec-dbb33eae4fb2","sequence":49}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.581143Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ddf208e4-9b73-422d-b8ab-4ec58b328907', 'block:d9374dc3-05fc-40b2-896d-f88bb8a33c92', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Loro-to-Turso materialization (CRDT → SQL cache → CDC)', 'text', NULL, NULL, '{"sequence":50,"ID":"ddf208e4-9b73-422d-b8ab-4ec58b328907"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.581548Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Org-Mode Sync [/]', 'text', NULL, NULL, '{"sequence":51,"ID":"9af3a008-c1d7-422b-a1c8-e853f3ccb6fa"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.581934Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7bc5f362-0bf9-45a1-b2b7-6882585ed169', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'OrgRenderer as single path for producing org text', 'text', NULL, NULL, '{"sequence":52,"ID":"7bc5f362-0bf9-45a1-b2b7-6882585ed169"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.582318Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8eab3453-25d2-4e7a-89f8-f9f79be939c9', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Document identity & aliases (UUID ↔ file path mapping)', 'text', NULL, NULL, '{"sequence":53,"ID":"8eab3453-25d2-4e7a-89f8-f9f79be939c9"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.582730Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fc60da1b-6065-4d36-8551-5479ff145df0', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'OrgSyncController with echo suppression', 'text', NULL, NULL, '{"sequence":54,"ID":"fc60da1b-6065-4d36-8551-5479ff145df0"}', 1773939024042, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.583124Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6e5a1157-b477-45a1-892f-57807b4d969b', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Bidirectional sync (file changes ↔ block changes)', 'text', NULL, NULL, '{"sequence":55,"ID":"6e5a1157-b477-45a1-892f-57807b4d969b"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.583528Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6e4dab75-cd13-4c5e-9168-bf266d11aa3f', 'block:9af3a008-c1d7-422b-a1c8-e853f3ccb6fa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Org file parsing (headlines, properties, source blocks)', 'text', NULL, NULL, '{"sequence":56,"ID":"6e4dab75-cd13-4c5e-9168-bf266d11aa3f"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.583937Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Flutter Frontend [/]', 'text', NULL, NULL, '{"ID":"bb3bc716-ca9a-438a-936d-03631e2ee929","sequence":57}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.584343Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b4753cd8-47ea-4f7d-bd00-e1ec563aa43f', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'FFI bridge via flutter_rust_bridge', 'text', NULL, NULL, '{"sequence":58,"ID":"b4753cd8-47ea-4f7d-bd00-e1ec563aa43f"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.584724Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3289bc82-f8a9-4cad-8545-ad1fee9dc282', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Navigation system (history, cursor, focus)', 'text', NULL, NULL, '{"ID":"3289bc82-f8a9-4cad-8545-ad1fee9dc282","sequence":59,"task_state":"DOING"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.585114Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ebca0a24-f6f6-4c49-8a27-9d9973acf737', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block editor (outliner interactions)', 'text', NULL, NULL, '{"sequence":60,"ID":"ebca0a24-f6f6-4c49-8a27-9d9973acf737"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.585494Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eb7e34f8-19f5-48f5-a22d-8f62493bafdd', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Reactive UI updates from CDC change streams', 'text', NULL, NULL, '{"ID":"eb7e34f8-19f5-48f5-a22d-8f62493bafdd","sequence":61}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.586036Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7a0a4905-59c5-4277-8114-1e9ca9d425e3', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Three-column layout (sidebar, main, right panel)', 'text', NULL, NULL, '{"sequence":62,"ID":"7a0a4905-59c5-4277-8114-1e9ca9d425e3"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.586439Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:19d7b512-e5e0-469c-917b-eb27d7a38bed', 'block:bb3bc716-ca9a-438a-936d-03631e2ee929', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Flutter desktop app shell', 'text', NULL, NULL, '{"ID":"19d7b512-e5e0-469c-917b-eb27d7a38bed","sequence":63}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.586891Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Petri-Net Task Ranking (WSJF) [/]', 'text', NULL, NULL, '{"ID":"afe4f75c-7948-4d4c-9724-4bfab7d47d88","sequence":64}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.587459Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d81b05ee-70f9-4b19-b43e-40a93fd5e1b7', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Prototype blocks with =computed Rhai expressions', 'text', NULL, NULL, '{"sequence":65,"task_state":"DOING","ID":"d81b05ee-70f9-4b19-b43e-40a93fd5e1b7"}', 1773939024043, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.587995Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2d399fd7-79d8-41f1-846b-31dabcec208a', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Verb dictionary (~30 German + English verbs → transition types)', 'text', NULL, NULL, '{"ID":"2d399fd7-79d8-41f1-846b-31dabcec208a","sequence":66}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.588494Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2385f4e3-25e1-4911-bf75-77cefd394206', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'rank_tasks() engine with tiebreak ordering', 'text', NULL, NULL, '{"task_state":"DOING","ID":"2385f4e3-25e1-4911-bf75-77cefd394206","sequence":67}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.588907Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cae619f2-26fe-464e-b67a-0a04f76543c9', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Block → Petri Net materialization (petri.rs)', 'text', NULL, NULL, '{"sequence":68,"ID":"cae619f2-26fe-464e-b67a-0a04f76543c9","task_state":"DOING"}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.589313Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eaee1c9b-5466-428f-8dbb-f4882ccdb066', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Self Descriptor (person block with is_self: true)', 'text', NULL, NULL, '{"ID":"eaee1c9b-5466-428f-8dbb-f4882ccdb066","task_state":"DOING","sequence":69}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.589748Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:023da362-ce5d-4a3b-827a-29e745d6f778', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'WSJF scoring (priority_weight × urgency_weight + position_weight)', 'text', NULL, NULL, '{"ID":"023da362-ce5d-4a3b-827a-29e745d6f778","task_state":"DOING","sequence":70}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.590286Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:46a8c75e-8ab8-4a5a-b4af-a1388f6a4812', 'block:afe4f75c-7948-4d4c-9724-4bfab7d47d88', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Task syntax parser (@, ?, >, [[links]])', 'text', NULL, NULL, '{"ID":"46a8c75e-8ab8-4a5a-b4af-a1388f6a4812","sequence":71}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.590701Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 2: First Integration (Todoist) [/]\nGoal: Prove hybrid architecture', 'text', NULL, NULL, '{"ID":"29c0aa5f-d9ca-46f3-8601-6023f87cefbd","sequence":72}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.591090Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Reconciliation [/]', 'text', NULL, NULL, '{"ID":"00fa0916-2681-4699-9554-44fcb8e2ea6a","sequence":73}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.591477Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:632af903-5459-4d44-921a-43145e20dc82', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Sync token management to prevent duplicate processing', 'text', NULL, NULL, '{"ID":"632af903-5459-4d44-921a-43145e20dc82","sequence":74}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.591860Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:78f9d6e3-42d4-4975-910d-3728e23410b1', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Conflict detection and resolution UI', 'text', NULL, NULL, '{"ID":"78f9d6e3-42d4-4975-910d-3728e23410b1","sequence":75}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.592240Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fa2854d1-2751-4a07-8f83-70c2f9c6c190', 'block:00fa0916-2681-4699-9554-44fcb8e2ea6a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Last-write-wins for concurrent edits', 'text', NULL, NULL, '{"ID":"fa2854d1-2751-4a07-8f83-70c2f9c6c190","sequence":76}', 1773939024044, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.592592Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Operation Queue & Offline Support [/]', 'text', NULL, NULL, '{"sequence":77,"ID":"043ed925-6bf2-4db3-baf8-2277f1a5afaa"}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.592954Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5c1ce94f-fcf2-44d8-b94d-27cc91186ce3', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Offline operation queue with retry logic', 'text', NULL, NULL, '{"sequence":78,"ID":"5c1ce94f-fcf2-44d8-b94d-27cc91186ce3"}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.593312Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7de8d37b-49ba-4ada-9b1e-df1c41c0db05', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Sync status indicators (synced, pending, conflict, error)', 'text', NULL, NULL, '{"ID":"7de8d37b-49ba-4ada-9b1e-df1c41c0db05","sequence":79}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.593673Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:302eb0c5-56fe-4980-8292-bae8a9a0450a', 'block:043ed925-6bf2-4db3-baf8-2277f1a5afaa', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Optimistic updates with ID mapping (internal ↔ external)', 'text', NULL, NULL, '{"sequence":80,"ID":"302eb0c5-56fe-4980-8292-bae8a9a0450a"}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.594032Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist-Specific Features [/]', 'text', NULL, NULL, '{"ID":"b1b2037e-b2e9-45db-8cb9-2ed783ede2ce","sequence":81}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.594508Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a27cd79b-63bd-4704-b20f-f3b595838e89', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Bi-directional task completion sync', 'text', NULL, NULL, '{"sequence":82,"ID":"a27cd79b-63bd-4704-b20f-f3b595838e89"}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.594853Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ab2868f6-ac6a-48de-b56f-ffa755f6cd22', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist due dates → deadline penalty functions', 'text', NULL, NULL, '{"sequence":83,"ID":"ab2868f6-ac6a-48de-b56f-ffa755f6cd22"}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.595226Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f6e32a19-a659-47f7-b2dc-24142c6616f7', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '@person labels → delegation/waiting_for tracking', 'text', NULL, NULL, '{"ID":"f6e32a19-a659-47f7-b2dc-24142c6616f7","sequence":84}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.595569Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:19923c1b-89ab-42f3-97a2-d78e994a2e1c', 'block:b1b2037e-b2e9-45db-8cb9-2ed783ede2ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist priority → WSJF CoD weight mapping', 'text', NULL, NULL, '{"ID":"19923c1b-89ab-42f3-97a2-d78e994a2e1c","sequence":85}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.595926Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'MCP Client Bridge [/]', 'text', NULL, NULL, '{"ID":"f37ab7bc-c89e-4b47-9317-3a9f7a440d2a","sequence":86}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.596299Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4d30926a-54c4-40b4-978e-eeca2d273fd1', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Tool name normalization (kebab-case ↔ snake_case)', 'text', NULL, NULL, '{"sequence":87,"ID":"4d30926a-54c4-40b4-978e-eeca2d273fd1"}', 1773939024045, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.596646Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c30b7e5a-4e9f-41e8-ab19-e803c93dc467', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'McpOperationProvider converting MCP tool schemas → OperationDescriptors', 'text', NULL, NULL, '{"ID":"c30b7e5a-4e9f-41e8-ab19-e803c93dc467","sequence":88}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.597004Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:836bab0e-5ac1-4df1-9f40-4005320c406e', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'holon-mcp-client crate for connecting to external MCP servers', 'text', NULL, NULL, '{"sequence":89,"ID":"836bab0e-5ac1-4df1-9f40-4005320c406e"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.597558Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ceb59dae-6090-41be-aff7-89de33ec600a', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'YAML sidecar for UI annotations (affected_fields, triggered_by, preconditions)', 'text', NULL, NULL, '{"sequence":90,"ID":"ceb59dae-6090-41be-aff7-89de33ec600a"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.598103Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:419e493f-c2de-47c2-a612-787db669cd89', 'block:f37ab7bc-c89e-4b47-9317-3a9f7a440d2a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'JSON Schema → TypeHint mapping', 'text', NULL, NULL, '{"sequence":91,"ID":"419e493f-c2de-47c2-a612-787db669cd89"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.598541Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'block:29c0aa5f-d9ca-46f3-8601-6023f87cefbd', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist API Integration [/]', 'text', NULL, NULL, '{"sequence":92,"ID":"bdce9ec2-1508-47e9-891e-e12a7b228fcc"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.599078Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e9398514-1686-4fef-a44a-5fef1742d004', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'TodoistOperationProvider for operation routing', 'text', NULL, NULL, '{"sequence":93,"ID":"e9398514-1686-4fef-a44a-5fef1742d004"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.599469Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9670e586-5cda-42a2-8071-efaf855fd5d4', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist REST API client', 'text', NULL, NULL, '{"sequence":94,"ID":"9670e586-5cda-42a2-8071-efaf855fd5d4"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.599839Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f41aeaa5-fe1d-45a5-806d-1f815040a33d', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Todoist entity types (tasks, projects, sections, labels)', 'text', NULL, NULL, '{"sequence":95,"ID":"f41aeaa5-fe1d-45a5-806d-1f815040a33d"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.600207Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d041e942-f3a1-4b7d-80b8-7de6eb289ebe', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'TodoistSyncProvider with incremental sync tokens', 'text', NULL, NULL, '{"sequence":96,"ID":"d041e942-f3a1-4b7d-80b8-7de6eb289ebe"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.600579Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f3b43be1-5503-4b1a-a724-fc657b47e18c', 'block:bdce9ec2-1508-47e9-891e-e12a7b228fcc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'TodoistTaskDataSource implementing DataSource<TodoistTask>', 'text', NULL, NULL, '{"ID":"f3b43be1-5503-4b1a-a724-fc657b47e18c","sequence":97}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.600929Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 3: Multiple Integrations [/]\nGoal: Validate type unification scales', 'text', NULL, NULL, '{"sequence":98,"ID":"88810f15-a95b-4343-92e2-909c5113cc9c"}', 1773939024046, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.601281Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Unified Item Types [/]', 'text', NULL, NULL, '{"sequence":99,"ID":"9ea38e3d-383e-4c27-9533-d53f1f8b1fb2"}', 1773939024047, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.601628Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5b1e8251-be26-4099-b169-a330cc16f0a6', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Macro-generated serialization boilerplate', 'text', NULL, NULL, '{"ID":"5b1e8251-be26-4099-b169-a330cc16f0a6","sequence":100}', 1773939024047, 1773939024086, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.601981Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5b49aefd-e14f-4151-bf9e-ccccae3545ec', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Trait-based protocol for common task interface', 'text', NULL, NULL, '{"ID":"5b49aefd-e14f-4151-bf9e-ccccae3545ec","sequence":101}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.602331Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e6162a0a-e9ae-494e-b3f5-4cf98cb2f447', 'block:9ea38e3d-383e-4c27-9533-d53f1f8b1fb2', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Extension structs for system-specific features', 'text', NULL, NULL, '{"sequence":102,"ID":"e6162a0a-e9ae-494e-b3f5-4cf98cb2f447"}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.602728Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-System Features [/]', 'text', NULL, NULL, '{"ID":"d6ab6d5f-68ae-404a-bcad-b5db61586634","sequence":103}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.603079Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5403c088-a551-4ca6-8830-34e00d5e5820', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Context Bundles assembling related items from all sources', 'text', NULL, NULL, '{"sequence":104,"ID":"5403c088-a551-4ca6-8830-34e00d5e5820"}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.603429Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:091caad8-1689-472d-9130-e3c855c510a8', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Embedding third-party items anywhere in the graph', 'text', NULL, NULL, '{"ID":"091caad8-1689-472d-9130-e3c855c510a8","sequence":105}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.603914Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cfb257f0-1a9c-426c-ab24-940eb18853ea', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Unified search across all systems', 'text', NULL, NULL, '{"sequence":106,"ID":"cfb257f0-1a9c-426c-ab24-940eb18853ea"}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.604274Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:52a440c1-4099-4911-8d9d-e2d583dbdde7', 'block:d6ab6d5f-68ae-404a-bcad-b5db61586634', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'P.A.R.A. project-based organization with auto-linking', 'text', NULL, NULL, '{"ID":"52a440c1-4099-4911-8d9d-e2d583dbdde7","sequence":107}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.604721Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'block:88810f15-a95b-4343-92e2-909c5113cc9c', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Additional Integrations [/]', 'text', NULL, NULL, '{"ID":"34fa9276-cc30-4fcb-95b5-a97b5d708757","sequence":108}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.605322Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9240c0d7-d60a-46e0-8265-ceacfbf04d50', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Linear integration (cycles, projects)', 'text', NULL, NULL, '{"sequence":109,"ID":"9240c0d7-d60a-46e0-8265-ceacfbf04d50"}', 1773939024047, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.605780Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8ea813ff-b355-4165-b377-fbdef4d3d7d8', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Google Calendar integration (events as time tokens)', 'text', NULL, NULL, '{"sequence":110,"ID":"8ea813ff-b355-4165-b377-fbdef4d3d7d8"}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.606218Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Gmail integration (email threads, labels)', 'text', NULL, NULL, '{"sequence":111,"ID":"ede2fbf4-2c0d-423f-a8ad-22c52ac6cd29"}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.606638Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f583e6d9-f67d-4997-a658-ed00149a34cc', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'JIRA integration (sprints, story points, epics)', 'text', NULL, NULL, '{"ID":"f583e6d9-f67d-4997-a658-ed00149a34cc","sequence":112}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.607238Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9fed69a3-9180-4eba-a778-fa93bc398064', 'block:34fa9276-cc30-4fcb-95b5-a97b5d708757', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'GPUI Components', 'text', NULL, NULL, '{"ID":"9fed69a3-9180-4eba-a778-fa93bc398064","sequence":113}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.607649Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9f523ce8-5449-4a2f-81c8-8ee08399fc31', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'https://github.com/MeowLynxSea/yororen-ui', 'text', NULL, NULL, '{"sequence":114,"ID":"9f523ce8-5449-4a2f-81c8-8ee08399fc31"}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.608063Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fd965570-883d-48f7-82b0-92ba257b2597', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Pomodoro\nhttps://github.com/rubbieKelvin/bmo', 'text', NULL, NULL, '{"ID":"fd965570-883d-48f7-82b0-92ba257b2597","sequence":115}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.608461Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9657e201-4426-4091-891b-eb40e299d81d', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Diff viewer\nhttps://github.com/BlixtWallet/hunk', 'text', NULL, NULL, '{"sequence":116,"ID":"9657e201-4426-4091-891b-eb40e299d81d"}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.608869Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:61a47437-c394-42db-b195-3dabbd5d87ab', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Animation\nhttps://github.com/chi11321/gpui-animation', 'text', NULL, NULL, '{"ID":"61a47437-c394-42db-b195-3dabbd5d87ab","sequence":117}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.609301Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5841efc0-cfe6-4e69-9dbc-9f627693e59a', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Editor\nhttps://github.com/iamnbutler/gpui-editor', 'text', NULL, NULL, '{"ID":"5841efc0-cfe6-4e69-9dbc-9f627693e59a","sequence":118}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.609786Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:482c5cbb-dd4f-4225-9329-ca9ca0beea4c', 'block:9fed69a3-9180-4eba-a778-fa93bc398064', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'WebView\nhttps://github.com/longbridge/wef', 'text', NULL, NULL, '{"ID":"482c5cbb-dd4f-4225-9329-ca9ca0beea4c","sequence":119}', 1773939024048, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.610219Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 4: AI Foundation [/]\nGoal: Infrastructure for AI features', 'text', NULL, NULL, '{"sequence":120,"ID":"7b960cd0-3478-412b-b96f-15822117ac14"}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.610632Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Agent Embedding', 'text', NULL, NULL, '{"ID":"553f3545-4ec7-44e5-bccf-3d6443f22ecc","sequence":121}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.611032Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Via Terminal', 'text', NULL, NULL, '{"sequence":122,"ID":"d4c1533f-3a67-4314-b430-0e24bd62ce34"}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.611461Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6e2fd9a2-6f39-48d2-b323-935fc18a3f5e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Okena\nA fast, native terminal multiplexer built in Rust with GPUI\nhttps://github.com/contember/okena', 'text', NULL, NULL, '{"ID":"6e2fd9a2-6f39-48d2-b323-935fc18a3f5e","sequence":123}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.611878Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c4b1ce62-0ad1-4c33-90fe-d7463f40800e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'PMux\nhttps://github.com/zhoujinliang/pmux', 'text', NULL, NULL, '{"ID":"c4b1ce62-0ad1-4c33-90fe-d7463f40800e","sequence":124}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.612283Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Slick\nhttps://github.com/tristanpoland/Slick', 'text', NULL, NULL, '{"ID":"e204bbf1-dc16-4b78-86cd-5d99dfa5bd4e","sequence":125}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.612676Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d50a9a7a-0155-4778-ac99-5f83555a1952', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'https://github.com/zortax/gpui-terminal', 'text', NULL, NULL, '{"sequence":126,"ID":"d50a9a7a-0155-4778-ac99-5f83555a1952"}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.613081Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cf102b47-01db-427b-97b6-3c066d9dba24', 'block:d4c1533f-3a67-4314-b430-0e24bd62ce34', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'https://github.com/Xuanwo/gpui-ghostty', 'text', NULL, NULL, '{"ID":"cf102b47-01db-427b-97b6-3c066d9dba24","sequence":127}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.613481Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'block:553f3545-4ec7-44e5-bccf-3d6443f22ecc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Via Chat', 'text', NULL, NULL, '{"sequence":128,"ID":"1236a3b4-6e03-421a-a94b-fce9d7dc123c"}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.613880Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f47a6df7-abfc-47b8-bdfe-f19eaf35b847', 'block:1236a3b4-6e03-421a-a94b-fce9d7dc123c', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'coop\nhttps://github.com/lumehq/coop?tab=readme-ov-file', 'text', NULL, NULL, '{"sequence":129,"ID":"f47a6df7-abfc-47b8-bdfe-f19eaf35b847"}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.614275Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:671593d9-a9c6-4716-860b-8410c8616539', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Embeddings & Search [/]', 'text', NULL, NULL, '{"sequence":130,"ID":"671593d9-a9c6-4716-860b-8410c8616539"}', 1773939024049, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.614670Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d58b8367-14eb-4895-9e56-ffa7ff716d59', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Local vector embeddings (sentence-transformers)', 'text', NULL, NULL, '{"ID":"d58b8367-14eb-4895-9e56-ffa7ff716d59","sequence":131}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.615085Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5f3e7d1e-af67-4699-a591-fd9291bf0cdc', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Semantic search using local embeddings', 'text', NULL, NULL, '{"sequence":132,"ID":"5f3e7d1e-af67-4699-a591-fd9291bf0cdc"}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.615480Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:96f4647c-8b74-4b08-8952-4f87820aed86', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Entity linking (manual first, then automatic)', 'text', NULL, NULL, '{"sequence":133,"ID":"96f4647c-8b74-4b08-8952-4f87820aed86"}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.615875Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0da39f39-6635-4f9b-a468-34310147bea9', 'block:671593d9-a9c6-4716-860b-8410c8616539', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Tantivy full-text search integration', 'text', NULL, NULL, '{"ID":"0da39f39-6635-4f9b-a468-34310147bea9","sequence":134}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.616277Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Self Digital Twin [/]', 'text', NULL, NULL, '{"ID":"439af07e-3237-420c-8bc0-c71aeb37c61a","sequence":135}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.616908Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5f3e8ef3-df52-4fb9-80c1-ccb81be40412', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Energy/focus/flow_depth dynamics', 'text', NULL, NULL, '{"sequence":136,"ID":"5f3e8ef3-df52-4fb9-80c1-ccb81be40412"}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.617333Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:30406a65-8e66-4589-b070-3a1b4db6e4e0', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Peripheral awareness modeling', 'text', NULL, NULL, '{"ID":"30406a65-8e66-4589-b070-3a1b4db6e4e0","sequence":137}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.617751Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bed11feb-a634-4f8d-b930-f0021ec0512b', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Observable signals (window switches, typing cadence)', 'text', NULL, NULL, '{"ID":"bed11feb-a634-4f8d-b930-f0021ec0512b","sequence":138}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.618175Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:11c9c8bb-b72e-4752-8b6c-846e45920418', 'block:439af07e-3237-420c-8bc0-c71aeb37c61a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Mental slots tracking (materialized view of open transitions)', 'text', NULL, NULL, '{"ID":"11c9c8bb-b72e-4752-8b6c-846e45920418","sequence":139}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.618752Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Logging & Training Data [/]', 'text', NULL, NULL, '{"ID":"b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5","sequence":140}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.619163Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a186c88f-6ca5-49e2-8a0d-19632cb689fc', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Conflict logging system (capture every conflict + resolution)', 'text', NULL, NULL, '{"sequence":141,"ID":"a186c88f-6ca5-49e2-8a0d-19632cb689fc"}', 1773939024050, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.619577Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f342692d-5414-4c48-89fe-ed8f9ccf2172', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Pattern logging for Guide to learn from', 'text', NULL, NULL, '{"sequence":142,"ID":"f342692d-5414-4c48-89fe-ed8f9ccf2172"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.619970Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:30f04064-a58e-416d-b0d2-7533637effe8', 'block:b2b33f60-0002-4f0a-a6f3-7ac76bb0b7a5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Behavioral logging for search ranking', 'text', NULL, NULL, '{"sequence":143,"ID":"30f04064-a58e-416d-b0d2-7533637effe8"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.620376Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:84151cf1-696a-420f-b73c-4947b0a4437e', 'block:7b960cd0-3478-412b-b96f-15822117ac14', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Objective Function Engine [/]', 'text', NULL, NULL, '{"sequence":144,"ID":"84151cf1-696a-420f-b73c-4947b0a4437e"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.620779Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Evaluate token attributes via PRQL → scalar score', 'text', NULL, NULL, '{"sequence":145,"ID":"fa576a6c-ff29-40dc-89e5-c00fb5c9b1d7"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.621172Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:480f2628-c49f-4940-9e26-572ea23f25a3', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Store weights as prototype block properties', 'text', NULL, NULL, '{"sequence":146,"ID":"480f2628-c49f-4940-9e26-572ea23f25a3"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.621577Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e4e93198-6617-4c7c-b8f7-4b2d8188a77e', 'block:84151cf1-696a-420f-b73c-4947b0a4437e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Support multiple goal types (achievement, maintenance, process)', 'text', NULL, NULL, '{"sequence":147,"ID":"e4e93198-6617-4c7c-b8f7-4b2d8188a77e"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.621973Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 5: AI Features [/]\nGoal: Three AI services operational', 'text', NULL, NULL, '{"sequence":148,"ID":"8b962d6c-0246-4119-8826-d517e2357f21"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.622376Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'The Guide (Growth) [/]', 'text', NULL, NULL, '{"ID":"567e74d4-05c4-4f98-8ce1-1b78a8c7fd78","sequence":149}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.622772Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:37c082de-d10a-4f11-82ad-5fb3316bb3e4', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Velocity and capacity analysis', 'text', NULL, NULL, '{"ID":"37c082de-d10a-4f11-82ad-5fb3316bb3e4","sequence":150}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.623169Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:52bedd69-85ec-448d-81b6-0099bd413149', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Stuck task identification (postponement tracking)', 'text', NULL, NULL, '{"ID":"52bedd69-85ec-448d-81b6-0099bd413149","sequence":151}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.623570Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2b5ec929-a22d-4d7f-8640-66495331a40d', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Shadow Work prompts for avoided tasks', 'text', NULL, NULL, '{"sequence":152,"ID":"2b5ec929-a22d-4d7f-8640-66495331a40d"}', 1773939024051, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.623981Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:dd9075a4-5c64-4d6b-9661-7937897337d3', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Growth tracking and visualization', 'text', NULL, NULL, '{"ID":"dd9075a4-5c64-4d6b-9661-7937897337d3","sequence":153}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.624380Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:15a61916-b0c1-4d24-9046-4e066a312401', 'block:567e74d4-05c4-4f98-8ce1-1b78a8c7fd78', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Pattern recognition across time', 'text', NULL, NULL, '{"sequence":154,"ID":"15a61916-b0c1-4d24-9046-4e066a312401"}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.624773Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Intelligent Conflict Reconciliation [/]', 'text', NULL, NULL, '{"ID":"8ae21b36-6f48-41f1-80d9-bb7ce43b4545","sequence":155}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.625175Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0db1be3e-ae11-4341-8aa8-b1d80e22963a', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LLM-based resolution for low-confidence cases', 'text', NULL, NULL, '{"ID":"0db1be3e-ae11-4341-8aa8-b1d80e22963a","sequence":156}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.625576Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:314e7db7-fb5e-40b6-ac10-a589ff3c809d', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Rule-based conflict resolver', 'text', NULL, NULL, '{"sequence":157,"ID":"314e7db7-fb5e-40b6-ac10-a589ff3c809d"}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.625979Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:655e2f77-d02e-4347-aa5f-dcd03ac140eb', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Train classifier on logged conflicts', 'text', NULL, NULL, '{"sequence":158,"ID":"655e2f77-d02e-4347-aa5f-dcd03ac140eb"}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.626387Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3bbdc016-4f08-49e4-b550-ba3d09a03933', 'block:8ae21b36-6f48-41f1-80d9-bb7ce43b4545', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Conflict resolution UI with reasoning display', 'text', NULL, NULL, '{"sequence":159,"ID":"3bbdc016-4f08-49e4-b550-ba3d09a03933"}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.626791Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'AI Trust Ladder [/]', 'text', NULL, NULL, '{"ID":"be9e6d6e-f995-4a27-bd5e-b2f70f12c93e","sequence":160}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.627193Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8a72f072-cc14-4e5f-987c-72bd27d94ced', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Level 3 (Agentic) with permission prompts', 'text', NULL, NULL, '{"sequence":161,"ID":"8a72f072-cc14-4e5f-987c-72bd27d94ced"}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.627598Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c2289c19-1733-476e-9b50-43da1d70221f', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Level 4 (Autonomous) for power users', 'text', NULL, NULL, '{"sequence":162,"ID":"c2289c19-1733-476e-9b50-43da1d70221f"}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.628012Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Level 2 (Advisory) features', 'text', NULL, NULL, '{"ID":"c83b6ed3-2c3b-4e31-90d7-865d33dbd7c0","sequence":163}', 1773939024052, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.628419Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:84706843-7132-4c12-a2ae-32fb7109982c', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Per-feature trust tracking', 'text', NULL, NULL, '{"sequence":164,"ID":"84706843-7132-4c12-a2ae-32fb7109982c"}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.628825Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:66b47313-a556-4628-954e-1da7fb1d402d', 'block:be9e6d6e-f995-4a27-bd5e-b2f70f12c93e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Trust level visualization UI', 'text', NULL, NULL, '{"ID":"66b47313-a556-4628-954e-1da7fb1d402d","sequence":165}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.629236Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Background Enrichment Agents [/]', 'text', NULL, NULL, '{"sequence":166,"ID":"d1e6541b-0c6b-4065-aea5-ad9057dc5bb5"}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.629643Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2618de83-3d90-4dc6-b586-98f95e351fb5', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Infer likely token types from context', 'text', NULL, NULL, '{"sequence":167,"ID":"2618de83-3d90-4dc6-b586-98f95e351fb5"}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.630048Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:edd212e6-16a9-4dfd-95f9-e2a2a3a55eec', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Suggest dependencies between siblings', 'text', NULL, NULL, '{"ID":"edd212e6-16a9-4dfd-95f9-e2a2a3a55eec","sequence":168}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.630647Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Suggest [[links]] for plain-text nouns (local LLM)', 'text', NULL, NULL, '{"sequence":169,"ID":"44a3c9e7-a4ed-4d03-a32d-9b0b2f9d9cda"}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.631138Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2ff960fa-38a4-42dd-8eb0-77e15c89659e', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Classify tasks as question/delegation/action', 'text', NULL, NULL, '{"ID":"2ff960fa-38a4-42dd-8eb0-77e15c89659e","sequence":170}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.631582Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:864527d2-65d4-4716-a65e-73a868c7e63b', 'block:d1e6541b-0c6b-4065-aea5-ad9057dc5bb5', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Suggest via: routes for questions', 'text', NULL, NULL, '{"sequence":171,"ID":"864527d2-65d4-4716-a65e-73a868c7e63b"}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.632003Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'The Integrator (Wholeness) [/]', 'text', NULL, NULL, '{"sequence":172,"ID":"8a4a658e-d773-4528-8c61-ff3e5e425f47"}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.632417Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Smart linking suggestions', 'text', NULL, NULL, '{"ID":"2b18aedf-f0e3-462e-b7fa-1991e1a8ba4a","sequence":173}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.632833Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Context Bundle assembly for Flow mode', 'text', NULL, NULL, '{"ID":"4025eb6a-7e10-4a0c-8ca1-0a6e4da0bbb6","sequence":174}', 1773939024053, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.633433Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7efa2454-274c-4304-8641-e3b8171c5b5a', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-system deduplication', 'text', NULL, NULL, '{"sequence":175,"ID":"7efa2454-274c-4304-8641-e3b8171c5b5a"}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.633838Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:311aa51c-88af-446f-8cb6-b791b9740665', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Related item discovery', 'text', NULL, NULL, '{"sequence":176,"ID":"311aa51c-88af-446f-8cb6-b791b9740665"}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.634239Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9b6b2563-21b8-4286-9fac-dbdddc1a79be', 'block:8a4a658e-d773-4528-8c61-ff3e5e425f47', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Automatic entity linking via embeddings', 'text', NULL, NULL, '{"sequence":177,"ID":"9b6b2563-21b8-4286-9fac-dbdddc1a79be"}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.634644Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'block:8b962d6c-0246-4119-8826-d517e2357f21', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'The Watcher (Awareness) [/]', 'text', NULL, NULL, '{"ID":"d385afbe-5bc9-4341-b879-6d14b8d763bc","sequence":178}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.635088Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:244abb7d-ef0f-4768-9e4e-b4bd7f3eec23', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Risk and deadline tracking', 'text', NULL, NULL, '{"ID":"244abb7d-ef0f-4768-9e4e-b4bd7f3eec23","sequence":179}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.635439Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f9a2e27c-218f-402a-b405-b6b14b498bcf', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Capacity analysis across all systems', 'text', NULL, NULL, '{"ID":"f9a2e27c-218f-402a-b405-b6b14b498bcf","sequence":180}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.635782Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:92d9dee2-3c16-4d14-9d54-1a93313ee1f4', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-system monitoring and alerts', 'text', NULL, NULL, '{"sequence":181,"ID":"92d9dee2-3c16-4d14-9d54-1a93313ee1f4"}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.636141Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e6c28ce7-c659-49e7-874b-334f05852cc4', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Daily/weekly synthesis for Orient mode', 'text', NULL, NULL, '{"ID":"e6c28ce7-c659-49e7-874b-334f05852cc4","sequence":182}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.636577Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1ffa7eb6-174a-4bed-85d2-9c47d9d55519', 'block:d385afbe-5bc9-4341-b879-6d14b8d763bc', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Dependency chain analysis', 'text', NULL, NULL, '{"sequence":183,"ID":"1ffa7eb6-174a-4bed-85d2-9c47d9d55519"}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.637064Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 6: Flow Optimization [/]\nGoal: Users achieve flow states regularly', 'text', NULL, NULL, '{"ID":"c74fcc72-883d-4788-911a-0632f6145e4d","sequence":184}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.637505Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f908d928-db6f-495e-a941-22fcdfdba73a', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Self DT Work Rhythms [/]', 'text', NULL, NULL, '{"sequence":185,"ID":"f908d928-db6f-495e-a941-22fcdfdba73a"}', 1773939024054, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.637916Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0570c0bf-84b4-4734-b6f3-25242a12a154', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Emergent break suggestions from energy/focus dynamics', 'text', NULL, NULL, '{"ID":"0570c0bf-84b4-4734-b6f3-25242a12a154","sequence":186}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.638331Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:9d85cad6-1e74-499a-8d8e-899c5553c3d6', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Flow depth tracking with peripheral awareness alerts', 'text', NULL, NULL, '{"ID":"9d85cad6-1e74-499a-8d8e-899c5553c3d6","sequence":187}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.638740Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:adc7803b-9318-4ca5-877b-83f213445aba', 'block:f908d928-db6f-495e-a941-22fcdfdba73a', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Quick task suggestions during breaks (2-minute rule)', 'text', NULL, NULL, '{"sequence":188,"ID":"adc7803b-9318-4ca5-877b-83f213445aba"}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.639165Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Three Modes [/]', 'text', NULL, NULL, '{"sequence":189,"ID":"b5771daa-0208-43fe-a890-ef1fcebf5f2f"}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.639570Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:be15792f-21f3-476f-8b5f-e2e6b478b864', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Orient mode (Watcher Dashboard, daily/weekly review)', 'text', NULL, NULL, '{"sequence":190,"ID":"be15792f-21f3-476f-8b5f-e2e6b478b864"}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.639977Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c68e8d5a-3f4b-4e8c-a887-2341e9b98bde', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Flow mode (single task focus, context on demand)', 'text', NULL, NULL, '{"ID":"c68e8d5a-3f4b-4e8c-a887-2341e9b98bde","sequence":191}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.640380Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b1b2db9a-fc0d-4f51-98ae-9c5ab056a963', 'block:b5771daa-0208-43fe-a890-ef1fcebf5f2f', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Capture mode (global hotkey, quick input overlay)', 'text', NULL, NULL, '{"ID":"b1b2db9a-fc0d-4f51-98ae-9c5ab056a963","sequence":192}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.640787Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a3e31c87-d10b-432e-987c-0371e730f753', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Review Workflows [/]', 'text', NULL, NULL, '{"sequence":193,"ID":"a3e31c87-d10b-432e-987c-0371e730f753"}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.641186Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4c020c67-1726-46d8-92e3-b9e0dbc90b62', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Daily orientation ("What does today look like?")', 'text', NULL, NULL, '{"ID":"4c020c67-1726-46d8-92e3-b9e0dbc90b62","sequence":194}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.641631Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0906f769-52eb-47a2-917a-f9b57b7e80d1', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Inbox zero workflow', 'text', NULL, NULL, '{"sequence":195,"ID":"0906f769-52eb-47a2-917a-f9b57b7e80d1"}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.642218Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:091e7648-5314-4b4d-8e9c-bd7e0b8efc6f', 'block:a3e31c87-d10b-432e-987c-0371e730f753', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Weekly review (comprehensive synthesis)', 'text', NULL, NULL, '{"sequence":196,"ID":"091e7648-5314-4b4d-8e9c-bd7e0b8efc6f"}', 1773939024055, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.642629Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:240acff4-cf06-445e-99ee-42040da1bb84', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Context Bundles in Flow [/]', 'text', NULL, NULL, '{"sequence":197,"ID":"240acff4-cf06-445e-99ee-42040da1bb84"}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.643031Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:90702048-5baf-4732-96fb-ddae16824257', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Hide distractions, show progress', 'text', NULL, NULL, '{"ID":"90702048-5baf-4732-96fb-ddae16824257","sequence":198}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.643439Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e4aeb8f0-4c63-48f6-b745-92a89cfd4130', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Slide-in context panel from edge', 'text', NULL, NULL, '{"sequence":199,"ID":"e4aeb8f0-4c63-48f6-b745-92a89cfd4130"}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.643838Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:3907168e-eaf8-48ee-8ccc-6dfef069371e', 'block:240acff4-cf06-445e-99ee-42040da1bb84', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Assemble all related items for focused task', 'text', NULL, NULL, '{"ID":"3907168e-eaf8-48ee-8ccc-6dfef069371e","sequence":200}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.644263Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e233124d-8711-4dd4-8153-c884f889bc07', 'block:c74fcc72-883d-4788-911a-0632f6145e4d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Progressive Concealment [/]', 'text', NULL, NULL, '{"sequence":201,"ID":"e233124d-8711-4dd4-8153-c884f889bc07"}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.644646Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:70485255-a2be-4356-bb9e-967270878b7e', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Peripheral element dimming during sustained typing', 'text', NULL, NULL, '{"ID":"70485255-a2be-4356-bb9e-967270878b7e","sequence":202}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.645043Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ea7f8d72-f963-4a51-ab4f-d10f981eafcc', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Focused block emphasis, surrounding content fades', 'text', NULL, NULL, '{"ID":"ea7f8d72-f963-4a51-ab4f-d10f981eafcc","sequence":203}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.645422Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:30a71e2f-f070-4745-947d-c443a86a7149', 'block:e233124d-8711-4dd4-8153-c884f889bc07', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Automatic visibility restore on cursor movement', 'text', NULL, NULL, '{"sequence":204,"ID":"30a71e2f-f070-4745-947d-c443a86a7149"}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.645813Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Phase 7: Team Features [/]\nGoal: Teams leverage individual excellence', 'text', NULL, NULL, '{"ID":"4c647dfe-0639-4064-8ab6-491d57c7e367","sequence":205}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.646207Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Delegation System [/]', 'text', NULL, NULL, '{"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede","sequence":206}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.646611Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '@[[Person]]: syntax for delegation sub-nets', 'text', NULL, NULL, '{"sequence":207,"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264"}', 1773939024056, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.647015Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:fbbce845-023e-438b-963e-471833c51505', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Waiting-for tracking (automatic from delegation patterns)', 'text', NULL, NULL, '{"sequence":208,"ID":"fbbce845-023e-438b-963e-471833c51505"}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.647417Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:25e19c99-63c2-4edb-8fb1-deb1daf4baf0', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Delegation status sync with external systems', 'text', NULL, NULL, '{"sequence":209,"ID":"25e19c99-63c2-4edb-8fb1-deb1daf4baf0"}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.647819Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:938f03b8-6129-4eda-9c5f-31a76ad8b8dc', 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', '@anyone: team pool transitions', 'text', NULL, NULL, '{"ID":"938f03b8-6129-4eda-9c5f-31a76ad8b8dc","sequence":210}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.648226Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Sharing & Collaboration [/]', 'text', NULL, NULL, '{"ID":"5bdf3ba6-f617-4bc1-93c2-15d84d925e01","sequence":211}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.648775Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:88b467b1-5a46-4b64-acb3-fcf9f377030e', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Collaborative editing', 'text', NULL, NULL, '{"ID":"88b467b1-5a46-4b64-acb3-fcf9f377030e","sequence":212}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.649171Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f3ce62cd-5817-4a7c-81f6-7a7077aff7da', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Shared views and dashboards', 'text', NULL, NULL, '{"sequence":213,"ID":"f3ce62cd-5817-4a7c-81f6-7a7077aff7da"}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.649551Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:135c74b1-8341-4719-b5d1-492eb26e2189', 'block:5bdf3ba6-f617-4bc1-93c2-15d84d925e01', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Read-only sharing for documentation', 'text', NULL, NULL, '{"ID":"135c74b1-8341-4719-b5d1-492eb26e2189","sequence":214}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.649944Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'block:4c647dfe-0639-4064-8ab6-491d57c7e367', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Competition analysis', 'text', NULL, NULL, '{"sequence":215,"ID":"e0f90f1e-5468-4229-9b6d-438b31f09ed6"}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.650326Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ceb203d0-0b59-4aa0-a840-2e4763234112', 'block:e0f90f1e-5468-4229-9b6d-438b31f09ed6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'https://github.com/3xpyth0n/ideon\nOrganize repositories, notes, links and more on a shared infinite canvas.', 'text', NULL, NULL, '{"ID":"ceb203d0-0b59-4aa0-a840-2e4763234112","sequence":216}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.650735Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-Cutting Concerns [/]', 'text', NULL, NULL, '{"ID":"f407a7ec-c924-4a38-96e0-7e73472e7353","sequence":217}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.651118Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Privacy & Security [/]', 'text', NULL, NULL, '{"ID":"ad1d8307-134f-4a34-b58e-07d6195b2466","sequence":218}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.652094Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:717db234-61eb-41ef-a8bf-b67e870f9aa6', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Plugin sandboxing (WASM)', 'text', NULL, NULL, '{"sequence":219,"ID":"717db234-61eb-41ef-a8bf-b67e870f9aa6"}', 1773939024057, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.652558Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:75604518-b736-4653-a2a3-941215e798c7', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Self-hosted LLM option (Ollama/vLLM)', 'text', NULL, NULL, '{"ID":"75604518-b736-4653-a2a3-941215e798c7","sequence":220}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.652989Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bfaedc82-3bc7-4b16-8314-273721ea997f', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Optional cloud LLM with explicit consent', 'text', NULL, NULL, '{"sequence":221,"ID":"bfaedc82-3bc7-4b16-8314-273721ea997f"}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.653411Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4b96f182-61e5-4f0e-861d-1a7d2413abe7', 'block:ad1d8307-134f-4a34-b58e-07d6195b2466', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Local-first by default (all data on device)', 'text', NULL, NULL, '{"ID":"4b96f182-61e5-4f0e-861d-1a7d2413abe7","sequence":222}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.653827Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eac105ca-efda-4976-9856-6c39a9b1502e', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Petri-Net Advanced [/]', 'text', NULL, NULL, '{"ID":"eac105ca-efda-4976-9856-6c39a9b1502e","sequence":223}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.654227Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'SOP extraction from repeated interaction patterns', 'text', NULL, NULL, '{"ID":"0ce53f54-c9c4-433c-9e0f-0ab2ce1c8a59","sequence":224}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.654636Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:143d071e-2b90-4f93-98d3-7aa5d3a14933', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Delegation sub-nets (waiting_for pattern)', 'text', NULL, NULL, '{"ID":"143d071e-2b90-4f93-98d3-7aa5d3a14933","sequence":225}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.655047Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc499de0-f953-4f41-b795-0864b366d8ab', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Token type hierarchy with mixins', 'text', NULL, NULL, '{"ID":"cc499de0-f953-4f41-b795-0864b366d8ab","sequence":226}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.655452Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:bd99d866-66ed-4474-8a4d-7ac1c1b08fbb', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Projections as views on flat net (Kanban, SOP, pipeline)', 'text', NULL, NULL, '{"sequence":227,"ID":"bd99d866-66ed-4474-8a4d-7ac1c1b08fbb"}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.656013Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4041eb2e-23a6-4fea-9a69-0c152a6311e8', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Question/Information tokens with confidence tracking', 'text', NULL, NULL, '{"ID":"4041eb2e-23a6-4fea-9a69-0c152a6311e8","sequence":228}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.656420Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e1027d2-4c0f-4975-ba59-c3c601d1f661', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Simulation engine (fork marking, compare scenarios)', 'text', NULL, NULL, '{"ID":"1e1027d2-4c0f-4975-ba59-c3c601d1f661","sequence":229}', 1773939024058, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.656835Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a80f6d58-c876-48f5-8bfe-69390a8f9bde', 'block:eac105ca-efda-4976-9856-6c39a9b1502e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Browser plugin for web app Digital Twins', 'text', NULL, NULL, '{"ID":"a80f6d58-c876-48f5-8bfe-69390a8f9bde","sequence":230}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.657239Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:723a51a9-3861-429c-bb10-f73c01f8463d', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'PRQL Automation [/]', 'text', NULL, NULL, '{"ID":"723a51a9-3861-429c-bb10-f73c01f8463d","sequence":231}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.657628Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Cross-system status propagation rules', 'text', NULL, NULL, '{"sequence":232,"ID":"e3b82a24-5dc7-43a9-bcd7-8cb07958b5c7"}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.658219Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c1338a15-080b-4dba-bbdc-87b6b8467f28', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Auto-tag blocks based on content analysis', 'text', NULL, NULL, '{"ID":"c1338a15-080b-4dba-bbdc-87b6b8467f28","sequence":233}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.658628Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5707965a-6578-443c-aeff-bf40170edea9', 'block:723a51a9-3861-429c-bb10-f73c01f8463d', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'PRQL-based automation rules (query → action)', 'text', NULL, NULL, '{"sequence":234,"ID":"5707965a-6578-443c-aeff-bf40170edea9"}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.659022Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Platform Support [/]', 'text', NULL, NULL, '{"sequence":235,"ID":"8e2b4ddd-e428-4950-bc41-76ee8a0e27ce"}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.659408Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4c4ff372-c3b9-44e6-9d46-33b7a4e7882e', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Android mobile', 'text', NULL, NULL, '{"ID":"4c4ff372-c3b9-44e6-9d46-33b7a4e7882e","sequence":236}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.659801Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e5b9db2d-f39a-439d-99f8-b4e7c4ff6857', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'WASM compatibility (MaybeSendSync trait)', 'text', NULL, NULL, '{"ID":"e5b9db2d-f39a-439d-99f8-b4e7c4ff6857","sequence":237}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.660198Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d61290d4-e1f6-41e7-89e0-a7ed7a6662db', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Windows desktop', 'text', NULL, NULL, '{"sequence":238,"ID":"d61290d4-e1f6-41e7-89e0-a7ed7a6662db"}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.660587Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e729eef-3fff-43cb-8d13-499a8a8d4203', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'iOS mobile', 'text', NULL, NULL, '{"sequence":239,"ID":"1e729eef-3fff-43cb-8d13-499a8a8d4203"}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.660980Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:500b7aae-5c3b-4dd5-a3c8-373fe746990b', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Linux desktop', 'text', NULL, NULL, '{"sequence":240,"ID":"500b7aae-5c3b-4dd5-a3c8-373fe746990b"}', 1773939024059, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.661406Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a79ab251-4685-4728-b98b-0a652774f06c', 'block:8e2b4ddd-e428-4950-bc41-76ee8a0e27ce', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'macOS desktop (Flutter)', 'text', NULL, NULL, '{"ID":"a79ab251-4685-4728-b98b-0a652774f06c","sequence":241}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.661833Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'block:f407a7ec-c924-4a38-96e0-7e73472e7353', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'UI/UX Design System [/]', 'text', NULL, NULL, '{"ID":"ac137431-daf6-4741-9808-6dc71c13e7c6","sequence":242}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.662227Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a85de368-9546-446d-ad61-17b72c7dbc3e', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Which-Key navigation system (Space → mnemonic keys)', 'text', NULL, NULL, '{"ID":"a85de368-9546-446d-ad61-17b72c7dbc3e","sequence":243}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.662700Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1cea6bd3-680f-46c3-bdbc-5989da5ed7d9', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Micro-interactions (checkbox animation, smooth reorder)', 'text', NULL, NULL, '{"sequence":244,"ID":"1cea6bd3-680f-46c3-bdbc-5989da5ed7d9"}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.663047Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d1fbee2c-3a11-4adc-a3db-fd93f5b117e3', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Light and dark themes', 'text', NULL, NULL, '{"ID":"d1fbee2c-3a11-4adc-a3db-fd93f5b117e3","sequence":245}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.663441Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:beeec959-ba87-4c57-9531-c1d7f24d2b2c', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Color palette (warm, professional, calm technology)', 'text', NULL, NULL, '{"sequence":246,"ID":"beeec959-ba87-4c57-9531-c1d7f24d2b2c"}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.663795Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d36014da-518a-4da5-b360-218d027ee104', 'block:ac137431-daf6-4741-9808-6dc71c13e7c6', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Typography system (Inter + JetBrains Mono)', 'text', NULL, NULL, '{"sequence":247,"ID":"d36014da-518a-4da5-b360-218d027ee104"}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.664136Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'LogSeq replacement', 'text', NULL, NULL, '{"ID":"01806047-9cf8-42fe-8391-6d608bfade9e","sequence":248}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.664466Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Editing experience', 'text', NULL, NULL, '{"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","sequence":249}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.664817Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'GitHub Flavored Markdown parser & renderer for GPUI\nhttps://github.com/joris-gallot/gpui-gfm', 'text', NULL, NULL, '{"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2","sequence":250}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.665215Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Desktop Markdown viewer built with Rust and GPUI\nhttps://github.com/chunghha/markdown_viewer', 'text', NULL, NULL, '{"sequence":251,"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8"}', 1773939024060, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.665616Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f7730a68-6268-4e65-ac93-3fdf79e92133', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Markdown Editor and Viewer\nhttps://github.com/kumarUjjawal/aster', 'text', NULL, NULL, '{"sequence":252,"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133"}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.665957Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:8594ab7c-5f36-44cf-8f92-248b31508441', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'PDF Viewer & Annotator', 'text', NULL, NULL, '{"sequence":253,"ID":"8594ab7c-5f36-44cf-8f92-248b31508441"}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.666295Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Combining gpui and hayro for a little application that render pdfs\nhttps://github.com/vincenthz/gpui-hayro?tab=readme-ov-file', 'text', NULL, NULL, '{"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c","sequence":254}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.666644Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b95a19a6-5448-42f0-af06-177e95e27f49', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Libera Reader\nModern, performance-oriented desktop e-book reader built with Rust and GPUI.\nhttps://github.com/RikaKit2/libera-reader', 'text', NULL, NULL, '{"ID":"b95a19a6-5448-42f0-af06-177e95e27f49","sequence":255}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.667006Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Monica: On-screen anotation software\nhttps://github.com/tasuren/monica', 'text', NULL, NULL, '{"sequence":256,"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad"}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.667352Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:419b2df8-0121-4532-8dcd-21f04df806d8', 'block:01806047-9cf8-42fe-8391-6d608bfade9e', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'Graph vis', 'text', NULL, NULL, '{"sequence":257,"ID":"419b2df8-0121-4532-8dcd-21f04df806d8"}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T16:50:24.667684Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:f520a9ff-71bf-4a72-8777-9864bad7c535', 'block:419b2df8-0121-4532-8dcd-21f04df806d8', 'doc:b5af8c53-ac31-420d-be58-3fb35a999916', 'https://github.com/jerlendds/gpug', 'text', NULL, NULL, '{"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535","sequence":258}', 1773939024061, 1773939024087, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 838ms
-- [actor_query] 2026-03-19T16:50:25.505787Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:25.513646Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.514412Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_exec] 2026-03-19T16:50:25.514598Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.515381Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.516113Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_441ba8cd9ee4ed5d';

-- [actor_exec] 2026-03-19T16:50:25.516312Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T16:50:25.523844Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_441ba8cd9ee4ed5d';

-- [actor_exec] 2026-03-19T16:50:25.524061Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.524734Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_441ba8cd9ee4ed5d';

-- [actor_exec] 2026-03-19T16:50:25.524984Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T16:50:25.525665Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_441ba8cd9ee4ed5d AS SELECT _v2.*, json_extract(_v2."properties", '$.sequence') AS "sequence", json_extract(_v2."properties", '$.collapse_to') AS "collapse_to", json_extract(_v2."properties", '$.ideal_width') AS "ideal_width", json_extract(_v2."properties", '$.column_priority') AS "priority" FROM block AS _v0 JOIN block AS _v2 ON _v2.parent_id = _v0.id WHERE _v0."id" = 'block:root-layout' AND _v2."content_type" = 'text';

-- Wait 96ms
-- [actor_exec] 2026-03-19T16:50:25.622537Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T16:50:25.630283Z
SELECT * FROM watch_view_441ba8cd9ee4ed5d;

-- [actor_exec] 2026-03-19T16:50:25.630555Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.631417Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.632140Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.632783Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.633438Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.634053Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:25.634733Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.635398Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_64c720ee4172de97';

-- [actor_query] 2026-03-19T16:50:25.635608Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T16:50:25.635779Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_108228dcd523dde5';

-- [actor_exec] 2026-03-19T16:50:25.635942Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.636538Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_64c720ee4172de97';

-- [actor_exec] 2026-03-19T16:50:25.636730Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.637367Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_64c720ee4172de97';

-- [actor_exec] 2026-03-19T16:50:25.637592Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T16:50:25.638214Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_64c720ee4172de97 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c' OR parent_id = 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c';

-- Wait 15ms
-- [actor_exec] 2026-03-19T16:50:25.653635Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.654442Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T16:50:25.654720Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T16:50:25.655260Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.655934Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_15d1b245264ba81d';

-- [actor_query] 2026-03-19T16:50:25.656160Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:25.663508Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T16:50:25.664229Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_15d1b245264ba81d AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa' OR parent_id = 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa';

-- Wait 5ms
-- [actor_exec] 2026-03-19T16:50:25.669663Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T16:50:25.677491Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_108228dcd523dde5';

-- [actor_query] 2026-03-19T16:50:25.677720Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T16:50:25.678247Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.678906Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T16:50:25.679078Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_108228dcd523dde5';

-- [actor_query] 2026-03-19T16:50:25.679296Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_exec] 2026-03-19T16:50:25.679415Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T16:50:25.680015Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_108228dcd523dde5 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c' OR parent_id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c';

-- Wait 12ms
-- [actor_exec] 2026-03-19T16:50:25.692397Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.693115Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_4348389a5df1b560';

-- [actor_query] 2026-03-19T16:50:25.693325Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T16:50:25.693505Z
-- Loads a block together with its query source child and optional render source sibling.
-- The ('holon_prql', 'holon_gql', 'holon_sql') placeholder is filled at compile time with QueryLanguage::sql_;

-- [actor_exec] 2026-03-19T16:50:25.694016Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.694655Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_a41eaf3ca30d73c2';

-- [actor_query] 2026-03-19T16:50:25.694901Z
SELECT path FROM block_with_path WHERE id = $block_id LIMIT 1;

-- [actor_exec] 2026-03-19T16:50:25.695050Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T16:50:25.695681Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_a41eaf3ca30d73c2 AS SELECT * FROM document WHERE name <> '' AND name <> 'index' AND name <> '__default__';

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:25.702817Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T16:50:25.710609Z
SELECT * FROM watch_view_a41eaf3ca30d73c2;

-- [actor_query] 2026-03-19T16:50:25.710768Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_4348389a5df1b560';

-- [actor_exec] 2026-03-19T16:50:25.711067Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:25.711739Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_c76e152ae78174ad';

-- [actor_query] 2026-03-19T16:50:25.711934Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_4348389a5df1b560';

-- [actor_exec] 2026-03-19T16:50:25.712163Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_ddl] 2026-03-19T16:50:25.712807Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_4348389a5df1b560 AS WITH RECURSIVE _vl2 AS (SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth, CAST(_v1.id AS TEXT) AS visited FROM block AS _v1 UNION ALL SELECT _fk.id, _vl2.source_id, _vl2.depth + 1, _vl2.visited || ',' || CAST(_fk.id AS TEXT) FROM _vl2 JOIN block _fk ON _fk.parent_id = _vl2.node_id WHERE _vl2.depth < 20 AND ',' || _vl2.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%') SELECT _v3.*, json_extract(_v3."properties", '$.sequence') AS "sequence" FROM focus_roots AS _v0 JOIN block AS _v1 ON _v1."id" = _v0."root_id" JOIN _vl2 ON _vl2.source_id = _v1.id JOIN block AS _v3 ON _v3.id = _vl2.node_id WHERE _v0."region" = 'main' AND _v3."content_type" <> 'source' AND _vl2.depth >= 0 AND _vl2.depth <= 20;

-- Wait 833ms
-- [actor_exec] 2026-03-19T16:50:26.546518Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:26.547219Z
SELECT * FROM watch_view_4348389a5df1b560;

-- [actor_query] 2026-03-19T16:50:26.547372Z
SELECT name FROM sqlite_master WHERE type='view' AND name='watch_view_c76e152ae78174ad';

-- [actor_exec] 2026-03-19T16:50:26.547594Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_query] 2026-03-19T16:50:26.548373Z
SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '__turso_internal_dbsp_state_v%_watch_view_c76e152ae78174ad';

-- [actor_exec] 2026-03-19T16:50:26.548669Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_ddl] 2026-03-19T16:50:26.556842Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_c76e152ae78174ad AS WITH children AS (SELECT * FROM block WHERE parent_id = 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c' AND content_type <> 'source') SELECT * FROM children;

-- Wait 30ms
-- [actor_exec] 2026-03-19T16:50:26.587442Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_query] 2026-03-19T16:50:26.595067Z
SELECT * FROM watch_view_c76e152ae78174ad;

-- [actor_exec] 2026-03-19T16:50:26.595311Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.596116Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.596767Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.597418Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.598088Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.598717Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.599334Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.599945Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.600586Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.601184Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.601857Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.609514Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.610206Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.610800Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.611381Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.618919Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.619643Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.620276Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.627948Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.628652Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.629227Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.636795Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.637616Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.638250Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.638869Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.639510Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.640097Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.640706Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.641299Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.641905Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.642474Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.643058Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.643608Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.644185Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.644766Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.652349Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.653104Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.653779Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.661619Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.662346Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.662971Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.670558Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.671254Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.671947Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.679600Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.680300Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.680879Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.681480Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.682055Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.682616Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.683160Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.683759Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.684386Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.684997Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.685579Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.686150Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.686720Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.694356Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.695074Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.695641Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.696203Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.703639Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.704348Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.704986Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.705581Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.712993Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.713794Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.714383Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.722109Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.722857Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.723450Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.724024Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.724592Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.725162Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.725755Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.726319Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.726887Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.727492Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.728074Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.728639Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.729198Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.736955Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.737662Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.738270Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.738839Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.746390Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.747040Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.747618Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.748212Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.755733Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.756435Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.757007Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.764705Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.765377Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.765966Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.766543Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.767122Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.767758Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.768380Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.768957Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.769534Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.770103Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.770701Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.771301Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.771863Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.779547Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.780265Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.780839Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.781393Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.788931Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.789575Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.790163Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.790730Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.798190Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.798874Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.799466Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.806915Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.807602Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.808214Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.808804Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.809381Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.809944Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.810552Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.811128Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.811707Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.812328Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.812891Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.813444Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.814005Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.814547Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.822251Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.823044Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.823720Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.824292Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.832084Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.832839Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.833499Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.841123Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.841857Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.842425Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.843002Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.843553Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.851192Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.851914Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.852495Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.853080Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.853680Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.854238Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.854808Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.855372Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.855916Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.856455Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.857013Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 8ms
-- [actor_exec] 2026-03-19T16:50:26.865562Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.866280Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.866907Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.867492Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.874943Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.875638Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.876238Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.876794Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.884362Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.885034Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.885623Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.886174Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.893625Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.894300Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.894881Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.895452Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.895998Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.896585Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.897131Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.897668Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.898226Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.898774Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.899351Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.899968Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.900533Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.907854Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.908539Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.909121Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.909689Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.917128Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.917760Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.918349Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.918943Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.926229Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.926889Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.927439Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.928032Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.935450Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.936085Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.936648Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.937248Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.937809Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.938383Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.938983Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.939638Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.940214Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.940781Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.941327Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.941901Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.942443Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.950060Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.950734Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.951318Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.951872Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.959148Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.959840Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.960428Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.960973Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.968607Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.969285Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.969826Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.970378Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 7ms
-- [actor_exec] 2026-03-19T16:50:26.977913Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.978585Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.979199Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.979769Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.980351Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.980904Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.981441Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.981997Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.982542Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.983309Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- [actor_exec] 2026-03-19T16:50:26.983864Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;

-- Wait 64888ms
-- [actor_exec] 2026-03-19T16:51:31.872090Z
UPDATE block SET properties = json_set(COALESCE(properties, '{}'), '$.task_state', 'DONE') WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';

-- Wait 5ms
-- [actor_query] 2026-03-19T16:51:31.877926Z
SELECT parent_id FROM block WHERE id = 'block:225edb45-f670-445a-9162-18c150210ee6';

-- [actor_query] 2026-03-19T16:51:31.878233Z
SELECT parent_id FROM block WHERE id = 'block:661368d9-e4bd-4722-b5c2-40f32006c643';

-- [actor_query] 2026-03-19T16:51:31.878443Z
SELECT parent_id FROM block WHERE id = 'block:599b60af-960d-4c9c-b222-d3d9de95c513';

-- [actor_exec] 2026-03-19T16:51:31.878630Z
INSERT INTO events (
    id, event_type, aggregate_type, aggregate_id, origin, status,
    payload, trace_id, command_id, created_at, speculative_id, rejection_reason
) VALUES (?, ?, ?, ?, ?, ?, ?, ?,;

-- Wait 12ms
-- [actor_query] 2026-03-19T16:51:31.890711Z
UPDATE operation SET status = $new_status WHERE status = $old_status;

-- [actor_query] 2026-03-19T16:51:31.891157Z
INSERT INTO operation (operation, inverse, status, created_at, display_name, entity_name, op_name)
                          VALUES ($operation, $inverse, $status, $created_at, $display_name, $entity_;

-- [actor_query] 2026-03-19T16:51:31.891490Z
SELECT last_insert_rowid() as id;

-- [actor_query] 2026-03-19T16:51:31.891778Z
SELECT COUNT(*) as count FROM operation;

-- Wait 6ms
-- [actor_exec] 2026-03-19T16:51:31.898087Z
UPDATE events SET processed_by_cache = 1 WHERE id = ?;
