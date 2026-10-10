//! What each table of a Holon database is, for a stored table that drifted
//! from its declaration (`table_shape`): the org files and the Loro store
//! refill every [`Class::Rebuilt`] table, each connection re-syncs its
//! [`Class::IntegrationCache`] tables, and a [`Class::Lost`] table holds state
//! nothing else restores. `holon-app/tests/database_open_disclosure.rs` fails
//! for a table in no class.

use std::collections::HashMap;

use crate::turso_adapter::RAW_TABLE_SUFFIX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    Rebuilt,
    /// Filled by this provider's sync. A row its source no longer holds does
    /// not come back.
    IntegrationCache(String),
    /// What the user loses, in words a disclosure can show.
    Lost(&'static str),
}

const REBUILT: &[&str] = &[
    // The block tree and its edges: the org files and the Loro store.
    "block_tags",
    "block_requires",
    "block_contributes_to",
    "block_links",
    "block_derived",
    "file",
    // Time as data: the scheduler reseeds it; a sidecar view records its grain
    // again when it is created again.
    "clock",
    "clock_reader",
    "holon_db_fn_set",
    "integration_cache",
    // Mirrors of state kept elsewhere, or of live watches.
    "integration_state",
    "watch_context",
];

/// `<name>_raw` tables (a type's write table, `turso_adapter`) that a source
/// outside this database restores in full after a rebuild: with the Loro store
/// on, the org files and the Loro store are `block_raw`'s only write path, so
/// both its rows and its columns come back whole.
/// `recipe_raw`/`ingredient_use_raw` are NOT here: the generic op surface can
/// write a `source_path`/`recipe_id` the cooklang plugin's `.cook` files never
/// back, and columns outside its restored list (`ingredient_use.product_id`,
/// either type's `properties` overflow) — same shape as `shopping_item_raw`,
/// below. Any other `_raw` table — a bundled type with no such source
/// (`pantry_item_raw`, `person_raw`) or a user-added type minted at runtime and
/// written only through `SqlOperationProvider` — is `Lost` too.
const REBUILT_RAW: &[&str] = &["block_raw"];

/// What a `_raw` table not literally named above holds: nothing outside this
/// database restores it, because its type's write authority is SQL only.
const RAW_TABLE_LOST_WHAT: &str =
    "this type's rows: no replica or file restores them after a rebuild";

/// Prefixes the bundled sidecars (`assets/integrations/*.yaml`) give the
/// tables they cache entities into. A table under one of these prefixes is an
/// integration cache even when `integration_cache` has not yet recorded it —
/// the first rebuild of a database from before that table existed, or before
/// this provider has reconnected on this build.
const CACHE_PREFIXES: &[(&str, &str)] = &[
    ("cc_", "claude-history"),
    ("gcal_", "gcal"),
    ("gmail_", "gmail"),
    ("ics_", "ics-calendar"),
    ("todoist_", "todoist"),
    ("jp_", "jsonplaceholder"),
];

const LOST: &[(&str, &str)] = &[
    ("undo_log", "undo history"),
    ("operation", "the operation log"),
    (
        "block_history",
        "the op and effect history behind the journal feed",
    ),
    (
        "navigation_history",
        "back and forward history and sidebar pins",
    ),
    (
        "navigation_cursor",
        "the position in the back and forward history",
    ),
    ("advice_suppressed", "dismissed advice"),
    (
        "_holon_quarantine",
        "the record of which tables keep the rows of types Holon did not serve",
    ),
    ("block_redirects", "redirects from merged or renamed blocks"),
    ("local_ui_state", "local view settings"),
    (
        "sync_states",
        "sync tokens: every connection re-syncs in full",
    ),
    ("canonical_entity", "cross-system identities"),
    ("entity_alias", "cross-system identity links"),
    ("proposal_queue", "pending identity proposals"),
    (
        "identifiers",
        "identity rows from a removed identity scheme",
    ),
    (
        "shopping_item_raw",
        "locally-set product links and pending deletions on shopping items \
         (names, categories and check state come back from the next sync)",
    ),
    (
        "recipe_raw",
        "hand-created recipes with no .cook file behind them, and any column \
         the cooklang plugin does not restore (title/course/source_path come \
         back from the next parse)",
    ),
    (
        "ingredient_use_raw",
        "product links on hand-created ingredient rows, and any ingredient \
         row for a recipe with no .cook file behind it (raw name, quantity, \
         unit and step come back from the next parse)",
    ),
];

/// `caches` maps each table `integration_cache` records to its provider.
pub fn class_of(table: &str, caches: &HashMap<String, String>) -> Option<Class> {
    if REBUILT.contains(&table) || REBUILT_RAW.contains(&table) {
        return Some(Class::Rebuilt);
    }
    if let Some(provider) = caches.get(table) {
        return Some(Class::IntegrationCache(provider.clone()));
    }
    if let Some((_, provider)) = CACHE_PREFIXES
        .iter()
        .find(|(prefix, _)| table.starts_with(prefix))
    {
        return Some(Class::IntegrationCache((*provider).to_string()));
    }
    if let Some((_, what)) = LOST.iter().find(|(name, _)| *name == table) {
        return Some(Class::Lost(what));
    }
    if table.ends_with(RAW_TABLE_SUFFIX) {
        return Some(Class::Lost(RAW_TABLE_LOST_WHAT));
    }
    None
}

/// Without the Loro store (`crdt.enabled = false`) the block tree's tables
/// hold the only durable copy of the blocks, so a drifted one has its rows
/// carried into the declared shape, never dropped.
pub fn carries_rows(table: &str) -> bool {
    [
        "block_raw",
        "block_tags",
        "block_requires",
        "block_contributes_to",
        "block_links",
    ]
    .contains(&table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_table_is_in_both_lists() {
        for (table, _) in LOST {
            assert!(!REBUILT.contains(table), "{table} is both rebuilt and lost");
        }
    }

    /// A `_raw` table is rebuilt only when org files or a read-only format
    /// plugin restores every column of it AND every row: the org files and
    /// the Loro store are the sole write path for `block_raw`, so both hold.
    #[test]
    fn a_file_backed_types_raw_table_is_rebuilt() {
        assert_eq!(class_of("block_raw", &HashMap::new()), Some(Class::Rebuilt));
    }

    /// `ingredient_use.product_id` (Inc D's product binding) is not in the
    /// cooklang sidecar's restored column list, and a `create` through the
    /// generic op surface can name a `source_path` no `.cook` file backs. The
    /// plugin then never restores either, so the table discloses as lost —
    /// same shape as `shopping_item_raw`.
    #[test]
    fn recipe_raw_is_lost() {
        assert!(matches!(
            class_of("recipe_raw", &HashMap::new()),
            Some(Class::Lost(_))
        ));
    }

    #[test]
    fn ingredient_use_raw_is_lost() {
        assert!(matches!(
            class_of("ingredient_use_raw", &HashMap::new()),
            Some(Class::Lost(_))
        ));
    }

    /// `product_id` and `deleted_at` are local-only (`shopping_item.yaml`):
    /// the peer's snapshot carries neither, so they never come back. A raw
    /// table with any such column must disclose as lost, not rebuilt.
    #[test]
    fn shopping_item_raw_is_lost() {
        assert!(matches!(
            class_of("shopping_item_raw", &HashMap::new()),
            Some(Class::Lost(_))
        ));
    }

    /// A user-added entity type's write authority is `SqlOperationProvider`
    /// (SQL only, no Loro/org leg), so nothing restores its runtime-minted
    /// `<name>_raw` table after a rebuild.
    #[test]
    fn a_user_added_types_raw_table_is_lost() {
        assert!(matches!(
            class_of("some_user_type_raw", &HashMap::new()),
            Some(Class::Lost(_))
        ));
    }

    /// H4: on the first rebuild ever for a build that just added
    /// `integration_cache`, the table exists in the file already (from before
    /// that feature) but `integration_cache` has recorded nothing for it yet.
    /// The prefix still classifies it as a cache, not a loss.
    #[test]
    fn a_known_providers_table_is_a_cache_even_with_no_runtime_record() {
        for table in [
            "cc_message",
            "gcal_event",
            "gmail_thread",
            "todoist_projects",
        ] {
            assert!(
                matches!(
                    class_of(table, &HashMap::new()),
                    Some(Class::IntegrationCache(_))
                ),
                "{table} must classify as an integration cache by name alone"
            );
        }
    }

    /// D232.a: a legacy identity scheme nothing reads any more is still user
    /// state, so its rows are disclosed as lost, not excluded.
    #[test]
    fn identifiers_is_lost_not_excluded() {
        assert!(matches!(
            class_of("identifiers", &HashMap::new()),
            Some(Class::Lost(_))
        ));
    }

    /// No `Lost` literal is shadowed by the generic `_raw` rule or a cache
    /// prefix: a table both rules could reach would silently classify
    /// through whichever rule the code checks first.
    #[test]
    fn no_lost_name_matches_a_rule_that_would_override_it() {
        for (table, _) in LOST {
            assert!(
                !REBUILT_RAW.contains(table),
                "{table} is both a rebuilt raw table and lost"
            );
            assert!(
                !CACHE_PREFIXES
                    .iter()
                    .any(|(prefix, _)| table.starts_with(prefix)),
                "{table} is both a cache prefix match and lost"
            );
        }
    }
}
