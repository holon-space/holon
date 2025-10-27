// Should fire — bare "block" / "block_tags" / "task_blockers" literals.
fn use_table_name() {
    let t1 = "block";
    let t2 = "block_tags";
    let t3 = "task_blockers";
    let _ = (t1, t2, t3);
}

// Should NOT fire — substring inside a SQL statement.
fn embedded_in_sql() {
    let _q = "SELECT * FROM block WHERE id = ?";
}

// Should NOT fire — using a const reference. The const definition itself
// is the legitimate home of the literal and gets explicit allow.
#[allow(hardcoded_table_name)]
const BLOCK_WRITE_TABLE: &str = "block_raw";
fn via_const() {
    let _ = BLOCK_WRITE_TABLE;
}

fn main() {
    use_table_name();
    embedded_in_sql();
    via_const();
}
