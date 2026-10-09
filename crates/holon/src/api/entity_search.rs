//! [`EntitySearch`] over Turso: one `UNION ALL` statement with a ranked,
//! limited branch per searchable type and search group.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntitySearch;
use holon_api::EntityUri;
use holon_api::SearchHit;
use holon_api::SearchQuery;
use holon_api::TypeDefinition;
use holon_api::TypeServices;
use holon_api::computation::FieldIdent;
use holon_profiles::TypeRegistry;
use holon_turso::turso_adapter::TursoAdapter;

use crate::api::backend_engine::query_retrying_schema_change;
use crate::di::schema_providers::UnservedTypes;
use crate::storage::turso::DbHandle;

/// The search group quick-open lists as its Pages section.
pub const PAGE_GROUP: &str = "Page";

/// A named subset of one type's entities, searched and limited apart from the
/// rest of the type.
#[derive(Debug, Clone)]
pub struct SearchGroup {
    pub type_name: String,
    pub name: String,
    /// SQL predicate over the type's row, given its primary-key column.
    pub member_sql: fn(pk: &str) -> String,
}

/// Types this session does not serve are left out: their tables are
/// quarantined, so a branch over one would fail the whole statement.
pub struct UnionAllSearch {
    db: DbHandle,
    types: Arc<TypeRegistry>,
    groups: Vec<SearchGroup>,
    unserved: UnservedTypes,
}

impl UnionAllSearch {
    pub fn new(
        db: DbHandle,
        types: Arc<TypeRegistry>,
        groups: Vec<SearchGroup>,
        unserved: UnservedTypes,
    ) -> Self {
        Self {
            db,
            types,
            groups,
            unserved,
        }
    }

    fn sql(&self, query: &SearchQuery<'_>, m: &SearchMatch) -> String {
        let mut branches = Vec::new();
        let mut unsearched = Vec::new();
        for type_def in self.types.all() {
            let Some(services) = &type_def.services else {
                continue;
            };
            if services.searchable.is_empty() || (query.linkable_only && !services.linkable) {
                continue;
            }
            if self.unserved.condition(&type_def.name).is_some() {
                unsearched.push(type_def.name.to_string());
                continue;
            }
            let pk = column(&type_def.primary_key);
            let groups: Vec<&SearchGroup> = self
                .groups
                .iter()
                .filter(|g| g.type_name == type_def.name)
                .collect();
            for group in &groups {
                let limit = query
                    .group_limits
                    .iter()
                    .find(|(name, _)| *name == group.name)
                    .map(|(_, limit)| *limit)
                    .unwrap_or_else(|| {
                        panic!(
                            "search group {:?} of type {:?} has no limit in the query",
                            group.name, type_def.name
                        )
                    });
                branches.push(branch(
                    &type_def,
                    services,
                    m,
                    Some(group),
                    &(group.member_sql)(&pk),
                    limit,
                ));
            }
            let outside = groups
                .iter()
                .map(|g| format!("NOT ({})", (g.member_sql)(&pk)))
                .collect::<Vec<_>>()
                .join(" AND ");
            branches.push(branch(&type_def, services, m, None, &outside, query.limit));
        }
        if !unsearched.is_empty() {
            tracing::warn!(
                "search leaves out {} unserved type(s): {}",
                unsearched.len(),
                unsearched.join(", ")
            );
        }
        branches.join(" UNION ALL ")
    }
}

/// One `SELECT` over `type_def`'s raw table, wrapped so its `ORDER BY` and
/// `LIMIT` stay its own inside the `UNION ALL`.
fn branch(
    type_def: &TypeDefinition,
    services: &TypeServices,
    m: &SearchMatch,
    group: Option<&SearchGroup>,
    membership: &str,
    limit: usize,
) -> String {
    let title = column(&services.title);
    let matches: Vec<(String, &FieldIdent)> = services
        .searchable
        .iter()
        .map(|f| (m.contained_in(&column(f)), f))
        .collect();
    let matched = matches
        .iter()
        .map(|(predicate, field)| format!("WHEN {predicate} THEN {}", column(field)))
        .collect::<Vec<_>>()
        .join(" ");
    let matched_is_title = matches
        .iter()
        .map(|(predicate, field)| {
            format!(
                "WHEN {predicate} THEN {}",
                i64::from(*field == &services.title)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut filter = vec![format!(
        "({})",
        matches
            .iter()
            .map(|(predicate, _)| predicate.as_str())
            .collect::<Vec<_>>()
            .join(" OR ")
    )];
    if !membership.is_empty() {
        filter.push(membership.to_string());
    }
    if let Some(soft_delete) = &type_def.soft_delete {
        filter.push(format!("{} IS NULL", column(&soft_delete.tombstone_field)));
    }
    let group = group.map_or("NULL".to_string(), |g| sql_string(&g.name));
    format!(
        "SELECT * FROM (SELECT {scheme} AS scheme, {pk} AS id, {group} AS grp, {title} AS title, \
         CASE {matched} END AS matched, CASE {matched_is_title} END AS matched_is_title FROM \
         \"{table}\" e WHERE {filter} ORDER BY ({prefix}) DESC, length({title}) ASC LIMIT {limit})",
        scheme = sql_string(EntityName::new(&type_def.name).as_str()),
        pk = column(&type_def.primary_key),
        table = TursoAdapter::raw_table_name(type_def),
        filter = filter.join(" AND "),
        prefix = m.prefix_of(&title),
    )
}

/// `field` of the branch's row, aliased `e`.
fn column(field: &FieldIdent) -> String {
    format!("e.\"{field}\"")
}

fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[async_trait]
impl EntitySearch for UnionAllSearch {
    async fn search(&self, query: SearchQuery<'_>) -> Result<Vec<SearchHit>> {
        let text = query.text.as_str();
        let m = SearchMatch::new(text)?;
        let sql = self.sql(&query, &m);
        let rows = query_retrying_schema_change(&self.db, &sql, HashMap::new())
            .await
            .with_context(|| format!("search {text:?}"))?;
        rows.into_iter().map(|row| parse_hit(row, text)).collect()
    }
}

fn parse_hit(row: holon_api::StorageEntity, text: &str) -> Result<SearchHit> {
    let text_of = |column: &str| {
        row.get(column)
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("search row has no text {column:?}: {row:?}"))
    };
    let id = EntityUri::from_raw_for(text_of("scheme")?, text_of("id")?);
    // The sentinel's content is empty, so it matches no non-empty text.
    assert!(
        !id.is_sentinel(),
        "search matched the sentinel {id}: {row:?}"
    );
    let title = row.get("title").and_then(|v| v.as_string()).unwrap_or("");
    let matched_is_title = row
        .get("matched_is_title")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow::anyhow!("search row has no matched_is_title: {row:?}"))?
        == 1;
    let snippet = text_of("matched")?
        .split('\n')
        .enumerate()
        .filter(|(i, line)| !(matched_is_title && *i == 0) && folded_contains(line, text))
        .map(|(_, line)| line.to_string())
        .collect();
    Ok(SearchHit {
        group: row
            .get("grp")
            .and_then(|v| v.as_string())
            .map(str::to_string),
        label: title
            .split_once('\n')
            .map_or(title, |(first, _)| first)
            .to_string(),
        snippet,
        id,
    })
}

/// Whether `haystack` contains `needle` under the fold [`SearchMatch`] matches
/// with.
fn folded_contains(haystack: &str, needle: &str) -> bool {
    let fold = |s: &str| s.chars().map(simple_lower).collect::<String>();
    fold(haystack).contains(&fold(needle))
}

/// Unicode *simple* lowercase: the lowercase form when that is a single
/// character, else the character unchanged. Only simple folding is available
/// here because a `GLOB` character class holds single characters, so `ß` → `ss`
/// is inexpressible.
fn simple_lower(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Every character sharing a `simple_lower` form, keyed by that form — the
/// equivalence classes of the fold, so a class holds `ß` *and* `ẞ`, `k` *and*
/// the Kelvin sign, all four spellings of the `ǅ` digraph.
///
/// Derived by scanning the Unicode scalar values rather than listed, so a
/// character added by a future Unicode table joins its class with no edit here.
/// Classes of one are dropped: those characters need no class at all.
static FOLD_CLASSES: std::sync::LazyLock<HashMap<char, Box<[char]>>> =
    std::sync::LazyLock::new(|| {
        let mut groups: HashMap<char, Vec<char>> = HashMap::new();
        for c in (0..=0x0010_FFFF_u32).filter_map(char::from_u32) {
            if c.is_lowercase() || c.is_uppercase() || simple_lower(c) != c {
                groups.entry(simple_lower(c)).or_default().push(c);
            }
        }
        groups.retain(|_, members| members.len() > 1);
        groups
            .into_iter()
            .map(|(fold, mut members)| {
                members.sort_unstable();
                (fold, members.into_boxed_slice())
            })
            .collect()
    });

/// The largest `GLOB` pattern the storage engine accepts, in bytes
/// (`MAX_GLOB_PATTERN_LENGTH`, turso `core/vdbe/value.rs`). A class costs up to
/// eight bytes per query character, so the ceiling is reached at roughly 12 500
/// ASCII or 8 300 Cyrillic characters.
const MAX_GLOB_PATTERN_BYTES: usize = 50_000;

/// A query whose folded pattern would exceed [`MAX_GLOB_PATTERN_BYTES`]. Parsed
/// at construction so no over-long pattern can reach the engine, where the same
/// condition surfaces as an opaque "GLOB pattern too complex".
#[derive(Debug)]
pub struct SearchQueryTooLong {
    pattern_bytes: usize,
}

impl std::fmt::Display for SearchQueryTooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "search query is too long: case folding expands it to a {}-byte GLOB pattern, over \
             the {MAX_GLOB_PATTERN_BYTES}-byte limit the storage engine accepts",
            self.pattern_bytes
        )
    }
}

impl std::error::Error for SearchQueryTooLong {}

/// A user-typed search string parsed into a case-insensitive SQL `GLOB`
/// pattern in which every character the user typed matches only itself.
///
/// `GLOB` rather than `LIKE` because the fold has to live in the pattern:
/// `GLOB` is case-sensitive, so each cased character carries a character class
/// holding its whole [`FOLD_CLASSES`] equivalence class, and folding costs one
/// flat class per character. Folding the stored side instead — the `LIKE`
/// spelling — nests one `replace()` per cased letter, and that depth overflowed
/// the stack on a Cyrillic or Greek phrase (entry
/// `search-folding-crashes-the-app-on-cyrillic-and-greek`).
///
/// The pattern is bounded by [`MAX_GLOB_PATTERN_BYTES`]; a longer query is
/// refused as [`SearchQueryTooLong`] rather than sent to the engine.
#[derive(Debug)]
struct SearchMatch {
    /// The pattern between the quotes, already escaped for both `GLOB` and the
    /// SQL string literal that carries it.
    body: String,
}

impl SearchMatch {
    fn new(query: &str) -> Result<Self, SearchQueryTooLong> {
        let mut body = String::new();
        for c in query.chars() {
            match c {
                // `GLOB` has no escape character, so a one-element class is the
                // only way to spell its own metacharacters literally. None of
                // the three is cased, so this arm never hides a fold class.
                '*' | '?' | '[' => body.extend(['[', c, ']']),
                '\'' => body.push_str("''"),
                _ => match FOLD_CLASSES.get(&simple_lower(c)) {
                    Some(members) => {
                        body.push('[');
                        body.extend(members.iter().copied());
                        body.push(']');
                    }
                    None => body.push(c),
                },
            }
        }
        // Both predicates wrap the body in at most one leading and one trailing
        // `*`, so this is the largest pattern either of them can produce.
        let pattern_bytes = body.len() + 2;
        if pattern_bytes > MAX_GLOB_PATTERN_BYTES {
            return Err(SearchQueryTooLong { pattern_bytes });
        }
        Ok(Self { body })
    }

    /// Predicate: `column` contains the query anywhere.
    fn contained_in(&self, column: &str) -> String {
        format!("{column} GLOB '*{}*'", self.body)
    }

    /// Predicate: `column` starts with the query — the prefix ranker.
    fn prefix_of(&self, column: &str) -> String {
        format!("{column} GLOB '{}*'", self.body)
    }
}

#[cfg(test)]
mod fold_class_tests {
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

    use super::MAX_GLOB_PATTERN_BYTES;
    use super::SearchMatch;

    /// The reference oracle, restated here independently of the engine: the
    /// same simple fold the keystone's `Search` transition compares against
    /// (`holon-integration-tests/src/pbt/transitions/search.rs`).
    fn oracle_fold(c: char) -> char {
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            (Some(l), None) => l,
            _ => c,
        }
    }

    /// The characters of the one class `SearchMatch` emitted, or `None` when it
    /// emitted the character bare.
    fn emitted_class(c: char) -> Option<BTreeSet<char>> {
        let body = SearchMatch::new(&c.to_string())
            .expect("one character is never too long")
            .body;
        let inner = body.strip_prefix('[')?.strip_suffix(']')?;
        Some(inner.chars().collect())
    }

    /// A query for any spelling of a character finds every other spelling that
    /// folds with it.
    #[test]
    fn glob_class_is_the_oracles_whole_equivalence_class_across_the_bmp() {
        let mut expected: BTreeMap<char, BTreeSet<char>> = BTreeMap::new();
        for c in (0..=0xFFFF_u32).filter_map(char::from_u32) {
            expected.entry(oracle_fold(c)).or_default().insert(c);
        }

        let mut checked = 0_usize;
        for c in (0..=0xFFFF_u32).filter_map(char::from_u32) {
            if !c.is_lowercase() && !c.is_uppercase() && oracle_fold(c) == c {
                continue;
            }
            let equivalents = &expected[&oracle_fold(c)];
            match emitted_class(c) {
                Some(class) => assert_eq!(
                    &class, equivalents,
                    "query {c:?} (U+{:04X}) emitted class {class:?}, but the oracle folds \
                     {equivalents:?} together — every one of those must be findable",
                    c as u32
                ),
                None => assert_eq!(
                    equivalents.len(),
                    1,
                    "query {c:?} (U+{:04X}) was emitted bare, but the oracle folds it together \
                     with {equivalents:?}",
                    c as u32
                ),
            }
            checked += 1;
        }
        assert!(
            checked > 2000,
            "the sweep must reach the cased BMP, checked only {checked}"
        );
    }

    /// One case per many-to-one fold family, so a failure names the family.
    #[test]
    fn the_many_to_one_folds_reach_every_spelling() {
        for (query, must_contain) in [
            ('ß', 'ẞ'),
            ('ẞ', 'ß'),
            ('ǅ', 'Ǆ'),
            ('ǆ', 'ǅ'),
            ('Ǆ', 'ǆ'),
            ('k', '\u{212A}'),
            ('\u{212A}', 'K'),
            ('ω', '\u{2126}'),
            ('å', '\u{212B}'),
        ] {
            let class = emitted_class(query)
                .unwrap_or_else(|| panic!("{query:?} must fold, so it must emit a class"));
            assert!(
                class.contains(&must_contain),
                "searching for {query:?} must find stored {must_contain:?}, class was {class:?}"
            );
        }
    }

    /// The engine's own pattern ceiling, refused at the boundary instead of
    /// surfacing as an opaque "GLOB pattern too complex" from storage.
    #[test]
    fn an_over_long_query_is_refused_at_the_exact_threshold() {
        // A Cyrillic letter costs 4 bytes of class body (2 chars × 2 bytes).
        let per_char = SearchMatch::new("а").expect("one char").body.len();
        let fits = (MAX_GLOB_PATTERN_BYTES - 2) / per_char;
        assert_eq!(
            SearchMatch::new(&"а".repeat(fits))
                .expect("the threshold itself fits")
                .body
                .len()
                + 2,
            MAX_GLOB_PATTERN_BYTES,
            "the largest accepted query must land exactly on the limit"
        );
        let err = SearchMatch::new(&"а".repeat(fits + 1))
            .expect_err("one character over the threshold must be refused");
        assert!(
            err.to_string()
                .contains(&MAX_GLOB_PATTERN_BYTES.to_string()),
            "the error must name the limit, got {err}"
        );
    }
}

#[cfg(test)]
mod tests {
    use holon_api::SearchText;

    use super::*;

    fn note_type(name: &str) -> TypeDefinition {
        serde_yaml::from_str(&format!(
            "name: {name}\nprimary_key: id\nsource: pre_configured\nfields:\n  - name: id\n    \
             sql_type: TEXT\n    primary_key: true\n  - name: title\n    sql_type: TEXT\n\
             services:\n  title: title\n  searchable: [title]\n"
        ))
        .expect("note type yaml")
    }

    #[tokio::test]
    async fn a_quarantined_searchable_type_leaves_the_other_types_searchable() {
        let (_backend, db) = crate::storage::turso::TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        let types = Arc::new(TypeRegistry::new());
        for name in ["note_kept", "note_refused"] {
            let type_def = note_type(name);
            TursoAdapter::register(&type_def, &db)
                .await
                .expect("register note type");
            types.register(type_def).expect("registry");
        }
        db.execute(
            "INSERT INTO note_kept_raw (id, title) VALUES ('k1', 'hello kept')",
            vec![],
        )
        .await
        .expect("insert kept note");
        holon_turso::table_shape::quarantine(&db, "note_refused_raw")
            .await
            .expect("quarantine");
        let unserved = UnservedTypes::default();
        unserved.insert(
            "note_refused",
            "note_refused_raw",
            holon_api::ConditionKind::TYPE_TABLE_REFUSED,
            "test refusal".to_string(),
        );

        let hits = UnionAllSearch::new(db, types, vec![], unserved)
            .search(SearchQuery {
                text: SearchText::new("hello").expect("search text"),
                linkable_only: false,
                limit: 10,
                group_limits: &[],
            })
            .await
            .expect("a refused type must not break search over the others");
        let ids: Vec<String> = hits.iter().map(|h| h.id.to_string()).collect();
        assert_eq!(ids.len(), 1, "the kept note is found: {ids:?}");
    }
}
