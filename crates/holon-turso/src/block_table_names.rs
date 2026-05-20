//! Centralized SQL identifiers for the block table/matview pair.
//!
//! Reads target the `block` matview (hydrates tags + requires from
//! the junction tables); writes target the underlying `block_raw`
//! table. The matview DDL lives in `sql/schema/block_matview.sql` and
//! is owned by `BlockMatviewSchemaModule`.
//!
//! NOT for use in: static `.sql` files (matview DDLs, FK clauses).
//! Those are referenced via `include_str!` and don't substitute the
//! const — they reference the literal table/matview names directly.

/// Underlying table for INSERT / UPDATE / DELETE of block rows.
pub const BLOCK_WRITE_TABLE: &str = "block_raw";

/// Matview to read hydrated block rows from (includes `tags` and
/// `requires` JSON arrays projected from the junction tables).
pub const BLOCK_READ_TABLE: &str = "block";
