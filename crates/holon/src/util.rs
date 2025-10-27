use std::collections::{HashMap, HashSet, VecDeque};

/// Strip ORDER BY clause from SQL (Turso materialized views don't support it).
///
/// Uses case-insensitive search without allocating an uppercase copy when no
/// ORDER BY is present.
pub fn strip_order_by(sql: &str) -> String {
    let order_idx = sql
        .as_bytes()
        .windows(8)
        .position(|w| w.eq_ignore_ascii_case(b"ORDER BY"));

    let Some(order_idx) = order_idx else {
        return sql.to_string();
    };

    let rest_upper: String = sql[order_idx..].to_uppercase();
    let end_idx = rest_upper
        .find("LIMIT")
        .or_else(|| rest_upper.find("OFFSET"))
        .unwrap_or(rest_upper.len());

    let mut result = sql[..order_idx].to_string();
    result.push_str(&sql[order_idx + end_idx..]);
    result.trim().to_string()
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
    fn strip_order_by_preserves_limit() {
        let sql = "SELECT * FROM t ORDER BY name ASC LIMIT 10";
        assert_eq!(strip_order_by(sql), "SELECT * FROM t LIMIT 10");
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
