-- Minimized replay (89 statements)

-- [actor_ddl]
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

-- [actor_ddl]
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

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content", "document_id", "id", "content_type", "updated_at", "created_at", "parent_id", "properties") VALUES ('Left Sidebar', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'text', 1773158378751, 1773158378749, 'block:root-layout', '{"sequence":3,"ID":"e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("id", "content", "content_type", "parent_id", "document_id", "created_at", "updated_at", "properties") VALUES ('block:66c6aae4-4829-4d54-b92f-6638fda03368', 'Test', 'text', 'block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 1773158378749, 1773158378751, '{"sequence":7,"ID":"66c6aae4-4829-4d54-b92f-6638fda03368"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content", "updated_at", "parent_id", "created_at", "document_id", "id", "content_type", "properties") VALUES ('Favorites', 1773158378752, 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 1773158378749, 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'text', '{"sequence":8,"ID":"88862721-ed4f-43ba-9222-f84f17c6692e"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("document_id", "created_at", "updated_at", "content", "id", "content_type", "parent_id", "properties") VALUES ('doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 1773158378750, 1773158378752, 'Another pointer that gets shuffled around', 'block:510a2669-402e-4d35-a161-4a2c259ed519', 'text', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', '{"ID":"510a2669-402e-4d35-a161-4a2c259ed519","sequence":19}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "document_id", "id", "updated_at", "created_at", "content", "content_type", "properties") VALUES ('doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 1773158378752, 1773158378750, 'Block Profiles', 'text', '{"sequence":22,"ID":"0c5c95a1-5202-427f-b714-86bec42fae89"}');

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:main::render::0', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})\n', 'source', 'render', NULL, '{"sequence":12,"ID":"main::render::0"}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Graph View', 'text', NULL, NULL, '{"ID":"aaca22e0-1b52-479b-891e-c55dcfc308f4","sequence":13}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cffccf2a-7792-4b6d-a600-f8b31dc086b0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Context Panel is reactive again!', 'text', NULL, NULL, '{"ID":"cffccf2a-7792-4b6d-a600-f8b31dc086b0","sequence":20}', 1773158378750, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'list(#{item_template: row(text(col("content")))})\n', 'source', 'render', NULL, '{"sequence":14,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::1"}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:main::src::0', 'block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'MATCH (fr:FocusRoot), (root:Block)<-[:CHILD_OF*0..20]-(d:Block)\nWHERE fr.region = ''main'' AND root.id = fr.root_id AND d.content_type <> ''source''\nRETURN d, d.properties.sequence AS sequence\nORDER BY d.properties.sequence\n', 'source', 'holon_gql', NULL, '{"ID":"main::src::0","sequence":11}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:right_sidebar::src::0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'from children\n', 'source', 'holon_prql', NULL, '{"ID":"block:right_sidebar::src::0","sequence":18}', 1773158378750, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:right_sidebar::render::0', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'list(#{item_template: render_entity()})\n', 'source', 'render', NULL, '{"ID":"block:right_sidebar::render::0","sequence":17}', 1773158378750, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:left_sidebar::src::0', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'from document\nfilter name != ""\n', 'source', 'holon_prql', NULL, '{"ID":"block:left_sidebar::src::0","sequence":5}', 1773158378749, 1773158378751, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0', 'block:aaca22e0-1b52-479b-891e-c55dcfc308f4', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'MATCH (b:Block) WHERE b.content_type = ''text'' RETURN b\n', 'source', 'holon_gql', NULL, '{"sequence":15,"ID":"block:39471ed2-64b6-4b98-9782-30c6caf8f061::src::0"}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:e8b05308-37ed-49a6-9c94-bccf9e3499bc', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'All Documents', 'text', NULL, NULL, '{"sequence":6,"ID":"e8b05308-37ed-49a6-9c94-bccf9e3499bc"}', 1773158378749, 1773158378751, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:holon-app-layout::render::0', 'block:root-layout', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'columns(#{gap: 4, sort_key: col("sequence"), item_template: block_ref()})\n', 'source', 'render', NULL, '{"sequence":2,"ID":"holon-app-layout::render::0"}', 1773158378748, 1773158378751, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:03ad3820-2c9d-42d1-85f4-8b5695df22fa', 'block:root-layout', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Main Panel', 'text', NULL, NULL, '{"sequence":10,"ID":"03ad3820-2c9d-42d1-85f4-8b5695df22fa"}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:left_sidebar::render::0', 'block:e7fcc60b-2173-4a21-9f7d-52ecb1cf1b9c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'list(#{sortkey: "name", item_template: clickable(row(icon("folder"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})\n', 'source', 'render', NULL, '{"ID":"block:left_sidebar::render::0","sequence":4}', 1773158378749, 1773158378751, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'block:root-layout', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Right Sidebar', 'text', NULL, NULL, '{"sequence":16,"ID":"cf7e0570-0e50-46ae-8b33-8c4b4f82e79c"}', 1773158378750, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:4510fef8-f1c5-47b8-805b-8cd2c4905909', 'block:cf7e0570-0e50-46ae-8b33-8c4b4f82e79c', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Quick Capture', 'text', NULL, NULL, '{"ID":"4510fef8-f1c5-47b8-805b-8cd2c4905909","sequence":21}', 1773158378750, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:blocks-profile::src::0', 'block:0c5c95a1-5202-427f-b714-86bec42fae89', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'entity_name: block\n\ncomputed:\n  is_task: ''= task_state != ()''\n  is_source: ''= content_type == "source"''\n  has_query_source: ''= query_source(id) != ()''\n\ndefault:\n  render: ''row(icon("orgmode"), spacer(8), editable_text(col("content")))''\n\nvariants:\n  - name: query_block\n    condition: ''= has_query_source''\n    render: ''block_ref()''\n  - name: task\n    condition: ''= is_task''\n    render: ''row(state_toggle(col("task_state")), spacer(8), editable_text(col("content")))''\n  - name: source\n    condition: ''= is_source''\n    render: ''source_editor(#{language: col("source_language"), content: col("content")})''\n', 'source', 'holon_entity_profile_yaml', NULL, '{"ID":"block:blocks-profile::src::0","sequence":23}', 1773158378750, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:a5d47f54-8632-412b-8844-7762121788b6', 'block:88862721-ed4f-43ba-9222-f84f17c6692e', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Recently Opened', 'text', NULL, NULL, '{"sequence":9,"ID":"a5d47f54-8632-412b-8844-7762121788b6"}', 1773158378749, 1773158378752, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:root-layout::src::0', 'block:root-layout', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'MATCH (root:Block)<-[:CHILD_OF]-(d:Block)\nWHERE root.id = ''block:root-layout'' AND d.content_type = ''text''\nRETURN d, d.properties.sequence AS sequence, d.properties.collapse_to AS collapse_to, d.properties.ideal_width AS ideal_width, d.properties.column_priority AS priority\nORDER BY d.properties.sequence\n', 'source', 'holon_gql', NULL, '{"sequence":1,"ID":"root-layout::src::0"}', 1773158378748, 1773158378751, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:root-layout', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'doc:537763bf-59bf-414c-8bcc-9dbcc1fcb40f', 'Holon Layout', 'text', NULL, NULL, '{"ID":"root-layout","sequence":0}', 1773158378748, 1773158378751, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-right-sidebar::src::0', 'block:default-right-sidebar', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'from children\n', 'source', 'holon_prql', NULL, '{"ID":"default-right-sidebar::src::0","sequence":11}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-layout-root::render::0', 'block:default-layout-root', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'columns(#{gap: 4, item_template: block_ref()})\n', 'source', 'render', NULL, '{"sequence":1,"ID":"default-layout-root::render::0"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-block-profiles::src::0', 'block:default-block-profiles', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'entity_name: block\n\ncomputed:\n  is_task: ''= task_state != ()''\n  is_source: ''= content_type == "source"''\n  has_query_source: ''= query_source(id) != ()''\n  todo_states: ''= if document_id != () { let d = document(document_id); if d != () { d.todo_keywords } else { () } } else { () }''\n\ndefault:\n  render: ''row(icon("orgmode"), spacer(8), editable_text(col("content")))''\n\nvariants:\n  - name: query_block\n    condition: ''= has_query_source''\n    render: ''block_ref()''\n  - name: task\n    condition: ''= is_task''\n    render: ''row(state_toggle(col("task_state"), #{states: col("todo_states")}), spacer(8), editable_text(col("content")))''\n  - name: source\n    condition: ''= is_source''\n    render: ''source_editor(#{language: col("source_language"), content: col("content")})''\n', 'source', 'holon_entity_profile_yaml', NULL, '{"ID":"default-block-profiles::src::0","sequence":13}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-main-panel::src::0', 'block:default-main-panel', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'MATCH (fr:FocusRoot), (root:Block)<-[:CHILD_OF*0..20]-(d:Block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d\n', 'source', 'holon_gql', NULL, '{"sequence":7,"ID":"default-main-panel::src::0"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-layout-root::src::0', 'block:default-layout-root', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'from children\nfilter content_type != "source"\nderive {\n  seq = s"json_extract(properties, ''$.\\"column-order\\"'')" ?? 999999,\n  collapse_to = s"json_extract(properties, ''$.\\"collapse-to\\"'')",\n  ideal_width = s"json_extract(properties, ''$.\\"ideal-width\\"'')",\n  priority = s"json_extract(properties, ''$.\\"column-priority\\"'')"\n}\nsort seq\n', 'source', 'holon_prql', NULL, '{"sequence":2,"ID":"default-layout-root::src::0"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-block-profiles', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'Block Profiles', 'text', NULL, NULL, '{"ID":"default-block-profiles","sequence":12}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-layout-root', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'Holon Layout', 'text', NULL, NULL, '{"sequence":0,"ID":"default-layout-root"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-left-sidebar', 'block:default-layout-root', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'Left Sidebar', 'text', NULL, NULL, '{"sequence":3,"ID":"default-left-sidebar"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-right-sidebar::render::0', 'block:default-right-sidebar', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'list(#{item_template: render_entity()})\n', 'source', 'render', NULL, '{"ID":"default-right-sidebar::render::0","sequence":10}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-left-sidebar::src::0', 'block:default-left-sidebar', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'from document\nfilter name != ""\n', 'source', 'holon_prql', NULL, '{"sequence":5,"ID":"default-left-sidebar::src::0"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-main-panel', 'block:default-layout-root', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'Main Panel', 'text', NULL, NULL, '{"sequence":6,"ID":"default-main-panel"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-left-sidebar::render::0', 'block:default-left-sidebar', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'list(#{sortkey: "name", item_template: clickable(row(icon("folder"), spacer(6), text(col("name"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})\n', 'source', 'render', NULL, '{"sequence":4,"ID":"default-left-sidebar::render::0"}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-main-panel::render::0', 'block:default-main-panel', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'tree(#{parent_id: col("parent_id"), sortkey: col("sequence"), item_template: render_entity()})\n', 'source', 'render', NULL, '{"ID":"default-main-panel::render::0","sequence":8}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:default-right-sidebar', 'block:default-layout-root', 'doc:cd915f14-5920-4710-8434-a692ef0a5479', 'Right Sidebar', 'text', NULL, NULL, '{"ID":"default-right-sidebar","sequence":9}', 1773158378972, 1773158378976, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-projects', 'block:cc-history-root', 'doc:790610e9-6025-4878-b210-25744d984250', 'Projects', 'text', NULL, NULL, '{"ID":"cc-projects","sequence":1}', 1773158379011, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-tasks', 'block:cc-history-root', 'doc:790610e9-6025-4878-b210-25744d984250', 'Tasks', 'text', NULL, NULL, '{"ID":"cc-tasks","sequence":7}', 1773158379012, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-projects::src::0', 'block:cc-projects', 'doc:790610e9-6025-4878-b210-25744d984250', 'from project\nselect {id, original_path, session_count, last_activity}\nsort {-last_activity}\n', 'source', 'holon_prql', NULL, '{"ID":"block:cc-projects::src::0","sequence":2}', 1773158379011, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-sessions', 'block:cc-history-root', 'doc:790610e9-6025-4878-b210-25744d984250', 'Recent Sessions', 'text', NULL, NULL, '{"ID":"cc-sessions","sequence":4}', 1773158379012, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:cc-history-root', 'doc:790610e9-6025-4878-b210-25744d984250', 'doc:790610e9-6025-4878-b210-25744d984250', 'Claude Code History', 'text', NULL, NULL, '{"sequence":0,"ID":"cc-history-root"}', 1773158379011, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-projects::render::0', 'block:cc-projects', 'doc:790610e9-6025-4878-b210-25744d984250', 'list(#{item_template: row(text(col("original_path")), spacer(16), text(col("session_count")), spacer(8), text(col("last_activity")))})\n', 'source', 'render', NULL, '{"ID":"block:cc-projects::render::0","sequence":3}', 1773158379011, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-sessions::src::0', 'block:cc-sessions', 'doc:790610e9-6025-4878-b210-25744d984250', 'from session\nfilter message_count > 0\nselect {id, first_prompt, message_count, model, modified, git_branch}\nsort {-modified}\ntake 30\n', 'source', 'holon_prql', NULL, '{"ID":"block:cc-sessions::src::0","sequence":5}', 1773158379012, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-tasks::src::0', 'block:cc-tasks', 'doc:790610e9-6025-4878-b210-25744d984250', 'from task\nfilter status == "in_progress"\nselect {id, subject, status, created_at}\nsort {-created_at}\n', 'source', 'holon_prql', NULL, '{"sequence":8,"ID":"block:cc-tasks::src::0"}', 1773158379012, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-tasks::render::0', 'block:cc-tasks', 'doc:790610e9-6025-4878-b210-25744d984250', 'list(#{item_template: row(text(col("status")), spacer(8), text(col("subject")))})\n', 'source', 'render', NULL, '{"ID":"block:cc-tasks::render::0","sequence":9}', 1773158379012, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO block (id, parent_id, document_id, content, content_type, source_language, source_name, properties, created_at, updated_at, _change_origin) VALUES ('block:block:cc-sessions::render::0', 'block:cc-sessions', 'doc:790610e9-6025-4878-b210-25744d984250', 'list(#{item_template: row(text(col("first_prompt")), spacer(16), text(col("message_count")), spacer(8), text(col("modified")))})\n', 'source', 'render', NULL, '{"sequence":6,"ID":"block:cc-sessions::render::0"}', 1773158379012, 1773158379014, '{"Remote":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, parent_id = excluded.parent_id, document_id = excluded.document_id, content = excluded.content, content_type = excluded.content_type, source_language = excluded.source_language, source_name = excluded.source_name, properties = excluded.properties, created_at = excluded.created_at, updated_at = excluded.updated_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "updated_at", "parent_id", "content_type", "document_id", "id", "content", "properties") VALUES (1773158379085, 1773158379112, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'Phase 1: Core Outliner', '{"ID":"599b60af-960d-4c9c-b222-d3d9de95c513","sequence":0}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "content_type", "document_id", "created_at", "updated_at", "content", "id", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379085, 1773158379112, 'MCP Server Frontend [/]', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', '{"ID":"035cac65-27b7-4e1c-8a09-9af9d128dceb","sequence":1}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("id", "document_id", "parent_id", "content", "updated_at", "created_at", "content_type", "properties") VALUES ('block:db59d038-8a47-43e9-9502-0472b493a6b9', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'Context parameter support ($context_id, $context_parent_id)', 1773158379112, 1773158379085, 'text', '{"ID":"db59d038-8a47-43e9-9502-0472b493a6b9","sequence":2}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "document_id", "parent_id", "content_type", "id", "updated_at", "content", "properties") VALUES (1773158379085, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 'text', 'block:95ad6166-c03c-4417-a435-349e88b8e90a', 1773158379112, 'MCP server (stdio + HTTP modes)', '{"sequence":3,"ID":"95ad6166-c03c-4417-a435-349e88b8e90a"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "created_at", "content_type", "updated_at", "document_id", "content", "id", "properties") VALUES ('block:035cac65-27b7-4e1c-8a09-9af9d128dceb', 1773158379085, 'text', 1773158379112, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'MCP tools for query execution and operations', 'block:d365c9ef-c9aa-49ee-bd19-960c0e12669b', '{"ID":"d365c9ef-c9aa-49ee-bd19-960c0e12669b","sequence":4}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "document_id", "updated_at", "id", "content_type", "created_at", "content", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379112, 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'text', 1773158379085, 'Block Operations [/]', '{"sequence":5,"ID":"661368d9-e4bd-4722-b5c2-40f32006c643"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content_type", "document_id", "created_at", "content", "id", "parent_id", "updated_at", "properties") VALUES ('text', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379085, 'Block hierarchy (parent/child, indent/outdent)', 'block:346e7a61-62a5-4813-8fd1-5deea67d9007', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 1773158379112, '{"sequence":6,"ID":"346e7a61-62a5-4813-8fd1-5deea67d9007"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("updated_at", "created_at", "id", "parent_id", "content", "content_type", "document_id", "properties") VALUES (1773158379112, 1773158379085, 'block:4fb5e908-31a0-47fb-8280-fe01cebada34', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'Split block operation', 'text', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', '{"ID":"4fb5e908-31a0-47fb-8280-fe01cebada34","sequence":7}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "updated_at", "created_at", "content", "document_id", "id", "content_type", "properties") VALUES ('block:661368d9-e4bd-4722-b5c2-40f32006c643', 1773158379112, 1773158379086, 'Block CRUD (create, read, update, delete)', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb', 'text', '{"ID":"5df48242-c3c0-42ca-ba3a-ba73d0e9b0fb","sequence":8}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content_type", "updated_at", "id", "parent_id", "content", "created_at", "document_id", "properties") VALUES ('text', 1773158379112, 'block:c3ad7889-3d40-4d07-88fb-adf569e50a63', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'Block movement (move_up, move_down, move_block)', 1773158379086, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', '{"ID":"c3ad7889-3d40-4d07-88fb-adf569e50a63","sequence":9}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "content_type", "parent_id", "document_id", "content", "id", "updated_at", "properties") VALUES (1773158379086, 'text', 'block:661368d9-e4bd-4722-b5c2-40f32006c643', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'Undo/redo system (UndoStack + persistent OperationLogStore)', 'block:225edb45-f670-445a-9162-18c150210ee6', 1773158379112, '{"ID":"225edb45-f670-445a-9162-18c150210ee6","task_state":"TODO","sequence":10}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "content", "document_id", "updated_at", "id", "parent_id", "content_type", "properties") VALUES (1773158379086, 'Storage & Data Layer [/]', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379112, 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', '{"sequence":11,"ID":"444b24f6-d412-43c4-a14b-6e725b673cee"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content_type", "id", "updated_at", "document_id", "content", "created_at", "parent_id", "properties") VALUES ('text', 'block:c5007917-6723-49e2-95d4-c8bd3c7659ae', 1773158379112, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'Schema Module system with topological dependency ordering', 1773158379086, 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"sequence":12,"ID":"c5007917-6723-49e2-95d4-c8bd3c7659ae"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("updated_at", "created_at", "document_id", "id", "content_type", "content", "parent_id", "properties") VALUES (1773158379112, 1773158379086, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:ecafcad8-15e9-4883-9f4a-79b9631b2699', 'text', 'Fractional indexing for block ordering', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', '{"sequence":13,"ID":"ecafcad8-15e9-4883-9f4a-79b9631b2699"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "content_type", "parent_id", "content", "document_id", "updated_at", "id", "properties") VALUES (1773158379086, 'text', 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'Turso (embedded SQLite) backend with connection pooling', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379112, 'block:1e0cf8f7-28e1-4748-a682-ce07be956b57', '{"sequence":14,"ID":"1e0cf8f7-28e1-4748-a682-ce07be956b57"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "document_id", "created_at", "id", "content_type", "updated_at", "content", "properties") VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379086, 'block:eff0db85-3eb2-4c9b-ac02-3c2773193280', 'text', 1773158379112, 'QueryableCache wrapping DataSource with local caching', '{"sequence":15,"ID":"eff0db85-3eb2-4c9b-ac02-3c2773193280"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "content", "document_id", "content_type", "updated_at", "properties") VALUES ('block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:d4ae0e9f-d370-49e7-b777-bd8274305ad7', 1773158379086, 'Entity derive macro (#[derive(Entity)]) for schema generation', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 1773158379112, '{"sequence":16,"ID":"d4ae0e9f-d370-49e7-b777-bd8274305ad7"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("id", "created_at", "parent_id", "document_id", "content", "content_type", "updated_at", "properties") VALUES ('block:d318cae4-759d-487b-a909-81940223ecc1', 1773158379086, 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'CDC (Change Data Capture) streaming from storage to UI', 'text', 1773158379112, '{"sequence":17,"ID":"d318cae4-759d-487b-a909-81940223ecc1"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content", "document_id", "updated_at", "parent_id", "id", "created_at", "content_type", "properties") VALUES ('Command sourcing infrastructure (append-only operation log)', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379112, 'block:444b24f6-d412-43c4-a14b-6e725b673cee', 'block:d587e8d0-8e96-4b98-8a8f-f18f47e45222', 1773158379087, 'text', '{"ID":"d587e8d0-8e96-4b98-8a8f-f18f47e45222","sequence":18,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "document_id", "updated_at", "content", "content_type", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773158379087, 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379112, 'Procedural Macros [/]', 'text', '{"ID":"6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72","sequence":19}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content", "updated_at", "id", "created_at", "parent_id", "document_id", "content_type", "properties") VALUES ('#[operations_trait] macro for operation dispatch generation', 1773158379112, 'block:b90a254f-145b-4e0d-96ca-ad6139f13ce4', 1773158379087, 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', '{"ID":"b90a254f-145b-4e0d-96ca-ad6139f13ce4","sequence":20}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("id", "document_id", "content", "content_type", "parent_id", "created_at", "updated_at", "properties") VALUES ('block:5657317c-dedf-4ae5-9db0-83bd3c92fc44', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', '#[triggered_by(...)] for operation availability', 'text', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 1773158379087, 1773158379112, '{"ID":"5657317c-dedf-4ae5-9db0-83bd3c92fc44","sequence":21}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "document_id", "content_type", "updated_at", "content", "id", "parent_id", "properties") VALUES (1773158379087, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 1773158379112, 'Type inference for OperationDescriptor parameters', 'block:f745c580-619b-4dc3-8a5b-c4a216d1b9cd', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', '{"sequence":22,"ID":"f745c580-619b-4dc3-8a5b-c4a216d1b9cd"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content", "parent_id", "id", "updated_at", "created_at", "document_id", "content_type", "properties") VALUES ('#[affects(...)] for field-level reactivity', 'block:6cafa6e9-7c9d-408a-8d68-4b0b7cf6df72', 'block:f161b0a4-e54f-4ad8-9540-77b5d7d550b2', 1773158379112, 1773158379087, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', '{"sequence":23,"ID":"f161b0a4-e54f-4ad8-9540-77b5d7d550b2"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("updated_at", "content_type", "id", "document_id", "content", "parent_id", "created_at", "properties") VALUES (1773158379112, 'text', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'Performance [/]', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773158379087, '{"ID":"b4351bc7-6134-4dbd-8fc2-832d9d875b0a","sequence":24}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "content_type", "content", "updated_at", "created_at", "id", "document_id", "properties") VALUES ('block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'text', 'Virtual scrolling and lazy loading', 1773158379112, 1773158379087, 'block:6463c700-3e8b-42a7-ae49-ce13520f8c73', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', '{"task_state":"DOING","ID":"6463c700-3e8b-42a7-ae49-ce13520f8c73","sequence":25}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "id", "created_at", "content", "document_id", "content_type", "updated_at", "properties") VALUES ('block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 'block:eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e', 1773158379087, 'Connection pooling for Turso', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 1773158379112, '{"ID":"eccb09e2-a7ae-4be0-9ca5-a2c5833cd30e","task_state":"DOING","sequence":26}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("document_id", "content", "content_type", "id", "parent_id", "created_at", "updated_at", "properties") VALUES ('doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'Full-text search indexing (Tantivy)', 'text', 'block:e0567a06-5a62-4957-9457-c55a6661cee5', 'block:b4351bc7-6134-4dbd-8fc2-832d9d875b0a', 1773158379087, 1773158379112, '{"ID":"e0567a06-5a62-4957-9457-c55a6661cee5","sequence":27}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "content_type", "updated_at", "created_at", "document_id", "id", "content", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 1773158379112, 1773158379087, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'Cross-Device Sync [/]', '{"ID":"3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34","sequence":28}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content_type", "document_id", "parent_id", "id", "created_at", "updated_at", "content", "properties") VALUES ('text', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'block:43f329da-cfb4-4764-b599-06f4b6272f91', 1773158379088, 1773158379112, 'CollaborativeDoc with ALPN routing', '{"ID":"43f329da-cfb4-4764-b599-06f4b6272f91","sequence":29}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("id", "document_id", "content", "content_type", "updated_at", "created_at", "parent_id", "properties") VALUES ('block:7aef40b2-14e1-4df0-a825-18603c55d198', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'Offline-first with background sync', 'text', 1773158379112, 1773158379088, 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', '{"sequence":30,"ID":"7aef40b2-14e1-4df0-a825-18603c55d198"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("updated_at", "id", "created_at", "parent_id", "document_id", "content_type", "content", "properties") VALUES (1773158379112, 'block:e148d7b7-c505-4201-83b7-36986a981a56', 1773158379088, 'block:3fd58f88-3b34-4f53-ba2a-e3ecff2d7b34', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 'Iroh P2P transport for Loro documents', '{"sequence":31,"ID":"e148d7b7-c505-4201-83b7-36986a981a56"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("created_at", "content", "parent_id", "content_type", "updated_at", "document_id", "id", "properties") VALUES (1773158379088, 'Dependency Injection [/]', 'block:599b60af-960d-4c9c-b222-d3d9de95c513', 'text', 1773158379112, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', '{"ID":"20e00c3a-2550-4791-a5e0-509d78137ce9","sequence":32}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("id", "updated_at", "content", "document_id", "content_type", "created_at", "parent_id", "properties") VALUES ('block:b980e51f-0c91-4708-9a17-3d41284974b2', 1773158379112, 'OperationDispatcher routing to providers', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 1773158379088, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', '{"ID":"b980e51f-0c91-4708-9a17-3d41284974b2","sequence":33}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("updated_at", "document_id", "content", "parent_id", "created_at", "content_type", "id", "properties") VALUES (1773158379112, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'BackendEngine as main orchestration point', 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 1773158379088, 'text', 'block:97cc8506-47d2-44cb-bdca-8e9a507953a0', '{"ID":"97cc8506-47d2-44cb-bdca-8e9a507953a0","sequence":34}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "content", "id", "document_id", "content_type", "created_at", "updated_at", "properties") VALUES ('block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'ferrous-di based service composition', 'block:1c1f07b1-c801-47b2-8480-931cfb7930a8', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 1773158379088, 1773158379112, '{"ID":"1c1f07b1-c801-47b2-8480-931cfb7930a8","sequence":35}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("content_type", "created_at", "parent_id", "document_id", "content", "updated_at", "id", "properties") VALUES ('text', 1773158379088, 'block:20e00c3a-2550-4791-a5e0-509d78137ce9', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'SchemaRegistry with topological initialization', 1773158379112, 'block:0de5db9d-b917-4e03-88c3-b11ea3f2bb47', '{"ID":"0de5db9d-b917-4e03-88c3-b11ea3f2bb47","sequence":36}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("parent_id", "created_at", "id", "updated_at", "document_id", "content_type", "content", "properties") VALUES ('block:599b60af-960d-4c9c-b222-d3d9de95c513', 1773158379088, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 1773158379112, 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 'text', 'Query & Render Pipeline [/]', '{"sequence":37,"ID":"b489c622-6c87-4bf6-8d35-787eb732d670"}');

-- [transaction_stmt]
INSERT OR REPLACE INTO block ("updated_at", "content_type", "parent_id", "id", "document_id", "created_at", "content", "properties") VALUES (1773158379112, 'text', 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 'block:1bbec456-7217-4477-a49c-0b8422e441e9', 'doc:d033efae-c793-4cbe-9abc-a4bbf9cbb391', 1773158379088, 'Transform pipeline (ChangeOrigin, EntityType, ColumnPreservation, JsonAggregation)', '{"sequence":38,"ID":"1bbec456-7217-4477-a49c-0b8422e441e9"}');
