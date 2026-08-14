//! Architecture test over every production `WITH RECURSIVE` CTE.
//!
//! Our Turso fork mis-executes several recursive-CTE shapes. Production is
//! unexposed only because every production CTE happens to avoid them; nothing
//! enforced that, so a hand-written `holon_sql` block could silently return
//! wrong data or abort the process. This test makes the accident deliberate.
//!
//! Two properties are enforced here, both measured against the engine at our
//! pin (`lane-logs/research-j1p-unpark.md`, B-audit):
//!
//! - **Single-arm base case.** A multi-arm base case (`VALUES(1),(2)`) makes
//!   `UNION ALL` drop distinct rows and makes recursion run to the 100000
//!   iteration abort even when the recursive arm cannot produce rows.
//! - **Join-bearing recursive arm.** When the recursive arm selects from the
//!   CTE alone, a `||` over a bare column freezes at the column's own name
//!   (silent wrong data), and a qualified self-reference panics the process in
//!   `translate/expr/translator.rs` — a hard abort, not a returned `Err`. A
//!   tree walk inherently joins a base table, which is why no production CTE
//!   has the join-free shape.
//!
//! A third property is deliberately **not** enforced yet — see
//! `a_recursive_arm_never_left_joins_on_its_own_base_row` below. Do not
//! "finish" it by widening either test above into a blanket ban on `LEFT JOIN`
//! in the recursive arm: four of the six production CTEs contain one and are
//! measured safe, so that ban is a false red. The narrow rule is the correct
//! one, and it is red on `turso_seams.rs` today.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

/// A recursive CTE found in production source, normalised to one line.
struct FoundCte {
    file: PathBuf,
    line: usize,
    name: String,
    /// The two arms of the CTE body, split at the top-level `UNION [ALL]`.
    arms: Vec<String>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("holon-turso sits two levels below the workspace root")
        .to_path_buf()
}

/// Production source only: `src/` trees plus the checked-in schema SQL.
/// `tests/`, `examples/` and `benches/` carry deliberate defect reproducers,
/// which must not be mistaken for production shapes.
fn production_files(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.join("crates/holon-turso/sql")];
    for group in ["crates", "frontends"] {
        let dir = root.join(group);
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for entry in entries {
            let src = entry.expect("readable dir entry").path().join("src");
            if src.is_dir() {
                roots.push(src);
            }
        }
    }

    let mut files = Vec::new();
    for r in roots {
        if r.is_dir() {
            collect(&r, &mut files);
        }
    }
    files.sort();
    files
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs") | Some("sql")
        ) {
            out.push(path);
        }
    }
}

/// Byte ranges covered by `#[cfg(test)]` items, whose SQL is test fixture text.
fn cfg_test_ranges(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut search = 0;
    while let Some(rel) = text[search..].find("#[cfg(test)]") {
        let attr = search + rel;
        let open = match text[attr..].find('{') {
            Some(o) => attr + o,
            None => break,
        };
        let mut depth = 0usize;
        let mut end = open;
        for (i, b) in bytes[open..].iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        ranges.push((attr, end));
        search = end.max(attr + 1);
    }
    ranges
}

/// Undo Rust string-literal escaping so the SQL can be parsed as SQL.
fn unescape_rust(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // A line continuation: backslash, newline, then the indent.
            Some('\n') => {
                chars.next();
                while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                    chars.next();
                }
            }
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('"') => {
                chars.next();
                out.push('"');
            }
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

fn strip_sql_comments(sql: &str) -> String {
    sql.lines()
        .map(|l| match l.find("--") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalise_whitespace(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The parenthesised body of the CTE that starts at `WITH RECURSIVE` in `sql`,
/// which must already be comment-free and whitespace-normalised.
fn cte_name_and_body(sql: &str) -> (String, String) {
    let after = &sql["WITH RECURSIVE ".len()..];
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    assert!(!name.is_empty(), "recursive CTE has no name: {after:.120}");

    // `name(col, col) AS (…)` — the column list is not the body.
    let mut cursor = name.len();
    while after[cursor..].starts_with(' ') {
        cursor += 1;
    }
    if after[cursor..].starts_with('(') {
        cursor += balanced_len(&after[cursor..])
            .unwrap_or_else(|| panic!("recursive CTE {name} has an unbalanced column list"));
        while after[cursor..].starts_with(' ') {
            cursor += 1;
        }
    }
    assert!(
        after[cursor..].to_uppercase().starts_with("AS"),
        "recursive CTE {name} is not followed by AS — the scanner matched prose, \
         not SQL: {after:.200}"
    );

    let open = cursor
        + after[cursor..]
            .find('(')
            .unwrap_or_else(|| panic!("recursive CTE {name} has no body: {after:.200}"));
    let bytes = after.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return (name, after[open + 1..open + i].to_string());
                }
            }
            _ => {}
        }
    }
    panic!("recursive CTE {name} body is unbalanced: {after:.200}");
}

/// Byte length of the balanced parenthesised group starting at `s[0] == '('`.
fn balanced_len(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in s.as_bytes().iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split the body at every top-level `UNION` / `UNION ALL`.
fn split_arms(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let upper = body.to_uppercase();
    let mut arms = Vec::new();
    let mut depth = 0usize;
    let mut arm_start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && upper[i..].starts_with("UNION") && is_word_boundary(&upper, i, 5) {
            arms.push(body[arm_start..i].to_string());
            let rest = &upper[i + 5..];
            let skip = if rest.trim_start().starts_with("ALL")
                && is_word_boundary(&upper, i + 5 + rest.find("ALL").unwrap_or(0), 3)
            {
                5 + rest.find("ALL").unwrap_or(0) + 3
            } else {
                5
            };
            i += skip;
            arm_start = i;
            continue;
        }
        i += 1;
    }
    arms.push(body[arm_start..].to_string());
    arms
}

fn is_word_boundary(upper: &str, at: usize, len: usize) -> bool {
    let before = upper[..at].chars().next_back();
    let after = upper[at + len..].chars().next();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    !word(before) && !word(after)
}

/// Every identifier that appears as a `FROM <t>` or `JOIN <t>` target at the
/// top level of `arm`.
fn table_refs(arm: &str) -> BTreeSet<String> {
    let upper = arm.to_uppercase();
    let bytes = arm.as_bytes();
    let mut refs = BTreeSet::new();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            for (kw, len) in [("FROM", 4usize), ("JOIN", 4)] {
                if upper[i..].starts_with(kw) && is_word_boundary(&upper, i, len) {
                    let rest = arm[i + len..].trim_start();
                    let ident: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '{' || *c == '}')
                        .collect();
                    if !ident.is_empty() {
                        refs.insert(ident.to_lowercase());
                    }
                }
            }
        }
        i += 1;
    }
    refs
}

/// Prose in a `//` comment that merely *mentions* a recursive CTE is not one.
fn in_rust_comment(text: &str, at: usize) -> bool {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..at].contains("//")
}

fn find_production_ctes() -> Vec<FoundCte> {
    let root = repo_root();
    let mut found = Vec::new();
    for file in production_files(&root) {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let excluded = if file.extension().is_some_and(|e| e == "rs") {
            cfg_test_ranges(&text)
        } else {
            Vec::new()
        };
        let upper = text.to_uppercase();

        let mut search = 0;
        while let Some(rel) = upper[search..].find("WITH RECURSIVE") {
            let at = search + rel;
            search = at + 14;
            if excluded.iter().any(|(s, e)| at >= *s && at <= *e) || in_rust_comment(&text, at) {
                continue;
            }
            let window = &text[at..(at + 8000).min(text.len())];
            let sql = normalise_whitespace(&strip_sql_comments(&unescape_rust(window)));
            let (name, body) = cte_name_and_body(&sql);
            found.push(FoundCte {
                file: file.strip_prefix(&root).unwrap_or(&file).to_path_buf(),
                line: text[..at].matches('\n').count() + 1,
                name,
                arms: split_arms(&body),
            });
        }
    }
    found
}

/// The scanner silently returning nothing would turn every assertion below
/// into a vacuous pass, so the known inventory is pinned by count.
const KNOWN_PRODUCTION_CTES: usize = 6;

#[test]
fn every_production_recursive_cte_has_a_single_arm_base_and_a_join_bearing_recursive_arm() {
    let ctes = find_production_ctes();
    assert!(
        ctes.len() >= KNOWN_PRODUCTION_CTES,
        "found only {} production recursive CTEs, expected at least {KNOWN_PRODUCTION_CTES} — \
         the scanner is broken, not the code. Found: {:?}",
        ctes.len(),
        ctes.iter().map(|c| &c.name).collect::<Vec<_>>()
    );

    let mut violations = Vec::new();
    for cte in &ctes {
        let where_ = format!("{}:{} CTE `{}`", cte.file.display(), cte.line, cte.name);

        if cte.arms.len() != 2 {
            violations.push(format!(
                "{where_}: body splits into {} arms at the top-level UNION, expected 2 (one base, \
                 one recursive) — a multi-arm base case makes UNION ALL drop distinct rows and \
                 drives recursion to the 100000-iteration abort",
                cte.arms.len()
            ));
            continue;
        }

        let recursive = &cte.arms[1];
        let refs = table_refs(recursive);
        let joins_base_table = refs.iter().any(|r| *r != cte.name.to_lowercase());
        if !joins_base_table {
            violations.push(format!(
                "{where_}: the recursive arm selects from the CTE alone (refs: {refs:?}). A \
                 join-free recursive arm freezes `||` expressions at the column's own name and \
                 panics the process on a qualified self-reference. Join a base table."
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "production recursive CTEs violate the shape that keeps the Turso \
         recursive-CTE defects unreachable:\n  {}",
        violations.join("\n  ")
    );
}

/// NOT YET ENFORCED — `#[ignore]` is deliberate and tracked by task #22.
///
/// A recursive arm that `LEFT JOIN`s on the base-table row that arm itself
/// produces never terminates at the fork head (`a94102c2`). Measured trigger,
/// bisected: the `IS NULL` anti-join predicate is irrelevant (it hangs with no
/// predicate at all), an `INNER JOIN` in the same position is fine, a
/// `LEFT JOIN` whose `ON` references the CTE row is fine, and the arm's driving
/// table does not matter. Our pin `54f3cc5` does not have the defect, so this
/// is a regression that only bites on re-pin.
///
/// `turso_seams.rs` violates this today, at both the `get_blocks` walk and the
/// doc shape gate. Un-ignore this test as part of the re-pin, once those two
/// are rewritten to `LEFT JOIN` on the CTE row.
#[test]
#[ignore = "red on turso_seams.rs until the re-pin rewrites it (task #22)"]
fn a_recursive_arm_never_left_joins_on_its_own_base_row() {
    let ctes = find_production_ctes();
    assert!(ctes.len() >= KNOWN_PRODUCTION_CTES, "scanner is broken");

    let mut violations = Vec::new();
    for cte in &ctes {
        if cte.arms.len() != 2 {
            continue;
        }
        let recursive = &cte.arms[1];
        for (alias, on) in left_join_targets(recursive) {
            let cte_lower = cte.name.to_lowercase();
            let references_cte = on
                .to_lowercase()
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| {
                    w == cte_lower || cte_aliases(recursive, &cte_lower).contains(&w.to_string())
                });
            if !references_cte {
                violations.push(format!(
                    "{}:{} CTE `{}`: recursive arm LEFT JOINs `{alias}` on the base row it \
                     produces (`ON {on}`) — this never terminates at the fork head",
                    cte.file.display(),
                    cte.line,
                    cte.name
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "recursive arms LEFT JOIN on their own base row:\n  {}",
        violations.join("\n  ")
    );
}

/// `(alias, on-clause)` for every top-level `LEFT JOIN` in `arm`.
fn left_join_targets(arm: &str) -> Vec<(String, String)> {
    let upper = arm.to_uppercase();
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(rel) = upper[search..].find("LEFT JOIN ") {
        let at = search + rel;
        search = at + 10;
        let rest = &arm[at + 10..];
        let alias: String = rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let on = match rest.to_uppercase().find(" ON ") {
            Some(i) => {
                let tail = &rest[i + 4..];
                let end = tail
                    .to_uppercase()
                    .find(" WHERE ")
                    .or_else(|| tail.to_uppercase().find(" LEFT JOIN "))
                    .or_else(|| tail.to_uppercase().find(" JOIN "))
                    .unwrap_or(tail.len());
                tail[..end].to_string()
            }
            None => String::new(),
        };
        out.push((alias, on));
    }
    out
}

/// Aliases bound to the CTE inside `arm` (`FROM descendants d` binds `d`).
fn cte_aliases(arm: &str, cte_lower: &str) -> Vec<String> {
    let lower = arm.to_lowercase();
    let mut aliases = vec![cte_lower.to_string()];
    let mut search = 0;
    while let Some(rel) = lower[search..].find(cte_lower) {
        let at = search + rel;
        search = at + cte_lower.len();
        let rest = lower[at + cte_lower.len()..].trim_start();
        let word: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !word.is_empty() && !matches!(word.as_str(), "on" | "where" | "join" | "as" | "union") {
            aliases.push(word);
        }
    }
    aliases
}
