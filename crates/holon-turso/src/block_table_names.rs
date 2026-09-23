//! Centralized SQL identifiers for the block table/matview pair.
//!
//! Reads target the `block` matview (hydrates tags + requires from
//! the junction tables); writes target the underlying `block_raw`
//! table. The matview DDL is synthesized by `BlockMatviewSchemaModule`
//! (`schema_modules.rs`) from the `EdgeFieldDescriptor` registry.
//!
//! NOT for use in: static `.sql` files (matview DDLs, FK clauses).
//! Those are referenced via `include_str!` and don't substitute the
//! const — they reference the literal table/matview names directly.

/// Underlying table for INSERT / UPDATE / DELETE of block rows.
pub const BLOCK_WRITE_TABLE: &str = "block_raw";

/// Matview to read hydrated block rows from (includes `tags` and
/// `requires` JSON arrays projected from the junction tables).
pub const BLOCK_READ_TABLE: &str = "block";

/// The `block_raw` projection every hydrated block read selects: native
/// columns aliased `b.*`, plus one `json_group_array` subquery per edge field.
/// `Block::try_from` REQUIRES every edge column, so a read that omits one fails
/// the whole row rather than yielding a block with a silently empty edge set.
pub const HYDRATED_BLOCK_COLUMNS: &str = "b.id, b.parent_id, b.sort_key, b.content, b.content_type, \
     b.source_language, b.source_name, b.properties, b.property_kinds, b.marks, b.collapsed, \
     b.widget_only, \
     b.completed, b.block_type, b.created_at, b.updated_at, \
     COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS \
     tags, \
     COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = b.id), \
     '[]') AS requires, \
     COALESCE((SELECT json_group_array(lesson_id) FROM advice_suppressed WHERE anchor_id = \
     b.id), '[]') AS advice_suppressed, \
     COALESCE((SELECT json_group_array(target_id) FROM block_contributes_to WHERE block_id = \
     b.id), '[]') AS contributes_to";
