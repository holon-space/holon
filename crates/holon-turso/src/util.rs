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

/// Strip ORDER BY, LIMIT, and OFFSET clauses from SQL so it can become a
/// Turso materialized-view body (IVM only supports Filter/Projection/Join/
/// Aggregate/Union/EmptyRelation/Values).
///
/// Word-boundary matching avoids false positives on identifiers like
/// `cursor_offset` that contain a keyword as a substring.
pub fn strip_order_by(sql: &str) -> String {
    let upper = sql.to_uppercase();
    let keywords = ["ORDER BY", "LIMIT", "OFFSET"];
    let earliest = keywords
        .iter()
        .filter_map(|kw| find_keyword(&upper, kw))
        .min();
    match earliest {
        Some(idx) => sql[..idx].trim().to_string(),
        None => sql.to_string(),
    }
}

/// Find a SQL keyword at a word boundary in the already-uppercased string.
fn find_keyword(upper: &str, keyword: &str) -> Option<usize> {
    let mut search_start = 0;
    while let Some(rel) = upper[search_start..].find(keyword) {
        let idx = search_start + rel;
        let before_ok = idx == 0
            || upper.as_bytes()[idx - 1].is_ascii_whitespace()
            || upper.as_bytes()[idx - 1] == b')';
        let after = idx + keyword.len();
        let after_ok = after >= upper.len()
            || upper.as_bytes()[after].is_ascii_whitespace()
            || upper.as_bytes()[after].is_ascii_digit();
        if before_ok && after_ok {
            return Some(idx);
        }
        search_start = idx + keyword.len();
    }
    None
}
