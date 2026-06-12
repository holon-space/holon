//! Small utility functions shared across Holon crates.

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

    let computed: HashSet<&str> = result.iter().map(|s| s.as_str()).collect();
    for name in names {
        if !computed.contains(*name) {
            panic!(
                "Dependency cycle detected: {} is part of a cycle ({:?})",
                name,
                in_deg
                    .iter()
                    .filter(|(_, d)| **d > 0)
                    .map(|(n, _)| n)
                    .collect::<Vec<_>>()
            );
        }
    }

    result
}
