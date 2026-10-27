use super::*;

#[test]
fn composite_primary_key_emits_table_level_clause() {
    let td = TypeDefinition {
        name: "gh_issue".to_string(),
        graph_label: None,
        primary_key: FieldIdent::parse("owner").unwrap(),
        fields: vec![
            FieldSchema::new("owner", "TEXT").primary_key(),
            FieldSchema::new("repo", "TEXT").primary_key(),
            FieldSchema::new("number", "INTEGER").primary_key(),
            FieldSchema::new("title", "TEXT"),
        ],
        id_references: None,
        profile_variants: vec![],
        write_authority: WriteAuthority::Local,
        home: None,
        soft_delete: None,
        services: None,
        default_lifetime: FieldLifetime::default(),
        source: TypeSource::default(),
    };
    let sql = td.to_create_table_sql();
    // Inline PRIMARY KEY annotations would have made SQLite reject this DDL
    // with "table has more than one primary key" — the table-level clause
    // is the only legal form.
    assert!(
        !sql.contains("\"owner\" TEXT PRIMARY KEY"),
        "inline PK leaked: {sql}"
    );
    assert!(
        sql.contains("PRIMARY KEY (\"owner\", \"repo\", \"number\")"),
        "expected composite PK clause with quoted identifiers; got: {sql}"
    );
}

#[test]
fn single_primary_key_keeps_inline_form() {
    let td = TypeDefinition {
        name: "single".to_string(),
        graph_label: None,
        primary_key: default_primary_key(),
        fields: vec![
            FieldSchema::new("id", "TEXT").primary_key(),
            FieldSchema::new("name", "TEXT"),
        ],
        id_references: None,
        profile_variants: vec![],
        write_authority: WriteAuthority::Local,
        home: None,
        soft_delete: None,
        services: None,
        default_lifetime: FieldLifetime::default(),
        source: TypeSource::default(),
    };
    let sql = td.to_create_table_sql();
    assert!(sql.contains("\"id\" TEXT PRIMARY KEY"), "got: {sql}");
    assert!(!sql.contains("PRIMARY KEY (\"id\")"), "got: {sql}");
}
