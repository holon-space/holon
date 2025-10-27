-- Minimized replay (110 statements)

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
INSERT INTO block_raw ("created_at", "content_type", "updated_at", "content", "parent_id", "id", "properties") VALUES (1779016366152, 'text', 1779016366169, 'Inspiration', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:5f42e828-f9bd-4ef4-81a6-c6037e36fbb1', '{"ID":"5f42e828-f9bd-4ef4-81a6-c6037e36fbb1","sequence":37}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content", "created_at", "updated_at", "parent_id", "content_type", "properties") VALUES ('block:64ab3203-2b18-4fc7-8a26-e46736973f2a', 'Dogfooding & Agents', 1779016366152, 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '{"ID":"64ab3203-2b18-4fc7-8a26-e46736973f2a","sequence":38,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "updated_at", "content", "content_type", "created_at", "id", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366169, 'README', 'text', 1779016366152, 'block:65b11d93-b526-4179-a1fc-22b5f64619c8', '{"ID":"65b11d93-b526-4179-a1fc-22b5f64619c8","sequence":39}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "id", "content", "updated_at", "parent_id", "created_at", "properties") VALUES ('text', 'block:666cbfbc-e9f8-4927-bd15-3222e2deb609', 'Inspiration', 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366152, '{"ID":"666cbfbc-e9f8-4927-bd15-3222e2deb609","sequence":40}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "content_type", "created_at", "updated_at", "id", "content", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366152, 1779016366169, 'block:686fbc5b-1e64-4f16-ab7c-2506b13550bf', 'Multi-Frontend Strategy', '{"ID":"686fbc5b-1e64-4f16-ab7c-2506b13550bf","sequence":41,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "created_at", "id", "parent_id", "content_type", "updated_at", "properties") VALUES ('Test Quality & Performance', 1779016366153, 'block:6970ced1-3717-4d13-964f-21d3a7751f3d', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366169, '{"ID":"6970ced1-3717-4d13-964f-21d3a7751f3d","sequence":42,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "created_at", "content", "content_type", "parent_id", "properties") VALUES (1779016366169, 'block:69759b57-989c-41cc-83d3-10145fe7e3ef', 1779016366153, 'LogSeq replacement', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"69759b57-989c-41cc-83d3-10145fe7e3ef","sequence":43}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content" = excluded."content", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "created_at", "content_type", "id", "updated_at", "parent_id", "properties") VALUES ('Now', 1779016366153, 'text', 'block:6e253a11-5b11-4566-8a7f-14ae660cc2a7', 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"6e253a11-5b11-4566-8a7f-14ae660cc2a7","sequence":44,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "parent_id", "content", "content_type", "created_at", "id", "properties") VALUES (1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Market launch', 'text', 1779016366153, 'block:6e40f290-926e-4059-acc8-e0675a6dafe4', '{"ID":"6e40f290-926e-4059-acc8-e0675a6dafe4","sequence":45}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "id", "content", "content_type", "created_at", "updated_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:7003c7e8-96aa-40ad-8e60-505e7da72ff1', 'LogSeq replacement', 'text', 1779016366153, 1779016366169, '{"ID":"7003c7e8-96aa-40ad-8e60-505e7da72ff1","sequence":46}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "created_at", "content_type", "updated_at", "id", "parent_id", "properties") VALUES ('Engine Foundations', 1779016366153, 'text', 1779016366169, 'block:745022be-1079-4452-af0d-17fe4a0f87bd', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"745022be-1079-4452-af0d-17fe4a0f87bd","sequence":47,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "updated_at", "parent_id", "content", "id", "created_at", "properties") VALUES ('text', 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Market launch', 'block:79e53e13-6523-48e9-b8b6-5076217408aa', 1779016366153, '{"ID":"79e53e13-6523-48e9-b8b6-5076217408aa","sequence":48}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "id", "parent_id", "content", "updated_at", "content_type", "properties") VALUES (1779016366154, 'block:7acebf7d-40f2-48b8-8d9c-7b93382e73aa', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Multi-Frontend Strategy', 1779016366169, 'text', '{"ID":"7acebf7d-40f2-48b8-8d9c-7b93382e73aa","sequence":49,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "updated_at", "content", "content_type", "id", "parent_id", "properties") VALUES (1779016366154, 1779016366169, 'Dogfooding & Agents', 'text', 'block:7cb2d696-9884-45e3-bf70-6a40abe6ae98', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"7cb2d696-9884-45e3-bf70-6a40abe6ae98","sequence":50,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "parent_id", "content", "updated_at", "content_type", "created_at", "properties") VALUES ('block:81458f33-9442-445f-bae5-89128973aad6', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Test Quality & Performance', 1779016366169, 'text', 1779016366154, '{"ID":"81458f33-9442-445f-bae5-89128973aad6","sequence":51,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content" = excluded."content", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content_type", "parent_id", "content", "created_at", "updated_at", "properties") VALUES ('block:845a40ab-0723-4214-b247-c1cb854fc648', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'README', 1779016366154, 1779016366169, '{"ID":"845a40ab-0723-4214-b247-c1cb854fc648","sequence":52}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "parent_id", "id", "content", "content_type", "created_at", "properties") VALUES (1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:86ccb52e-7ba4-4b97-b413-2a21ebc25897', 'Multi-Frontend Strategy', 'text', 1779016366154, '{"ID":"86ccb52e-7ba4-4b97-b413-2a21ebc25897","sequence":53,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "id", "updated_at", "parent_id", "content", "content_type", "properties") VALUES (1779016366154, 'block:8b590af4-9099-4844-87a9-1e915c66fcf5', 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Frontends', 'text', '{"ID":"8b590af4-9099-4844-87a9-1e915c66fcf5","sequence":55}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "content", "content_type", "parent_id", "created_at", "properties") VALUES (1779016366169, 'block:8bf39d00-d438-43d6-93b4-31e2c766d576', 'Plain-Text Layer', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366154, '{"ID":"8bf39d00-d438-43d6-93b4-31e2c766d576","sequence":56,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "content", "id", "parent_id", "content_type", "created_at", "properties") VALUES (1779016366169, 'Now', 'block:8c2b1c5e-8dac-43a3-a23a-8907c2100c6f', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366155, '{"ID":"8c2b1c5e-8dac-43a3-a23a-8907c2100c6f","sequence":57,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "created_at", "id", "updated_at", "content", "content_type", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366155, 'block:8c973d34-0a01-444c-bcae-bcd1b53c670c', 1779016366169, 'Dogfooding & Agents', 'text', '{"ID":"8c973d34-0a01-444c-bcae-bcd1b53c670c","sequence":58,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "content", "id", "parent_id", "updated_at", "created_at", "properties") VALUES ('text', 'README', 'block:8ce38157-7a61-4735-ae7a-53b67f3056d4', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366169, 1779016366155, '{"ID":"8ce38157-7a61-4735-ae7a-53b67f3056d4","sequence":59}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "created_at", "id", "content", "updated_at", "parent_id", "properties") VALUES ('text', 1779016366155, 'block:8e0dcc49-ccee-4004-b29d-7853415edaa1', 'Plain-Text Layer', 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"8e0dcc49-ccee-4004-b29d-7853415edaa1","sequence":60,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "content", "content_type", "id", "parent_id", "updated_at", "properties") VALUES (1779016366155, 'Now', 'text', 'block:918a3173-e4e0-4383-9680-24154943d8dc', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366169, '{"ID":"918a3173-e4e0-4383-9680-24154943d8dc","sequence":61,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content" = excluded."content", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "parent_id", "content_type", "updated_at", "content", "created_at", "properties") VALUES ('block:91e909ee-e977-4b2a-91e1-54e3dc5e5bff', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366169, 'README', 1779016366155, '{"ID":"91e909ee-e977-4b2a-91e1-54e3dc5e5bff","sequence":62}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "content" = excluded."content", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "parent_id", "content", "content_type", "created_at", "properties") VALUES (1779016366169, 'block:9858dad7-8384-4314-9ed9-62dfc8c01f06', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Entity Identity', 'text', 1779016366155, '{"ID":"9858dad7-8384-4314-9ed9-62dfc8c01f06","sequence":63,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "updated_at", "id", "content_type", "parent_id", "created_at", "properties") VALUES ('Hypotheses', 1779016366169, 'block:9b5f2c35-3d7b-49d1-b10f-c1ef725fba64', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366155, '{"ID":"9b5f2c35-3d7b-49d1-b10f-c1ef725fba64","sequence":64,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content", "created_at", "updated_at", "parent_id", "content_type", "properties") VALUES ('block:9c068c00-7646-411f-9394-32cabf9d2e8b', 'LogSeq replacement', 1779016366156, 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '{"ID":"9c068c00-7646-411f-9394-32cabf9d2e8b","sequence":65}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "id", "content", "content_type", "updated_at", "parent_id", "properties") VALUES (1779016366156, 'block:9c56ee76-41c8-4319-ba6c-59c2fbb75816', 'Dogfooding & Agents', 'text', 1779016366169, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"9c56ee76-41c8-4319-ba6c-59c2fbb75816","sequence":66,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content" = excluded."content", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "created_at", "parent_id", "id", "content_type", "content", "properties") VALUES (1779016366169, 1779016366156, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:9c60f80c-08c4-4b74-ac6a-8e7258dd6c79', 'text', 'README', '{"ID":"9c60f80c-08c4-4b74-ac6a-8e7258dd6c79","sequence":67}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "updated_at", "id", "content_type", "parent_id", "created_at", "properties") VALUES ('_archive', 1779016366169, 'block:9d11b498-d410-417c-b644-efb0f9384d92', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366156, '{"ID":"9d11b498-d410-417c-b644-efb0f9384d92","sequence":68}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "content_type", "parent_id", "id", "updated_at", "content", "properties") VALUES (1779016366156, 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:9da07002-1ff4-45d2-9c91-1fe5a558345a', 1779016366169, 'Market launch', '{"ID":"9da07002-1ff4-45d2-9c91-1fe5a558345a","sequence":69}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "updated_at", "content", "parent_id", "created_at", "content_type", "properties") VALUES ('block:a198b0ae-291b-4d7f-8629-ccbaecb95840', 1779016366169, 'Frontends', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366156, 'text', '{"ID":"a198b0ae-291b-4d7f-8629-ccbaecb95840","sequence":70}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "content_type", "id", "parent_id", "updated_at", "created_at", "properties") VALUES ('GPUI', 'text', 'block:0eba88ae-0fc9-4438-a814-78a2ba2ec3a3', 'block:a198b0ae-291b-4d7f-8629-ccbaecb95840', 1779016366169, 1779016366156, '{"ID":"0eba88ae-0fc9-4438-a814-78a2ba2ec3a3","sequence":71,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "parent_id", "created_at", "content_type", "id", "content", "properties") VALUES (1779016366169, 'block:a198b0ae-291b-4d7f-8629-ccbaecb95840', 1779016366156, 'text', 'block:cd715d12-4a2e-4836-9ce9-0df9afecc7dc', 'TUI', '{"ID":"cd715d12-4a2e-4836-9ce9-0df9afecc7dc","sequence":72,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "content", "parent_id", "content_type", "created_at", "id", "properties") VALUES (1779016366169, 'Engine Foundations', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366157, 'block:a1e99644-637e-4db4-a08c-9a39eb8efed1', '{"ID":"a1e99644-637e-4db4-a08c-9a39eb8efed1","sequence":73,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "content", "parent_id", "id", "content_type", "updated_at", "properties") VALUES (1779016366157, 'MVP Definition', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:a9c98597-1a7c-498d-a405-f629ab290633', 'text', 1779016366169, '{"ID":"a9c98597-1a7c-498d-a405-f629ab290633","sequence":74,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "parent_id", "content_type", "updated_at", "created_at", "content", "properties") VALUES ('block:a9f079a0-da9e-48b9-9d2e-b267c51e4383', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366169, 1779016366157, 'Hypotheses', '{"ID":"a9f079a0-da9e-48b9-9d2e-b267c51e4383","sequence":75,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "updated_at", "created_at", "id", "content_type", "parent_id", "properties") VALUES ('Engine Foundations', 1779016366169, 1779016366157, 'block:ab58fd5f-39c5-4206-96eb-5d7bf4dd4cfc', 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"ab58fd5f-39c5-4206-96eb-5d7bf4dd4cfc","sequence":76,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content", "updated_at", "created_at", "parent_id", "content_type", "properties") VALUES ('block:ace69db3-9820-4f88-bb57-c95bc6b03b93', 'Entity Identity', 1779016366169, 1779016366157, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', '{"ID":"ace69db3-9820-4f88-bb57-c95bc6b03b93","sequence":77,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "parent_id", "created_at", "id", "content_type", "updated_at", "properties") VALUES ('MVP Definition', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366157, 'block:ae15f87d-4da4-42e7-a5c9-c06ab06a3410', 'text', 1779016366169, '{"ID":"ae15f87d-4da4-42e7-a5c9-c06ab06a3410","sequence":78,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "content_type", "created_at", "id", "content", "updated_at", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366159, 'block:c02ae544-1fbc-4ab5-8117-c75f5520b615', 'Frontends', 1779016366170, '{"ID":"c02ae544-1fbc-4ab5-8117-c75f5520b615","sequence":89}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "parent_id", "id", "updated_at", "content", "content_type", "properties") VALUES (1779016366159, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:c8fbf859-e8db-4cbe-a900-b90f4e24df9c', 1779016366170, 'Plain-Text Layer', 'text', '{"ID":"c8fbf859-e8db-4cbe-a900-b90f4e24df9c","sequence":90,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "updated_at", "content_type", "created_at", "content", "id", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366170, 'text', 1779016366159, 'Dogfooding & Agents', 'block:c9448d8d-5c8f-4b5d-808c-c7ba1690b6bf', '{"ID":"c9448d8d-5c8f-4b5d-808c-c7ba1690b6bf","sequence":91,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content_type", "content", "updated_at", "parent_id", "created_at", "properties") VALUES ('block:cc0045c0-2c2d-4044-8f49-2d59f648050d', 'text', 'Dogfooding & Agents', 1779016366170, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366159, '{"ID":"cc0045c0-2c2d-4044-8f49-2d59f648050d","sequence":92,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "created_at", "content", "parent_id", "id", "updated_at", "properties") VALUES ('text', 1779016366159, 'Hypotheses', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:d56bc1c5-9769-4e75-9e49-dcb78e29b1f6', 1779016366170, '{"ID":"d56bc1c5-9769-4e75-9e49-dcb78e29b1f6","sequence":94,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "content", "parent_id", "created_at", "id", "updated_at", "properties") VALUES ('text', 'Entity Identity', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366159, 'block:db06ac66-eebd-41bb-80cb-1f722216534c', 1779016366170, '{"ID":"db06ac66-eebd-41bb-80cb-1f722216534c","sequence":95,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "content_type", "created_at", "updated_at", "id", "parent_id", "properties") VALUES ('_archive', 'text', 1779016366159, 1779016366170, 'block:df8ed5f8-e5a5-49d6-9ee1-5aec6939e73c', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"df8ed5f8-e5a5-49d6-9ee1-5aec6939e73c","sequence":96}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "parent_id", "id", "created_at", "content_type", "updated_at", "properties") VALUES ('Hypotheses', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:e035f91d-52af-480f-92f7-a2d40b65497a', 1779016366160, 'text', 1779016366170, '{"ID":"e035f91d-52af-480f-92f7-a2d40b65497a","sequence":97,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content_type", "content", "updated_at", "parent_id", "created_at", "properties") VALUES ('block:e3f3e97e-cda9-40f0-a916-2fc4b393603a', 'text', 'Test Quality & Performance', 1779016366170, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366160, '{"ID":"e3f3e97e-cda9-40f0-a916-2fc4b393603a","sequence":98,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "content", "id", "parent_id", "created_at", "updated_at", "properties") VALUES ('text', 'LogSeq replacement', 'block:e43634ff-1903-43aa-b68a-e037d61e50e2', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366160, 1779016366170, '{"ID":"e43634ff-1903-43aa-b68a-e037d61e50e2","sequence":99}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "parent_id", "content", "updated_at", "id", "created_at", "properties") VALUES ('text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '_archive', 1779016366170, 'block:e77aad4f-0892-478c-87f1-6b113713d8a8', 1779016366160, '{"ID":"e77aad4f-0892-478c-87f1-6b113713d8a8","sequence":100}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "content" = excluded."content", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "created_at", "content", "parent_id", "content_type", "updated_at", "properties") VALUES ('block:e78de1a6-3381-4d00-9537-cf27c7caf256', 1779016366160, 'MVP Definition', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366170, '{"ID":"e78de1a6-3381-4d00-9537-cf27c7caf256","sequence":101,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "id", "content", "created_at", "updated_at", "content_type", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', 'Frontends', 1779016366160, 1779016366170, 'text', '{"ID":"e8d97c1d-efa3-44ca-93af-322b2a9a5087","sequence":102}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "created_at", "updated_at", "id", "content_type", "parent_id", "properties") VALUES ('GPUI', 1779016366160, 1779016366170, 'block:29934291-22da-4eb4-bdcb-9c7ee5fe3ea3', 'text', 'block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', '{"ID":"29934291-22da-4eb4-bdcb-9c7ee5fe3ea3","sequence":103,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "created_at", "updated_at", "content", "content_type", "parent_id", "properties") VALUES ('block:4afaefac-86a0-4348-8a0e-39f465937116', 1779016366160, 1779016366170, 'TUI', 'text', 'block:e8d97c1d-efa3-44ca-93af-322b2a9a5087', '{"ID":"4afaefac-86a0-4348-8a0e-39f465937116","sequence":104,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "parent_id", "content_type", "content", "updated_at", "created_at", "properties") VALUES ('block:f04bc668-a50f-417f-bda8-0a43bd90c4a2', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'Plain-Text Layer', 1779016366170, 1779016366161, '{"ID":"f04bc668-a50f-417f-bda8-0a43bd90c4a2","sequence":105,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content", "created_at", "parent_id", "content_type", "updated_at", "properties") VALUES ('block:f2c2ebdc-c94f-41d7-b706-14cb04d743cc', 'Hypotheses', 1779016366161, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366170, '{"ID":"f2c2ebdc-c94f-41d7-b706-14cb04d743cc","sequence":106,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "created_at", "parent_id", "content_type", "id", "content", "properties") VALUES (1779016366170, 1779016366161, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 'block:f35c4a0c-290a-431a-8ec5-96ba313ee736', 'Now', '{"ID":"f35c4a0c-290a-431a-8ec5-96ba313ee736","sequence":107,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "id", "updated_at", "created_at", "content", "content_type", "properties") VALUES ('block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:f40e02f8-7ce2-4357-9baa-6fea40df1d10', 1779016366170, 1779016366161, 'Entity Identity', 'text', '{"ID":"f40e02f8-7ce2-4357-9baa-6fea40df1d10","sequence":108,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content" = excluded."content", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "content_type", "updated_at", "parent_id", "created_at", "id", "properties") VALUES ('_archive', 'text', 1779016366170, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366161, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', '{"ID":"f465af55-f5e9-46cc-ba18-1f2905e274b7","sequence":109}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "content_type", "id", "content", "parent_id", "created_at", "properties") VALUES (1779016366170, 'text', 'block:2bf6a036-bb66-4c78-8e06-6dc5fe5f8278', 'Phase 6: Flow Optimization', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 1779016366161, '{"ID":"2bf6a036-bb66-4c78-8e06-6dc5fe5f8278","sequence":110}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "id", "created_at", "content_type", "content", "updated_at", "properties") VALUES ('block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'block:36cd0f0c-cf0c-4668-94bd-bf03ea79c55c', 1779016366161, 'text', 'Research Competition', 1779016366170, '{"ID":"36cd0f0c-cf0c-4668-94bd-bf03ea79c55c","sequence":111}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "content", "created_at", "updated_at", "id", "content_type", "properties") VALUES ('block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'Phase 5: AI Features', 1779016366161, 1779016366170, 'block:5c56dfba-65fb-4bc7-9d82-499563e3ddc3', 'text', '{"ID":"5c56dfba-65fb-4bc7-9d82-499563e3ddc3","sequence":112}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "content", "content_type", "id", "created_at", "updated_at", "properties") VALUES ('block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'Architecture Alternatives', 'text', 'block:5e6b85cd-e687-4f30-a1d4-a244170e5605', 1779016366161, 1779016366170, '{"ID":"5e6b85cd-e687-4f30-a1d4-a244170e5605","sequence":113}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "content", "updated_at", "id", "parent_id", "created_at", "properties") VALUES ('text', 'Query-Triggered Actions (Reactive Automation)', 1779016366170, 'block:5f920be0-0b4b-4eff-b579-f699117b0173', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 1779016366162, '{"ID":"5f920be0-0b4b-4eff-b579-f699117b0173","sequence":114}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "content_type", "created_at", "content", "parent_id", "properties") VALUES (1779016366170, 'block:61d662d4-97fd-46ba-b0cb-375af194564d', 'text', 1779016366162, 'Phase 7: Team Features', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', '{"ID":"61d662d4-97fd-46ba-b0cb-375af194564d","sequence":115}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "parent_id", "updated_at", "created_at", "content", "id", "properties") VALUES ('text', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 1779016366170, 1779016366162, 'Phase 1: Core Outliner', 'block:7eb8f9ca-bbed-4dd6-9e56-1dd54eb0d7c5', '{"ID":"7eb8f9ca-bbed-4dd6-9e56-1dd54eb0d7c5","sequence":116}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "created_at", "parent_id", "content", "updated_at", "id", "properties") VALUES ('text', 1779016366162, 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'Cross-Cutting Concerns', 1779016366170, 'block:e09ca9f1-d582-4bd5-80f6-f7ec8ee8e5b9', '{"ID":"e09ca9f1-d582-4bd5-80f6-f7ec8ee8e5b9","sequence":117}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "created_at", "content", "id", "content_type", "updated_at", "properties") VALUES ('block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 1779016366162, 'Phase 2: First Integration (Todoist)', 'block:e6de5a25-44c3-45fa-9b54-6de6063d1ada', 'text', 1779016366170, '{"ID":"e6de5a25-44c3-45fa-9b54-6de6063d1ada","sequence":118}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content" = excluded."content", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "parent_id", "updated_at", "content_type", "content", "created_at", "properties") VALUES ('block:f3f2e112-63f5-40c4-88a7-318c4671e6b8', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 1779016366170, 'text', 'Phase 3: Multiple Integrations', 1779016366162, '{"ID":"f3f2e112-63f5-40c4-88a7-318c4671e6b8","sequence":119}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "content" = excluded."content", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "content", "parent_id", "id", "content_type", "created_at", "properties") VALUES (1779016366170, 'Phase 4: AI Foundation', 'block:f465af55-f5e9-46cc-ba18-1f2905e274b7', 'block:f7356a51-16a3-4fc6-b9d7-9e7fcd5b15fe', 'text', 1779016366162, '{"ID":"f7356a51-16a3-4fc6-b9d7-9e7fcd5b15fe","sequence":120}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "content_type", "created_at", "id", "parent_id", "updated_at", "properties") VALUES ('Test Quality & Performance', 'text', 1779016366162, 'block:f7c74625-a7d3-43b2-8aa5-799e2604ebac', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366170, '{"ID":"f7c74625-a7d3-43b2-8aa5-799e2604ebac","sequence":121,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content_type", "updated_at", "created_at", "content", "parent_id", "properties") VALUES ('block:fa16e713-3722-42a2-9445-fb2e7929a4a9', 'text', 1779016366170, 1779016366162, 'Frontends', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', '{"ID":"fa16e713-3722-42a2-9445-fb2e7929a4a9","sequence":122}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "updated_at", "content_type", "content", "parent_id", "id", "properties") VALUES (1779016366163, 1779016366170, 'text', 'Hypotheses', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'block:fd2b053b-295d-4e94-a87c-817481b2e646', '{"ID":"fd2b053b-295d-4e94-a87c-817481b2e646","sequence":123,"todo_keywords":"[{\"keyword\":\"HYPO\",\"category\":\"Active\"},{\"keyword\":\"TESTING(t)\",\"category\":\"Active\"},{\"keyword\":\"VALIDATED(v)\",\"category\":\"Done\"},{\"keyword\":\"FALSIFIED(f)\",\"category\":\"Done\"},{\"keyword\":\"DEFERRED(d)\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "parent_id", "created_at", "updated_at", "id", "content_type", "properties") VALUES ('README', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 1779016366163, 1779016366170, 'block:fdcbdda5-f75e-4c37-bd42-e7bf9cefeacf', 'text', '{"ID":"fdcbdda5-f75e-4c37-bd42-e7bf9cefeacf","sequence":124}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "content", "created_at", "parent_id", "content_type", "updated_at", "properties") VALUES ('block:fe607af4-9a59-4037-8739-41922bdb9674', 'MVP Definition', 1779016366163, 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'text', 1779016366170, '{"ID":"fe607af4-9a59-4037-8739-41922bdb9674","sequence":125,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "updated_at", "content_type", "parent_id", "content", "created_at", "properties") VALUES ('block:febe1f73-4d58-4e98-92b2-6ab448fc79ef', 1779016366170, 'text', 'block:1ea23eea-929b-4eb1-9fdb-785fa0090a66', 'Multi-Frontend Strategy', 1779016366163, '{"ID":"febe1f73-4d58-4e98-92b2-6ab448fc79ef","sequence":126,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "id", "parent_id", "created_at", "content", "updated_at", "properties") VALUES ('text', 'block:32a48c60-e32a-4fa1-a30e-fccfd1f84350', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', 1779016366163, 'Holon', 1779016366170, '{"ID":"32a48c60-e32a-4fa1-a30e-fccfd1f84350","sequence":127}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content" = excluded."content", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "content_type", "created_at", "id", "parent_id", "updated_at", "properties") VALUES ('Holon', 'text', 1779016366163, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', 1779016366170, '{"ID":"8bc61d20-9e48-481f-9c56-835177e61a1b","sequence":128}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "id", "parent_id", "updated_at", "content", "content_type", "properties") VALUES (1779016366163, 'block:0751b1c4-d580-4c84-945c-1ad1fb877b3a', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 1779016366170, 'Engine Foundations', 'text', '{"ID":"0751b1c4-d580-4c84-945c-1ad1fb877b3a","sequence":129,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "updated_at", "content", "id", "created_at", "parent_id", "properties") VALUES ('text', 1779016366170, 'Test Quality & Performance', 'block:260ca1a2-694f-45b7-a1ba-df9b27346e8b', 1779016366163, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', '{"ID":"260ca1a2-694f-45b7-a1ba-df9b27346e8b","sequence":130,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "content" = excluded."content", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "updated_at", "parent_id", "content_type", "created_at", "content", "properties") VALUES ('block:69799677-ed42-4f5a-8f99-c138c7511718', 1779016366170, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'text', 1779016366165, 'Research Competition', '{"ID":"69799677-ed42-4f5a-8f99-c138c7511718","sequence":142}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "content_type", "created_at", "content", "parent_id", "properties") VALUES (1779016366170, 'block:a283085e-3386-40a4-ab8d-8578b295c6be', 'text', 1779016366165, 'Phase 3: Multiple Integrations', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', '{"ID":"a283085e-3386-40a4-ab8d-8578b295c6be","sequence":143}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "content", "created_at", "content_type", "parent_id", "properties") VALUES (1779016366170, 'block:b5e6ecb3-0d72-4464-9de9-31788e0003a6', 'Phase 6: Flow Optimization', 1779016366165, 'text', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', '{"ID":"b5e6ecb3-0d72-4464-9de9-31788e0003a6","sequence":144}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "parent_id", "content_type", "updated_at", "id", "created_at", "properties") VALUES ('Phase 1: Core Outliner', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'text', 1779016366170, 'block:c7aa2ca8-31e4-40a1-92cc-ce4f83cf5b43', 1779016366165, '{"ID":"c7aa2ca8-31e4-40a1-92cc-ce4f83cf5b43","sequence":145}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "content", "created_at", "parent_id", "updated_at", "id", "properties") VALUES ('text', 'Phase 5: AI Features', 1779016366165, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 1779016366170, 'block:d14b4916-17b5-4646-b8c2-81278bb9ca9d', '{"ID":"d14b4916-17b5-4646-b8c2-81278bb9ca9d","sequence":146}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "content" = excluded."content", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "id", "updated_at", "parent_id", "content_type", "content", "properties") VALUES (1779016366165, 'block:f644dde8-ab67-4b41-b682-b151cff9b368', 1779016366170, 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'text', 'Cross-Cutting Concerns', '{"ID":"f644dde8-ab67-4b41-b682-b151cff9b368","sequence":147}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content", "parent_id", "id", "content_type", "updated_at", "created_at", "properties") VALUES ('Phase 7: Team Features', 'block:5e821ae2-2d94-4401-a55d-f4ab1da6aedd', 'block:f9ce6214-52a5-4f70-a8a8-4c179e5e0665', 'text', 1779016366170, 1779016366165, '{"ID":"f9ce6214-52a5-4f70-a8a8-4c179e5e0665","sequence":148}') ON CONFLICT(id) DO UPDATE SET "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "id", "content_type", "content", "updated_at", "parent_id", "properties") VALUES (1779016366166, 'block:6e34b64e-170a-4cea-9917-2bd02e07b6b6', 'text', 'Entity Identity', 1779016366170, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', '{"ID":"6e34b64e-170a-4cea-9917-2bd02e07b6b6","sequence":149,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "content_type", "parent_id", "content", "created_at", "id", "properties") VALUES (1779016366170, 'text', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'Dogfooding & Agents', 1779016366166, 'block:a4c10cc0-108c-4af0-90b8-106dfe7703ce', '{"ID":"a4c10cc0-108c-4af0-90b8-106dfe7703ce","sequence":150,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "content", "id", "parent_id", "created_at", "content_type", "properties") VALUES (1779016366170, 'Frontends', 'block:b475739b-a4c7-4779-8730-9cc8f5aaf083', 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 1779016366166, 'text', '{"ID":"b475739b-a4c7-4779-8730-9cc8f5aaf083","sequence":151}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("created_at", "updated_at", "id", "content", "parent_id", "content_type", "properties") VALUES (1779016366166, 1779016366170, 'block:1b9a4fcd-859f-449d-86c4-b2c2612790b7', 'GPUI', 'block:b475739b-a4c7-4779-8730-9cc8f5aaf083', 'text', '{"ID":"1b9a4fcd-859f-449d-86c4-b2c2612790b7","sequence":152,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "created_at", "parent_id", "content", "updated_at", "id", "properties") VALUES ('text', 1779016366166, 'block:b475739b-a4c7-4779-8730-9cc8f5aaf083', 'TUI', 1779016366170, 'block:f41426cd-46c3-463a-b245-2675b5a6ff86', '{"ID":"f41426cd-46c3-463a-b245-2675b5a6ff86","sequence":153,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "created_at" = excluded."created_at", "parent_id" = excluded."parent_id", "content" = excluded."content", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "updated_at", "content_type", "id", "content", "created_at", "properties") VALUES ('block:8bc61d20-9e48-481f-9c56-835177e61a1b', 1779016366170, 'text', 'block:d83294d3-3e9f-4911-96d0-616f85619668', 'README', 1779016366166, '{"ID":"d83294d3-3e9f-4911-96d0-616f85619668","sequence":154}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "content" = excluded."content", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "id", "created_at", "updated_at", "content_type", "content", "properties") VALUES ('block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'block:eceecd11-e83c-40c1-b2eb-68cca313b256', 1779016366166, 1779016366170, 'text', 'LogSeq replacement', '{"ID":"eceecd11-e83c-40c1-b2eb-68cca313b256","sequence":155}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content_type" = excluded."content_type", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("id", "updated_at", "parent_id", "content_type", "created_at", "content", "properties") VALUES ('block:fd9b52e4-35bb-49d3-8b7e-6bb392f0e8dc', 1779016366170, 'block:8bc61d20-9e48-481f-9c56-835177e61a1b', 'text', 1779016366166, 'Multi-Frontend Strategy', '{"ID":"fd9b52e4-35bb-49d3-8b7e-6bb392f0e8dc","sequence":156,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "parent_id" = excluded."parent_id", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "content" = excluded."content", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("content_type", "created_at", "updated_at", "id", "content", "parent_id", "properties") VALUES ('text', 1779016366166, 1779016366170, 'block:b42f8024-da78-4874-8526-22f8913effcf', 'Holon', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', '{"ID":"b42f8024-da78-4874-8526-22f8913effcf","sequence":157}') ON CONFLICT(id) DO UPDATE SET "content_type" = excluded."content_type", "created_at" = excluded."created_at", "updated_at" = excluded."updated_at", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "updated_at", "content", "id", "content_type", "created_at", "properties") VALUES ('block:db147710-ef57-40f3-bb67-b3674bbc874a', 1779016366170, 'Holon', 'block:bf537acb-ff72-4c91-a3da-c2d524c4a830', 'text', 1779016366167, '{"ID":"bf537acb-ff72-4c91-a3da-c2d524c4a830","sequence":158}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "updated_at" = excluded."updated_at", "content" = excluded."content", "content_type" = excluded."content_type", "created_at" = excluded."created_at", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("updated_at", "id", "created_at", "content_type", "content", "parent_id", "properties") VALUES (1779016366170, 'block:c0e0b480-2b02-4bfe-8d95-086268704d6c', 1779016366167, 'text', 'Holon', 'block:db147710-ef57-40f3-bb67-b3674bbc874a', '{"ID":"c0e0b480-2b02-4bfe-8d95-086268704d6c","sequence":159}') ON CONFLICT(id) DO UPDATE SET "updated_at" = excluded."updated_at", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "content" = excluded."content", "parent_id" = excluded."parent_id", "properties" = excluded."properties";

-- [transaction_stmt]
INSERT INTO block_raw ("parent_id", "created_at", "id", "content_type", "content", "updated_at", "properties") VALUES ('block:db147710-ef57-40f3-bb67-b3674bbc874a', 1779016366167, 'block:e2a5b165-8245-49b1-955b-851dd73184fb', 'text', 'Holon', 1779016366170, '{"ID":"e2a5b165-8245-49b1-955b-851dd73184fb","sequence":160}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "content" = excluded."content", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [actor_exec]
INSERT INTO block_raw ("parent_id", "content", "created_at", "content_type", "updated_at", "id", "properties") VALUES ('sentinel:no_parent', 'GPUI', 1779016366671, 'text', 1779016366671, 'block:d09025cc-3748-404e-ad4d-432fcdc194d5', '{"ID":"d09025cc-3748-404e-ad4d-432fcdc194d5","sequence":0}') ON CONFLICT(id) DO UPDATE SET "parent_id" = excluded."parent_id", "content" = excluded."content", "created_at" = excluded."created_at", "content_type" = excluded."content_type", "updated_at" = excluded."updated_at", "properties" = excluded."properties";

-- [actor_exec]
INSERT INTO block_tags ("block_id", "tag") VALUES ('block:d09025cc-3748-404e-ad4d-432fcdc194d5', 'Page');

-- [actor_exec]
UPDATE block_raw SET "created_at" = 1779016366671, "updated_at" = 1779016366710, "properties" = '{"ID":"d09025cc-3748-404e-ad4d-432fcdc194d5","sequence":0,"todo_keywords":"[{\"keyword\":\"TODO\",\"category\":\"Active\"},{\"keyword\":\"DOING\",\"category\":\"Active\"},{\"keyword\":\"BLOCKED\",\"category\":\"Active\"},{\"keyword\":\"DONE\",\"category\":\"Done\"},{\"keyword\":\"WONT\",\"category\":\"Done\"}]"}' WHERE id = 'block:d09025cc-3748-404e-ad4d-432fcdc194d5';

-- [actor_exec]
DELETE FROM block_tags WHERE "block_id" = 'block:d09025cc-3748-404e-ad4d-432fcdc194d5';

-- Wait 18ms

