-- Minimal reproducer for AggregateOperator delta-consolidation drift on
-- the holon `block` matview (json_group_array over LEFT-joined block_tags).
--
-- Source: live mcp-pkm trace captured 2026-05-13 from a fresh
-- `holon-mcp --http` startup against the real holon-pkm/ org tree.
-- The actor panicked 56× with 'Reached invalid state' + 2× 'multiset went
-- negative' inside ~10s of steady-state polling. Single-threaded replay
-- of the SQL surfaces the same root cause as matview drift instead of a
-- panic — the panic itself is timing-dependent on the AggregateOperator's
-- commit_state std::mem::replace() round-trip across an I/O yield.
--
-- Original capture: 5554 directives / 3985 SQL stmts / ~2.1MB.
-- Minimized via turso-sql-replay minimize (Phase 3 ddmin) to:
--   174 directives, 173 SQL stmts, ~0.4s replay → INCONSISTENCY in block
--   matview=164, fresh=164, extra=1 missing=1 rows
--
-- Pin: nightscape/turso@holon 71ae352e6b04e74830f702f1ecb32a5a714c45cd
-- Upstream panic site: core/incremental/aggregate_operator.rs:2201
-- Run: cargo run --bin turso-sql-replay -- replay <this-file>
--
-- Minimized replay (174 statements)

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

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "sort_key", "parent_id", "created_at", "updated_at", "content_type", "properties") VALUES ('block:8b2bd784-9bfc-44f3-a29d-01833344495b', 'Now', '7E81817F80', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719974, 1778679720060, 'text', '{"ID":"8b2bd784-9bfc-44f3-a29d-01833344495b","sequence":58,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "sort_key", "parent_id", "id", "updated_at", "content", "properties") VALUES (1778679719974, 'text', '7E818180', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:8c973d34-0a01-444c-bcae-bcd1b53c670c', 1778679720060, 'Dogfooding & Agents', '{"ID":"8c973d34-0a01-444c-bcae-bcd1b53c670c","sequence":59,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "content", "id", "created_at", "updated_at", "sort_key", "parent_id", "properties") VALUES ('text', 'LogSeq replacement', 'block:9c068c00-7646-411f-9394-32cabf9d2e8b', 1778679719974, 1778679720060, '7E8280', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"9c068c00-7646-411f-9394-32cabf9d2e8b","sequence":60}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "id", "content_type", "created_at", "content", "parent_id", "properties") VALUES ('7E828180', 1778679720071, 'block:9da07002-1ff4-45d2-9c91-1fe5a558345a', 'text', 1778679719974, 'Market launch', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"9da07002-1ff4-45d2-9c91-1fe5a558345a","sequence":61}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "sort_key", "created_at", "content_type", "parent_id", "content", "id", "properties") VALUES (1778679720071, '7E8380', 1778679719974, 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Inspiration', 'block:b6571179-bde2-4477-a2cc-14ea060459f1', '{"ID":"b6571179-bde2-4477-a2cc-14ea060459f1","sequence":62}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "content_type", "content", "updated_at", "parent_id", "created_at", "properties") VALUES ('block:bf18b39c-8f9e-4fcf-a662-cacca66f821d', '7F80', 'text', '_archive', 1778679720071, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719974, '{"ID":"bf18b39c-8f9e-4fcf-a662-cacca66f821d","sequence":63}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "content", "parent_id", "sort_key", "content_type", "created_at", "properties") VALUES ('block:c02ae544-1fbc-4ab5-8117-c75f5520b615', 1778679720071, 'Frontends', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '7F817C80', 'text', 1778679719974, '{"ID":"c02ae544-1fbc-4ab5-8117-c75f5520b615","sequence":64}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "created_at", "content", "parent_id", "content_type", "sort_key", "id", "properties") VALUES (1778679720113, 1778679719974, 'Hypotheses', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '7F817D80', 'block:e035f91d-52af-480f-92f7-a2d40b65497a', '{"ID":"e035f91d-52af-480f-92f7-a2d40b65497a","sequence":65,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "parent_id", "content", "id", "updated_at", "sort_key", "content_type", "properties") VALUES (1778679719974, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Entity Identity', 'block:f40e02f8-7ce2-4357-9baa-6fea40df1d10', 1778679720158, '7F817D8180', 'text', '{"ID":"f40e02f8-7ce2-4357-9baa-6fea40df1d10","sequence":66,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "parent_id", "sort_key", "content", "updated_at", "content_type", "id", "properties") VALUES (1778679719974, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '7F817E80', 'Multi-Frontend Strategy', 1778679720158, 'text', 'block:01a36d6e-042c-48cd-8348-8cd5ea46ecd0', '{"ID":"01a36d6e-042c-48cd-8348-8cd5ea46ecd0","sequence":67,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "id", "updated_at", "parent_id", "content_type", "sort_key", "content", "properties") VALUES (1778679719974, 'block:34c79d4b-b7bc-4d2b-98cc-3c259c77c071', 1778679720158, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '7F817E817F80', 'Dogfooding & Agents', '{"ID":"34c79d4b-b7bc-4d2b-98cc-3c259c77c071","sequence":68,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "created_at", "content", "content_type", "id", "sort_key", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720159, 1778679719974, 'Engine Foundations', 'text', 'block:4eb56ab8-bc4e-4818-9ade-940dde8de8a8', '7F817E8180', '{"ID":"4eb56ab8-bc4e-4818-9ade-940dde8de8a8","sequence":69,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "id", "content_type", "updated_at", "parent_id", "sort_key", "properties") VALUES ('LogSeq replacement', 1778679719974, 'block:7003c7e8-96aa-40ad-8e60-505e7da72ff1', 'text', 1778679720159, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '7F817F80', '{"ID":"7003c7e8-96aa-40ad-8e60-505e7da72ff1","sequence":70}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "parent_id", "created_at", "sort_key", "id", "updated_at", "properties") VALUES ('Test Quality & Performance', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719975, '7F817F817E80', 'block:81458f33-9442-445f-bae5-89128973aad6', 1778679720159, '{"ID":"81458f33-9442-445f-bae5-89128973aad6","sequence":71,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "content_type", "id", "parent_id", "updated_at", "sort_key", "properties") VALUES (1778679719975, 'Plain-Text Layer', 'text', 'block:8bf39d00-d438-43d6-93b4-31e2c766d576', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720159, '7F817F817F80', '{"ID":"8bf39d00-d438-43d6-93b4-31e2c766d576","sequence":72,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "parent_id", "content", "content_type", "created_at", "updated_at", "properties") VALUES ('block:8c2b1c5e-8dac-43a3-a23a-8907c2100c6f', '7F817F817F8180', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Now', 'text', 1778679719975, 1778679720159, '{"ID":"8c2b1c5e-8dac-43a3-a23a-8907c2100c6f","sequence":73,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "content", "sort_key", "id", "content_type", "created_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720159, 'README', '7F817F8180', 'block:91e909ee-e977-4b2a-91e1-54e3dc5e5bff', 'text', 1778679719975, '{"ID":"91e909ee-e977-4b2a-91e1-54e3dc5e5bff","sequence":74}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "id", "content", "sort_key", "updated_at", "parent_id", "properties") VALUES ('text', 1778679719975, 'block:9858dad7-8384-4314-9ed9-62dfc8c01f06', 'Entity Identity', '7F817F818180', 1778679720159, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"9858dad7-8384-4314-9ed9-62dfc8c01f06","sequence":75,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "created_at", "id", "content_type", "updated_at", "sort_key", "properties") VALUES ('Inspiration', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719975, 'block:ba48f093-6fdb-401c-9009-24e94259cb73', 'text', 1778679720159, '7F817F8280', '{"ID":"ba48f093-6fdb-401c-9009-24e94259cb73","sequence":76}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "content", "updated_at", "created_at", "content_type", "sort_key", "properties") VALUES ('block:bc7c41b8-a3f3-49a4-8201-1c0f497159c0', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Market launch', 1778679720159, 1778679719975, 'text', '7F8180', '{"ID":"bc7c41b8-a3f3-49a4-8201-1c0f497159c0","sequence":77}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "content_type", "updated_at", "sort_key", "created_at", "id", "properties") VALUES ('MVP Definition', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778679720159, '7F81817E80', 1778679719975, 'block:e78de1a6-3381-4d00-9537-cf27c7caf256', '{"ID":"e78de1a6-3381-4d00-9537-cf27c7caf256","sequence":78,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content", "parent_id", "updated_at", "id", "content_type", "created_at", "properties") VALUES ('7F81817F80', '_archive', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720182, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'text', 1778679719975, '{"ID":"f465af55-f5e9-46cc-ba18-1f2905e274b7","sequence":79}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "sort_key", "content", "updated_at", "parent_id", "id", "properties") VALUES (1778679719975, 'text', '7D80', 'Phase 6: Flow Optimization', 1778679720185, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'block:2bf6a036-bb66-4c78-8e06-6dc5fe5f8278', '{"ID":"2bf6a036-bb66-4c78-8e06-6dc5fe5f8278","sequence":80}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "sort_key", "parent_id", "content_type", "updated_at", "id", "created_at", "properties") VALUES ('Research Competition', '7E80', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'text', 1778679720185, 'block:36cd0f0c-cf0c-4668-94bd-bf03ea79c55c', 1778679719975, '{"ID":"36cd0f0c-cf0c-4668-94bd-bf03ea79c55c","sequence":81}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "sort_key", "updated_at", "content", "parent_id", "id", "properties") VALUES (1778679719975, 'text', '7F80', 1778679720185, 'Phase 5: AI Features', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'block:5c56dfba-65fb-4bc7-9d82-499563e3ddc3', '{"ID":"5c56dfba-65fb-4bc7-9d82-499563e3ddc3","sequence":82}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "updated_at", "parent_id", "created_at", "sort_key", "content", "id", "properties") VALUES ('text', 1778679720185, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 1778679719975, '7F817F80', 'Architecture Alternatives', 'block:5e6b85cd-e687-4f30-a1d4-a244170e5605', '{"ID":"5e6b85cd-e687-4f30-a1d4-a244170e5605","sequence":83}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "created_at", "content", "parent_id", "sort_key", "content_type", "id", "properties") VALUES (1778679720185, 1778679719975, 'Query-Triggered Actions (Reactive Automation)', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', '7F8180', 'text', 'block:5f920be0-0b4b-4eff-b579-f699117b0173', '{"ID":"5f920be0-0b4b-4eff-b579-f699117b0173","sequence":84}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "content", "created_at", "parent_id", "content_type", "updated_at", "properties") VALUES ('80', 'block:7eb8f9ca-bbed-4dd6-9e56-1dd54eb0d7c5', 'Phase 1: Core Outliner', 1778679719976, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'text', 1778679720185, '{"ID":"7eb8f9ca-bbed-4dd6-9e56-1dd54eb0d7c5","sequence":85}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "sort_key", "id", "updated_at", "created_at", "content_type", "properties") VALUES ('Phase 7: Team Features', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', '817E80', 'block:61d662d4-97fd-46ba-b0cb-375af194564d', 1778679720185, 1778679719976, 'text', '{"ID":"61d662d4-97fd-46ba-b0cb-375af194564d","sequence":86}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "id", "parent_id", "content", "updated_at", "sort_key", "properties") VALUES ('text', 1778679719976, 'block:e6de5a25-44c3-45fa-9b54-6de6063d1ada', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'Phase 2: First Integration (Todoist)', 1778679720185, '817F80', '{"ID":"e6de5a25-44c3-45fa-9b54-6de6063d1ada","sequence":87}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "created_at", "sort_key", "parent_id", "content", "content_type", "properties") VALUES (1778679720185, 'block:e09ca9f1-d582-4bd5-80f6-f7ec8ee8e5b9', 1778679719976, '8180', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'Cross-Cutting Concerns', 'text', '{"ID":"e09ca9f1-d582-4bd5-80f6-f7ec8ee8e5b9","sequence":88}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "parent_id", "id", "updated_at", "sort_key", "created_at", "content", "properties") VALUES ('text', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'block:f3f2e112-63f5-40c4-88a7-318c4671e6b8', 1778679720185, '818180', 1778679719976, 'Phase 3: Multiple Integrations', '{"ID":"f3f2e112-63f5-40c4-88a7-318c4671e6b8","sequence":89}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "updated_at", "parent_id", "content", "created_at", "sort_key", "id", "properties") VALUES ('text', 1778679720186, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'Phase 4: AI Foundation', 1778679719976, '8280', 'block:f7356a51-16a3-4fc6-b9d7-9e7fcd5b15fe', '{"ID":"f7356a51-16a3-4fc6-b9d7-9e7fcd5b15fe","sequence":90}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "sort_key", "id", "content", "content_type", "created_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720186, '7F81817F8180', 'block:fa16e713-3722-42a2-9445-fb2e7929a4a9', 'Frontends', 'text', 1778679719976, '{"ID":"fa16e713-3722-42a2-9445-fb2e7929a4a9","sequence":91}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "updated_at", "id", "content", "parent_id", "created_at", "properties") VALUES ('text', '7F818180', 1778679720186, 'block:fd2b053b-295d-4e94-a87c-817481b2e646', 'Hypotheses', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719976, '{"ID":"fd2b053b-295d-4e94-a87c-817481b2e646","sequence":92,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "parent_id", "content", "sort_key", "created_at", "content_type", "properties") VALUES (1778679720186, 'block:0f46091a-6931-40af-b13a-e4e7eeed18e9', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Market launch', '7F81818180', 1778679719976, 'text', '{"ID":"0f46091a-6931-40af-b13a-e4e7eeed18e9","sequence":93}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "content_type", "sort_key", "created_at", "id", "content", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720186, 'text', '7F818280', 1778679719976, 'block:1745e531-4abd-4011-a440-c7df839ed0a8', 'Inspiration', '{"ID":"1745e531-4abd-4011-a440-c7df839ed0a8","sequence":94}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "created_at", "content_type", "id", "updated_at", "sort_key", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'MVP Definition', 1778679719976, 'text', 'block:1832d86e-642b-45cb-8055-745b6a259449', 1778679720187, '7F8280', '{"ID":"1832d86e-642b-45cb-8055-745b6a259449","sequence":95,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "parent_id", "id", "content_type", "sort_key", "updated_at", "properties") VALUES ('_archive', 1778679719976, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:1ad3b6b6-ba07-4478-9502-dcda687fd5a4', 'text', '7F82817F80', 1778679720252, '{"ID":"1ad3b6b6-ba07-4478-9502-dcda687fd5a4","sequence":96}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "content_type", "sort_key", "updated_at", "id", "parent_id", "properties") VALUES ('Frontends', 1778679719976, 'text', '7F828180', 1778679720252, 'block:1fd0acc1-6836-40ab-a345-ffd79374b901', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"1fd0acc1-6836-40ab-a345-ffd79374b901","sequence":97}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "updated_at", "content", "sort_key", "parent_id", "id", "properties") VALUES (1778679719976, 'text', 1778679720252, 'TUI', '7F80', 'block:1fd0acc1-6836-40ab-a345-ffd79374b901', 'block:8d0807b3-565d-49a8-9dd9-35c46f9afc7a', '{"ID":"8d0807b3-565d-49a8-9dd9-35c46f9afc7a","sequence":98,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "created_at", "content_type", "id", "content", "updated_at", "properties") VALUES ('block:1fd0acc1-6836-40ab-a345-ffd79374b901', '80', 1778679719976, 'text', 'block:f7be176b-4e12-4ec7-9e4a-ba108b37717d', 'GPUI', 1778679720252, '{"ID":"f7be176b-4e12-4ec7-9e4a-ba108b37717d","sequence":99,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "created_at", "id", "updated_at", "parent_id", "content", "properties") VALUES ('text', '7F8380', 1778679719976, 'block:3c8cafd6-9e42-422b-80b7-4fa432dd34d3', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Engine Foundations', '{"ID":"3c8cafd6-9e42-422b-80b7-4fa432dd34d3","sequence":100,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "content_type", "parent_id", "sort_key", "content", "created_at", "properties") VALUES (1778679720252, 'block:3fb5f4d7-46ff-4f79-bcd8-0d1393a5f165', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '7F838180', 'Entity Identity', 1778679719977, '{"ID":"3fb5f4d7-46ff-4f79-bcd8-0d1393a5f165","sequence":101,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "created_at", "updated_at", "sort_key", "parent_id", "content_type", "content", "properties") VALUES ('block:69759b57-989c-41cc-83d3-10145fe7e3ef', 1778679719977, 1778679720252, '7F8480', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'LogSeq replacement', '{"ID":"69759b57-989c-41cc-83d3-10145fe7e3ef","sequence":102}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "parent_id", "created_at", "id", "content_type", "content", "properties") VALUES ('80', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719977, 'block:7cb2d696-9884-45e3-bf70-6a40abe6ae98', 'text', 'Dogfooding & Agents', '{"ID":"7cb2d696-9884-45e3-bf70-6a40abe6ae98","sequence":103,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "sort_key", "id", "content", "parent_id", "content_type", "updated_at", "properties") VALUES (1778679719977, '817B80', 'block:918a3173-e4e0-4383-9680-24154943d8dc', 'Now', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778679720252, '{"ID":"918a3173-e4e0-4383-9680-24154943d8dc","sequence":104,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "updated_at", "parent_id", "sort_key", "id", "content_type", "properties") VALUES (1778679719977, 'README', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '817C80', 'block:9c60f80c-08c4-4b74-ac6a-8e7258dd6c79', 'text', '{"ID":"9c60f80c-08c4-4b74-ac6a-8e7258dd6c79","sequence":105}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "sort_key", "updated_at", "parent_id", "content_type", "created_at", "id", "properties") VALUES ('Hypotheses', '817C8180', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778679719977, 'block:d56bc1c5-9769-4e75-9e49-dcb78e29b1f6', '{"ID":"d56bc1c5-9769-4e75-9e49-dcb78e29b1f6","sequence":106,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "parent_id", "updated_at", "id", "sort_key", "content", "properties") VALUES (1778679719977, 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, 'block:e3f3e97e-cda9-40f0-a916-2fc4b393603a', '817D80', 'Test Quality & Performance', '{"ID":"e3f3e97e-cda9-40f0-a916-2fc4b393603a","sequence":107,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "created_at", "parent_id", "updated_at", "id", "sort_key", "properties") VALUES ('Plain-Text Layer', 'text', 1778679719977, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, 'block:f04bc668-a50f-417f-bda8-0a43bd90c4a2', '817D817F80', '{"ID":"f04bc668-a50f-417f-bda8-0a43bd90c4a2","sequence":108,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "sort_key", "parent_id", "id", "content", "content_type", "created_at", "properties") VALUES (1778679720252, '817D8180', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:febe1f73-4d58-4e98-92b2-6ab448fc79ef', 'Multi-Frontend Strategy', 'text', 1778679719977, '{"ID":"febe1f73-4d58-4e98-92b2-6ab448fc79ef","sequence":109,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "id", "parent_id", "content_type", "sort_key", "updated_at", "properties") VALUES ('MVP Definition', 1778679719977, 'block:0d510133-f308-487f-b39c-89511aaf1014', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '817E80', 1778679720252, '{"ID":"0d510133-f308-487f-b39c-89511aaf1014","sequence":110,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "sort_key", "content", "created_at", "updated_at", "content_type", "properties") VALUES ('block:19684f85-aaa6-400d-bd1f-07e4bb5174d7', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '817E817E80', 'Now', 1778679719977, 1778679720252, 'text', '{"ID":"19684f85-aaa6-400d-bd1f-07e4bb5174d7","sequence":111,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "updated_at", "parent_id", "content", "created_at", "content_type", "properties") VALUES ('817E817F80', 'block:2092ab66-4128-4c30-81cf-0d8f0c380c0b', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Multi-Frontend Strategy', 1778679719977, 'text', '{"ID":"2092ab66-4128-4c30-81cf-0d8f0c380c0b","sequence":112,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "content", "parent_id", "sort_key", "updated_at", "created_at", "id", "properties") VALUES ('text', 'Hypotheses', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '817E817F8180', 1778679720252, 1778679719977, 'block:2df4d60f-7c7e-443e-9ce3-6464b9c979c4', '{"ID":"2df4d60f-7c7e-443e-9ce3-6464b9c979c4","sequence":113,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "updated_at", "created_at", "id", "sort_key", "content_type", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Engine Foundations', 1778679720252, 1778679719977, 'block:5a1cef98-aec7-49f4-81f8-c96fb7a2a640', '817E8180', 'text', '{"ID":"5a1cef98-aec7-49f4-81f8-c96fb7a2a640","sequence":114,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "content", "sort_key", "content_type", "id", "updated_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719977, 'Inspiration', '817E818180', 'text', 'block:5f42e828-f9bd-4ef4-81a6-c6037e36fbb1', 1778679720252, '{"ID":"5f42e828-f9bd-4ef4-81a6-c6037e36fbb1","sequence":115}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "content", "updated_at", "parent_id", "content_type", "id", "properties") VALUES ('817E8280', 1778679719978, 'Test Quality & Performance', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'block:6970ced1-3717-4d13-964f-21d3a7751f3d', '{"ID":"6970ced1-3717-4d13-964f-21d3a7751f3d","sequence":116,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "updated_at", "id", "content", "parent_id", "created_at", "properties") VALUES ('text', '817F80', 1778679720252, 'block:8ce38157-7a61-4735-ae7a-53b67f3056d4', 'README', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719978, '{"ID":"8ce38157-7a61-4735-ae7a-53b67f3056d4","sequence":117}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "updated_at", "id", "parent_id", "created_at", "content", "sort_key", "properties") VALUES ('text', 1778679720252, 'block:9d11b498-d410-417c-b644-efb0f9384d92', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719978, '_archive', '817F817D80', '{"ID":"9d11b498-d410-417c-b644-efb0f9384d92","sequence":118}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "content_type", "parent_id", "sort_key", "created_at", "updated_at", "properties") VALUES ('block:a198b0ae-291b-4d7f-8629-ccbaecb95840', 'Frontends', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '817F817E80', 1778679719978, 1778679720252, '{"ID":"a198b0ae-291b-4d7f-8629-ccbaecb95840","sequence":119}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "updated_at", "id", "sort_key", "content", "parent_id", "properties") VALUES ('text', 1778679719978, 1778679720252, 'block:0eba88ae-0fc9-4438-a814-78a2ba2ec3a3', '7F80', 'GPUI', 'block:a198b0ae-291b-4d7f-8629-ccbaecb95840', '{"ID":"0eba88ae-0fc9-4438-a814-78a2ba2ec3a3","sequence":120,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "parent_id", "content", "created_at", "sort_key", "id", "updated_at", "properties") VALUES ('text', 'block:a198b0ae-291b-4d7f-8629-ccbaecb95840', 'TUI', 1778679719978, '80', 'block:cd715d12-4a2e-4836-9ce9-0df9afecc7dc', 1778679720252, '{"ID":"cd715d12-4a2e-4836-9ce9-0df9afecc7dc","sequence":121,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "content_type", "parent_id", "updated_at", "id", "content", "properties") VALUES ('817F817E8180', 1778679719978, 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, 'block:ace69db3-9820-4f88-bb57-c95bc6b03b93', 'Entity Identity', '{"ID":"ace69db3-9820-4f88-bb57-c95bc6b03b93","sequence":122,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "sort_key", "updated_at", "created_at", "parent_id", "id", "content_type", "properties") VALUES ('Plain-Text Layer', '817F817F80', 1778679720252, 1778679719978, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:c8fbf859-e8db-4cbe-a900-b90f4e24df9c', 'text', '{"ID":"c8fbf859-e8db-4cbe-a900-b90f4e24df9c","sequence":123,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "id", "updated_at", "parent_id", "content_type", "content", "properties") VALUES ('817F817F817F80', 1778679719978, 'block:cc0045c0-2c2d-4044-8f49-2d59f648050d', 1778679720252, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'Dogfooding & Agents', '{"ID":"cc0045c0-2c2d-4044-8f49-2d59f648050d","sequence":124,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "content", "id", "parent_id", "created_at", "content_type", "sort_key", "properties") VALUES (1778679720252, 'Market launch', 'block:d3270b23-bcb7-4c4f-a431-bc1eb934f21a', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719979, 'text', '817F817F8180', '{"ID":"d3270b23-bcb7-4c4f-a431-bc1eb934f21a","sequence":125}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "content", "content_type", "parent_id", "updated_at", "created_at", "properties") VALUES ('817F8180', 'block:e43634ff-1903-43aa-b68a-e037d61e50e2', 'LogSeq replacement', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, 1778679719979, '{"ID":"e43634ff-1903-43aa-b68a-e037d61e50e2","sequence":126}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "updated_at", "sort_key", "created_at", "content_type", "id", "content", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, '817F81817F80', 1778679719979, 'text', 'block:09ffabe6-fe37-4bc8-b3d8-ca676b339e31', 'Entity Identity', '{"ID":"09ffabe6-fe37-4bc8-b3d8-ca676b339e31","sequence":127,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "updated_at", "content", "sort_key", "id", "created_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778679720252, 'LogSeq replacement', '817F818180', 'block:13c60fef-f52d-4d9a-b199-59870b98ec5b', 1778679719979, '{"ID":"13c60fef-f52d-4d9a-b199-59870b98ec5b","sequence":128}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content", "content_type", "created_at", "parent_id", "updated_at", "sort_key", "properties") VALUES ('block:4aae5a65-c12b-43ec-880f-c302b706e659', 'Now', 'text', 1778679719979, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, '817F8280', '{"ID":"4aae5a65-c12b-43ec-880f-c302b706e659","sequence":129,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "parent_id", "content_type", "updated_at", "id", "content", "sort_key", "properties") VALUES (1778679719979, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778679720252, 'block:5b4e7db7-ea22-4d8d-b1af-2747f4694d35', 'Inspiration', '817F828180', '{"ID":"5b4e7db7-ea22-4d8d-b1af-2747f4694d35","sequence":130}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "parent_id", "updated_at", "content", "content_type", "created_at", "properties") VALUES ('block:6e40f290-926e-4059-acc8-e0675a6dafe4', '817F8380', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, 'Market launch', 'text', 1778679719980, '{"ID":"6e40f290-926e-4059-acc8-e0675a6dafe4","sequence":131}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "sort_key", "updated_at", "id", "parent_id", "content", "created_at", "properties") VALUES ('text', '8180', 1778679720252, 'block:845a40ab-0723-4214-b247-c1cb854fc648', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'README', 1778679719980, '{"ID":"845a40ab-0723-4214-b247-c1cb854fc648","sequence":132}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "parent_id", "updated_at", "id", "content", "created_at", "sort_key", "properties") VALUES ('text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720252, 'block:86ccb52e-7ba4-4b97-b413-2a21ebc25897', 'Multi-Frontend Strategy', 1778679719980, '81817D80', '{"ID":"86ccb52e-7ba4-4b97-b413-2a21ebc25897","sequence":133,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "content_type", "updated_at", "sort_key", "created_at", "id", "parent_id", "properties") VALUES ('Engine Foundations', 'text', 1778679720252, '81817E80', 1778679719980, 'block:a1e99644-637e-4db4-a08c-9a39eb8efed1', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"a1e99644-637e-4db4-a08c-9a39eb8efed1","sequence":134,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "parent_id", "sort_key", "id", "updated_at", "created_at", "content", "properties") VALUES ('text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '81817E8180', 'block:a9f079a0-da9e-48b9-9d2e-b267c51e4383', 1778679720252, 1778679719980, 'Hypotheses', '{"ID":"a9f079a0-da9e-48b9-9d2e-b267c51e4383","sequence":135,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "sort_key", "id", "content", "parent_id", "created_at", "content_type", "properties") VALUES (1778679720252, '81817F80', 'block:bf83e1e3-2eeb-447c-ab70-53e5191226d3', 'Plain-Text Layer', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719980, 'text', '{"ID":"bf83e1e3-2eeb-447c-ab70-53e5191226d3","sequence":136,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "parent_id", "content_type", "id", "updated_at", "created_at", "sort_key", "properties") VALUES ('Dogfooding & Agents', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'block:c9448d8d-5c8f-4b5d-808c-c7ba1690b6bf', 1778679720253, 1778679719980, '81817F817F80', '{"ID":"c9448d8d-5c8f-4b5d-808c-c7ba1690b6bf","sequence":137,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "content", "sort_key", "id", "created_at", "updated_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '_archive', '81817F8180', 'block:df8ed5f8-e5a5-49d6-9ee1-5aec6939e73c', 1778679719980, 1778679720253, '{"ID":"df8ed5f8-e5a5-49d6-9ee1-5aec6939e73c","sequence":138}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "created_at", "sort_key", "content", "parent_id", "content_type", "properties") VALUES ('block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 1778679720253, 1778679719980, '818180', 'Frontends', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '{"ID":"e8d97c1d-efa3-44ca-93af-322b2a9a5087","sequence":139}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "sort_key", "content", "parent_id", "updated_at", "id", "properties") VALUES ('text', 1778679719981, '7F80', 'GPUI', 'block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 1778679720253, 'block:29934291-22da-4eb4-bdcb-9c7ee5fe3ea3', '{"ID":"29934291-22da-4eb4-bdcb-9c7ee5fe3ea3","sequence":140,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content_type", "updated_at", "sort_key", "content", "created_at", "properties") VALUES ('block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 'block:4afaefac-86a0-4348-8a0e-39f465937116', 'text', 1778679720253, '80', 'TUI', 1778679719981, '{"ID":"4afaefac-86a0-4348-8a0e-39f465937116","sequence":141,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "id", "content_type", "created_at", "parent_id", "updated_at", "content", "properties") VALUES ('8181817F80', 'block:f7c74625-a7d3-43b2-8aa5-799e2604ebac', 'text', 1778679719981, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720253, 'Test Quality & Performance', '{"ID":"f7c74625-a7d3-43b2-8aa5-799e2604ebac","sequence":142,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "content_type", "parent_id", "id", "updated_at", "sort_key", "properties") VALUES (1778679719981, 'MVP Definition', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:fe607af4-9a59-4037-8739-41922bdb9674', 1778679720253, '81818180', '{"ID":"fe607af4-9a59-4037-8739-41922bdb9674","sequence":143,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "created_at", "updated_at", "content", "content_type", "id", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '818280', 1778679719981, 1778679720253, 'Plain-Text Layer', 'text', 'block:1326420e-c84d-4e9b-b9ef-169bb32af21b', '{"ID":"1326420e-c84d-4e9b-b9ef-169bb32af21b","sequence":144,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "parent_id", "updated_at", "sort_key", "content_type", "id", "properties") VALUES ('Inspiration', 1778679719981, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720253, '81828180', 'text', 'block:1a9cf7c6-a38b-4250-b5e3-69c43a43898b', '{"ID":"1a9cf7c6-a38b-4250-b5e3-69c43a43898b","sequence":145}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "created_at", "content_type", "content", "id", "parent_id", "properties") VALUES ('818380', 1778679720253, 1778679719982, 'text', 'Engine Foundations', 'block:2ec9239c-2eb6-4a7f-8ec8-af84ab61a860', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"2ec9239c-2eb6-4a7f-8ec8-af84ab61a860","sequence":146,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "parent_id", "created_at", "content_type", "content", "sort_key", "properties") VALUES (1778679720253, 'block:3613d5b3-c5bd-404d-8542-f935c744f57a', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719982, 'text', 'Test Quality & Performance', '8280', '{"ID":"3613d5b3-c5bd-404d-8542-f935c744f57a","sequence":147,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "content", "created_at", "sort_key", "updated_at", "parent_id", "id", "properties") VALUES ('text', '_archive', 1778679719983, '82817E80', 1778679720253, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:50c057fc-d631-4bb4-ac9f-b1b3a295408e', '{"ID":"50c057fc-d631-4bb4-ac9f-b1b3a295408e","sequence":148}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "parent_id", "content", "content_type", "id", "sort_key", "created_at", "properties") VALUES (1778679720253, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'LogSeq replacement', 'text', 'block:5757f157-f8a7-4bcd-a68b-dde09b30d3b9', '82817F80', 1778679719983, '{"ID":"5757f157-f8a7-4bcd-a68b-dde09b30d3b9","sequence":149}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "content", "id", "parent_id", "content_type", "updated_at", "properties") VALUES ('82817F8180', 1778679719983, 'Multi-Frontend Strategy', 'block:686fbc5b-1e64-4f16-ab7c-2506b13550bf', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1778679720253, '{"ID":"686fbc5b-1e64-4f16-ab7c-2506b13550bf","sequence":150,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "updated_at", "parent_id", "content_type", "sort_key", "content", "id", "properties") VALUES (1778679719983, 1778679720253, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '828180', 'Hypotheses', 'block:9b5f2c35-3d7b-49d1-b10f-c1ef725fba64', '{"ID":"9b5f2c35-3d7b-49d1-b10f-c1ef725fba64","sequence":151,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "updated_at", "parent_id", "created_at", "id", "content", "sort_key", "properties") VALUES ('text', 1778679720253, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679719983, 'block:9c56ee76-41c8-4319-ba6c-59c2fbb75816', 'Dogfooding & Agents', '82818180', '{"ID":"9c56ee76-41c8-4319-ba6c-59c2fbb75816","sequence":152,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "parent_id", "updated_at", "id", "created_at", "content_type", "content", "properties") VALUES ('828280', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1778679720253, 'block:ae15f87d-4da4-42e7-a5c9-c06ab06a3410', 1778679719983, 'text', 'MVP Definition', '{"ID":"ae15f87d-4da4-42e7-a5c9-c06ab06a3410","sequence":153,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "updated_at", "created_at", "id", "content_type", "sort_key", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Frontends', 1778679720253, 1778679719983, 'block:ba587ce2-4401-452d-a39c-b9bd424483d0', 'text', '8380', '{"ID":"ba587ce2-4401-452d-a39c-b9bd424483d0","sequence":154}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "created_at", "sort_key", "content_type", "updated_at", "content", "properties") VALUES ('block:c373b382-75ce-419a-b594-b1d3007856f0', 'block:ba587ce2-4401-452d-a39c-b9bd424483d0', 1778679719983, '7F80', 'text', 1778679720253, 'TUI', '{"ID":"c373b382-75ce-419a-b594-b1d3007856f0","sequence":155,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "id", "content_type", "parent_id", "created_at", "content", "sort_key", "properties") VALUES (1778679720253, 'block:d1c5f763-3a5c-44a5-9ba0-d35cee48423e', 'text', 'block:ba587ce2-4401-452d-a39c-b9bd424483d0', 1778679719983, 'GPUI', '80', '{"ID":"d1c5f763-3a5c-44a5-9ba0-d35cee48423e","sequence":156,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "id", "updated_at", "content_type", "created_at", "content", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '83817F80', 'block:bdcd8978-fc82-434b-8577-c61e87076089', 1778679720253, 'text', 1778679719983, 'Market launch', '{"ID":"bdcd8978-fc82-434b-8577-c61e87076089","sequence":157}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "created_at", "parent_id", "sort_key", "content", "content_type", "id", "properties") VALUES (1778679720253, 1778679719983, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '838180', 'Entity Identity', 'text', 'block:db06ac66-eebd-41bb-80cb-1f722216534c', '{"ID":"db06ac66-eebd-41bb-80cb-1f722216534c","sequence":158,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "created_at", "updated_at", "parent_id", "content", "sort_key", "properties") VALUES ('text', 'block:f35c4a0c-290a-431a-8ec5-96ba313ee736', 1778679719983, 1778679720253, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Now', '8480', '{"ID":"f35c4a0c-290a-431a-8ec5-96ba313ee736","sequence":159,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content_type", "created_at", "parent_id", "content", "updated_at", "sort_key", "properties") VALUES ('block:fdcbdda5-f75e-4c37-bd42-e7bf9cefeacf', 'text', 1778679719983, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'README', 1778679720253, '848180', '{"ID":"fdcbdda5-f75e-4c37-bd42-e7bf9cefeacf","sequence":160}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content", "parent_id", "content_type", "id", "updated_at", "sort_key", "properties") VALUES (1778679719983, 'Frontends', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'block:8b590af4-9099-4844-87a9-1e915c66fcf5', 1778679720253, '8580', '{"ID":"8b590af4-9099-4844-87a9-1e915c66fcf5","sequence":161}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("updated_at", "sort_key", "parent_id", "created_at", "content_type", "id", "content", "properties") VALUES (1778679727334, 'A0', 'sentinel:no_parent', 1778679727334, 'text', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'GPUI', '{"ID":"d09025cc-3748-404e-ad4d-432fcdc194d5","sequence":0}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw (id, parent_id, content, content_type, sort_key, properties, created_at, updated_at) VALUES ('block:journals', 'sentinel:no_parent', 'Journals', 'text', 'A0', '{}', 1778679729137, 1778679729137);

-- [actor_exec]
INSERT OR IGNORE INTO block_raw (id, parent_id, sort_key, content, properties, created_at, updated_at) VALUES ('sentinel:no_parent', 'sentinel:no_parent', 'A0', '__default__', '{}', 1778679729989, 1778679729989);

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("content", "id", "updated_at", "content_type", "created_at", "sort_key", "parent_id", "properties") VALUES ('Holon Layout', 'block:root-layout', 1778679730011, 'text', 1778679729988, '80', 'sentinel:no_parent', '{"requires":"Array([])","ID":"root-layout","_routing_doc_uri":"sentinel:no_parent","sequence":0}');

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("sort_key", "created_at", "id", "content_type", "updated_at", "content", "parent_id", "properties") VALUES ('7F80', 1778679729989, 'block:default-left-sidebar', 'text', 1778679730088, 'Left Sidebar', 'block:root-layout', '{"collapse_to":"drawer","ID":"default-left-sidebar","requires":"Array([])","sequence":1,"_routing_doc_uri":"sentinel:no_parent"}');

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("sort_key", "created_at", "id", "updated_at", "source_language", "content_type", "content", "parent_id", "properties") VALUES ('7F80', 1778679729989, 'block:block:left_sidebar::render::0', 1778679730100, 'render', 'source', 'list(#{sortkey: "content", item_template: selectable(row(icon("notebook"), spacer(6), text(col("content"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})})', 'block:default-left-sidebar', '{"ID":"block:left_sidebar::render::0","_routing_doc_uri":"sentinel:no_parent","requires":"Array([])","sequence":2}');

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("source_language", "content", "updated_at", "created_at", "content_type", "parent_id", "id", "sort_key", "properties") VALUES ('holon_gql', 'MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''main'' AND root.id = fr.root_id RETURN d', 1778679730252, 1778679729989, 'source', 'block:default-main-panel', 'block:default-main-panel::src::0', '80', '{"requires":"Array([])","sequence":5,"ID":"default-main-panel::src::0","_routing_doc_uri":"sentinel:no_parent"}');

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("content", "updated_at", "created_at", "parent_id", "content_type", "id", "sort_key", "properties") VALUES ('Right Sidebar', 1778679730273, 1778679729989, 'block:root-layout', 'text', 'block:default-right-sidebar', '8180', '{"sequence":6,"collapse_to":"drawer","requires":"Array([])","_routing_doc_uri":"sentinel:no_parent","ID":"default-right-sidebar"}');

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("id", "source_language", "sort_key", "created_at", "updated_at", "parent_id", "content", "content_type", "properties") VALUES ('block:default-right-sidebar::render::0', 'render', '7F80', 1778679729989, 1778679730427, 'block:default-right-sidebar', 'tree(#{parent_id: col("parent_id"), sortkey: col("sort_key"), item_template: render_entity(), rules: [#{when: eq("level", 0), override: #{role: "page_title", show_bullet: false, show_chevron: false}}]})', 'source', '{"requires":"Array([])","ID":"default-right-sidebar::render::0","_routing_doc_uri":"sentinel:no_parent","sequence":7}');

-- [actor_exec]
INSERT OR REPLACE INTO block_raw ("content_type", "sort_key", "created_at", "id", "content", "parent_id", "source_language", "updated_at", "properties") VALUES ('source', '80', 1778679729989, 'block:default-right-sidebar::src::0', 'MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = ''right'' AND root.id = fr.root_id RETURN d ORDER BY fr.added_ts DESC, d.sort_key', 'block:default-right-sidebar', 'holon_gql', 1778679730432, '{"_routing_doc_uri":"sentinel:no_parent","sequence":8,"requires":"Array([])","ID":"default-right-sidebar::src::0"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "updated_at", "content", "sort_key", "created_at", "content_type", "properties") VALUES ('block:gpui-primary-rationale', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 1778679730526, 'Why this is the primary frontend
GPUI runs natively on macOS, Android, and iOS. Per memory, it''s the #1
frontend; Flutter is parked. Since AC-1 covers all three platforms with
GPUI alone, this file owns those tasks.', '7D80', 1778679730096, 'text', '{"ID":"gpui-primary-rationale","reference-memory":"gpui_primary_frontend.md","sequence":0}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "created_at", "updated_at", "content", "content_type", "sort_key", "properties") VALUES ('block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'block:gpui-macos', 1778679730096, 1778679730526, 'macOS', 'text', '7E80', '{"ID":"gpui-macos","sequence":1,"status":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "content", "created_at", "parent_id", "content_type", "sort_key", "properties") VALUES ('block:gpui-macos-stable', 1778679730526, 'Daily-driveable polish
The desktop reference target. Most engine + reactive-shell PBT
investigations land here. Soak in main while remaining HANDOFF items
clear.', 1778679730096, 'block:gpui-macos', 'text', '80', '{"Effort":"-","ID":"gpui-macos-stable","priority":3,"sequence":2,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "updated_at", "sort_key", "created_at", "content_type", "content", "properties") VALUES ('block:gpui-android', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 1778679730526, '7F80', 1778679730097, 'text', 'Android', '{"ID":"gpui-android","reference-worktree":".claude/worktrees/agent-* (per existing PBT layout)","sequence":3,"status":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "updated_at", "content", "content_type", "id", "parent_id", "sort_key", "properties") VALUES (1778679730097, 1778679730526, 'Android build target stable
GPUI on Android is the leverage point that lets us drop Flutter. Stabilize
the build pipeline and confirm core flows work. Cross-device sync (AC-2)
should be tested macOS↔Android↔iOS.', 'text', 'block:gpui-android-stable', 'block:gpui-android', '7F80', '{"Effort":"-","ID":"gpui-android-stable","priority":3,"sequence":4,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "updated_at", "sort_key", "content_type", "parent_id", "id", "properties") VALUES ('Android-specific UX adaptations
Touch targets, soft keyboard handling, gesture navigation. Don''t expect
desktop ergonomics to translate.', 1778679730097, 1778679730526, '80', 'text', 'block:gpui-android', 'block:gpui-android-ux', '{"Effort":"2:00","ID":"gpui-android-ux","REQUIRES":"gpui-android-stable","priority":2,"sequence":5,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:gpui-android-ux', 'block:gpui-android-stable');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "id", "parent_id", "content_type", "updated_at", "sort_key", "properties") VALUES ('iOS', 1778679730097, 'block:gpui-ios', 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'text', 1778679730526, '7F8180', '{"ID":"gpui-ios","reference-worktree":".claude/worktrees/gpui-ios","sequence":6,"status":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "sort_key", "created_at", "id", "parent_id", "content", "content_type", "properties") VALUES (1778679730526, '7F80', 1778679730097, 'block:gpui-ios-stable', 'block:gpui-ios', 'iOS build target stable
Per existing worktree gpui-ios, in progress. Wife''s iPhone is the test
device — read-only sharing case (AC-4) should work end-to-end on iOS.', 'text', '{"Effort":"-","ID":"gpui-ios-stable","priority":3,"sequence":7,"task_state":"DOING"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "updated_at", "content_type", "parent_id", "content", "id", "properties") VALUES ('80', 1778679730097, 1778679730527, 'text', 'block:gpui-ios', 'iOS-specific UX adaptations', 'block:gpui-ios-ux', '{"Effort":"2:00","ID":"gpui-ios-ux","REQUIRES":"gpui-ios-stable","priority":2,"sequence":8,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:gpui-ios-ux', 'block:gpui-ios-stable');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "updated_at", "parent_id", "content_type", "id", "created_at", "content", "properties") VALUES ('80', 1778679730527, 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'text', 'block:gpui-cross-platform', 1778679730097, 'Cross-platform infrastructure', '{"ID":"gpui-cross-platform","sequence":9}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "content_type", "updated_at", "content", "sort_key", "created_at", "properties") VALUES ('block:gpui-cross-platform', 'block:2b4f3e8e-57f0-46de-a47a-be1242dd9e12', 'text', 1778679730527, 'Outliner editing
The bulk of the engine investigations land here. Per the memory entries,
many split-block, BulkExternalAdd, ReactiveShell, FocusRegistry items have
been resolved or are in flight.', '7F80', 1778679730097, '{"ID":"2b4f3e8e-57f0-46de-a47a-be1242dd9e12","sequence":10}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "parent_id", "created_at", "updated_at", "sort_key", "content", "properties") VALUES ('text', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', 'block:gpui-cross-platform', 1778679730097, 1778679730527, '80', 'Three-mode UX shells', '{"ID":"ba5ad62d-bd47-47b8-873b-1b668a85e9eb","sequence":11}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "created_at", "sort_key", "content_type", "content", "parent_id", "properties") VALUES ('block:capture-mode-overlay', 1778679730527, 1778679730097, '7F80', 'text', 'Capture mode overlay (global hotkey)
Global hotkey summons input field, captures to inbox, dismisses. Spotlight-
shaped UX. See docs/Vision/UI.md §Capture Mode.', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', '{"Effort":"4:00","ID":"capture-mode-overlay","priority":3,"sequence":12,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "created_at", "sort_key", "parent_id", "id", "content_type", "content", "properties") VALUES (1778679730527, 1778679730098, '80', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', 'block:orient-daily-view', 'text', 'Orient daily view
Today''s focus + inbox + watcher placeholder. Reads from the Now query
(per Engine Foundations). No AI yet at G1; structural-only.', '{"Effort":"4:00","ID":"orient-daily-view","REQUIRES":"now-query-mcp","priority":3,"sequence":13,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:orient-daily-view', 'block:now-query-mcp');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content_type", "parent_id", "created_at", "updated_at", "content", "sort_key", "properties") VALUES ('block:flow-mode-shell', 'text', 'block:ba5ad62d-bd47-47b8-873b-1b668a85e9eb', 1778679730098, 1778679730527, 'Flow mode shell
Single-task focus. Context-on-demand panel from the right edge. Hide
distractions (other panels fade). The polish (text recency effect,
timer animation) is G5; the shell is G1.', '8180', '{"Effort":"3:00","ID":"flow-mode-shell","REQUIRES":"orient-daily-view","priority":2,"sequence":14,"task_state":"TODO"}');

-- [transaction_stmt]
INSERT INTO block_requires ("block_id", "required_id") VALUES ('block:flow-mode-shell', 'block:orient-daily-view');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "updated_at", "parent_id", "sort_key", "id", "content_type", "content", "properties") VALUES (1778679730099, 1778679730527, 'block:handoff-displayed-text-invariant', '7D80', 'block:ca3da762-4def-4756-8cf4-3576a52beb54', 'text', 'What we were chasing
After Enter (splitloc) inside an existing block''s text — and analogously after
Backspace at column 0 (joinloc) — the GPUI UI shows stale content for the original
block. SQL and the Org file are correct (truncated/merged as expected). The on-screen
editableex fails to follow.
Concrete repro from the user''s screenshot (#[triggered(...)] for operation availability):
| Source | block:5657317c-… content | block:674e5a08-… content |
|--------+----------------------------+----------------------------|
| Holon.org line 104 | #[triggered(...)] for operation | — |
| Holon.org line 109 | — | availabilityabc |
| Live SQL (MCP executeaq on port 8520) | #[triggered(...)] for operation | availabilityabc |
| GPUI screen | #[triggered(...)] for operation availability (stale) | availabilityabc |
DB ↔ org file agree. The bug is purely on the read-back path to the editor.
The same family is documented in tasktatoggleug. "Other manifestations".', '{"ID":"ca3da762-4def-4756-8cf4-3576a52beb54","sequence":17}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "created_at", "parent_id", "content_type", "updated_at", "content", "properties") VALUES ('block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', '7F80', 1778679730100, 'block:handoff-displayed-text-invariant', 'text', 1778679730527, 'What landed (this session)', '{"ID":"92bd7471-a03b-4cc0-8981-e24e1ba833a3","sequence":19}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "sort_key", "parent_id", "updated_at", "content_type", "id", "properties") VALUES ('1. New field — ElementInfo.displayedex: Option<String>
- crates/holon-frontend/src/geometry.rs:12-… — added field with rationale comment.', 1778679730100, '7E80', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', 1778679730527, 'text', 'block:9be6a99d-f5a8-4dd3-9cc2-35611c1fc0a3', '{"ID":"9be6a99d-f5a8-4dd3-9cc2-35611c1fc0a3","sequence":20}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "id", "parent_id", "updated_at", "content_type", "content", "sort_key", "properties") VALUES (1778679730100, 'block:1be0e5f8-a64d-4b91-9434-6a03ecf9957b', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', 1778679730527, 'text', '2. GPUI plumbing
- =frontends/gpui/src/geometry.rs::tracked()= — extra displayedex: Option<String> arg.
- =frontends/gpui/src/geometry.rs::BoundsTracker= — extra field, propagated into record() during prepaint.
- =frontends/gpui/src/geometry.rs::TransparentTracker::prepaint= — fills displayedex: None.', '7F80', '{"ID":"1be0e5f8-a64d-4b91-9434-6a03ecf9957b","sequence":21}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "created_at", "sort_key", "content", "parent_id", "updated_at", "id", "properties") VALUES ('text', 1778679730101, '80', '3. Builder captures
- frontends/gpui/src/render/builders/editableext.r
  - Cached path: reads entity.read(cx).inputntit().read(cx).value().totrin() before
    entity.intonlemen(). Passes Some(displayedex) to tracked().
  - Fresh-create path: reads the InputState value right after cx.new(|cx| EditorView::new(...)).
- selectable.rs and renderntity.r: pass None (not text-bearing).
- frontends/gpui/tests/layoutmoke.r:231: test helper updated.', 'block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', 1778679730527, 'block:d3aa021f-1c89-48b0-b025-ae8392730c18', '{"ID":"d3aa021f-1c89-48b0-b025-ae8392730c18","sequence":22}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content", "id", "content_type", "sort_key", "created_at", "updated_at", "properties") VALUES ('block:92bd7471-a03b-4cc0-8981-e24e1ba833a3', '4. Invariant — inv-displayed-text in sut.rs::checknvariantsyn
- crates/holon-integration-tests/src/pbt/sut.rs — appended after inv15.
- Iterates geometry.alllement(), filters widgetyp = "editableex"= with
  displayedext.iom() and entity starting with block:.
- Skips the currently-focused block — production deliberately doesn''t overwrite
  InputState while focused.
- Looks up the block in reftate.bloctate.block and compares displayedex
  against block.contentex().
- Skipped on navnl transitions.', 'block:8d0f44bf-8505-43e6-9e91-a6695515ba4c', 'text', '8180', 1778679730101, 1778679730527, '{"ID":"8d0f44bf-8505-43e6-9e91-a6695515ba4c","sequence":23}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "sort_key", "id", "parent_id", "content_type", "content", "updated_at", "properties") VALUES (1778679730101, '7F8180', 'block:4e69c69d-68f0-47ce-b154-720a6f7dbfc9', 'block:handoff-displayed-text-invariant', 'text', 'Build status
- cargo check -p holon-frontend — clean (warnings only, pre-existing).
- cargo check -p holon-integration-tests — clean.
- cargo check -p holon-gpui — fails with 8 pre-existing errors unrelated to this work:
  EditorView::new callers missing NavigationState, FocusRegistry, Arc<RwLock<Option<String>>>
  args. Constructor-signature drift from a half-finished refactor; resolving them is out
  of scope and required before the PBT can run.', 1778679730527, '{"ID":"4e69c69d-68f0-47ce-b154-720a6f7dbfc9","sequence":24}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "parent_id", "content", "content_type", "id", "sort_key", "updated_at", "properties") VALUES (1778679730102, 'block:handoff-displayed-text-invariant', 'TODOs
- [ ] Resolve pre-existing holon-gpui compile errors (constructor-signature drift for NavigationState, FocusRegistry, focusedi) before the PBT can run.
- [ ] Run the GPUI PBT after compile is clean: cargo test -p holon-gpui --test gpuib 2>&1 | tee /tmp/gpui_pbt.log. Expectation: shrunk seed hits SplitBlock/JoinBlock where displayedex doesn''t match reftate.block[uri].contentex().
- [ ] If PBT doesn''t fail: re-check split positions, navnl flag on SplitBlock/JoinBlock, and focus-at-invariant-check timing.
- [ ] Consider adding displayedex to text(...) builders (non-editable text — cheap to add, would catch stale non-editable widgets).', 'text', 'block:22da2e7f-dc3c-457c-81db-1fe73ca9d7d7', '80', 1778679730527, '{"ID":"22da2e7f-dc3c-457c-81db-1fe73ca9d7d7","sequence":25}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "parent_id", "created_at", "id", "content", "updated_at", "properties") VALUES ('817F80', 'text', 'block:handoff-displayed-text-invariant', 1778679730102, 'block:b1a5c00e-5bdb-4b87-be3d-9496d787e6f7', 'Files touched
- crates/holon-frontend/src/geometry.rs
- frontends/gpui/src/geometry.rs
- frontends/gpui/src/render/builders/editableext.r
- frontends/gpui/src/render/builders/selectable.rs
- frontends/gpui/src/render/builders/renderntity.r
- frontends/gpui/tests/layoutmoke.r
- crates/holon-integration-tests/src/pbt/sut.rs', 1778679730527, '{"ID":"b1a5c00e-5bdb-4b87-be3d-9496d787e6f7","sequence":26}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "content_type", "content", "sort_key", "created_at", "updated_at", "parent_id", "properties") VALUES ('block:c8f65c84-3843-4bbd-bdd5-e4c4ed340562', 'text', 'MCP one-liner used to confirm DB state', '8180', 1778679730102, 1778679730527, 'block:handoff-displayed-text-invariant', '{"ID":"c8f65c84-3843-4bbd-bdd5-e4c4ed340562","sequence":27}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "content_type", "sort_key", "created_at", "parent_id", "content", "source_language", "properties") VALUES ('block:c8f65c84-3843-4bbd-bdd5-e4c4ed340562::src::0', 1778679730527, 'source', '80', 1778679730102, 'block:c8f65c84-3843-4bbd-bdd5-e4c4ed340562', 'curl -s -X POST http://127.0.0.1:8520/mcp \
  -H ''Content-Type: application/json'' -H ''Accept: application/json,text/event-stream'' \
  -H "mcp-session-id: $SESSION" \
  -d ''{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"execute_raw_sql","arguments":{"sql":"SELECT id, content FROM block WHERE content LIKE ''\''''%triggered%''\'''' OR content LIKE ''\''''%availability%''\''''"}}}''', 'bash', '{"ID":"c8f65c84-3843-4bbd-bdd5-e4c4ed340562::src::0","sequence":28}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "sort_key", "content_type", "created_at", "content", "parent_id", "updated_at", "properties") VALUES ('block:64723edc-36ca-45aa-bf4b-9937e167e687', '8280', 'text', 1778679730102, 'Open questions
- After holon-gpui compiles, will inv-displayed-text be too strict for the cached
  EditorView path? If editoriew is cached and InputState lags during normal CDC
  (not just bug cases), a short retry/settle may be needed — see how inv16 handles
  prenv1_settle.
- Should text(...) builders also fill displayedex?', 'block:handoff-displayed-text-invariant', 1778679730527, '{"ID":"64723edc-36ca-45aa-bf4b-9937e167e687","sequence":29}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "content_type", "content", "updated_at", "created_at", "sort_key", "id", "properties") VALUES ('block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'text', 'Handoff — GPUI PBT: inv-displayed-text fix + remaining open items', 1778679730527, 1778679730102, '8180', 'block:handoff-gpui-pbt-remaining', '{"ID":"handoff-gpui-pbt-remaining","sequence":30,"source-date":"2026-04-29","source-file":"HANDOFF_GPUI_PBT_REMAINING.md"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "sort_key", "created_at", "id", "content_type", "updated_at", "parent_id", "properties") VALUES ('What landed this session', '7F80', 1778679730102, 'block:80beb3b6-a231-4bd8-b676-600f991d261e', 'text', 1778679730527, 'block:handoff-gpui-pbt-remaining', '{"ID":"80beb3b6-a231-4bd8-b676-600f991d261e","sequence":31}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "sort_key", "content_type", "parent_id", "id", "content", "created_at", "properties") VALUES (1778679730527, '80', 'text', 'block:80beb3b6-a231-4bd8-b676-600f991d261e', 'block:f29b6733-709e-4ea1-8961-37532605de9b', 'inv-displayed-text false positive fix
The inv-displayed-text invariant in sut.rs:5152 was panicking after TypeChars
because the skip set only consulted reftate.focuse_entity. But
FocusEditableText deliberately does NOT update focusedntit (to keep inv15
stable) — it sets activedito instead. The actively-edited block was incorrectly
checked, and since TypeChars updates InputState without committing to SQL (commit
only happens on Blur / PressKey(Enter)), the invariant fired a false positive.
Fix: 6-line insertion at sut.rs:5157-5164 — added reftate.activditor.bloc
to the skip set.
Result: gpuib passes 50/50. The invariant transitions from "GATED" to fully
operational.', 1778679730102, '{"ID":"f29b6733-709e-4ea1-8961-37532605de9b","sequence":32}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "created_at", "updated_at", "content_type", "content", "parent_id", "sort_key", "properties") VALUES ('block:05e9b86d-54df-4524-91c9-8216a4c5b17c', 1778679730102, 1778679730527, 'text', 'Remaining open items', 'block:handoff-gpui-pbt-remaining', '80', '{"ID":"05e9b86d-54df-4524-91c9-8216a4c5b17c","sequence":33}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "content", "created_at", "updated_at", "parent_id", "sort_key", "properties") VALUES ('text', 'block:f92f3df5-4e7d-4dfb-b675-ae65b8e5d6f1', '1. Production staleness: _dataubscriptio skips updates while focused
File: frontends/gpui/src/views/editoriew.r:237-240
Symptom: After splitloc (Enter) or joinloc (Backspace at 0), the original
block''s editableex shows stale content. SQL and org file are correct.
Root cause: When the editor''s InputState is focused, the _dataubscriptio
returns early without applying CDC updates. Structural changes (split/join) that alter
the block''s content via CDC are swallowed.
The fix in splitendinditix. ensures commit happens before the structural
op reads DB state. But the CDC echo that follows (block''s content column changing from
"full text" to "truncated text") is still dropped by the focused-skip guard.
Family: Same root cause as tasktatoggleug. "Other manifestations."
Why the PBT doesn''t catch it: The inv-displayed-text check skips the focused block
precisely because production intentionally doesn''t update while focused. To catch this
class of bug, an invariant that runs after blur is needed.
Possible fixes (ordered by risk):
1. Blur-before-check in PBT: Add a Blur step after TypeChars=/=DeleteBackward=/
   =PressKey sequences that end with the editor still focused, then check
   inv-displayed-text post-blur. Catches the staleness without touching production code.
2. Targeted override: After commitheispatchtructur dispatches the structural
   intent, force-apply the post-commit content to InputState even while focused. Risk:
   cursor position may need adjustment.
3. Rebind subscription on structural ops: When a chord handler dispatches a structural
   intent, replace the _dataubscriptio''s data handle with one pointing at the new
   row (post-split). Most architecturally clean but touches the subscription lifecycle.', 1778679730103, 1778679730527, 'block:05e9b86d-54df-4524-91c9-8216a4c5b17c', '7F80', '{"ID":"f92f3df5-4e7d-4dfb-b675-ae65b8e5d6f1","sequence":34}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("content_type", "id", "updated_at", "content", "parent_id", "sort_key", "created_at", "properties") VALUES ('text', 'block:bc4ba724-08b4-4d9d-b7c2-67095c3ae16b', 1778679746073, 'LogSeq replacement', 'sentinel:no_parent', 'A0', 1778679746073, '{"ID":"bc4ba724-08b4-4d9d-b7c2-67095c3ae16b","sequence":0}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "created_at", "sort_key", "id", "updated_at", "parent_id", "content_type", "properties") VALUES ('Editing experience', 1778679746293, '7E80', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 1778679746447, 'block:bc4ba724-08b4-4d9d-b7c2-67095c3ae16b', 'text', '{"ID":"07241ece-d9fe-4f25-80a4-63b4c1f1bbc9","sequence":0,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "content_type", "sort_key", "id", "parent_id", "created_at", "properties") VALUES ('GitHub Flavored Markdown parser & renderer for GPUI
https://github.com/joris-gallot/gpui-gfm', 1778679746465, 'text', '7F80', 'block:ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 1778679746293, '{"ID":"ec330e4e-fc7a-45dc-8a88-5a74dd4f3ec2","sequence":1,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "id", "created_at", "updated_at", "content", "content_type", "sort_key", "properties") VALUES ('block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 'block:e96b21d4-8b3a-4f53-aead-f0969b1ba3f8', 1778679746293, 1778679746465, 'Desktop Markdown viewer built with Rust and GPUI
https://github.com/chunghha/markdowniewe', 'text', '80', '{"ID":"e96b21d4-8b3a-4f53-aead-f0969b1ba3f8","sequence":2,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content", "parent_id", "updated_at", "id", "content_type", "created_at", "properties") VALUES ('8180', 'Markdown Editor and Viewer
https://github.com/kumarUjjawal/aster', 'block:07241ece-d9fe-4f25-80a4-63b4c1f1bbc9', 1778679746465, 'block:f7730a68-6268-4e65-ac93-3fdf79e92133', 'text', 1778679746293, '{"ID":"f7730a68-6268-4e65-ac93-3fdf79e92133","sequence":3,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "id", "content", "sort_key", "updated_at", "parent_id", "properties") VALUES (1778679746293, 'text', 'block:cc6c3307-9a98-4d98-a8f2-ede3de67affc', '', '7F80', 1778679746465, 'block:bc4ba724-08b4-4d9d-b7c2-67095c3ae16b', '{"ID":"cc6c3307-9a98-4d98-a8f2-ede3de67affc","sequence":4}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "updated_at", "sort_key", "created_at", "content", "content_type", "parent_id", "properties") VALUES ('block:8594ab7c-5f36-44cf-8f92-248b31508441', 1778679746465, '80', 1778679746293, 'PDF Viewer & Annotator', 'text', 'block:bc4ba724-08b4-4d9d-b7c2-67095c3ae16b', '{"ID":"8594ab7c-5f36-44cf-8f92-248b31508441","sequence":5,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "updated_at", "id", "created_at", "sort_key", "content_type", "parent_id", "properties") VALUES ('Combining gpui and hayro for a little application that render pdfs
https://github.com/vincenthz/gpui-hayro?tab=readme-ov-file', 1778679746466, 'block:d4211fbe-8b94-47e0-bb48-a9ea6b95898c', 1778679746294, '7F80', 'text', 'block:8594ab7c-5f36-44cf-8f92-248b31508441', '{"ID":"d4211fbe-8b94-47e0-bb48-a9ea6b95898c","sequence":6,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("updated_at", "parent_id", "sort_key", "created_at", "content", "content_type", "id", "properties") VALUES (1778679746466, 'block:8594ab7c-5f36-44cf-8f92-248b31508441', '80', 1778679746294, 'Libera Reader
Modern, performance-oriented desktop e-book reader built with Rust and GPUI.
https://github.com/RikaKit2/libera-reader', 'text', 'block:b95a19a6-5448-42f0-af06-177e95e27f49', '{"ID":"b95a19a6-5448-42f0-af06-177e95e27f49","sequence":7,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("content", "id", "parent_id", "updated_at", "created_at", "sort_key", "content_type", "properties") VALUES ('https://github.com/RikaKit2/libera-reader', 'block:f2a8571b-821f-46c1-84be-e6dac6a84028', 'block:b95a19a6-5448-42f0-af06-177e95e27f49', 1778679746466, 1778679746294, '80', 'text', '{"ID":"f2a8571b-821f-46c1-84be-e6dac6a84028","sequence":8}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "parent_id", "id", "content", "content_type", "sort_key", "updated_at", "properties") VALUES (1778679746294, 'block:8594ab7c-5f36-44cf-8f92-248b31508441', 'block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', 'Monica: On-screen anotation software
https://github.com/tasuren/monica', 'text', '8180', 1778679746466, '{"ID":"812924a9-0bc2-41a7-8820-1c60a40bd1ad","sequence":9,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("created_at", "content_type", "updated_at", "id", "content", "parent_id", "sort_key", "properties") VALUES (1778679746294, 'text', 1778679746466, 'block:91054e85-0cc1-4968-8643-9e33a1160930', 'https://github.com/tasuren/monica', 'block:812924a9-0bc2-41a7-8820-1c60a40bd1ad', '80', '{"ID":"91054e85-0cc1-4968-8643-9e33a1160930","sequence":10}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("id", "created_at", "content", "parent_id", "updated_at", "sort_key", "content_type", "properties") VALUES ('block:419b2df8-0121-4532-8dcd-21f04df806d8', 1778679746294, 'Graph vis', 'block:bc4ba724-08b4-4d9d-b7c2-67095c3ae16b', 1778679746466, '8180', 'text', '{"ID":"419b2df8-0121-4532-8dcd-21f04df806d8","sequence":11,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "created_at", "updated_at", "id", "content_type", "content", "properties") VALUES ('block:419b2df8-0121-4532-8dcd-21f04df806d8', '80', 1778679746294, 1778679746466, 'block:f520a9ff-71bf-4a72-8777-9864bad7c535', 'text', 'https://github.com/jerlendds/gpug', '{"ID":"f520a9ff-71bf-4a72-8777-9864bad7c535","sequence":12,"shared-tree-id":"86d2c04e-ee00-4f6e-bb5b-b67c8678a3fe"}');

-- [actor_exec]
INSERT OR IGNORE INTO block_raw ("id", "parent_id", "updated_at", "content", "created_at", "content_type", "sort_key", "properties") VALUES ('block:e37a1996-06e0-429a-8364-5e83b4599556', 'sentinel:no_parent', 1778679747293, 'Phase 7: Team Features', 1778679747293, 'text', 'A0', '{"ID":"e37a1996-06e0-429a-8364-5e83b4599556","sequence":0}');

-- [actor_exec]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:e37a1996-06e0-429a-8364-5e83b4599556', 'Page');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "content_type", "parent_id", "created_at", "updated_at", "id", "content", "properties") VALUES ('7F80', 'text', 'block:e37a1996-06e0-429a-8364-5e83b4599556', 1778679747455, 1778679747566, 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', 'Delegation System [/]', '{"ID":"8cf3b868-2970-4d45-93e5-8bca58e3bede","sequence":0}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "sort_key", "content", "created_at", "updated_at", "id", "content_type", "properties") VALUES ('block:8cf3b868-2970-4d45-93e5-8bca58e3bede', '7E80', '@Person: syntax for delegation sub-nets', 1778679747455, 1778679747566, 'block:15c4b164-b29f-4fb0-b882-e6408f2e3264', 'text', '{"ID":"15c4b164-b29f-4fb0-b882-e6408f2e3264","sequence":1}');

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("sort_key", "created_at", "content_type", "content", "id", "updated_at", "parent_id", "properties") VALUES ('7F80', 1778679747455, 'text', 'Waiting-for tracking (automatic from delegation patterns)', 'block:fbbce845-023e-438b-963e-471833c51505', 1778679747566, 'block:8cf3b868-2970-4d45-93e5-8bca58e3bede', '{"ID":"fbbce845-023e-438b-963e-471833c51505","sequence":2}');

-- Wait 3ms

