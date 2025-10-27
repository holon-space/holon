-- [actor_ddl] 2026-03-04T09:02:18.658834Z
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

-- Wait 5ms
-- [actor_ddl] 2026-03-04T09:02:18.664046Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block(parent_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-04T09:02:18.665064Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block(document_id);

-- [actor_ddl] 2026-03-04T09:02:18.665711Z
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

-- [actor_ddl] 2026-03-04T09:02:18.666367Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document(parent_id);

-- [actor_ddl] 2026-03-04T09:02:18.666898Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document(name);

-- Wait 1ms
-- [actor_ddl] 2026-03-04T09:02:18.668376Z
CREATE TABLE IF NOT EXISTS file (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    document_id TEXT,
    _change_origin TEXT
);

-- Wait 1ms
-- [actor_ddl] 2026-03-04T09:02:18.669616Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file(document_id);

-- [actor_ddl] 2026-03-04T09:02:18.670158Z
CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

-- [actor_ddl] 2026-03-04T09:02:18.670929Z
CREATE INDEX IF NOT EXISTS idx_navigation_history_region
ON navigation_history(region);

-- [actor_ddl] 2026-03-04T09:02:18.671350Z
CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- [actor_ddl] 2026-03-04T09:02:18.671981Z
DROP VIEW IF EXISTS focus_roots;

-- [actor_ddl] 2026-03-04T09:02:18.672108Z
DROP VIEW IF EXISTS current_focus;

-- [actor_ddl] 2026-03-04T09:02:18.672176Z
CREATE MATERIALIZED VIEW current_focus AS
SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Wait 4ms
-- [actor_ddl] 2026-03-04T09:02:18.676510Z
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
-- [execute_sql] 2026-03-04T09:02:18.683945Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ('main', NULL);

-- [execute_sql] 2026-03-04T09:02:18.684777Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ('left_sidebar', NULL);

-- [execute_sql] 2026-03-04T09:02:18.685336Z
INSERT OR IGNORE INTO navigation_cursor (region, history_id) VALUES ('right_sidebar', NULL);

-- Wait 3ms
-- [actor_ddl] 2026-03-04T09:02:18.688364Z
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

-- Wait 40ms
-- [actor_ddl] 2026-03-04T09:02:18.728667Z
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

-- [actor_ddl] 2026-03-04T09:02:18.728997Z
CREATE INDEX IF NOT EXISTS idx_block_parent_id ON block (parent_id);

-- [actor_ddl] 2026-03-04T09:02:18.729111Z
CREATE INDEX IF NOT EXISTS idx_block_document_id ON block (document_id);

-- Wait 5ms
-- [actor_ddl] 2026-03-04T09:02:18.734419Z
CREATE TABLE IF NOT EXISTS file (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  parent_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  document_id TEXT,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-04T09:02:18.734596Z
CREATE INDEX IF NOT EXISTS idx_file_document_id ON file (document_id);

-- [actor_ddl] 2026-03-04T09:02:18.734955Z
CREATE MATERIALIZED VIEW events_view_block AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'block';

-- Wait 13ms
-- [actor_ddl] 2026-03-04T09:02:18.748207Z
CREATE MATERIALIZED VIEW events_view_directory AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'directory';

-- Wait 12ms
-- [actor_ddl] 2026-03-04T09:02:18.760672Z
CREATE MATERIALIZED VIEW events_view_file AS SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'file';

-- Wait 9411ms
-- [actor_ddl] 2026-03-04T09:02:28.171626Z
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

-- Wait 2ms
-- [actor_ddl] 2026-03-04T09:02:28.173697Z
CREATE INDEX IF NOT EXISTS idx_document_parent_id ON document (parent_id);

-- Wait 1ms
-- [actor_ddl] 2026-03-04T09:02:28.174698Z
CREATE INDEX IF NOT EXISTS idx_document_name ON document (name);

-- [execute_sql] 2026-03-04T09:02:28.175426Z
INSERT OR IGNORE INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('doc:__root__', 'sentinel:no_parent', '', 'a0', $properties, 1772614948175, 1772614948175);

-- Wait 10ms
-- [execute_sql] 2026-03-04T09:02:28.185437Z
SELECT * FROM document WHERE parent_id = 'doc:__root__' AND name = 'index';

-- Wait 1ms
-- [execute_sql] 2026-03-04T09:02:28.186585Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'doc:__root__', 'index', 'a0', $properties, 1772614948186, 1772614948186);

-- Wait 18ms
-- [transaction_stmt] 2026-03-04T09:02:28.204862Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:root-layout', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Holon Layout', 'text', NULL, NULL, '{"sequence":0,"ID":"root-layout"}', 1772614948189, 1772614948192, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 9ms
-- [transaction_stmt] 2026-03-04T09:02:28.213795Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:root-layout::src::0', 'block:root-layout', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'MATCH (root:Block)<-[:CHILD_OF]-(d:Block)\nWHERE root.id = ''block:root-layout'' AND d.content_type = ''text''\nRETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority\nORDER BY d.properties.sequence\n', 'source', 'holon_gql', NULL, '{"ID":"root-layout::src::0","sequence":1}', 1772614948189, 1772614948206, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 7ms
-- [transaction_stmt] 2026-03-04T09:02:28.221046Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:holon-app-layout::render::0', 'block:root-layout', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'columns(#{gap: 4, sort_key: col("sequence"), item_template: block_ref()})\n', 'source', 'render', NULL, '{"ID":"holon-app-layout::render::0","sequence":2}', 1772614948189, 1772614948213, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 8ms
-- [transaction_stmt] 2026-03-04T09:02:28.229295Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'block:root-layout', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Left Sidebar', 'text', NULL, NULL, '{"sequence":3,"ID":"e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c"}', 1772614948189, 1772614948220, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 127ms
-- [transaction_stmt] 2026-03-04T09:02:28.356214Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:left_sidebar::render::0', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'list(#{sortkey: "name", item_template: clickable(row(icon("folder"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})\n', 'source', 'render', NULL, '{"ID":"block:left_sidebar::render::0","sequence":4}', 1772614948189, 1772614948227, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 96ms
-- [transaction_stmt] 2026-03-04T09:02:28.452419Z
INSERT INTO file (id, name, parent_id, content_hash, document_id, _change_origin) VALUES ('doc:__default__.org', '__default__.org', 'null', 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855', NULL, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, name = excluded.name, parent_id = excluded.parent_id, content_hash = excluded.content_hash, document_id = excluded.document_id, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-04T09:02:28.452732Z
INSERT INTO file (id, name, parent_id, content_hash, document_id, _change_origin) VALUES ('doc:ClaudeCode.org', 'ClaudeCode.org', 'null', '06fbabdbbf5c6d8cfd807aeb0733f6355c187f71d2a9d6827eaac0165ba6fb4a', NULL, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, name = excluded.name, parent_id = excluded.parent_id, content_hash = excluded.content_hash, document_id = excluded.document_id, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-04T09:02:28.453055Z
INSERT INTO file (id, name, parent_id, content_hash, document_id, _change_origin) VALUES ('doc:Projects/Holon.org', 'Holon.org', 'Projects', '81462c1d5fecae89e85aab31f2bfd612a88531b7e917a9f01117426535d1c213', NULL, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, name = excluded.name, parent_id = excluded.parent_id, content_hash = excluded.content_hash, document_id = excluded.document_id, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-04T09:02:28.453436Z
INSERT INTO file (id, name, parent_id, content_hash, document_id, _change_origin) VALUES ('doc:index.org', 'index.org', 'null', 'f66dddf0a70c64a7b2bde27c6a7f2b5dab680b59756b3b94f4d67753908da9d2', NULL, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, name = excluded.name, parent_id = excluded.parent_id, content_hash = excluded.content_hash, document_id = excluded.document_id, _change_origin = excluded._change_origin;

-- Wait 157ms
-- [transaction_stmt] 2026-03-04T09:02:28.610927Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:left_sidebar::src::0', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'from document\nfilter name != ""\n', 'source', 'holon_prql', NULL, '{"ID":"block:left_sidebar::src::0","sequence":5}', 1772614948189, 1772614948237, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 12ms
-- [transaction_stmt] 2026-03-04T09:02:28.623318Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'All Documents', 'text', NULL, NULL, '{"ID":"e8b05308-37ed-49a6-9c94-bccf9e3499bc","sequence":6}', 1772614948189, 1772614948449, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.627180Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:66c6aae4-4829-4d54-b92f-6638fda03368', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Test', 'text', NULL, NULL, '{"ID":"66c6aae4-4829-4d54-b92f-6638fda03368","sequence":7}', 1772614948189, 1772614948624, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.631127Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:88862721-ed4f-43ba-9222-f84f17c6692e', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Favorites', 'text', NULL, NULL, '{"ID":"88862721-ed4f-43ba-9222-f84f17c6692e","sequence":8}', 1772614948189, 1772614948628, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.635234Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a5d47f54-8632-412b-8844-7762121788b6', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Recently Opened', 'text', NULL, NULL, '{"ID":"a5d47f54-8632-412b-8844-7762121788b6","sequence":9}', 1772614948189, 1772614948632, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.639468Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'block:root-layout', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Main Panel', 'text', NULL, NULL, '{"ID":"03ad3820-2c9d-42d1-85f4-8b5695df22fa","sequence":10}', 1772614948189, 1772614948636, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.644003Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:main::src::0', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'MATCH (fr:FocusRoot), (root:Block)<-[:CHILD_OF*0..20]-(d:Block)\nWHERE fr.region = ''main'' AND root.id = fr.root_id AND d.content_type <> ''source''\nRETURN d, d.properties.sequence AS sequence\nORDER BY d.properties.sequence\n', 'source', 'holon_gql', NULL, '{"ID":"main::src::0","sequence":11}', 1772614948189, 1772614948641, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.648623Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:main::render::0', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_block()})\n', 'source', 'render', NULL, '{"sequence":12,"ID":"main::render::0"}', 1772614948189, 1772614948645, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.653406Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Graph View', 'text', NULL, NULL, '{"ID":"aaca22e0-1b52-479b-891e-c55dcfc308f4","sequence":13}', 1772614948189, 1772614948650, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.659031Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'list(#{item_template: row(text(col("content")))})\n', 'source', 'render', NULL, '{"sequence":14,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1"}', 1772614948189, 1772614948654, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.664071Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'MATCH (b:Block) WHERE b.content_type = ''text'' RETURN b\n', 'source', 'holon_gql', NULL, '{"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0","sequence":15}', 1772614948189, 1772614948660, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.668830Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'block:root-layout', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Right Sidebar', 'text', NULL, NULL, '{"sequence":16,"ID":"cf7e0570-0e50-46ae-8b33-8c4b4f82e79c"}', 1772614948189, 1772614948665, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.673341Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:right_sidebar::render::0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'list(#{item_template: render_block()})\n', 'source', 'render', NULL, '{"ID":"block:right_sidebar::render::0","sequence":17}', 1772614948189, 1772614948670, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.678110Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:right_sidebar::src::0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'from children\n', 'source', 'holon_prql', NULL, '{"ID":"block:right_sidebar::src::0","sequence":18}', 1772614948189, 1772614948674, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.683837Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:510a2669-402e-4d35-a161-4a2c259ed519', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Another pointer that gets shuffled around', 'text', NULL, NULL, '{"ID":"510a2669-402e-4d35-a161-4a2c259ed519","sequence":19}', 1772614948189, 1772614948679, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.688888Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cffccf2a-7792-4b6d-a600-f8b31dc086b0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Context Panel is reactive again!', 'text', NULL, NULL, '{"sequence":20,"ID":"cffccf2a-7792-4b6d-a600-f8b31dc086b0"}', 1772614948189, 1772614948685, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.694264Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4510fef8-f1c5-47b8-805b-8cd2c4905909', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Quick Capture', 'text', NULL, NULL, '{"ID":"4510fef8-f1c5-47b8-805b-8cd2c4905909","sequence":21}', 1772614948189, 1772614948690, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.698708Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:0c5c95a1-5202-427f-b714-86bec42fae89', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'Block Profiles', 'text', NULL, NULL, '{"ID":"0c5c95a1-5202-427f-b714-86bec42fae89","sequence":22}', 1772614948189, 1772614948695, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.704788Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:blocks-profile::src::0', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb', 'entity_name: block\n\ncomputed:\n  is_task: ''= task_state != ()''\n  is_source: ''= content_type == "source"''\n  has_query_source: ''= query_source(id) != ()''\n\ndefault:\n  render: ''row(icon("orgmode"), spacer(8), editable_text(col("content")))''\n\nvariants:\n  - name: query_block\n    condition: ''= has_query_source''\n    render: ''block_ref()''\n  - name: task\n    condition: ''= is_task''\n    render: ''row(state_toggle(col("task_state")), spacer(8), editable_text(col("content")))''\n  - name: source\n    condition: ''= is_source''\n    render: ''source_editor(#{language: col("source_language"), content: col("content")})''\n', 'source', 'holon_entity_profile_yaml', NULL, '{"sequence":23,"ID":"block:blocks-profile::src::0"}', 1772614948189, 1772614948700, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [execute_sql] 2026-03-04T09:02:28.709414Z
SELECT * FROM document WHERE parent_id = 'doc:__root__' AND name = '__default__';

-- [execute_sql] 2026-03-04T09:02:28.709895Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('doc:de557497-5c37-4ad2-8050-cf0baa719146', 'doc:__root__', '__default__', 'a0', $properties, 1772614948709, 1772614948709);

-- Wait 1ms
-- [execute_sql] 2026-03-04T09:02:28.711337Z
SELECT * FROM document WHERE parent_id = 'doc:__root__' AND name = 'ClaudeCode';

-- [execute_sql] 2026-03-04T09:02:28.711540Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'doc:__root__', 'ClaudeCode', 'a0', $properties, 1772614948711, 1772614948711);

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.717707Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-history-root', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'Claude Code History', 'text', NULL, NULL, '{"sequence":0,"ID":"cc-history-root"}', 1772614948712, 1772614948714, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 19ms
-- [transaction_stmt] 2026-03-04T09:02:28.736677Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-projects', 'block:cc-history-root', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'Projects', 'text', NULL, NULL, '{"sequence":1,"ID":"cc-projects"}', 1772614948712, 1772614948716, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 63ms
-- [transaction_stmt] 2026-03-04T09:02:28.799872Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-projects::src::0', 'block:cc-projects', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'from project\nselect {id, original_path, session_count, last_activity}\nsort {-last_activity}\n', 'source', 'holon_prql', NULL, '{"sequence":2,"ID":"block:cc-projects::src::0"}', 1772614948712, 1772614948720, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.806128Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-projects::render::0', 'block:cc-projects', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'list(#{item_template: row(text(col("original_path")), spacer(16), text(col("session_count")), spacer(8), text(col("last_activity")))})\n', 'source', 'render', NULL, '{"ID":"block:cc-projects::render::0","sequence":3}', 1772614948712, 1772614948799, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 9ms
-- [transaction_stmt] 2026-03-04T09:02:28.815203Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-sessions', 'block:cc-history-root', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'Recent Sessions', 'text', NULL, NULL, '{"sequence":4,"ID":"cc-sessions"}', 1772614948712, 1772614948806, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.820389Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-sessions::src::0', 'block:cc-sessions', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'from session\nfilter message_count > 0\nselect {id, first_prompt, message_count, model, modified, git_branch}\nsort {-modified}\ntake 30\n', 'source', 'holon_prql', NULL, '{"ID":"block:cc-sessions::src::0","sequence":5}', 1772614948712, 1772614948811, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 15ms
-- [transaction_stmt] 2026-03-04T09:02:28.835274Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-sessions::render::0', 'block:cc-sessions', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'list(#{item_template: row(text(col("first_prompt")), spacer(16), text(col("message_count")), spacer(8), text(col("modified")))})\n', 'source', 'render', NULL, '{"sequence":6,"ID":"block:cc-sessions::render::0"}', 1772614948712, 1772614948820, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 10ms
-- [transaction_stmt] 2026-03-04T09:02:28.844840Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-tasks', 'block:cc-history-root', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'Tasks', 'text', NULL, NULL, '{"ID":"cc-tasks","sequence":7}', 1772614948712, 1772614948825, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.849227Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-tasks::src::0', 'block:cc-tasks', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'from task\nfilter status == "in_progress"\nselect {id, subject, status, created_at}\nsort {-created_at}\n', 'source', 'holon_prql', NULL, '{"sequence":8,"ID":"block:cc-tasks::src::0"}', 1772614948712, 1772614948844, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 7ms
-- [transaction_stmt] 2026-03-04T09:02:28.855873Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-tasks::render::0', 'block:cc-tasks', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af', 'list(#{item_template: row(text(col("status")), spacer(8), text(col("subject")))})\n', 'source', 'render', NULL, '{"sequence":9,"ID":"block:cc-tasks::render::0"}', 1772614948712, 1772614948849, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [execute_sql] 2026-03-04T09:02:28.859633Z
SELECT * FROM document WHERE parent_id = 'doc:__root__' AND name = 'Projects';

-- [execute_sql] 2026-03-04T09:02:28.860316Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('doc:10371071-2098-43bf-9a0d-9e33e9248e10', 'doc:__root__', 'Projects', 'a0', $properties, 1772614948860, 1772614948860);

-- [execute_sql] 2026-03-04T09:02:28.860650Z
SELECT * FROM document WHERE parent_id = 'doc:10371071-2098-43bf-9a0d-9e33e9248e10' AND name = 'Holon';

-- [execute_sql] 2026-03-04T09:02:28.860797Z
SELECT * FROM document WHERE id = 'doc:10371071-2098-43bf-9a0d-9e33e9248e10' LIMIT 1;

-- [execute_sql] 2026-03-04T09:02:28.860932Z
INSERT INTO document (id, parent_id, name, sort_key, properties, created_at, updated_at)
VALUES ('doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10', 'Holon', 'a0', $properties, 1772614948860, 1772614948860);

-- Wait 15ms
-- [transaction_stmt] 2026-03-04T09:02:28.876133Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:92aee526-5e48-45fe-a0ba-c9c0857d7e5d', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Phase 1: Core Outliner', 'text', NULL, NULL, '{"sequence":0,"ID":"92aee526-5e48-45fe-a0ba-c9c0857d7e5d"}', 1772614948866, 1772614948872, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.881720Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:7b3ff315-4ed8-4602-a0cc-464fe0774acb', 'block:92aee526-5e48-45fe-a0ba-c9c0857d7e5d', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'MCP Server Frontend [/]', 'text', NULL, NULL, '{"sequence":1,"ID":"7b3ff315-4ed8-4602-a0cc-464fe0774acb"}', 1772614948866, 1772614948874, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.887691Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:db59d038-8a47-43e9-9502-0472b493a6b9', 'block:7b3ff315-4ed8-4602-a0cc-464fe0774acb', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Context parameter support ($context_id, $context_parent_id)', 'text', NULL, NULL, '{"ID":"db59d038-8a47-43e9-9502-0472b493a6b9","sequence":2}', 1772614948866, 1772614948880, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.893318Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:95ad6166-c03c-4417-a435-349e88b8e90a', 'block:7b3ff315-4ed8-4602-a0cc-464fe0774acb', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'MCP server (stdio + HTTP modes)', 'text', NULL, NULL, '{"ID":"95ad6166-c03c-4417-a435-349e88b8e90a","sequence":3}', 1772614948866, 1772614948887, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.899285Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', 'block:7b3ff315-4ed8-4602-a0cc-464fe0774acb', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'MCP tools for query execution and operations', 'text', NULL, NULL, '{"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b","sequence":4}', 1772614948866, 1772614948893, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.905284Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:661368d9-e4bd-4722-b5c2-40f32006c643', 'block:92aee526-5e48-45fe-a0ba-c9c0857d7e5d', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Block Operations [/]', 'text', NULL, NULL, '{"ID":"661368d9-e4bd-4722-b5c2-40f32006c643","sequence":5}', 1772614948866, 1772614948899, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.909940Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Block hierarchy (parent/child, indent/outdent)', 'text', NULL, NULL, '{"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007","sequence":6}', 1772614948866, 1772614948905, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.914852Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Split block operation', 'text', NULL, NULL, '{"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34","sequence":7}', 1772614948866, 1772614948909, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.919788Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Block CRUD (create, read, update, delete)', 'text', NULL, NULL, '{"sequence":8,"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb"}', 1772614948866, 1772614948914, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.925686Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Block movement (move_up, move_down, move_block)', 'text', NULL, NULL, '{"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63","sequence":9}', 1772614948866, 1772614948919, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.930183Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:225edb45-f670-445a-9162-18c150210ee6', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Undo/redo system (UndoStack + persistent OperationLogStore)', 'text', NULL, NULL, '{"ID":"225edb45-f670-445a-9162-18c150210ee6","sequence":10,"task_state":"TODO"}', 1772614948866, 1772614948925, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.935531Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:92aee526-5e48-45fe-a0ba-c9c0857d7e5d', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Storage & Data Layer [/]', 'text', NULL, NULL, '{"sequence":11,"ID":"444b24f6-d412-43c4-a14b-6e725b673cee"}', 1772614948866, 1772614948930, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 8ms
-- [transaction_stmt] 2026-03-04T09:02:28.943243Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Schema Module system with topological dependency ordering', 'text', NULL, NULL, '{"sequence":12,"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae"}', 1772614948866, 1772614948935, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 7ms
-- [transaction_stmt] 2026-03-04T09:02:28.949746Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Fractional indexing for block ordering', 'text', NULL, NULL, '{"sequence":13,"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699"}', 1772614948866, 1772614948943, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.955416Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:1e0cf8f7-28e1-4748-a682-ce07be956b57', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Turso (embedded SQLite) backend with connection pooling', 'text', NULL, NULL, '{"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57","sequence":14}', 1772614948866, 1772614948949, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.961351Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'QueryableCache wrapping DataSource with local caching', 'text', NULL, NULL, '{"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280","sequence":15}', 1772614948866, 1772614948955, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.966748Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Entity derive macro (#[derive(Entity)]) for schema generation', 'text', NULL, NULL, '{"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7","sequence":16}', 1772614948866, 1772614948961, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 4ms
-- [transaction_stmt] 2026-03-04T09:02:28.971236Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d318cae4-759d-487b-a909-81940223ecc1', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'CDC (Change Data Capture) streaming from storage to UI', 'text', NULL, NULL, '{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}', 1772614948866, 1772614948966, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [transaction_stmt] 2026-03-04T09:02:28.977034Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Command sourcing infrastructure (append-only operation log)', 'text', NULL, NULL, '{"task_state":"DOING","ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222","sequence":18}', 1772614948866, 1772614948971, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 7ms
-- [transaction_stmt] 2026-03-04T09:02:28.983833Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'block:92aee526-5e48-45fe-a0ba-c9c0857d7e5d', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', 'Procedural Macros [/]', 'text', NULL, NULL, '{"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","sequence":19}', 1772614948866, 1772614948977, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 5ms
-- [transaction_stmt] 2026-03-04T09:02:28.988775Z
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d', '#[operations_trait] macro for operation dispatch generation', 'text', NULL, NULL, '{"sequence":20,"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4"}', 1772614948866, 1772614948983, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- Wait 6ms
-- [actor_ddl] 2026-03-04T09:02:31.067559Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_eb3125ab79aead8f AS WITH RECURSIVE _vl2 AS (SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth, CAST(_v1.id AS TEXT) AS visited FROM block AS _v1 UNION ALL SELECT _fk.id, _vl2.source_id, _vl2.depth + 1, _vl2.visited || ',' || CAST(_fk.id AS TEXT) FROM _vl2 JOIN block _fk ON _fk.parent_id = _vl2.node_id WHERE _vl2.depth < 20 AND ',' || _vl2.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%') SELECT _v3.*, json_extract(_v3."properties", '$.sequence') AS "sequence", 'focus_roots' AS entity_name FROM focus_roots AS _v0 JOIN block AS _v1 ON _v1."id" = _v0."root_id" JOIN _vl2 ON _vl2.source_id = _v1.id JOIN block AS _v3 ON _v3.id = _vl2.node_id WHERE _v0."region" = 'main' AND _v3."content_type" <> 'source' AND _vl2.depth >= 0 AND _vl2.depth <= 20;

-- Wait 8ms
-- [execute_sql] 2026-03-04T09:02:44.229607Z
SELECT * FROM document WHERE parent_id = 'doc:10371071-2098-43bf-9a0d-9e33e9248e10' AND name = 'Holon';

-- Wait 16ms
-- [execute_sql] 2026-03-04T09:02:44.245898Z
SELECT * FROM document WHERE parent_id = 'doc:__root__' AND name = '__default__';

-- Wait 10ms
-- [execute_sql] 2026-03-04T09:02:44.255886Z
SELECT * FROM document WHERE parent_id = 'doc:__root__' AND name = 'ClaudeCode';

-- Wait 2303881ms
-- [execute_sql] 2026-03-04T09:41:08.136997Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- Wait 2ms
-- [execute_sql] 2026-03-04T09:41:08.138587Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 0;

-- [execute_sql] 2026-03-04T09:41:08.138908Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d');

-- [execute_sql] 2026-03-04T09:41:08.139817Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:08.140079Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1);

-- Wait 1513ms
-- [execute_sql] 2026-03-04T09:41:09.653119Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:09.653917Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 1;

-- [execute_sql] 2026-03-04T09:41:09.654543Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- Wait 2ms
-- [execute_sql] 2026-03-04T09:41:09.656270Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:09.656564Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 2);

-- Wait 1147ms
-- [execute_sql] 2026-03-04T09:41:10.803438Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:10.804368Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 2;

-- [execute_sql] 2026-03-04T09:41:10.804881Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af');

-- Wait 1ms
-- [execute_sql] 2026-03-04T09:41:10.806154Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:10.806514Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 3);

-- Wait 50ms
-- [actor_ddl] 2026-03-04T09:41:10.856472Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_5e4c31e8664a1ce3 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cc-projects' OR parent_id = 'block:cc-projects';

-- Wait 13ms
-- [actor_ddl] 2026-03-04T09:41:10.869188Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_226d0677b6b77cbb AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cc-sessions' OR parent_id = 'block:cc-sessions';

-- Wait 11ms
-- [actor_ddl] 2026-03-04T09:41:10.880049Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_bb3bb45b22aca539 AS SELECT id, content, content_type, source_language, parent_id FROM block WHERE id = 'block:cc-tasks' OR parent_id = 'block:cc-tasks';

-- Wait 11ms
-- [actor_ddl] 2026-03-04T09:41:10.890734Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_5bdc1503e9bf5cb5 AS SELECT id, original_path, session_count, last_activity, 'project' AS entity_name FROM project;

-- Wait 10ms
-- [actor_ddl] 2026-03-04T09:41:10.900312Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_c0a9f4b4dbda7fd5 AS SELECT id, first_prompt, message_count, model, modified, git_branch, 'session' AS entity_name FROM session WHERE message_count > 0 LIMIT 30;

-- Wait 9ms
-- [actor_ddl] 2026-03-04T09:41:10.908817Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_6ea6eed8aa5a8d88 AS SELECT id, subject, status, created_at, 'task' AS entity_name FROM task WHERE status = 'in_progress'
thread 'tokio-runtime-worker' (64166768) panicked at /Users/martin/Workspaces/bigdata/turso/core/storage/btree.rs:5813:21:
is_empty
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
══╡ EXCEPTION CAUGHT BY RENDERING LIBRARY ╞═════════════════════════════════════════════════════════
The following assertion was thrown during performResize():
Vertical viewport was given unbounded height.
Viewports expand in the scrolling direction to fill their container. In this case, a vertical
viewport was given an unlimited amount of vertical space in which to expand. This situation
typically happens when a scrollable widget is nested inside another scrollable widget.
If this widget is always nested in a scrollable widget there is no need to use a viewport because
there will always be enough vertical space for the children. In this case, consider using a Column
or Wrap instead. Otherwise, consider using a CustomScrollView to concatenate arbitrary slivers into
a single scrollable.;

-- Wait 844ms
-- [execute_sql] 2026-03-04T09:41:11.753142Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:11.753745Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 3;

-- [execute_sql] 2026-03-04T09:41:11.754204Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- [execute_sql] 2026-03-04T09:41:11.755063Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:11.755310Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 4);

-- Wait 1033ms
-- [execute_sql] 2026-03-04T09:41:12.787860Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- Wait 1ms
-- [execute_sql] 2026-03-04T09:41:12.788944Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 4;

-- [execute_sql] 2026-03-04T09:41:12.789358Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d');

-- Wait 2ms
-- [execute_sql] 2026-03-04T09:41:12.790967Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T09:41:12.791372Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 5);

-- Wait 1916620ms
-- [execute_sql] 2026-03-04T10:13:09.410944Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:09.411421Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 4;

-- Wait 262ms
-- [execute_sql] 2026-03-04T10:13:09.673623Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af');

-- [execute_sql] 2026-03-04T10:13:09.674312Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:09.674566Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 6);

-- Wait 64ms
-- [actor_ddl] 2026-03-04T10:13:09.738500Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_6ea6eed8aa5a8d88 AS SELECT id, subject, status, created_at, 'task' AS entity_name FROM task WHERE status = 'in_progress';

-- Wait 11ms
-- [actor_ddl] 2026-03-04T10:13:09.749442Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_c0a9f4b4dbda7fd5 AS SELECT id, first_prompt, message_count, model, modified, git_branch, 'session' AS entity_name FROM session WHERE message_count > 0 LIMIT 30
Another exception was thrown: RenderBox was not laid out: RenderViewport#39976 NEEDS-LAYOUT NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderViewport#39976 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderIgnorePointer#fc4bb relayoutBoundary=up23 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderSemanticsAnnotations#b6174 relayoutBoundary=up22 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderPointerListener#9e126 relayoutBoundary=up21 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderSemanticsGestureHandler#432f5 relayoutBoundary=up20 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderPointerListener#6f594 relayoutBoundary=up19 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: _RenderScrollSemantics#63ad6 relayoutBoundary=up18 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderRepaintBoundary#5ffef relayoutBoundary=up17 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderCustomPaint#259bd relayoutBoundary=up16 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderMouseRegion#09bd8 relayoutBoundary=up15 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderPointerListener#b028a relayoutBoundary=up14 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderSemanticsGestureHandler#3278d relayoutBoundary=up13 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderPointerListener#57895 relayoutBoundary=up12 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderRepaintBoundary#f50b0 relayoutBoundary=up11 NEEDS-PAINT NEEDS-COMPOSITING-BITS-UPDATE
Another exception was thrown: RenderBox was not laid out: RenderRepaintBoundary#f50b0 relayoutBoundary=up11 NEEDS-PAINT
Another exception was thrown: Null check operator used on a null value;

-- Wait 1746ms
-- [execute_sql] 2026-03-04T10:13:11.495320Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:11.495818Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 6;

-- [execute_sql] 2026-03-04T10:13:11.496048Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- [execute_sql] 2026-03-04T10:13:11.496972Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:11.497134Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 7);

-- Wait 965ms
-- [execute_sql] 2026-03-04T10:13:12.462324Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:12.462787Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 7;

-- [execute_sql] 2026-03-04T10:13:12.463024Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d');

-- [execute_sql] 2026-03-04T10:13:12.463897Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:12.464135Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 8);

-- Wait 1982ms
-- [execute_sql] 2026-03-04T10:13:14.446513Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- Wait 222ms
-- [execute_sql] 2026-03-04T10:13:14.668771Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 8;

-- [execute_sql] 2026-03-04T10:13:14.669028Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb');

-- [execute_sql] 2026-03-04T10:13:14.669543Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:14.669683Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 9);

-- Wait 2062ms
-- [execute_sql] 2026-03-04T10:13:16.731232Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:16.731660Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 9;

-- [execute_sql] 2026-03-04T10:13:16.731875Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:de557497-5c37-4ad2-8050-cf0baa719146');

-- [execute_sql] 2026-03-04T10:13:16.732672Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:16.732839Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 10);

-- Wait 1565ms
-- [execute_sql] 2026-03-04T10:13:18.298193Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:18.298635Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 10;

-- [execute_sql] 2026-03-04T10:13:18.298862Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af');

-- [execute_sql] 2026-03-04T10:13:18.299650Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:18.299804Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 11);

-- Wait 3317ms
-- [execute_sql] 2026-03-04T10:13:21.616341Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:21.616773Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 11;

-- [execute_sql] 2026-03-04T10:13:21.616985Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- [execute_sql] 2026-03-04T10:13:21.617774Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:21.617926Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 12);

-- Wait 1249ms
-- [execute_sql] 2026-03-04T10:13:22.866876Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:22.867301Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 12;

-- [execute_sql] 2026-03-04T10:13:22.867515Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d');

-- [execute_sql] 2026-03-04T10:13:22.868090Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:22.868244Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 13);

-- Wait 2050ms
-- [execute_sql] 2026-03-04T10:13:24.917888Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- Wait 36ms
-- [execute_sql] 2026-03-04T10:13:24.953693Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 13;

-- [execute_sql] 2026-03-04T10:13:24.953871Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- [execute_sql] 2026-03-04T10:13:24.954346Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:24.954487Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 14);

-- Wait 2131ms
-- [execute_sql] 2026-03-04T10:13:27.085586Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:27.086026Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 14;

-- [execute_sql] 2026-03-04T10:13:27.086248Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af');

-- [execute_sql] 2026-03-04T10:13:27.087070Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:27.087227Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 15);

-- Wait 7202ms
-- [execute_sql] 2026-03-04T10:13:34.288835Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:34.289302Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 15;

-- [execute_sql] 2026-03-04T10:13:34.289527Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:9881fc53-1af0-4ba2-a173-d4d1ce010c1d');

-- [execute_sql] 2026-03-04T10:13:34.290160Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:34.290369Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 16);

-- Wait 1716ms
-- [execute_sql] 2026-03-04T10:13:36.006307Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:36.006739Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 16;

-- [execute_sql] 2026-03-04T10:13:36.006960Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- Wait 2ms
-- [execute_sql] 2026-03-04T10:13:36.009210Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:36.009370Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 17);

-- Wait 17672ms
-- [execute_sql] 2026-03-04T10:13:53.681010Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:53.681439Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 17;

-- [execute_sql] 2026-03-04T10:13:53.681671Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:de557497-5c37-4ad2-8050-cf0baa719146');

-- [execute_sql] 2026-03-04T10:13:53.682487Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:53.682652Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 18);

-- Wait 1666ms
-- [execute_sql] 2026-03-04T10:13:55.348368Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:55.348800Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 18;

-- [execute_sql] 2026-03-04T10:13:55.349022Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:10371071-2098-43bf-9a0d-9e33e9248e10');

-- [execute_sql] 2026-03-04T10:13:55.349837Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:55.349999Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 19);

-- Wait 1566ms
-- [execute_sql] 2026-03-04T10:13:56.915807Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:56.916240Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 19;

-- [execute_sql] 2026-03-04T10:13:56.916492Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:95f383d2-26c9-42eb-97fd-6f856d2a79af');

-- [execute_sql] 2026-03-04T10:13:56.917328Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:56.917497Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 20);

-- Wait 899ms
-- [execute_sql] 2026-03-04T10:13:57.816159Z
SELECT history_id FROM navigation_cursor WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:57.816588Z
DELETE FROM navigation_history WHERE region = 'main' AND id > 20;

-- [execute_sql] 2026-03-04T10:13:57.816814Z
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:5f6a0da6-03e1-48df-9a05-b63fcb016bfb');

-- [execute_sql] 2026-03-04T10:13:57.817410Z
SELECT MAX(id) as max_id FROM navigation_history WHERE region = 'main';

-- [execute_sql] 2026-03-04T10:13:57.817612Z
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 21);

