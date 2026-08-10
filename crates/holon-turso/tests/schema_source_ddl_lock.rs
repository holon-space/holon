//! The single-source lock: the declared schema (`holon_api::schema`, the leaf
//! every field vocabulary derives from) against the DDL that actually creates
//! the tables.
//!
//! The DDL stays authoritative for SQL types, defaults and constraints — this
//! test only pins the FIELD NAMES, in BOTH directions. One direction alone is
//! half a lock: a column added to the DDL and forgotten in the declaration is
//! invisible to arcs, guards and the intent boundary; a field declared without
//! its column produces SQL that fails at query time with a matview parse error.
//!
//! This replaces the pairwise drift locks between the four former hand-written
//! vocabularies. Those lists are gone; each now derives from the declaration,
//! so their agreement is by construction and only this boundary needs a test.

use holon_api::schema::BLOCK;
use holon_api::schema::CLOCK;
use holon_api::schema::EntitySchema;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

const BLOCK_DDL: &str = include_str!("../sql/schema/blocks.sql");
const CLOCK_DDL: &str = include_str!("../sql/schema/clock.sql");

/// Column names of `CREATE TABLE <table>` in `ddl`, in declaration order.
///
/// Deliberately strict: an unparseable body panics rather than yielding a short
/// list that would make the lock vacuously pass.
fn ddl_columns(ddl: &str, table: &str) -> Vec<String> {
    let head = format!("CREATE TABLE IF NOT EXISTS {table} (");
    let start = ddl
        .find(&head)
        .unwrap_or_else(|| panic!("no `{head}` in the DDL"))
        + head.len();

    let mut depth = 0usize;
    let mut end = None;
    for (offset, ch) in ddl[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = Some(start + offset);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let body = &ddl[start..end.unwrap_or_else(|| panic!("unterminated CREATE TABLE for {table}"))];

    let mut entries = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("--") {
            continue;
        }
        for ch in line.chars() {
            match ch {
                '(' => {
                    depth += 1;
                    current.push(ch);
                }
                ')' => {
                    depth -= 1;
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    entries.push(std::mem::take(&mut current));
                }
                _ => current.push(ch),
            }
        }
        current.push(' ');
    }
    entries.push(current);

    entries
        .iter()
        .filter_map(|entry| {
            let name = entry.split_whitespace().next()?;
            let is_constraint = matches!(
                name.to_ascii_uppercase().as_str(),
                "FOREIGN" | "PRIMARY" | "UNIQUE" | "CHECK" | "CONSTRAINT"
            );
            (!is_constraint).then(|| name.to_string())
        })
        .collect()
}

fn assert_columns_match(schema: &EntitySchema, ddl: &str, table: &str) {
    let declared = schema.columns();
    let in_ddl = ddl_columns(ddl, table);

    assert!(
        !in_ddl.is_empty(),
        "parsed no columns out of {table}'s DDL — the lock would be vacuous"
    );
    for column in &in_ddl {
        assert!(
            declared.contains(&column.as_str()),
            "{table} has column `{column}` that `schema::{}` does not declare — it is invisible \
             to arcs, guards and the intent boundary. Declare it (arc_place: false is fine for \
             bookkeeping).",
            schema.relation.to_uppercase()
        );
    }
    for field in &declared {
        assert!(
            in_ddl.contains(&field.to_string()),
            "`schema::{}` declares `{field}` as a Column but {table} has no such column — SQL \
             built from the declaration would fail at query time",
            schema.relation.to_uppercase()
        );
    }
}

#[test]
fn the_block_declaration_and_the_block_raw_ddl_name_the_same_columns() {
    assert_columns_match(&BLOCK, BLOCK_DDL, "block_raw");
}

#[test]
fn the_clock_declaration_and_the_clock_ddl_name_the_same_columns() {
    assert_columns_match(&CLOCK, CLOCK_DDL, "clock");
}

/// The junction half of the same boundary: an edge field declared without its
/// `EdgeFieldDescriptor` produces a matview that selects a column no join
/// supplies.
#[test]
fn the_block_declaration_and_the_edge_field_registry_name_the_same_edge_sets() {
    let declared = BLOCK.edge_sets();
    let registered: Vec<String> = BlockSchemaModule
        .edge_fields()
        .into_iter()
        .filter(|d| d.entity == BLOCK.relation)
        .map(|d| d.field)
        .collect();

    assert!(!registered.is_empty(), "the lock would be vacuous");
    for field in &registered {
        assert!(
            declared.contains(&field.as_str()),
            "edge field `{field}` is registered but `schema::BLOCK` does not declare it"
        );
    }
    for field in &declared {
        assert!(
            registered.contains(&field.to_string()),
            "`schema::BLOCK` declares edge set `{field}` but no EdgeFieldDescriptor backs it"
        );
    }
}
