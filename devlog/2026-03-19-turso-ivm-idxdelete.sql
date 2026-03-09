-- Minimal reproducer for Turso IVM IdxDelete bug
-- Bug: "IdxDelete: no matching index entry found" when DELETE FROM table with matview
-- after batch INSERT ON CONFLICT DO UPDATE
--
-- To run: cargo run --bin turso-sql-replay -- replay devlog/2026-03-19-turso-ivm-idxdelete.sql --check-after-each

-- !SET_CHANGE_CALLBACK 2026-03-19T08:41:35.508546Z

-- [actor_ddl] 2026-03-19T08:41:35.631870Z
CREATE TABLE IF NOT EXISTS cc_task (
  id TEXT PRIMARY KEY NOT NULL,
  session_id TEXT,
  local_id TEXT,
  subject TEXT,
  description TEXT,
  status TEXT,
  created_at TEXT,
  completed_at TEXT,
  _change_origin TEXT
);

-- [actor_ddl] 2026-03-19T08:41:45.607178Z
CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_461f6dd248aa8888 AS SELECT id, subject, status, created_at FROM cc_task WHERE status = 'in_progress';

-- Batch INSERT ON CONFLICT DO UPDATE (380 rows in production, minimized sample below)
-- Key: some rows have status='in_progress' (matches matview), some have status='completed'
-- [transaction_stmt] 2026-03-19T08:41:48.289050Z
INSERT INTO cc_task (id, session_id, local_id, subject, description, status, created_at, completed_at, _change_origin) VALUES ('cc_task:198ed031-9c76-4c83-a05d-0275babc1c3e:1', '198ed031-9c76-4c83-a05d-0275babc1c3e', '1', 'Design GQL execution integration for Blinc frontend', 'Plan how GQL source blocks compile → SQL → render_entity()', 'completed', '2026-02-17T01:32:02.361Z', '2026-02-17T01:32:12.393Z', '{"Local":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, session_id = excluded.session_id, local_id = excluded.local_id, subject = excluded.subject, description = excluded.description, status = excluded.status, created_at = excluded.created_at, completed_at = excluded.completed_at, _change_origin = excluded._change_origin;

-- [transaction_stmt] 2026-03-19T08:41:48.289200Z
INSERT INTO cc_task (id, session_id, local_id, subject, description, status, created_at, completed_at, _change_origin) VALUES ('cc_task:198ed031-9c76-4c83-a05d-0275babc1c3e:2', '198ed031-9c76-4c83-a05d-0275babc1c3e', '2', 'Add compile_gql to BackendEngine', 'Implement gql_parser + gql_transform pipeline', 'completed', '2026-02-17T01:32:07.015Z', '2026-02-17T01:33:42.063Z', '{"Local":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, session_id = excluded.session_id, local_id = excluded.local_id, subject = excluded.subject, description = excluded.description, status = excluded.status, created_at = excluded.created_at, completed_at = excluded.completed_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO cc_task (id, session_id, local_id, subject, description, status, created_at, completed_at, _change_origin) VALUES ('cc_task:91c31f01-d0ee-4a11-8a1c-e22ea9ebcc71:1', '91c31f01-d0ee-4a11-8a1c-e22ea9ebcc71', '1', 'Debug empty Flutter UI', 'Flutter UI shows no data', 'in_progress', '2026-02-19T01:07:26.747Z', NULL, '{"Local":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, session_id = excluded.session_id, local_id = excluded.local_id, subject = excluded.subject, description = excluded.description, status = excluded.status, created_at = excluded.created_at, completed_at = excluded.completed_at, _change_origin = excluded._change_origin;

-- [transaction_stmt]
INSERT INTO cc_task (id, session_id, local_id, subject, description, status, created_at, completed_at, _change_origin) VALUES ('cc_task:852bca2f-6ef4-4ade-a959-eced70ad93ad:1', '852bca2f-6ef4-4ade-a959-eced70ad93ad', '1', 'Unify roundtrip PBT test files', 'Merge tests', 'in_progress', '2026-02-19T02:08:29.067Z', NULL, '{"Local":{"operation_id":null,"trace_id":null}}') ON CONFLICT(id) DO UPDATE SET id = excluded.id, session_id = excluded.session_id, local_id = excluded.local_id, subject = excluded.subject, description = excluded.description, status = excluded.status, created_at = excluded.created_at, completed_at = excluded.completed_at, _change_origin = excluded._change_origin;

-- Trigger: clear table (resync) - this is where the error occurs in production
-- Error: IdxDelete: no matching index entry found for key
--   [Value(Text("cc_task:198ed031-9c76-4c83-a05d-0275babc1c3e:1")), Value(Numeric(Integer(6)))]
-- [execute_sql] 2026-03-19T08:42:22.378426Z
DELETE FROM cc_task;
