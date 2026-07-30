//! Small Turso-local helpers. These mirror the equivalents in `holon::util`
//! but are kept here so the adapter has no dependency on the `holon` crate.

/// Spawn a fire-and-forget future onto the current async executor.
///
/// On native this uses `tokio::spawn`. On wasm32-unknown — where there is no
/// tokio reactor — it uses `wasm_bindgen_futures::spawn_local`.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn spawn_actor<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future);
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn spawn_actor<F>(future: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}

/// Strip trailing top-level ORDER BY, LIMIT, and OFFSET clauses from SQL so
/// it can become a Turso materialized-view body (IVM only supports
/// Filter/Projection/Join/Aggregate/Union/EmptyRelation/Values).
///
/// Quote- and paren-depth-aware: keywords inside string literals, quoted
/// identifiers, or subqueries are left untouched. Word-boundary matching
/// avoids false positives on identifiers like `cursor_offset` that contain
/// a keyword as a substring.
pub fn strip_order_by(sql: &str) -> String {
    match find_top_level_trailing_clause(sql) {
        Some(idx) => sql[..idx].trim().to_string(),
        None => sql.to_string(),
    }
}

/// The trailing top-level `ORDER BY` clause `strip_order_by` removes, so a
/// reader of the matview can re-apply it.
///
/// `LIMIT` / `OFFSET` are excluded: the matview holds the unbounded relation
/// and its CDC stream delivers changes beyond any window, so re-applying a
/// window to the snapshot alone would disagree with the stream.
pub fn trailing_order_by(sql: &str) -> Option<String> {
    let start = find_top_level_keyword(sql, &["ORDER BY"], 0)?;
    let end = find_top_level_keyword(sql, &["LIMIT", "OFFSET"], start + 1).unwrap_or(sql.len());
    Some(sql[start..end].trim().to_string())
}

/// Translate an `ORDER BY` clause into the render layer's sort-key spec —
/// `col` for ascending, `-col` for descending — so a watched query's declared
/// order can drive the rendered collection's sort.
///
/// Only a single plain column is expressible in that form. A multi-column
/// clause keeps its FIRST column and discloses the truncation; an
/// expression, collation, or `NULLS` ordering is not expressible at all and
/// yields `None` with a warning rather than a wrong order.
///
/// Panics if `order_by` is not an `ORDER BY` clause — the only producer is
/// [`trailing_order_by`], so anything else is a programming error.
pub fn order_by_sort_spec(order_by: &str) -> Option<String> {
    let trimmed = order_by.trim();
    let list = match trimmed.get(..8) {
        Some(kw) if kw.eq_ignore_ascii_case("ORDER BY") => trimmed[8..].trim(),
        _ => panic!("order_by_sort_spec expects an ORDER BY clause, got {order_by:?}"),
    };

    let mut terms = list.split(',');
    let first = terms.next().expect("split always yields one term").trim();
    let extra = terms.count();
    if extra > 0 {
        tracing::warn!(
            clause = %trimmed,
            "multi-column ORDER BY: the rendered sort key holds ONE column, so the {extra} \
             trailing column(s) are dropped and rows tying on the first column render in \
             row-key order"
        );
    }

    let tokens: Vec<&str> = first.split_whitespace().collect();
    let (column, descending) = match tokens.as_slice() {
        [col] => (*col, false),
        [col, dir] if dir.eq_ignore_ascii_case("ASC") => (*col, false),
        [col, dir] if dir.eq_ignore_ascii_case("DESC") => (*col, true),
        _ => {
            tracing::warn!(
                term = %first,
                "ORDER BY term is not `column [ASC|DESC]`; the rendered collection cannot honour \
                 it and falls back to its default row order"
            );
            return None;
        }
    };

    // Only `"` and `` ` `` quote an IDENTIFIER. A single-quoted term is a string
    // literal, so it keeps its quotes here and falls into the warn-and-decline
    // branch below instead of being read as a column of that name.
    let column = column.trim_matches('"').trim_matches('`');
    if column.is_empty()
        || column.starts_with(|c: char| c.is_ascii_digit())
        || !column
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        tracing::warn!(
            term = %first,
            "ORDER BY sorts on an expression, not a plain column; the rendered collection cannot \
             honour it and falls back to its default row order"
        );
        return None;
    }

    Some(if descending {
        format!("-{column}")
    } else {
        column.to_string()
    })
}

/// Byte index of the earliest top-level (depth 0, outside quotes) occurrence
/// of ORDER BY / LIMIT / OFFSET as a standalone keyword, if any.
fn find_top_level_trailing_clause(sql: &str) -> Option<usize> {
    find_top_level_keyword(sql, &["ORDER BY", "LIMIT", "OFFSET"], 0)
}

/// Byte index of the earliest top-level occurrence at or after `from` of any
/// of `keywords`. Quote- and paren-depth-aware; the scan always starts at 0 so
/// depth and quote state are correct, `from` only filters what counts as a hit.
fn find_top_level_keyword(sql: &str, keywords: &[&str], from: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth: i64 = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        // '' inside a '-string is an escaped quote, not a close
                        if quote == b'\'' && bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 0 && i >= from && keywords.iter().any(|kw| keyword_at(bytes, i, kw)) => {
                return Some(i);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn keyword_at(bytes: &[u8], idx: usize, keyword: &str) -> bool {
    let kw = keyword.as_bytes();
    let after = idx + kw.len();
    if after > bytes.len() || !bytes[idx..after].eq_ignore_ascii_case(kw) {
        return false;
    }
    let before_ok = idx == 0 || bytes[idx - 1].is_ascii_whitespace() || bytes[idx - 1] == b')';
    let after_ok =
        after >= bytes.len() || bytes[after].is_ascii_whitespace() || bytes[after].is_ascii_digit();
    before_ok && after_ok
}

#[cfg(test)]
mod tests {
    use super::order_by_sort_spec;
    use super::strip_order_by;

    #[test]
    fn sort_spec_single_column_defaults_to_ascending() {
        assert_eq!(order_by_sort_spec("ORDER BY name").as_deref(), Some("name"));
    }

    #[test]
    fn sort_spec_explicit_asc() {
        assert_eq!(
            order_by_sort_spec("ORDER BY name ASC").as_deref(),
            Some("name")
        );
    }

    #[test]
    fn sort_spec_desc_becomes_a_minus_prefix() {
        assert_eq!(
            order_by_sort_spec("ORDER BY last_activity DESC").as_deref(),
            Some("-last_activity")
        );
    }

    #[test]
    fn sort_spec_keyword_case_is_irrelevant() {
        assert_eq!(
            order_by_sort_spec("order by last_activity desc").as_deref(),
            Some("-last_activity")
        );
    }

    #[test]
    fn sort_spec_unquotes_a_quoted_column() {
        assert_eq!(
            order_by_sort_spec("ORDER BY \"last_activity\" DESC").as_deref(),
            Some("-last_activity")
        );
    }

    // Disclosed truncation: the sort-key spec holds one column, so the
    // secondary keys are dropped — loudly, never silently.
    #[test]
    fn sort_spec_multi_column_keeps_the_first_column() {
        assert_eq!(
            order_by_sort_spec("ORDER BY last_activity DESC, name ASC").as_deref(),
            Some("-last_activity")
        );
    }

    /// A single-quoted term is a string LITERAL, not an identifier. Reading it
    /// as a column of that name would render a wrong order with no signal —
    /// the one outcome this function must never produce.
    #[test]
    fn sort_spec_string_literal_is_not_read_as_a_column() {
        assert_eq!(order_by_sort_spec("ORDER BY 'last_activity'"), None);
        assert_eq!(order_by_sort_spec("ORDER BY 'literal' DESC"), None);
    }

    #[test]
    fn sort_spec_expression_is_not_expressible() {
        assert_eq!(order_by_sort_spec("ORDER BY lower(name)"), None);
        assert_eq!(order_by_sort_spec("ORDER BY name COLLATE NOCASE"), None);
        assert_eq!(order_by_sort_spec("ORDER BY name DESC NULLS LAST"), None);
        assert_eq!(order_by_sort_spec("ORDER BY 1"), None);
    }

    #[test]
    #[should_panic(expected = "expects an ORDER BY clause")]
    fn sort_spec_rejects_a_non_order_by_clause() {
        order_by_sort_spec("LIMIT 10");
    }

    #[test]
    fn strip_order_by_removes_clause() {
        assert_eq!(
            strip_order_by("SELECT * FROM t ORDER BY name ASC"),
            "SELECT * FROM t"
        );
    }

    #[test]
    fn strip_order_by_also_strips_limit() {
        assert_eq!(
            strip_order_by("SELECT * FROM t ORDER BY name ASC LIMIT 10"),
            "SELECT * FROM t"
        );
    }

    #[test]
    fn strip_limit_without_order_by() {
        assert_eq!(
            strip_order_by("SELECT * FROM t WHERE x = 1 LIMIT 10"),
            "SELECT * FROM t WHERE x = 1"
        );
    }

    #[test]
    fn strip_limit_and_offset() {
        assert_eq!(
            strip_order_by("SELECT * FROM t LIMIT 10 OFFSET 5"),
            "SELECT * FROM t"
        );
    }

    #[test]
    fn strip_order_by_no_clause() {
        let sql = "SELECT * FROM t WHERE x = 1";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn strip_order_by_case_insensitive() {
        assert_eq!(
            strip_order_by("SELECT * FROM t order by name"),
            "SELECT * FROM t"
        );
    }

    #[test]
    fn offset_in_column_name_not_stripped() {
        let sql = "SELECT block_id, cursor_offset FROM current_editor_focus WHERE region = 'main'";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn real_offset_clause_still_stripped() {
        assert_eq!(
            strip_order_by("SELECT block_id, cursor_offset FROM t LIMIT 10 OFFSET 5"),
            "SELECT block_id, cursor_offset FROM t"
        );
    }

    #[test]
    fn keyword_inside_string_literal_not_stripped() {
        let sql = "SELECT * FROM t WHERE note = 'a LIMIT 5'";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn keyword_inside_escaped_string_literal_not_stripped() {
        let sql = "SELECT * FROM t WHERE note = 'it''s ORDER BY x'";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn keyword_inside_subquery_not_stripped() {
        let sql = "SELECT * FROM (SELECT * FROM t ORDER BY x LIMIT 5) sub";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn trailing_clause_after_subquery_stripped() {
        assert_eq!(
            strip_order_by("SELECT * FROM (SELECT * FROM t LIMIT 5) sub ORDER BY x"),
            "SELECT * FROM (SELECT * FROM t LIMIT 5) sub"
        );
    }

    #[test]
    fn keyword_inside_quoted_identifier_not_stripped() {
        let sql = "SELECT \"weird LIMIT col\" FROM t";
        assert_eq!(strip_order_by(sql), sql);
    }

    // Keyword that runs exactly to end-of-input: pins the `after > len` bound
    // (kills `>`→`>=`) and the `after >= len` after-ok check (kills `>=`→`<`,
    // which would index past the slice and panic).
    #[test]
    fn bare_trailing_keyword_at_eof_is_stripped() {
        assert_eq!(strip_order_by("SELECT * FROM t LIMIT"), "SELECT * FROM t");
    }

    // Keyword immediately preceded by `)` with no whitespace: forces the
    // second `bytes[idx - 1] == b')'` before-ok branch (kills the `idx - 1`
    // index arithmetic mutants there, which the whitespace branch otherwise
    // masks).
    #[test]
    fn keyword_right_after_close_paren_is_stripped() {
        assert_eq!(
            strip_order_by("SELECT * FROM (SELECT 1)LIMIT 5"),
            "SELECT * FROM (SELECT 1)"
        );
    }

    // Unterminated quote reaching end-of-input: pins the inner `i < len` scan
    // bound (kills `<`→`<=`, which would read one past the slice and panic).
    #[test]
    fn unterminated_quote_to_eof_is_noop() {
        assert_eq!(strip_order_by("SELECT '"), "SELECT '");
    }
}
