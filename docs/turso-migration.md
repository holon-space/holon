# Turso Migration Guide

## Overview

This document tracks the migration from plain SQLite (via sqlx) to Turso (libSQL) for the Rusty Knowledge project.

**Why Turso?**
- **CDC (Change Data Capture)**: Built-in change tracking essential for incremental sync between devices
- **Embedded + Remote Replica**: Local-first with optional multi-device sync via Turso cloud
- **SQLite-compatible**: Same SQL syntax, proven reliability
- **Edge deployment**: Can deploy close to users for low latency (future capability)
- **Modern async client**: libsql provides clean async API

## Migration Status

### ✅ Completed - Documentation

1. **ADR 0001** (`docs/adr/0001-hybrid-sync-architecture.md`)
   - Replaced all SQLite references with "Turso (libSQL embedded)"
   - Added comprehensive "Why Turso?" rationale section
   - Documented deployment modes: embedded, remote replica, remote-only
   - Updated code examples to use `libsql::Database` API
   - Changed connection strings: `sqlite://` → `file:` (libsql embedded mode)

2. **ADR 0002** (`docs/adr/0002-trait-based-unified-type-system.md`)
   - No changes needed (abstract level, no direct storage references)

3. **Architecture Docs**
   - `docs/architecture.md` - All SQLite → Turso replacements applied
   - `docs/architecture2.md` - All SQLite → Turso replacements applied

4. **VISION.md** - Updated caching layer description

5. **Cargo.toml**
   - Upgraded `libsql = { version = "0.9", features = ["core", "remote"] }` (0.9.24 in Cargo.lock)
   - Removed `sqlx` dependency

### ✅ Completed - Rust Code Migration

| File | Status | Notes |
|------|--------|-------|
| `src/core/traits.rs` | ✅ DONE | Converted `SqlPredicate::bind_all_sqlite` to `to_params()` returning `Vec<libsql::Value>` |
| `src/storage/turso.rs` | ✅ DONE | Renamed from `sqlite.rs`, fully migrated to `libsql::Builder` and `libsql::Database` API |
| `src/core/queryable_cache.rs` | ✅ DONE | Migrated from `SqlitePool` to `libsql::Database` with `Builder` pattern |
| `src/storage/mod.rs` | ✅ DONE | Updated module declaration from `sqlite` to `turso` |
| `src/tasks_sqlite.rs` | ✅ DONE | Updated to use `TursoBackend` |
| `tests/cucumber.rs` | ✅ DONE | Updated to use `TursoBackend` |
| `src/storage/sqlite_tests.rs` | ✅ DONE | Updated all references to `TursoBackend` |

### ⚠️ Known Issues

**In-Memory Database Tests Failing**
- **Issue**: 25 tests failing with "no such table" errors when using in-memory databases
- **Root Cause**: Tables created on one connection not visible on subsequent connections from the same Database instance
- **Investigation**:
  - Upgraded from libsql 0.6.0 to 0.9.24
  - Changed in-memory path from `:memory:` to `file::memory:?cache=shared` (recommended by libsql maintainers)
  - Issue persists even after these changes
- **Status**: Requires further investigation or potential bug report to libsql team
- **Test Results**: 102 passed, 25 failed (all failures are in-memory database related)

## API Differences: sqlx → libsql

### Connection & Setup

**sqlx (old)**:
```rust
use sqlx::sqlite::SqlitePool;

let pool = SqlitePool::connect("sqlite://db.db").await?;
```

**libsql (new - embedded mode)**:
```rust
use libsql::Database;

// Local-only (no sync)
let db = Database::open("file:db.db").await?;
let conn = db.connect()?;
```

**libsql (remote replica mode - multi-device sync)**:
```rust
use libsql::Builder;

// Local file + sync to Turso cloud
let db = Builder::new_remote_replica(
    "local.db",                              // Local file path
    "libsql://[org]-[db].turso.io",         // Remote Turso URL
    std::env::var("TURSO_AUTH_TOKEN")?       // Auth token
).build().await?;

let conn = db.connect()?;

// Sync with remote
db.sync().await?;
```

### Queries

**sqlx (old)**:
```rust
// With compile-time checking (sqlx feature)
let row = sqlx::query!("SELECT * FROM tasks WHERE id = ?", task_id)
    .fetch_one(&pool)
    .await?;

// Or runtime (similar to libsql)
let row = sqlx::query("SELECT * FROM tasks WHERE id = ?")
    .bind(task_id)
    .fetch_one(&pool)
    .await?;
```

**libsql (new - runtime only)**:
```rust
// Parameters as array (not builder pattern)
let mut rows = conn.query("SELECT * FROM tasks WHERE id = ?", [task_id]).await?;

// Iterate results
while let Some(row) = rows.next().await? {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    // ...
}

// Or get single row
let row = rows.next().await?.ok_or("No rows found")?;
```

### Transactions

**sqlx (old)**:
```rust
let mut tx = pool.begin().await?;
sqlx::query("INSERT INTO ...").execute(&mut tx).await?;
tx.commit().await?;
```

**libsql (new)**:
```rust
let tx = conn.transaction().await?;
tx.execute("INSERT INTO ...", ()).await?;
tx.commit().await?;
```

### Parameter Binding

**sqlx (old - builder pattern)**:
```rust
let query = sqlx::query("SELECT * FROM tasks WHERE id = ? AND completed = ?")
    .bind(&task_id)
    .bind(completed);
```

**libsql (new - array/tuple)**:
```rust
// Use positional parameters
let params = (&task_id, completed);
let rows = conn.query("SELECT * FROM tasks WHERE id = ? AND completed = ?", params).await?;

// Or inline
let rows = conn.query("SELECT * FROM tasks WHERE id = ? AND completed = ?", [&task_id, &completed]).await?;
```

## Migration Strategy

### Phase 1: Trait Abstraction (Current)
Update `src/core/traits.rs` to remove sqlx-specific types:

```rust
// Before
impl SqlPredicate {
    pub fn bind_all_sqlite<'q>(
        &'q self,
        mut query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
        // ...
    }
}

// After
impl SqlPredicate {
    pub fn to_params(&self) -> Vec<libsql::Value> {
        self.params.iter().map(|p| match p {
            Value::String(s) => libsql::Value::Text(s.clone()),
            Value::Integer(i) => libsql::Value::Integer(*i),
            Value::Float(f) => libsql::Value::Real(*f),
            Value::Boolean(b) => libsql::Value::Integer(if *b { 1 } else { 0 }),
            Value::Null => libsql::Value::Null,
            _ => libsql::Value::Null,
        }).collect()
    }
}
```

### Phase 2: Storage Backend (High Priority)
Rename and update `src/storage/sqlite.rs` → `src/storage/turso.rs`:

1. Replace `SqlitePool` with `libsql::Database`
2. Update all query calls from builder pattern to array params
3. Update row access methods
4. Test with embedded mode first (`file:` URLs)

### Phase 3: QueryableCache (Medium Priority)
Update `src/core/queryable_cache.rs`:

1. Replace pool references with database references
2. Update connection acquisition
3. Update query execution patterns

### Phase 4: Testing & Validation
1. Run existing tests with libsql backend
2. Verify CDC functionality works
3. Test remote replica mode (optional, for multi-device)
4. Performance benchmarks (should be similar or better)

### Phase 5: Cleanup
1. Remove all sqlx imports
2. Remove sqlx from Cargo.toml (already done)
3. Update any remaining documentation

## CDC (Change Data Capture) Integration

Once migration is complete, we can leverage Turso's CDC for sync:

```rust
// Future: Listen for changes via CDC
let changes = db.changes_since(last_sync_version).await?;

for change in changes {
    match change.operation {
        ChangeOp::Insert { table, values } => {
            // Handle new row
        }
        ChangeOp::Update { table, old_values, new_values } => {
            // Handle update
        }
        ChangeOp::Delete { table, old_values } => {
            // Handle deletion
        }
    }
}
```

This enables efficient incremental sync between devices without polling.

## Timeline & Effort Estimate

| Phase | Effort | Priority | Blocker |
|-------|--------|----------|---------|
| Phase 1: Trait Abstraction | 2-4 hours | High | None |
| Phase 2: Storage Backend | 8-12 hours | High | Phase 1 |
| Phase 3: QueryableCache | 4-6 hours | Medium | Phase 2 |
| Phase 4: Testing | 4-8 hours | High | Phase 2-3 |
| Phase 5: Cleanup | 1-2 hours | Low | All above |
| **Total** | **~20-30 hours** | | |

## Next Steps

1. ✅ Documentation updated (completed 2025-11-02)
2. ⏳ Create feature branch: `feat/turso-migration`
3. ⏳ Implement Phase 1: Trait abstraction
4. ⏳ Implement Phase 2: Storage backend
5. ⏳ Run tests and validate functionality
6. ⏳ Enable CDC for sync (future enhancement)

## References

- [Turso Rust SDK Quickstart](https://docs.turso.tech/sdk/rust/quickstart)
- [libsql crate documentation](https://docs.rs/libsql)
- [ADR 0001: Hybrid Sync Architecture](./adr/0001-hybrid-sync-architecture.md)
- [Turso CDC Documentation](https://docs.turso.tech/features/change-data-capture)

## Questions & Decisions

### Q: Why not keep sqlx for compatibility?
**A**: Turso's CDC and replication features require libsql. sqlx doesn't support these Turso-specific features. The migration enables multi-device sync and change tracking.

### Q: Can we use both sqlx and libsql temporarily?
**A**: Not recommended. Mixing two database libraries adds complexity and binary size. Better to migrate cleanly in one branch.

### Q: What about compile-time checked queries?
**A**: libsql doesn't support sqlx's compile-time query checking. We accept this trade-off for CDC and replication capabilities. Our type-safe lens system provides safety at a different layer.

### Q: Will this break existing data?
**A**: No. libSQL is SQLite-compatible. Existing `.db` files work as-is. Just change connection string from `sqlite://` to `file:`.

---

**Status**: Documentation complete, implementation in progress
**Last updated**: 2025-11-02
