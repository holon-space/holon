//! The ONE in-place writer of the `properties` bag.
//!
//! The bag and the `property_kinds` column beside it are a PAIR: a kind entry
//! naming a key the bag no longer stores at that kind is corruption, and the
//! read boundary refuses such a row outright. So the two columns are never
//! written apart — [`bag_and_kinds_set_clause`] emits both assignments of one
//! `UPDATE`, and no reader can observe a state where they disagree.
//!
//! `only_this_module_patches_the_bag` pins that this is the only such writer.

use holon_api::AmbiguousKind;
use holon_api::Value;

/// What one key's write does to the bag.
pub(crate) struct BagEntry<'a> {
    pub key: &'a str,
    /// The SQL literal to store, or `None` to REMOVE the key.
    pub value_sql: Option<String>,
    /// The kind to record. `None` clears any entry the key held — either it is
    /// gone, or its new value's JSON form already names its kind, and a
    /// surviving entry would re-type the new value into the old kind.
    pub kind: Option<AmbiguousKind>,
}

impl<'a> BagEntry<'a> {
    /// Store `value`, spelled as `value_sql`, recording whatever kind it needs.
    ///
    /// Two kinds need `json(…)` around the literal, because `json_set` reads a
    /// bare SQL literal as a SQL scalar and the full-blob legs — which
    /// serialize into the bag directly — never had the question:
    ///
    /// * a DOCUMENT (`Json`/`Object`/`Array`) is spelled as TEXT by
    ///   `value_to_sql_literal`, so `{"a":1}` would be stored as the string of
    ///   its JSON rather than as the object;
    /// * a `Boolean` is spelled `1`/`0` — SQLite has no boolean type — so it
    ///   would be stored as a NUMBER and read back `Integer(1)`.
    ///
    /// Both spellings are chosen to MATCH what the create leg stores, so the
    /// two routes cannot disagree about a declared type.
    pub fn set(key: &'a str, value_sql: String, value: &Value) -> Self {
        let value_sql = match value {
            Value::Boolean(b) => format!("json('{b}')"),
            Value::Json(_) | Value::Object(_) | Value::Array(_) => format!("json({value_sql})"),
            _ => value_sql,
        };
        Self {
            key,
            value_sql: Some(value_sql),
            kind: AmbiguousKind::of(value),
        }
    }

    /// Store a literal the caller derived rather than a `Value` — always at a
    /// JSON-evident kind, so it never needs an entry.
    pub fn set_derived(key: &'a str, value_sql: String) -> Self {
        Self {
            key,
            value_sql: Some(value_sql),
            kind: None,
        }
    }

    pub fn remove(key: &'a str) -> Self {
        Self {
            key,
            value_sql: None,
            kind: None,
        }
    }
}

fn json_path(key: &str) -> String {
    format!("'$.{}'", key.replace('\'', "''"))
}

/// The `SET` assignments writing `entries` into the bag and keeping
/// `property_kinds` in step, for one `UPDATE` statement.
///
/// An empty kind map is stored as NULL, not `{}`, so "no key carries a
/// non-evident kind" has ONE spelling on disk whichever leg wrote it.
pub(crate) fn bag_and_kinds_set_clause(entries: &[BagEntry<'_>]) -> String {
    let removed: Vec<&str> = entries
        .iter()
        .filter(|e| e.value_sql.is_none())
        .map(|e| e.key)
        .collect();
    let stored: Vec<&BagEntry<'_>> = entries.iter().filter(|e| e.value_sql.is_some()).collect();

    let mut bag = "COALESCE(properties, '{}')".to_string();
    if !removed.is_empty() {
        let paths: Vec<String> = removed.iter().map(|k| json_path(k)).collect();
        bag = format!("json_remove({bag}, {})", paths.join(", "));
    }
    if !stored.is_empty() {
        let pairs: Vec<String> = stored
            .iter()
            .map(|e| {
                format!(
                    "{}, {}",
                    json_path(e.key),
                    e.value_sql.as_deref().expect("filtered to stored")
                )
            })
            .collect();
        bag = format!("json_set({bag}, {})", pairs.join(", "));
    }

    // Every key the write NAMES has its entry cleared before the new kinds are
    // laid down, so a key rewritten at a JSON-evident kind cannot keep the
    // entry it held.
    let mut kinds = "COALESCE(property_kinds, '{}')".to_string();
    let cleared: Vec<String> = entries.iter().map(|e| json_path(e.key)).collect();
    kinds = format!("json_remove({kinds}, {})", cleared.join(", "));
    let recorded: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            e.kind
                .map(|k| format!("{}, '{}'", json_path(e.key), k.as_str()))
        })
        .collect();
    if !recorded.is_empty() {
        kinds = format!("json_set({kinds}, {})", recorded.join(", "));
    }

    format!("properties = {bag}, property_kinds = NULLIF({kinds}, '{{}}')")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_plain_value_clears_the_keys_kind_entry() {
        let clause = bag_and_kinds_set_clause(&[BagEntry::set(
            "Probe",
            "'just text'".to_string(),
            &Value::String("just text".into()),
        )]);
        assert!(
            clause.contains("json_set(COALESCE(properties, '{}'), '$.Probe', 'just text')"),
            "{clause}"
        );
        assert!(
            clause.contains("json_remove(COALESCE(property_kinds, '{}'), '$.Probe')")
                && !clause.contains("date_time"),
            "a plain overwrite must CLEAR the entry, not leave it: {clause}"
        );
    }

    #[test]
    fn a_datetime_records_its_kind_in_the_same_statement() {
        let clause = bag_and_kinds_set_clause(&[BagEntry::set(
            "when",
            "'2026-08-22T10:00:00Z'".to_string(),
            &Value::DateTime("2026-08-22T10:00:00Z".into()),
        )]);
        assert!(clause.contains("'$.when', 'date_time'"), "{clause}");
        assert!(
            clause.starts_with("properties = ") && clause.contains(", property_kinds = "),
            "both columns must be assigned by one statement: {clause}"
        );
    }

    #[test]
    fn a_removal_drops_the_key_and_its_kind() {
        let clause = bag_and_kinds_set_clause(&[BagEntry::remove("when")]);
        assert!(
            clause.contains("json_remove(COALESCE(properties, '{}'), '$.when')"),
            "{clause}"
        );
        assert!(
            clause.contains("json_remove(COALESCE(property_kinds, '{}'), '$.when')"),
            "{clause}"
        );
    }

    #[test]
    fn an_emptied_kind_map_is_stored_as_null() {
        let clause = bag_and_kinds_set_clause(&[BagEntry::remove("when")]);
        assert!(
            clause.contains("NULLIF(") && clause.ends_with(", '{}')"),
            "an empty map must collapse to NULL so 'no kinds' has one spelling: {clause}"
        );
    }

    /// TRIPWIRE: a second in-place bag writer would drift from the kind map,
    /// which is exactly the defect this module exists to make unrepresentable.
    ///
    /// Scans EVERY workspace crate, not just this one: the bag is one column in
    /// one table and a writer in any crate would drift the same way. Matching
    /// is whitespace-insensitive and does not require the `COALESCE` — the
    /// first version of this guard caught only the one spelling that had
    /// already regressed, which is precisely the shape of guard that passes
    /// while the next variant walks through it.
    #[test]
    fn only_this_module_patches_the_bag() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("the workspace root must resolve");
        let mut offenders = Vec::new();
        let mut stack = vec![workspace.join("crates"), workspace.join("frontends")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.expect("a readable dir entry").path();
                if path.is_dir() {
                    // `target/` holds generated copies of the very sources
                    // being scanned, which would report phantom offenders.
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs")
                    || path.ends_with("properties_bag_write.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("a readable source file");
                for (n, line) in text.lines().enumerate() {
                    // Prose describing the mechanism is not a writer, and the
                    // codebase does describe it. Only the code half is matched.
                    let squeezed: String = line
                        .split("//")
                        .next()
                        .unwrap_or(line)
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>();
                    let patches_bag = ["json_set(", "json_remove("].iter().any(|f| {
                        [format!("{f}COALESCE(properties"), format!("{f}properties")]
                            .iter()
                            .any(|pat| squeezed.contains(pat.as_str()))
                    });
                    if patches_bag {
                        offenders.push(format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these sites patch the properties bag outside \
             `properties_bag_write::bag_and_kinds_set_clause`, so they cannot keep \
             `property_kinds` in step and will brick rows on the next read:\n  {}",
            offenders.join("\n  ")
        );
    }
}
