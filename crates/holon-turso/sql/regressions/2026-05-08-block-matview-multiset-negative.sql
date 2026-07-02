-- Minimized replay (147 statements)

-- [actor_ddl]
CREATE TABLE IF NOT EXISTS block_raw (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    depth INTEGER NOT NULL DEFAULT 0,
    sort_key TEXT NOT NULL DEFAULT 'A0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    properties TEXT,
    marks TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT
);

-- [actor_ddl]
CREATE TABLE IF NOT EXISTS block_requires (
    block_id TEXT NOT NULL,
    required_id TEXT NOT NULL,
    PRIMARY KEY (block_id, required_id),
    FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE,
    FOREIGN KEY (required_id) REFERENCES block_raw(id) ON DELETE CASCADE
);

-- [actor_ddl]
CREATE TABLE IF NOT EXISTS block_tags (
    block_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (block_id, tag),
    FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
);

-- [actor_ddl]
CREATE MATERIALIZED VIEW block AS -- The `block` matview: hydrates the block_raw rows with the
SELECT
    b.id,
    b.parent_id,
    b.depth,
    b.sort_key,
    b.content,
    b.content_type,
    b.source_language,
    b.source_name,
    b.properties,
    b.marks,
    b.collapsed,
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin,
    COALESCE(json_group_array(bt.tag)         FILTER (WHERE bt.tag         IS NOT NULL), '[]') AS tags,
    COALESCE(json_group_array(br.required_id) FILTER (WHERE br.required_id IS NOT NULL), '[]') AS requires
FROM block_raw b
LEFT OUTER JOIN block_tags     bt ON bt.block_id = b.id
LEFT OUTER JOIN block_requires br ON br.block_id = b.id
GROUP BY
    b.id,
    b.parent_id,
    b.depth,
    b.sort_key,
    b.content,
    b.content_type,
    b.source_language,
    b.source_name,
    b.properties,
    b.marks,
    b.collapsed,
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin;

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("content", "id", "sort_key", "parent_id", "updated_at", "content_type", "created_at", "properties") VALUES ('Projects', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', 'A0', 'sentinel:no_parent', 1778244845497, 'text', 1778244845497, '{"ID":"db147710-ef57-40f3-bb67-b3674bbc874a","sequence":0}');

-- [actor_exec]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:db147710-ef57-40f3-bb67-b3674bbc874a', 'Page');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "sort_key", "id", "parent_id", "content_type", "updated_at", "properties") VALUES ('Holon', 1778244845681, '7F8180', 'block:b42f8024-da78-4874-8526-22f8913effcf', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', 'text', 1778244845708, '{"ID":"b42f8024-da78-4874-8526-22f8913effcf","sequence":3}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "sort_key", "parent_id", "content_type", "content", "id", "updated_at", "properties") VALUES (1778244845682, '80', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', 'text', 'Holon', 'block:32a48c60-e32a-4fa1-a30e-fccfd1f84350', 1778244845708, '{"ID":"32a48c60-e32a-4fa1-a30e-fccfd1f84350","sequence":4}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "created_at", "parent_id", "sort_key", "content", "updated_at", "properties") VALUES ('text', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 1778244845682, 'block:db147710-ef57-40f3-bb67-b3674bbc874a', '817E80', 'Holon', 1778244845708, '{"ID":"8bc61d20-9e48-481f-9c56-835177e61a1b","sequence":5}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content_type", "content", "sort_key", "created_at", "updated_at", "properties") VALUES ('block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:0751b1c4-d580-4c84-945c-1ad1fb877b3a', 'text', 'Engine Foundations', '7D80', 1778244845682, 1778244845708, '{"ID":"0751b1c4-d580-4c84-945c-1ad1fb877b3a","sequence":6,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "created_at", "sort_key", "parent_id", "content_type", "id", "content", "properties") VALUES (1778244845708, 1778244845682, '7E80', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'text', 'block:260ca1a2-694f-45b7-a1ba-df9b27346e8b', 'Test Quality & Performance', '{"ID":"260ca1a2-694f-45b7-a1ba-df9b27346e8b","sequence":7,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "sort_key", "created_at", "parent_id", "id", "updated_at", "properties") VALUES ('Inspiration', 'text', '7E8180', 1778244845682, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:293b8999-1eb7-41d6-80f3-bbf7ed151c3c', 1778244845708, '{"ID":"293b8999-1eb7-41d6-80f3-bbf7ed151c3c","sequence":8}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "updated_at", "content_type", "created_at", "sort_key", "content", "properties") VALUES ('block:3d078e3b-5d19-4555-b464-9246ad4dd7ff', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 1778244845708, 'text', 1778244845682, '7F80', 'Hypotheses', '{"ID":"3d078e3b-5d19-4555-b464-9246ad4dd7ff","sequence":9,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "content_type", "updated_at", "created_at", "id", "sort_key", "properties") VALUES ('block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'Market launch', 'text', 1778244845708, 1778244845682, 'block:4e125c8c-07b1-469e-8d4d-bcd424fd7925', '7F817F80', '{"ID":"4e125c8c-07b1-469e-8d4d-bcd424fd7925","sequence":10}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "content", "updated_at", "sort_key", "id", "parent_id", "properties") VALUES ('text', 1778244845683, 'MVP Definition', 1778244845708, '7F8180', 'block:55646c08-6c4d-45f1-8855-892b72d152c2', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', '{"ID":"55646c08-6c4d-45f1-8855-892b72d152c2","sequence":11,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "created_at", "updated_at", "id", "content", "parent_id", "properties") VALUES ('7F8280', 'text', 1778244845683, 1778244845708, 'block:565203c5-433a-41bd-90d5-dfa324047b6e', 'Plain-Text Layer', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', '{"ID":"565203c5-433a-41bd-90d5-dfa324047b6e","sequence":12,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "id", "content_type", "created_at", "updated_at", "sort_key", "properties") VALUES ('Now', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:5da5d3ab-83fa-4231-9a7e-287688309fd6', 'text', 1778244845683, 1778244845708, '80', '{"ID":"5da5d3ab-83fa-4231-9a7e-287688309fd6","sequence":13,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "parent_id", "content_type", "created_at", "updated_at", "content", "properties") VALUES ('817E80', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'text', 1778244845683, 1778244845708, '_archive', '{"ID":"5e821ae2-2d94-4401-a55d-f4ab1da6aedd","sequence":14}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "content_type", "id", "parent_id", "created_at", "sort_key", "properties") VALUES ('Query-Triggered Actions (Reactive Automation)', 1778244845708, 'text', 'block:1167e0db-520f-457c-99e9-297d872b74f1', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 1778244845683, '7D80', '{"ID":"1167e0db-520f-457c-99e9-297d872b74f1","sequence":15}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "created_at", "id", "updated_at", "sort_key", "content_type", "properties") VALUES ('block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'Architecture Alternatives', 1778244845683, 'block:18120de7-ccb5-4611-9441-db05d18021a9', 1778244845708, '7E80', 'text', '{"ID":"18120de7-ccb5-4611-9441-db05d18021a9","sequence":16}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "content", "id", "created_at", "updated_at", "parent_id", "properties") VALUES ('7F80', 'text', 'Phase 2: First Integration (Todoist)', 'block:2a0543d5-e634-407b-a7bc-9b58ecd31348', 1778244845683, 1778244845708, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', '{"ID":"2a0543d5-e634-407b-a7bc-9b58ecd31348","sequence":17}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "parent_id", "id", "content", "sort_key", "created_at", "content_type", "properties") VALUES (1778244845708, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'block:652e62e0-0171-4be5-9509-ff9a1ca3481b', 'Phase 4: AI Foundation', '7F817F80', 1778244845683, 'text', '{"ID":"652e62e0-0171-4be5-9509-ff9a1ca3481b","sequence":18}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "content_type", "content", "created_at", "updated_at", "parent_id", "properties") VALUES ('7F8180', 'block:69799677-ed42-4f5a-8f99-c138c7511718', 'text', 'Research Competition', 1778244845683, 1778244845708, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', '{"ID":"69799677-ed42-4f5a-8f99-c138c7511718","sequence":19}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "sort_key", "id", "created_at", "content_type", "content", "properties") VALUES ('block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 1778244845708, '80', 'block:a283085e-3386-40a4-ab8d-8578b295c6be', 1778244845684, 'text', 'Phase 3: Multiple Integrations', '{"ID":"a283085e-3386-40a4-ab8d-8578b295c6be","sequence":20}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "content_type", "content", "created_at", "parent_id", "sort_key", "id", "properties") VALUES (1778244845708, 'text', 'Phase 6: Flow Optimization', 1778244845684, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', '817E80', 'block:b5e6ecb3-0d72-4464-9de9-31788e0003a6', '{"ID":"b5e6ecb3-0d72-4464-9de9-31788e0003a6","sequence":21}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "parent_id", "content", "content_type", "sort_key", "created_at", "properties") VALUES ('block:c7aa2ca8-31e4-40a1-92cc-ce4f83cf5b43', 1778244845708, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'Phase 1: Core Outliner', 'text', '817F80', 1778244845684, '{"ID":"c7aa2ca8-31e4-40a1-92cc-ce4f83cf5b43","sequence":22}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "parent_id", "created_at", "content", "content_type", "sort_key", "properties") VALUES (1778244845708, 'block:d14b4916-17b5-4646-b8c2-81278bb9ca9d', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 1778244845684, 'Phase 5: AI Features', 'text', '8180', '{"ID":"d14b4916-17b5-4646-b8c2-81278bb9ca9d","sequence":23}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "updated_at", "created_at", "id", "content_type", "sort_key", "properties") VALUES ('block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'Cross-Cutting Concerns', 1778244845708, 1778244845684, 'block:f644dde8-ab67-4b41-b682-b151cff9b368', 'text', '818180', '{"ID":"f644dde8-ab67-4b41-b682-b151cff9b368","sequence":24}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content", "updated_at", "content_type", "created_at", "sort_key", "properties") VALUES ('block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'block:f9ce6214-52a5-4f70-a8a8-4c179e5e0665', 'Phase 7: Team Features', 1778244845708, 'text', 1778244845684, '8280', '{"ID":"f9ce6214-52a5-4f70-a8a8-4c179e5e0665","sequence":25}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "content", "updated_at", "created_at", "parent_id", "id", "properties") VALUES ('817F80', 'text', 'Entity Identity', 1778244845708, 1778244845684, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:6e34b64e-170a-4cea-9917-2bd02e07b6b6', '{"ID":"6e34b64e-170a-4cea-9917-2bd02e07b6b6","sequence":26,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "id", "content_type", "created_at", "updated_at", "parent_id", "sort_key", "properties") VALUES ('Dogfooding & Agents', 'block:a4c10cc0-108c-4af0-90b8-106dfe7703ce', 'text', 1778244845684, 1778244845708, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', '817F8180', '{"ID":"a4c10cc0-108c-4af0-90b8-106dfe7703ce","sequence":27,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content", "updated_at", "content_type", "created_at", "sort_key", "properties") VALUES ('block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:b475739b-a4c7-4779-8730-9cc8f5aaf083', 'Frontends', 1778244845708, 'text', 1778244845685, '8180', '{"ID":"b475739b-a4c7-4779-8730-9cc8f5aaf083","sequence":28}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "content", "parent_id", "content_type", "sort_key", "created_at", "properties") VALUES (1778244845708, 'block:1b9a4fcd-859f-449d-86c4-b2c2612790b7', 'GPUI', 'block:b475739b-a4c7-4779-8730-9cc8f5aaf083', 'text', '7F80', 1778244845685, '{"ID":"1b9a4fcd-859f-449d-86c4-b2c2612790b7","sequence":29,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "content", "updated_at", "parent_id", "created_at", "sort_key", "id", "properties") VALUES ('text', 'TUI', 1778244845708, 'block:b475739b-a4c7-4779-8730-9cc8f5aaf083', 1778244845685, '80', 'block:f41426cd-46c3-463a-b245-2675b5a6ff86', '{"ID":"f41426cd-46c3-463a-b245-2675b5a6ff86","sequence":30,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "content", "content_type", "created_at", "updated_at", "parent_id", "properties") VALUES ('818180', 'block:d83294d3-3e9f-4911-96d0-616f85619668', 'README', 'text', 1778244845685, 1778244845708, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', '{"ID":"d83294d3-3e9f-4911-96d0-616f85619668","sequence":31}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "id", "parent_id", "content", "created_at", "updated_at", "properties") VALUES ('text', '8280', 'block:eceecd11-e83c-40c1-b2eb-68cca313b256', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'LogSeq replacement', 1778244845685, 1778244845708, '{"ID":"eceecd11-e83c-40c1-b2eb-68cca313b256","sequence":32}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "updated_at", "id", "parent_id", "sort_key", "created_at", "properties") VALUES ('Holon', 'text', 1778244845708, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', '8280', 1778244845685, '{"ID":"1ea23eea-929b-4eb1-9fdb-785fa0090a66","sequence":36}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "id", "created_at", "updated_at", "parent_id", "content_type", "sort_key", "properties") VALUES ('Multi-Frontend Strategy', 'block:3a83db34-c4ae-49a5-985d-6b9f8be6d5ea', 1778244845685, 1778244845708, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '7A80', '{"ID":"3a83db34-c4ae-49a5-985d-6b9f8be6d5ea","sequence":37,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "id", "parent_id", "updated_at", "created_at", "sort_key", "properties") VALUES ('Test Quality & Performance', 'text', 'block:49ac7d7f-286a-4711-a033-483d97898c8f', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845708, 1778244845686, '7B80', '{"ID":"49ac7d7f-286a-4711-a033-483d97898c8f","sequence":38,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "content_type", "parent_id", "sort_key", "id", "content", "created_at", "properties") VALUES (1778244845708, 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '7B8180', 'block:4aff512c-93ed-41db-9464-699861f950ed', 'Entity Identity', 1778244845686, '{"ID":"4aff512c-93ed-41db-9464-699861f950ed","sequence":39,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "sort_key", "id", "content", "created_at", "updated_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '7C80', 'block:55de52f1-6d5b-469f-92b9-e23f16901b95', 'Frontends', 1778244845686, 1778244845708, '{"ID":"55de52f1-6d5b-469f-92b9-e23f16901b95","sequence":40}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "content", "created_at", "id", "updated_at", "sort_key", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'Dogfooding & Agents', 1778244845686, 'block:64ab3203-2b18-4fc7-8a26-e46736973f2a', 1778244845708, '7C817F80', '{"ID":"64ab3203-2b18-4fc7-8a26-e46736973f2a","sequence":41,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "id", "updated_at", "parent_id", "content_type", "content", "sort_key", "properties") VALUES (1778244845686, 'block:65b11d93-b526-4179-a1fc-22b5f64619c8', 1778244845708, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'README', '7C8180', '{"ID":"65b11d93-b526-4179-a1fc-22b5f64619c8","sequence":42}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "parent_id", "sort_key", "updated_at", "id", "content_type", "properties") VALUES (1778244845686, 'Inspiration', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '7D80', 1778244845708, 'block:666cbfbc-e9f8-4927-bd15-3222e2deb609', 'text', '{"ID":"666cbfbc-e9f8-4927-bd15-3222e2deb609","sequence":43}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "parent_id", "created_at", "content", "id", "updated_at", "sort_key", "properties") VALUES ('text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845686, 'Now', 'block:6e253a11-5b11-4566-8a7f-14ae660cc2a7', 1778244845708, '7D817E80', '{"ID":"6e253a11-5b11-4566-8a7f-14ae660cc2a7","sequence":44,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "created_at", "content_type", "content", "id", "parent_id", "properties") VALUES ('7D817F80', 1778244845708, 1778244845686, 'text', 'Market launch', 'block:79e53e13-6523-48e9-b8b6-5076217408aa', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"79e53e13-6523-48e9-b8b6-5076217408aa","sequence":45}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "content", "content_type", "sort_key", "updated_at", "id", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845686, 'Plain-Text Layer', 'text', '7D817F8180', 1778244845708, 'block:8e0dcc49-ccee-4004-b29d-7853415edaa1', '{"ID":"8e0dcc49-ccee-4004-b29d-7853415edaa1","sequence":46,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "updated_at", "content", "sort_key", "parent_id", "content_type", "id", "properties") VALUES (1778244845687, 1778244845708, 'MVP Definition', '7D8180', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'block:a9c98597-1a7c-498d-a405-f629ab290633', '{"ID":"a9c98597-1a7c-498d-a405-f629ab290633","sequence":47,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "sort_key", "id", "content_type", "content", "parent_id", "updated_at", "properties") VALUES (1778244845687, '7D818180', 'block:ab58fd5f-39c5-4206-96eb-5d7bf4dd4cfc', 'text', 'Engine Foundations', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845708, '{"ID":"ab58fd5f-39c5-4206-96eb-5d7bf4dd4cfc","sequence":48,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content", "content_type", "created_at", "id", "parent_id", "updated_at", "properties") VALUES ('7D8280', 'LogSeq replacement', 'text', 1778244845687, 'block:b14f8a45-7175-47e4-bac2-d6d8cb4abcdc', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845708, '{"ID":"b14f8a45-7175-47e4-bac2-d6d8cb4abcdc","sequence":49}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "sort_key", "content_type", "updated_at", "created_at", "parent_id", "properties") VALUES ('block:e77aad4f-0892-478c-87f1-6b113713d8a8', '_archive', '7E80', 'text', 1778244845708, 1778244845687, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"e77aad4f-0892-478c-87f1-6b113713d8a8","sequence":50}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "content", "updated_at", "id", "parent_id", "created_at", "properties") VALUES ('text', '7E817D80', 'Hypotheses', 1778244845708, 'block:f2c2ebdc-c94f-41d7-b706-14cb04d743cc', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845687, '{"ID":"f2c2ebdc-c94f-41d7-b706-14cb04d743cc","sequence":51,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "parent_id", "sort_key", "content_type", "created_at", "id", "properties") VALUES ('Hypotheses', 1778244845709, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '81817E8180', 'text', 1778244845697, 'block:a9f079a0-da9e-48b9-9d2e-b267c51e4383', '{"ID":"a9f079a0-da9e-48b9-9d2e-b267c51e4383","sequence":135,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "content", "content_type", "sort_key", "created_at", "id", "parent_id", "properties") VALUES (1778244845709, 'Plain-Text Layer', 'text', '81817F80', 1778244845697, 'block:bf83e1e3-2eeb-447c-ab70-53e5191226d3', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"bf83e1e3-2eeb-447c-ab70-53e5191226d3","sequence":136,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "content_type", "parent_id", "updated_at", "created_at", "content", "properties") VALUES ('block:c9448d8d-5c8f-4b5d-808c-c7ba1690b6bf', '81817F817F80', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845709, 1778244845697, 'Dogfooding & Agents', '{"ID":"c9448d8d-5c8f-4b5d-808c-c7ba1690b6bf","sequence":137,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "content", "parent_id", "content_type", "created_at", "sort_key", "properties") VALUES (1778244845709, 'block:df8ed5f8-e5a5-49d6-9ee1-5aec6939e73c', '_archive', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778244845697, '81817F8180', '{"ID":"df8ed5f8-e5a5-49d6-9ee1-5aec6939e73c","sequence":138}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "created_at", "parent_id", "content_type", "sort_key", "updated_at", "properties") VALUES ('block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 'Frontends', 1778244845697, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '818180', 1778244845709, '{"ID":"e8d97c1d-efa3-44ca-93af-322b2a9a5087","sequence":139}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "updated_at", "content_type", "created_at", "id", "sort_key", "properties") VALUES ('block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 'GPUI', 1778244845709, 'text', 1778244845697, 'block:29934291-22da-4eb4-bdcb-9c7ee5fe3ea3', '7F80', '{"ID":"29934291-22da-4eb4-bdcb-9c7ee5fe3ea3","sequence":140,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "updated_at", "created_at", "sort_key", "parent_id", "content", "properties") VALUES ('text', 'block:4afaefac-86a0-4348-8a0e-39f465937116', 1778244845709, 1778244845697, '80', 'block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 'TUI', '{"ID":"4afaefac-86a0-4348-8a0e-39f465937116","sequence":141,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "parent_id", "content", "content_type", "id", "updated_at", "created_at", "properties") VALUES ('8181817F80', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Test Quality & Performance', 'text', 'block:f7c74625-a7d3-43b2-8aa5-799e2604ebac', 1778244845709, 1778244845697, '{"ID":"f7c74625-a7d3-43b2-8aa5-799e2604ebac","sequence":142,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "created_at", "updated_at", "content", "sort_key", "content_type", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:fe607af4-9a59-4037-8739-41922bdb9674', 1778244845697, 1778244845709, 'MVP Definition', '81818180', 'text', '{"ID":"fe607af4-9a59-4037-8739-41922bdb9674","sequence":143,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content", "content_type", "created_at", "updated_at", "id", "parent_id", "properties") VALUES ('818280', 'Plain-Text Layer', 'text', 1778244845698, 1778244845709, 'block:1326420e-c84d-4e9b-b9ef-169bb32af21b', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"1326420e-c84d-4e9b-b9ef-169bb32af21b","sequence":144,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "parent_id", "created_at", "id", "content", "updated_at", "sort_key", "properties") VALUES ('text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845698, 'block:1a9cf7c6-a38b-4250-b5e3-69c43a43898b', 'Inspiration', 1778244845709, '81828180', '{"ID":"1a9cf7c6-a38b-4250-b5e3-69c43a43898b","sequence":145}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "created_at", "sort_key", "updated_at", "content", "content_type", "properties") VALUES ('block:2ec9239c-2eb6-4a7f-8ec8-af84ab61a860', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845698, '818380', 1778244845709, 'Engine Foundations', 'text', '{"ID":"2ec9239c-2eb6-4a7f-8ec8-af84ab61a860","sequence":146,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "id", "parent_id", "created_at", "updated_at", "content_type", "sort_key", "properties") VALUES ('Test Quality & Performance', 'block:3613d5b3-c5bd-404d-8542-f935c744f57a', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845698, 1778244845709, 'text', '8280', '{"ID":"3613d5b3-c5bd-404d-8542-f935c744f57a","sequence":147,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "content_type", "sort_key", "updated_at", "id", "parent_id", "properties") VALUES ('_archive', 1778244845698, 'text', '82817E80', 1778244845709, 'block:50c057fc-d631-4bb4-ac9f-b1b3a295408e', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"50c057fc-d631-4bb4-ac9f-b1b3a295408e","sequence":148}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "content_type", "updated_at", "created_at", "sort_key", "id", "properties") VALUES ('LogSeq replacement', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778244845709, 1778244845698, '82817F80', 'block:5757f157-f8a7-4bcd-a68b-dde09b30d3b9', '{"ID":"5757f157-f8a7-4bcd-a68b-dde09b30d3b9","sequence":149}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "parent_id", "sort_key", "created_at", "content_type", "content", "properties") VALUES (1778244845709, 'block:686fbc5b-1e64-4f16-ab7c-2506b13550bf', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '82817F8180', 1778244845698, 'text', 'Multi-Frontend Strategy', '{"ID":"686fbc5b-1e64-4f16-ab7c-2506b13550bf","sequence":150,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "content", "content_type", "updated_at", "parent_id", "id", "properties") VALUES ('828180', 1778244845698, 'Hypotheses', 'text', 1778244845709, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:9b5f2c35-3d7b-49d1-b10f-c1ef725fba64', '{"ID":"9b5f2c35-3d7b-49d1-b10f-c1ef725fba64","sequence":151,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "created_at", "updated_at", "content_type", "content", "parent_id", "properties") VALUES ('block:9c56ee76-41c8-4319-ba6c-59c2fbb75816', '82818180', 1778244845698, 1778244845709, 'text', 'Dogfooding & Agents', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"9c56ee76-41c8-4319-ba6c-59c2fbb75816","sequence":152,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "sort_key", "content", "created_at", "content_type", "id", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845709, '828280', 'MVP Definition', 1778244845699, 'text', 'block:ae15f87d-4da4-42e7-a5c9-c06ab06a3410', '{"ID":"ae15f87d-4da4-42e7-a5c9-c06ab06a3410","sequence":153,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "created_at", "updated_at", "content", "parent_id", "sort_key", "content_type", "properties") VALUES ('block:ba587ce2-4401-452d-a39c-b9bd424483d0', 1778244845699, 1778244845709, 'Frontends', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '8380', 'text', '{"ID":"ba587ce2-4401-452d-a39c-b9bd424483d0","sequence":154}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "updated_at", "id", "created_at", "sort_key", "content_type", "properties") VALUES ('Market launch', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845709, 'block:bdcd8978-fc82-434b-8577-c61e87076089', 1778244845699, '83817F80', 'text', '{"ID":"bdcd8978-fc82-434b-8577-c61e87076089","sequence":157}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "sort_key", "content_type", "created_at", "parent_id", "id", "properties") VALUES ('Entity Identity', 1778244845709, '838180', 'text', 1778244845699, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:db06ac66-eebd-41bb-80cb-1f722216534c', '{"ID":"db06ac66-eebd-41bb-80cb-1f722216534c","sequence":158,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "content", "id", "created_at", "parent_id", "updated_at", "properties") VALUES ('text', '8480', 'Now', 'block:f35c4a0c-290a-431a-8ec5-96ba313ee736', 1778244845699, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845709, '{"ID":"f35c4a0c-290a-431a-8ec5-96ba313ee736","sequence":159,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "updated_at", "created_at", "content_type", "sort_key", "parent_id", "properties") VALUES ('block:fdcbdda5-f75e-4c37-bd42-e7bf9cefeacf', 'README', 1778244845709, 1778244845699, 'text', '848180', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"fdcbdda5-f75e-4c37-bd42-e7bf9cefeacf","sequence":160}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "content", "sort_key", "id", "content_type", "updated_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778244845699, 'Frontends', '8580', 'block:8b590af4-9099-4844-87a9-1e915c66fcf5', 'text', 1778244845709, '{"ID":"8b590af4-9099-4844-87a9-1e915c66fcf5","sequence":161}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "updated_at", "content_type", "id", "sort_key", "parent_id", "properties") VALUES (1778244846131, 'GPUI', 1778244846131, 'text', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'A0', 'sentinel:no_parent', '{"ID":"d09025cc-3748-404e-ad4d-432fcdc194d5","sequence":0}');

-- [actor_exec]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'Page');

-- [actor_exec]
UPDATE block_raw SET "content" = 'GPUI', "sort_key" = 'A0', "created_at" = 1778244846131, "parent_id" = 'sentinel:no_parent', "content_type" = 'text', "updated_at" = 1778244846170, "properties" = '{"ID":"d09025cc-3748-404e-ad4d-432fcdc194d5","sequence":0,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}' WHERE id = 'block:d09025cc-3748-404e-ad4d-432fcdc194d5' AND ("content" IS NOT 'GPUI' OR "sort_key" IS NOT 'A0' OR "parent_id" IS NOT 'sentinel:no_parent' OR "content_type" IS NOT 'text' OR "properties" IS NOT '{"ID":"d09025cc-3748-404e-ad4d-432fcdc194d5","sequence":0,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "parent_id", "id", "sort_key", "created_at", "content_type", "properties") VALUES ('Why this is the primary frontend
GPUI runs natively on macOS, Android, and iOS. Per memory, it''s the #1
frontend; Flutter is parked. Since AC-1 covers all three platforms with
GPUI alone, this file owns those tasks.', 1778244846195, 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'block:gpui-primary-rationale', '7D80', 1778244846149, 'text', '{"ID":"gpui-primary-rationale","reference-memory":"gpui_primary_frontend.md","sequence":0}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "id", "parent_id", "content", "created_at", "updated_at", "properties") VALUES ('text', '7E80', 'block:gpui-macos', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'macOS', 1778244846149, 1778244846195, '{"ID":"gpui-macos","sequence":1,"status":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "created_at", "sort_key", "parent_id", "content_type", "updated_at", "properties") VALUES ('block:gpui-macos-stable', 'Daily-driveable polish
The desktop reference target. Most engine + reactive-shell PBT
investigations land here. Soak in main while remaining HANDOFF items
clear.', 1778244846149, '80', 'block:gpui-macos', 'text', 1778244846195, '{"Effort":"-","ID":"gpui-macos-stable","priority":3,"sequence":2,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "updated_at", "id", "parent_id", "content", "content_type", "sort_key", "properties") VALUES (1778244846149, 1778244846195, 'block:gpui-android', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'Android', 'text', '7F80', '{"ID":"gpui-android","reference-worktree":".claude/worktrees/agent-* (per existing PBT layout)","sequence":3,"status":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "updated_at", "sort_key", "created_at", "content_type", "id", "properties") VALUES ('block:gpui-android', 'Android build target stable
GPUI on Android is the leverage point that lets us drop Flutter. Stabilize
the build pipeline and confirm core flows work. Cross-device sync (AC-2)
should be tested macOS↔Android↔iOS.', 1778244846195, '7F80', 1778244846149, 'text', 'block:gpui-android-stable', '{"Effort":"-","ID":"gpui-android-stable","priority":3,"sequence":4,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "id", "created_at", "sort_key", "parent_id", "content_type", "properties") VALUES ('Android-specific UX adaptations
Touch targets, soft keyboard handling, gesture navigation. Don''t expect
desktop ergonomics to translate.', 1778244846195, 'block:gpui-android-ux', 1778244846150, '80', 'block:gpui-android', 'text', '{"Effort":"2:00","ID":"gpui-android-ux","REQUIRES":"gpui-android-stable","priority":2,"sequence":5,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:gpui-android-ux', 'block:gpui-android-stable');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "parent_id", "sort_key", "content", "id", "content_type", "updated_at", "properties") VALUES (1778244846150, 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', '7F8180', 'iOS', 'block:gpui-ios', 'text', 1778244846195, '{"ID":"gpui-ios","reference-worktree":".claude/worktrees/gpui-ios","sequence":6,"status":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "sort_key", "created_at", "updated_at", "parent_id", "content_type", "id", "properties") VALUES ('iOS build target stable
Per existing worktree gpui-ios, in progress. Wife''s iPhone is the test
device — read-only sharing case (AC-4) should work end-to-end on iOS.', '7F80', 1778244846150, 1778244846195, 'block:gpui-ios', 'text', 'block:gpui-ios-stable', '{"Effort":"-","ID":"gpui-ios-stable","priority":3,"sequence":7,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "updated_at", "sort_key", "id", "content", "parent_id", "content_type", "properties") VALUES (1778244846150, 1778244846195, '80', 'block:gpui-ios-ux', 'iOS-specific UX adaptations', 'block:gpui-ios', 'text', '{"Effort":"2:00","ID":"gpui-ios-ux","REQUIRES":"gpui-ios-stable","priority":2,"sequence":8,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:gpui-ios-ux', 'block:gpui-ios-stable');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "content_type", "id", "sort_key", "created_at", "updated_at", "properties") VALUES ('block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'Cross-platform infrastructure', 'text', 'block:gpui-cross-platform', '80', 1778244846150, 1778244846195, '{"ID":"gpui-cross-platform","sequence":9}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content", "id", "updated_at", "parent_id", "created_at", "content_type", "properties") VALUES ('7F80', 'Outliner editing
The bulk of the engine investigations land here. Per the memory entries,
many split-block, BulkExternalAdd, ReactiveShell, FocusRegistry items have
been resolved or are in flight.', 'block:2b4f3e8e-57f0-46de-a47a-be1242dd9e12', 1778244846195, 'block:gpui-cross-platform', 1778244846150, 'text', '{"ID":"2b4f3e8e-57f0-46de-a47a-be1242dd9e12","sequence":10}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "content", "sort_key", "created_at", "parent_id", "content_type", "id", "properties") VALUES (1778244846195, 'Three-mode UX shells', '80', 1778244846150, 'block:gpui-cross-platform', 'text', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', '{"ID":"ba5ad62d-bd47-47b8-873b-1b668a85e9eb","sequence":11}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "updated_at", "content", "content_type", "id", "created_at", "properties") VALUES ('block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', '7F80', 1778244846195, 'Capture mode overlay (global hotkey)
Global hotkey summons input field, captures to inbox, dismisses. Spotlight-
shaped UX. See docs/Vision/UI.md §Capture Mode.', 'text', 'block:capture-mode-overlay', 1778244846150, '{"Effort":"4:00","ID":"capture-mode-overlay","priority":3,"sequence":12,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "created_at", "sort_key", "content", "updated_at", "content_type", "parent_id", "properties") VALUES ('block:orient-daily-view', 1778244846151, '80', 'Orient daily view
Today''s focus + inbox + watcher placeholder. Reads from the Now query
(per Engine Foundations). No AI yet at G1; structural-only.', 1778244846195, 'text', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', '{"Effort":"4:00","ID":"orient-daily-view","REQUIRES":"now-query-mcp","priority":3,"sequence":13,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:orient-daily-view', 'block:now-query-mcp');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "updated_at", "id", "content_type", "created_at", "sort_key", "properties") VALUES ('Flow mode shell
Single-task focus. Context-on-demand panel from the right edge. Hide
distractions (other panels fade). The polish (text recency effect,
timer animation) is G5; the shell is G1.', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', 1778244846195, 'block:flow-mode-shell', 'text', 1778244846151, '8180', '{"Effort":"3:00","ID":"flow-mode-shell","REQUIRES":"orient-daily-view","priority":2,"sequence":14,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:flow-mode-shell', 'block:orient-daily-view');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "parent_id", "content", "updated_at", "id", "created_at", "properties") VALUES ('817E80', 'text', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'Filed: investigations and worktree state
The remaining HANDOFF *.md files at repo root (per Idea 4 / AC-6) cover
GPUI-specific in-flight investigations:
- HANDOFFPUBTEMAINING.
- HANDOFFPUPLICEILENT.
- HANDOFFEACTIVHELLAACHE.m
Migrate these to topic blocks under Engine Foundations.org or here as
part of AC-6 / handoff-md-migration.', 1778244846195, 'block:gpui-investigations', 1778244846151, '{"ID":"gpui-investigations","sequence":15}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "content", "updated_at", "sort_key", "id", "created_at", "properties") VALUES ('block:handoff-displayed-text-invariant', 'text', 'What landed (this session)', 1778244846195, '7F80', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', 1778244846152, '{"ID":"92bd7471-a03b-4cc0-8981-e24e1ba833a3","sequence":19}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content_type", "parent_id", "created_at", "updated_at", "sort_key", "content", "properties") VALUES ('block:9be6a99d-f5a8-4dd3-9cc2-35611c1fc0a3', 'text', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', 1778244846152, 1778244846195, '7E80', '1. New field — ElementInfo.displayedex: Option<String>
- crates/holon-frontend/src/geometry.rs:12-… — added field with rationale comment.', '{"ID":"9be6a99d-f5a8-4dd3-9cc2-35611c1fc0a3","sequence":20}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "content", "content_type", "parent_id", "updated_at", "created_at", "properties") VALUES ('block:1be0e5f8-a64d-4b91-9434-6a03ecf9957b', '7F80', '2. GPUI plumbing
- =frontends/gpui/src/geometry.rs::tracked()= — extra displayedex: Option<String> arg.
- =frontends/gpui/src/geometry.rs::BoundsTracker= — extra field, propagated into record() during prepaint.
- =frontends/gpui/src/geometry.rs::TransparentTracker::prepaint= — fills displayedex: None.', 'text', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', 1778244846195, 1778244846152, '{"ID":"1be0e5f8-a64d-4b91-9434-6a03ecf9957b","sequence":21}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "id", "updated_at", "sort_key", "created_at", "parent_id", "properties") VALUES ('3. Builder captures
- frontends/gpui/src/render/builders/editableext.r
  - Cached path: reads entity.read(cx).inputntit().read(cx).value().totrin() before
    entity.intonlemen(). Passes Some(displayedex) to tracked().
  - Fresh-create path: reads the InputState value right after cx.new(|cx| EditorView::new(...)).
- selectable.rs and renderntity.r: pass None (not text-bearing).
- frontends/gpui/tests/layoutmoke.r:231: test helper updated.', 'text', 'block:d3aa021f-1c89-48b0-b025-ae8392730c18', 1778244846195, '80', 1778244846152, 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', '{"ID":"d3aa021f-1c89-48b0-b025-ae8392730c18","sequence":22}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "parent_id", "sort_key", "content_type", "updated_at", "id", "properties") VALUES (1778244846153, '4. Invariant — inv-displayed-text in sut.rs::checknvariantsyn
- crates/holon-integration-tests/src/pbt/sut.rs — appended after inv15.
- Iterates geometry.alllement(), filters widgetyp = "editableex"= with
  displayedext.iom() and entity starting with block:.
- Skips the currently-focused block — production deliberately doesn''t overwrite
  InputState while focused.
- Looks up the block in reftate.bloctate.block and compares displayedex
  against block.contentex().
- Skipped on navnl transitions.', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', '8180', 'text', 1778244846195, 'block:8d0f44bf-8505-43e6-9e91-a6695515ba4c', '{"ID":"8d0f44bf-8505-43e6-9e91-a6695515ba4c","sequence":23}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "parent_id", "id", "content_type", "sort_key", "updated_at", "properties") VALUES (1778244846153, 'Build status
- cargo check -p holon-frontend — clean (warnings only, pre-existing).
- cargo check -p holon-integration-tests — clean.
- cargo check -p holon-gpui — fails with 8 pre-existing errors unrelated to this work:
  EditorView::new callers missing NavigationState, FocusRegistry, Arc<RwLock<Option<String>>>
  args. Constructor-signature drift from a half-finished refactor; resolving them is out
  of scope and required before the PBT can run.', 'block:handoff-displayed-text-invariant', 'block:4e69c69d-68f0-47ce-b154-720a6f7dbfc9', 'text', '7F8180', 1778244846195, '{"ID":"4e69c69d-68f0-47ce-b154-720a6f7dbfc9","sequence":24}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "content_type", "updated_at", "content", "id", "created_at", "properties") VALUES ('block:handoff-displayed-text-invariant', '80', 'text', 1778244846195, 'TODOs
- [ ] Resolve pre-existing holon-gpui compile errors (constructor-signature drift for NavigationState, FocusRegistry, focusedi) before the PBT can run.
- [ ] Run the GPUI PBT after compile is clean: cargo test -p holon-gpui --test gpuib 2>&1 | tee /tmp/gpui_pbt.log. Expectation: shrunk seed hits SplitBlock/JoinBlock where displayedex doesn''t match reftate.block[uri].contentex().
- [ ] If PBT doesn''t fail: re-check split positions, navnl flag on SplitBlock/JoinBlock, and focus-at-invariant-check timing.
- [ ] Consider adding displayedex to text(...) builders (non-editable text — cheap to add, would catch stale non-editable widgets).', 'block:22da2e7f-dc3c-457c-81db-1fe73ca9d7d7', 1778244846153, '{"ID":"22da2e7f-dc3c-457c-81db-1fe73ca9d7d7","sequence":25}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "id", "content_type", "sort_key", "content", "created_at", "properties") VALUES ('block:handoff-displayed-text-invariant', 1778244846195, 'block:b1a5c00e-5bdb-4b87-be3d-9496d787e6f7', 'text', '817F80', 'Files touched
- crates/holon-frontend/src/geometry.rs
- frontends/gpui/src/geometry.rs
- frontends/gpui/src/render/builders/editableext.r
- frontends/gpui/src/render/builders/selectable.rs
- frontends/gpui/src/render/builders/renderntity.r
- frontends/gpui/tests/layoutmoke.r
- crates/holon-integration-tests/src/pbt/sut.rs', 1778244846153, '{"ID":"b1a5c00e-5bdb-4b87-be3d-9496d787e6f7","sequence":26}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content_type", "parent_id", "updated_at", "created_at", "content", "sort_key", "properties") VALUES ('block:c8f65c84-3843-4bbd-bdd5-e4c4ed340562', 'text', 'block:handoff-displayed-text-invariant', 1778244846195, 1778244846153, 'MCP one-liner used to confirm DB state', '8180', '{"ID":"c8f65c84-3843-4bbd-bdd5-e4c4ed340562","sequence":27}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "updated_at", "parent_id", "id", "content", "source_language", "sort_key", "created_at", "properties") VALUES ('source', 1778244846195, 'block:c8f65c84-3843-4bbd-bdd5-e4c4ed340562', 'block:c8f65c84-3843-4bbd-bdd5-e4c4ed340562::src::0', 'curl -s -X POST http://127.0.0.1:8520/mcp \
  -H ''Content-Type: application/json'' -H ''Accept: application/json,text/event-stream'' \
  -H "mcp-session-id: $SESSION" \
  -d ''{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"execute_raw_sql","arguments":{"sql":"SELECT id, content FROM block WHERE content LIKE ''\''''%triggered%''\'''' OR content LIKE ''\''''%availability%''\''''"}}}''', 'bash', '80', 1778244846153, '{"ID":"c8f65c84-3843-4bbd-bdd5-e4c4ed340562::src::0","sequence":28}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "sort_key", "updated_at", "content_type", "id", "content", "properties") VALUES ('block:handoff-displayed-text-invariant', 1778244846167, '8280', 1778244846195, 'text', 'block:64723edc-36ca-45aa-bf4b-9937e167e687', 'Open questions
- After holon-gpui compiles, will inv-displayed-text be too strict for the cached
  EditorView path? If editoriew is cached and InputState lags during normal CDC
  (not just bug cases), a short retry/settle may be needed — see how inv16 handles
  prenv1_settle.
- Should text(...) builders also fill displayedex?', '{"ID":"64723edc-36ca-45aa-bf4b-9937e167e687","sequence":29}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "parent_id", "created_at", "sort_key", "content_type", "content", "properties") VALUES ('block:handoff-gpui-pbt-remaining', 1778244846195, 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 1778244846167, '8180', 'text', 'Handoff — GPUI PBT: inv-displayed-text fix + remaining open items', '{"ID":"handoff-gpui-pbt-remaining","sequence":30,"source-date":"2026-04-29","source-file":"HANDOFF_GPUI_PBT_REMAINING.md"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content", "updated_at", "content_type", "created_at", "sort_key", "properties") VALUES ('block:handoff-gpui-pbt-remaining', 'block:80beb3b6-a231-4bd8-b676-600f991d261e', 'What landed this session', 1778244846195, 'text', 1778244846167, '7F80', '{"ID":"80beb3b6-a231-4bd8-b676-600f991d261e","sequence":31}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "content", "parent_id", "updated_at", "sort_key", "id", "properties") VALUES ('text', 1778244846167, 'inv-displayed-text false positive fix
The inv-displayed-text invariant in sut.rs:5152 was panicking after TypeChars
because the skip set only consulted reftate.focuse_entity. But
FocusEditableText deliberately does NOT update focusedntit (to keep inv15
stable) — it sets activedito instead. The actively-edited block was incorrectly
checked, and since TypeChars updates InputState without committing to SQL (commit
only happens on Blur / PressKey(Enter)), the invariant fired a false positive.
Fix: 6-line insertion at sut.rs:5157-5164 — added reftate.activditor.bloc
to the skip set.
Result: gpuib passes 50/50. The invariant transitions from "GATED" to fully
operational.', 'block:80beb3b6-a231-4bd8-b676-600f991d261e', 1778244846195, '80', 'block:f29b6733-709e-4ea1-8961-37532605de9b', '{"ID":"f29b6733-709e-4ea1-8961-37532605de9b","sequence":32}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "content", "id", "created_at", "sort_key", "content_type", "parent_id", "properties") VALUES (1778244846195, 'Remaining open items', 'block:05e9b86d-54df-4524-91c9-8216a4c5b17c', 1778244846167, '80', 'text', 'block:handoff-gpui-pbt-remaining', '{"ID":"05e9b86d-54df-4524-91c9-8216a4c5b17c","sequence":33}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content_type", "content", "parent_id", "updated_at", "sort_key", "created_at", "properties") VALUES ('block:5c8fe690-b9df-43f8-8b72-bd0f548c46cd', 'text', 'File map
- crates/holon-integration-tests/src/pbt/sut.rs — inv-displayed-text fix (this session)
- frontends/gpui/src/views/editoriew.r — _dataubscriptio focused-skip guard (open item #1)
- crates/holon-integration-tests/src/pbt/transitionudgets.r — render budgets (open item #2)
- crates/holon-integration-tests/src/uiriver.r — trynteractio trait (open item #3)
- crates/holon-integration-tests/src/pbt/phased.rs — trynteractio call site + fallback (open item #3)', 'block:handoff-gpui-pbt-remaining', 1778244846195, '8180', 1778244846168, '{"ID":"5c8fe690-b9df-43f8-8b72-bd0f548c46cd","sequence":40}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "sort_key", "parent_id", "updated_at", "id", "content_type", "created_at", "properties") VALUES ('Handoff — gpui-component changes needed for Holon MutableText Phase 2
Holon is wiring a CRDT-backed editor (MutableText) into GPUI. Phase 2 replaces
the EditorView _dataubscriptio (SQL CDC) with a Loro-backed remote-deltas stream.
Two blockers live in the gpui-component crate (v0.5.1).', '8280', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 1778244846195, 'block:handoff-gpui-splice-silent', 'text', 1778244846168, '{"ID":"handoff-gpui-splice-silent","sequence":41,"source-date":"2026-04-29","source-file":"HANDOFF_GPUI_SPLICE_SILENT.md"}');

-- [transaction_stmt]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:handoff-gpui-splice-silent', 'active');

-- [transaction_stmt]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:handoff-gpui-splice-silent', 'cross-repo');

-- [transaction_stmt]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:handoff-gpui-splice-silent', 'handoff');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "parent_id", "created_at", "id", "content_type", "sort_key", "content", "properties") VALUES (1778244846490, 'block:b489c622-6c87-4bf6-8d35-787eb732d670', 1778244846476, 'block:7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd', 'text', '8280', 'SQL direct execution support', '{"ID":"7496c8a3-d2a4-49ab-9400-d7e9d9e9a0dd","sequence":51,"shared-tree-id":"4f3686db-6f7b-40a3-ad67-7db8727c2bc1","task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "created_at", "sort_key", "parent_id", "content_type", "id", "content", "properties") VALUES (1778244848306, 1778244848264, '80', 'block:fb7c9160-ca3a-4e5e-88d6-ae0b28ffb1da', 'text', 'block:30f5af51-884f-4bbf-b43b-8e9c1c3241f4', 'Input handling
- ↑/↓: advanceocu(state, ±1), wraps modulo registry length.
- Tab / Shift-Tab / BackTab: aliases for the same.
- Enter: dispatchocusenten(state). If the region has an intent (Selectable),
  dispatches via engine.dispatchnten(intent) and clears focusi. Block regions
  ignore Enter (no edit mode yet at this point).', '{"ID":"30f5af51-884f-4bbf-b43b-8e9c1c3241f4","sequence":11}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "content", "id", "parent_id", "updated_at", "created_at", "properties") VALUES ('8180', 'text', 'Auto-jump-to-first-block
reconcileocu runs after each render walk:
1. If empty registry → NOOCU.
2. If focusi is Some((id, kind)) and a region with same id+kind still exists →
   keep focus.
3. Otherwise, fall back to first Block region (else idx 0) and re-pin.', 'block:9ae588b1-f38e-4061-af05-ffe4b95c6a65', 'block:fb7c9160-ca3a-4e5e-88d6-ae0b28ffb1da', 1778244848306, 1778244848265, '{"ID":"9ae588b1-f38e-4061-af05-ffe4b95c6a65","sequence":12}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content", "content_type", "created_at", "updated_at", "sort_key", "properties") VALUES ('block:handoff-tui-render', 'block:9ba3f847-ac23-4e80-925a-30b682a730b7', '2026-04-28 follow-up: gap, region nav, edit mode', 'text', 1778244848265, 1778244848306, '7F8180', '{"ID":"9ba3f847-ac23-4e80-925a-30b682a730b7","sequence":13}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "content", "content_type", "parent_id", "created_at", "id", "properties") VALUES ('7E80', 1778244848306, 'Gap support — renderolumn, rendero, renderolum
- gapoel(px) — converts logical-pixel gap into terminal cells; any non-zero
  gap rounds up to ≥ 1 cell.
- renderolumn reads =CollectionVariant::Columns { gap }=, deducts total gap from
  width budget before distributing fixed/flex slots, inserts gap cells between adjacent
  non-zero slots while painting.
- rendero reads prop6("gap").
- renderolum reads prop6("gap") and divides by 2 × PXEEL for vertical
  gap. Static children with consumed = 0= skip the gap.', 'text', 'block:9ba3f847-ac23-4e80-925a-30b682a730b7', 1778244848265, 'block:8280cdbd-02e3-4ddc-acce-d8bc93bda136', '{"ID":"8280cdbd-02e3-4ddc-acce-d8bc93bda136","sequence":14}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "content", "updated_at", "created_at", "sort_key", "content_type", "properties") VALUES ('block:5b97b3aa-1323-498f-b648-92553be36098', 'block:9ba3f847-ac23-4e80-925a-30b682a730b7', 'Region-scoped navigation
- SelectableRegion gained region: usize. The outermost renderolumn tags each
  slot''s subtree with region = i (sidebar = 0, main = 1, drawer = 2).
- advanceocu(state, ±1) (Up/Down) now filters to the active region''s selectables.
- switchegio(state, ±1) (Tab/Shift-Tab/BackTab) hops to the first selectable of
  the next/previous region.', 1778244848306, 1778244848265, '7F80', 'text', '{"ID":"5b97b3aa-1323-498f-b648-92553be36098","sequence":15}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "id", "created_at", "sort_key", "content_type", "updated_at", "properties") VALUES ('Edit mode
- EditableTarget { block_id, field, currentonten } captured per Block region.
- EditView<''a> { block_id, field, buffer, cursor } threaded through RenderCtx.
- renderditablex checks ctx.edit and paints the live buffer + cursor (yellow
  highlight at cursor''s grapheme; trailing block when cursor is at end-of-buffer).
- TuiState.edittat: Arc<Mutex<Option<EditState>>> holds the in-progress edit.
  EditState carries block, field, buffer, cursor (UTF-8 byte offset),
  and original (for no-change detection).
- Esc → cancel edit. Enter → dispatch setiel if buffer differs from original.
- Backspace/Delete/Left/Right/Home/End are multi-byte safe via prevhaoundar /
  nextha_boundary.', 'block:9ba3f847-ac23-4e80-925a-30b682a730b7', 'block:0dc723b7-aea5-4e63-b4cf-4e2d732be01c', 1778244848265, '80', 'text', 1778244848306, '{"ID":"0dc723b7-aea5-4e63-b4cf-4e2d732be01c","sequence":16}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "id", "created_at", "content", "parent_id", "content_type", "properties") VALUES ('8180', 1778244848306, 'block:7e919915-e068-4654-98f2-95e7c27cb485', 1778244848265, 'Spacing fix
An early refactor applied .max(1) to every column iteration, causing dropon
siblings (normally 0-height) to each consume 1 row, doubling visible row spacing.
Reverted to original split: collection items are floor-1, static children stay 0 if
they return 0.', 'block:9ba3f847-ac23-4e80-925a-30b682a730b7', 'text', '{"ID":"7e919915-e068-4654-98f2-95e7c27cb485","sequence":17}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "id", "updated_at", "content_type", "created_at", "content", "properties") VALUES ('block:handoff-tui-render', '80', 'block:ead449f8-4c5f-4567-8964-fcf4c0255841', 1778244848306, 'text', 1778244848266, '2026-04-28 second pass: h-scroll, cycle-guard, Ctrl+T', '{"ID":"ead449f8-4c5f-4567-8964-fcf4c0255841","sequence":18}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "id", "updated_at", "content", "parent_id", "created_at", "properties") VALUES ('text', '7E80', 'block:6241d766-9068-4d76-8695-7106167f03f2', 1778244848306, 'Horizontal scroll in renderdiuffe
When the buffer outgrows maxidt, renderdiuffe walks the buffer''s graphemes
into a Vec<(byte_offset, &str)>, computes cursoro (grapheme column), and chooses
a scrolltar that keeps the cursor in a viewportidt window. The viewport
reserves cells for ‹ / › indicators when content is hidden left/right.', 'block:ead449f8-4c5f-4567-8964-fcf4c0255841', 1778244848266, '{"ID":"6241d766-9068-4d76-8695-7106167f03f2","sequence":19}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "parent_id", "sort_key", "updated_at", "id", "content", "properties") VALUES ('text', 1778244848266, 'block:ead449f8-4c5f-4567-8964-fcf4c0255841', '7F80', 1778244848306, 'block:df2ab77a-786d-41bf-877b-79dd36e57ea6', 'MAXIVLOCKEP cycle-guard
RenderCtx.livelocept: usize, bumped on entry to renderivloc and
decremented on exit. When the depth hits 8, the renderer paints a "Recursive block"
placeholder, logs a tracing::warn!, and unwinds instead of recursing forever.', '{"ID":"df2ab77a-786d-41bf-877b-79dd36e57ea6","sequence":20}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "updated_at", "id", "sort_key", "content_type", "content", "properties") VALUES ('block:ead449f8-4c5f-4567-8964-fcf4c0255841', 1778244848266, 1778244848306, 'block:ae5551be-c410-4d62-859b-42384117dfd4', '7F8180', 'text', 'Ctrl+T to cycle task state on focused Block
cycleastatocuse(state) dispatches
OperationIntent::new("block", "cycleastat", { id }) against the focused Block
region''s entity. Bound to Ctrl+T.', '{"ID":"ae5551be-c410-4d62-859b-42384117dfd4","sequence":21}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "id", "sort_key", "updated_at", "content", "created_at", "properties") VALUES ('block:ead449f8-4c5f-4567-8964-fcf4c0255841', 'text', 'block:11c40f28-e8fc-4e4e-aaff-0fe9c5a7b746', '80', 1778244848306, 'Alt+s split / Backspace-at-0 join in edit mode
Sequencing: The buffer is local to EditState until commit, so we cannot dispatch
splitloc against stale SQL. Solution: chain setiel (when buffer differs from
original) → splitloc (or joinloc) inside a single tokio::spawn, awaiting
each via dispatchnten_sync.
- Alt+s = split_block. Plain Enter remains "save and exit edit mode". Alt+s sits above
  the generic Character(c) arm.
- Backspace-at-0 = join_block. Fires only when cursor = 0=.', 1778244848266, '{"ID":"11c40f28-e8fc-4e4e-aaff-0fe9c5a7b746","sequence":22}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "parent_id", "id", "content", "content_type", "updated_at", "properties") VALUES ('817F80', 1778244848266, 'block:ead449f8-4c5f-4567-8964-fcf4c0255841', 'block:d57d61d8-b7cd-4f77-92ae-037ee29ac75b', 'Alt+i / Alt+o for indent / outdent
dispatchlococuse(state, opam) generalised as a helper. Bound to
Alt+i (indent) and Alt+o (outdent). Alt+letter is reliably reported by every
crossterm-supported terminal.', 'text', 1778244848306, '{"ID":"d57d61d8-b7cd-4f77-92ae-037ee29ac75b","sequence":23}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "content", "sort_key", "parent_id", "updated_at", "source_language", "created_at", "properties") VALUES ('source', 'block:d99f75c9-c087-4c23-a945-e31988012367::src::0', 'kill <pid>  # kill any running TUI
rm -rf /tmp/holon-tui-test && mkdir -p /tmp/holon-tui-test
cp ~/.config/holon/holon.toml /tmp/holon-tui-test/holon.toml
HOLON_CONFIG_DIR=/tmp/holon-tui-test MCP_SERVER_PORT=8521 \
  ./target/debug/holon-tui 2>/tmp/holon-tui-stderr.log
# Navigate: ClaudeCode doc → main panel → "Claude Code History" → Enter (edit)
# Home, Right Right Right, Alt+s
grep "mp_event\|SET_FIELD\|LORO_DIFF" /tmp/holon-tui-stderr.log
# Look for missing: [mp_event]   change[N]: Updated id=block:cc-history-root', '80', 'block:d99f75c9-c087-4c23-a945-e31988012367', 1778244848306, 'bash', 1778244848267, '{"ID":"d99f75c9-c087-4c23-a945-e31988012367::src::0","sequence":27}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "created_at", "updated_at", "content", "parent_id", "content_type", "sort_key", "properties") VALUES ('block:f9bff85e-28f4-4cee-a0ea-750e7f5d2920', 1778244848267, 1778244848306, '2026-04-28 fourth pass — narrowed suspect to matview CDC; PBT can''t reach it', 'block:handoff-tui-render', 'text', '817F80', '{"ID":"f9bff85e-28f4-4cee-a0ea-750e7f5d2920","sequence":28}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "content_type", "sort_key", "updated_at", "parent_id", "created_at", "properties") VALUES ('block:05613a08-8614-4368-9bf7-12e4bd8193d8', 'What was tried
1. PBT with split-bias. Ran with PBTEIGHPLITLO=15 and other weights zeroed.
   All three executors FAILED — but on earlier, unrelated bugs, not on
   inv-displayed-text:
   - assertions.rs:60 — Org file diverged: reftat still uses synthetic block::split-N
     ids while SUT reads back resolved UUIDs from disk.
   - inv-loro-no-errors — LoroSyncController logged 1 error: "Cannot resolve parent
     URI to TreeID" for split where the new parent isn''t yet a TreeID.
   - DragDropBlock dropntit failed — chord-dispatch can''t find the source block''s
     draggable widget.
   None of those reach the post-split render where inv-displayed-text would fire.
2. Code-trace of the row-Mutable subscription path.
   crates/holon-frontend/src/reactive.rs:343-389 — applyhang on Change::Updated
   calls existing.set(row) on the existing per-row Mutable<Arc<DataRow>> (mutation
   in place, not replacement). This rules out the "subscription captures stale handle
   on Mutable replacement" hypothesis.', 'text', '7F80', 1778244848306, 'block:f9bff85e-28f4-4cee-a0ea-750e7f5d2920', 1778244848267, '{"ID":"05613a08-8614-4368-9bf7-12e4bd8193d8","sequence":29}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "parent_id", "created_at", "content_type", "content", "sort_key", "properties") VALUES (1778244848306, 'block:0e9ed199-0004-4a96-874d-03d9b98336ab', 'block:f9bff85e-28f4-4cee-a0ea-750e7f5d2920', 1778244848267, 'text', 'What changed
crates/holon-frontend/src/reactive.rs:1235-1283 — extended the existing [mpven] Data
log to also iterate batch.inner.items and emit one line per change with the variant tag,
entity_id, and first 40 chars of content (newlines escaped):', '80', '{"ID":"0e9ed199-0004-4a96-874d-03d9b98336ab","sequence":30}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "id", "content", "created_at", "content_type", "parent_id", "properties") VALUES ('8180', 1778244848306, 'block:1093b7a1-a1ff-4321-bdd3-7326b0152cc9', 'Updated diagnosis
The TUI handoff''s diagnosis (matview CDC drops the Updated for the modified block)
remains the leading hypothesis. The frontend applyhang is innocent — it would
propagate correctly if it received the event. Cannot reproduce in the existing PBT today:
pre-existing synthetic-ID and Loro-mirror bugs gate the split-block path before any
post-split UI invariant can fire.', 1778244848268, 'text', 'block:f9bff85e-28f4-4cee-a0ea-750e7f5d2920', '{"ID":"1093b7a1-a1ff-4321-bdd3-7326b0152cc9","sequence":32}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "parent_id", "content", "updated_at", "created_at", "id", "content_type", "properties") VALUES ('8280', 'block:handoff-tui-render', 'How to resume', 1778244848306, 1778244848268, 'block:8c66f1e3-c430-40d6-b538-0093d261599f', 'text', '{"ID":"8c66f1e3-c430-40d6-b538-0093d261599f","sequence":34}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "updated_at", "created_at", "parent_id", "sort_key", "content", "id", "properties") VALUES ('text', 1778244848525, 1778244848503, 'block:ded81712-3482-4952-a646-03b621b4e64d', '7F80', 'Add AI phrases, but mess them up', 'block:64e32a66-1189-4112-bbda-8d87cdd35e69', '{"ID":"64e32a66-1189-4112-bbda-8d87cdd35e69","sequence":2}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "id", "sort_key", "content_type", "updated_at", "content", "parent_id", "properties") VALUES (1778244848503, 'block:1f892ec0-4838-413f-af3b-4a7825df28ba', '80', 'text', 1778244848525, 'Post the obligatory "AI-slop" content myself, when someone asks say that at I''m hedging and least I can harvest a few Reddit likes', 'block:ded81712-3482-4952-a646-03b621b4e64d', '{"ID":"1f892ec0-4838-413f-af3b-4a7825df28ba","sequence":5}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "content_type", "id", "updated_at", "sort_key", "content", "properties") VALUES ('sentinel:no_parent', 1778244848596, 'text', 'block:475c5013-6f34-4b2b-9384-f3218da7b761', 1778244848596, 'A0', 'Inspiration', '{"ID":"475c5013-6f34-4b2b-9384-f3218da7b761","sequence":0}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("sort_key", "content", "created_at", "updated_at", "id", "content_type", "parent_id", "properties") VALUES ('A0', 'Engine Foundations', 1778244848705, 1778244848705, 'block:995ea45e-37a4-4014-a565-151dd962a802', 'text', 'sentinel:no_parent', '{"ID":"995ea45e-37a4-4014-a565-151dd962a802","sequence":0}');

-- [actor_exec]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:995ea45e-37a4-4014-a565-151dd962a802', 'Page');

-- [actor_exec]
DELETE FROM block_tags WHERE "block_id" = 'block:995ea45e-37a4-4014-a565-151dd962a802';

-- [transaction_stmt]
UPDATE block_raw SET "sort_key" = '817F80', "updated_at" = 1778244853357, "content" = 'Where Does the CRDT Merge Live?', "created_at" = 1778244853302, "content_type" = 'text', "parent_id" = 'block:handoff-collaborative-editing', "properties" = '{"ID":"36054e38-4d0b-4ae1-ab32-9858b3ea9197","sequence":41}' WHERE id = 'block:36054e38-4d0b-4ae1-ab32-9858b3ea9197' AND ("sort_key" IS NOT '817F80' OR "content" IS NOT 'Where Does the CRDT Merge Live?' OR "content_type" IS NOT 'text' OR "parent_id" IS NOT 'block:handoff-collaborative-editing' OR "properties" IS NOT '{"ID":"36054e38-4d0b-4ae1-ab32-9858b3ea9197","sequence":41}');

