use std::collections::{HashMap, HashSet, VecDeque};

/// Wall-clock milliseconds since Unix epoch.
///
/// On native this calls `std::time::SystemTime::now()`. On wasm32, where
/// `std::time` panics, it routes through `web_time` which forwards to the
/// browser's `Date.now()`.
pub fn now_unix_millis() -> i64 {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_millis() as i64
    }
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_millis() as i64
    }
}

/// Monotonic instant, wasm32-safe.
///
/// On wasm32 forwards to `performance.now()` via `web_time`, which is
/// monotonic. On native this is `std::time::Instant`.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant as MonotonicInstant;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use web_time::Instant as MonotonicInstant;

/// Spawn a future onto the current async executor.
///
/// On native this uses `tokio::spawn`. On wasm32 — where there is no tokio
/// reactor — it uses `wasm_bindgen_futures::spawn_local`, which schedules
/// onto the browser's microtask queue. The returned () means callers can't
/// join on the future on wasm; only fire-and-forget actor loops should use
/// this helper.
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

/// Strip ORDER BY, LIMIT, and OFFSET clauses from SQL.
///
/// Turso materialized views (IVM) only support Filter, Projection, Join,
/// Aggregate, Union, EmptyRelation, and Values operators. ORDER BY, LIMIT,
/// and OFFSET are not supported and must be removed before creating a matview.
pub fn strip_order_by(sql: &str) -> String {
    strip_unsupported_clauses(sql)
}

/// Strip ORDER BY, LIMIT, and OFFSET clauses from SQL for matview compatibility.
///
/// Uses word-boundary matching to avoid false positives on column names
/// like `cursor_offset` which contain "OFFSET" as a substring.
fn strip_unsupported_clauses(sql: &str) -> String {
    let upper = sql.to_uppercase();

    // Find the earliest of ORDER BY, LIMIT, OFFSET as standalone SQL keywords.
    // A keyword is standalone if preceded by whitespace (or start of string)
    // and followed by whitespace, digit, or end of string.
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

/// Find a SQL keyword in the uppercase string, ensuring it's at a word boundary.
/// Returns the byte position of the keyword if found as a standalone word.
fn find_keyword(upper: &str, keyword: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(pos) = upper[start..].find(keyword) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0 || upper.as_bytes()[abs_pos - 1].is_ascii_whitespace();
        let after_pos = abs_pos + keyword.len();
        let after_ok = after_pos >= upper.len()
            || upper.as_bytes()[after_pos].is_ascii_whitespace()
            || upper.as_bytes()[after_pos].is_ascii_digit();
        if before_ok && after_ok {
            return Some(abs_pos);
        }
        start = abs_pos + 1;
    }
    None
}

/// Check if a Rhai expression references a given variable name.
/// Uses word-boundary matching to avoid false positives.
pub fn expr_references(expr: &str, name: &str) -> bool {
    let name_bytes = name.as_bytes();
    let expr_bytes = expr.as_bytes();
    let name_len = name_bytes.len();

    for i in 0..expr_bytes.len() {
        if expr_bytes[i..].starts_with(name_bytes) {
            let before_ok =
                i == 0 || !expr_bytes[i - 1].is_ascii_alphanumeric() && expr_bytes[i - 1] != b'_';
            let after_ok = i + name_len >= expr_bytes.len()
                || !expr_bytes[i + name_len].is_ascii_alphanumeric()
                    && expr_bytes[i + name_len] != b'_';
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// Topological sort via Kahn's algorithm.
///
/// Given a set of `names` and a dependency map `deps` (where `deps[a]` lists
/// the names that `a` depends on), returns the names in an order where every
/// dependency appears before its dependents.
///
/// Panics if the dependency graph contains a cycle.
pub fn topo_sort_kahn<'a>(
    names: &HashSet<&'a str>,
    deps: &HashMap<&'a str, Vec<&'a str>>,
) -> Vec<String> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut in_deg: HashMap<&str, usize> = HashMap::new();
    for name in names {
        adj.entry(name).or_default();
        in_deg.entry(name).or_insert(0);
    }
    for (name, dep_list) in deps {
        for dep in dep_list {
            adj.entry(dep).or_default().push(name);
            *in_deg.entry(name).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = {
        let mut seeds: Vec<&str> = in_deg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(n, _)| *n)
            .collect();
        seeds.sort(); // deterministic order
        seeds.into_iter().collect()
    };

    let mut result = Vec::new();

    while let Some(node) = queue.pop_front() {
        result.push(node.to_string());
        if let Some(neighbors) = adj.get(node) {
            for &next in neighbors {
                let deg = in_deg.get_mut(next).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    // Insert in sorted position to keep deterministic order
                    let pos = queue.partition_point(|&x| x < next);
                    queue.insert(pos, next);
                }
            }
        }
    }

    assert_eq!(
        result.len(),
        names.len(),
        "Cycle in dependency graph: {:?}",
        names.iter().collect::<Vec<_>>()
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_order_by_removes_clause() {
        let sql = "SELECT * FROM t ORDER BY name ASC";
        assert_eq!(strip_order_by(sql), "SELECT * FROM t");
    }

    #[test]
    fn strip_order_by_also_strips_limit() {
        let sql = "SELECT * FROM t ORDER BY name ASC LIMIT 10";
        assert_eq!(strip_order_by(sql), "SELECT * FROM t");
    }

    #[test]
    fn strip_limit_without_order_by() {
        let sql = "SELECT * FROM t WHERE x = 1 LIMIT 10";
        assert_eq!(strip_order_by(sql), "SELECT * FROM t WHERE x = 1");
    }

    #[test]
    fn strip_limit_and_offset() {
        let sql = "SELECT * FROM t LIMIT 10 OFFSET 5";
        assert_eq!(strip_order_by(sql), "SELECT * FROM t");
    }

    #[test]
    fn strip_order_by_no_clause() {
        let sql = "SELECT * FROM t WHERE x = 1";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn strip_order_by_case_insensitive() {
        let sql = "SELECT * FROM t order by name";
        assert_eq!(strip_order_by(sql), "SELECT * FROM t");
    }

    #[test]
    fn offset_in_column_name_not_stripped() {
        let sql = "SELECT block_id, cursor_offset FROM current_editor_focus WHERE region = 'main'";
        assert_eq!(strip_order_by(sql), sql);
    }

    #[test]
    fn real_offset_clause_still_stripped() {
        let sql = "SELECT block_id, cursor_offset FROM t LIMIT 10 OFFSET 5";
        assert_eq!(strip_order_by(sql), "SELECT block_id, cursor_offset FROM t");
    }

    #[test]
    fn test_expr_references() {
        assert!(expr_references("is_task && priority > 0", "is_task"));
        assert!(expr_references("is_task", "is_task"));
        assert!(!expr_references("is_task_done", "is_task"));
        assert!(!expr_references("my_is_task", "is_task"));
        assert!(expr_references("a + is_task + b", "is_task"));
    }

    #[test]
    fn test_topo_sort_linear() {
        let names: HashSet<&str> = ["a", "b", "c"].into_iter().collect();
        // c depends on b, b depends on a
        let deps: HashMap<&str, Vec<&str>> = [("a", vec![]), ("b", vec!["a"]), ("c", vec!["b"])]
            .into_iter()
            .collect();
        let result = topo_sort_kahn(&names, &deps);
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_topo_sort_independent() {
        let names: HashSet<&str> = ["x", "y", "z"].into_iter().collect();
        let deps: HashMap<&str, Vec<&str>> = [("x", vec![]), ("y", vec![]), ("z", vec![])]
            .into_iter()
            .collect();
        let result = topo_sort_kahn(&names, &deps);
        // alphabetical since all independent
        assert_eq!(result, vec!["x", "y", "z"]);
    }

    #[test]
    #[should_panic(expected = "Cycle")]
    fn test_topo_sort_cycle() {
        let names: HashSet<&str> = ["a", "b"].into_iter().collect();
        let deps: HashMap<&str, Vec<&str>> =
            [("a", vec!["b"]), ("b", vec!["a"])].into_iter().collect();
        topo_sort_kahn(&names, &deps);
    }
}
