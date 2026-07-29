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

/// Byte index of the earliest top-level (depth 0, outside quotes) occurrence
/// of ORDER BY / LIMIT / OFFSET as a standalone keyword, if any.
fn find_top_level_trailing_clause(sql: &str) -> Option<usize> {
    const KEYWORDS: [&str; 3] = ["ORDER BY", "LIMIT", "OFFSET"];
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
            _ if depth == 0 && KEYWORDS.iter().any(|kw| keyword_at(bytes, i, kw)) => {
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
    use super::strip_order_by;

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
