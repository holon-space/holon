//! Architecture tests: detect defensive programming patterns that swallow errors.
//!
//! These tests use ast-grep as a library to parse Rust source files and match
//! AST patterns that violate our "no defensive programming" principle:
//! - Errors must propagate, not be hidden
//! - `.ok()` on Result is almost always wrong
//! - `Err(e) => { log; continue }` loses data silently
//!
//! To suppress a violation, add a comment on the same line or the line above:
//!     // ALLOW(ok): <reason>
//!     // ALLOW(filter_map_ok): <reason>
//!     // ALLOW(unwrap_or_default): <reason>
//!
//! Run via: just arch-test

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::SupportLang;

fn rust_lang() -> SupportLang {
    "rs".parse().unwrap()
}

/// Scan all .rs files under the given directories, applying `check` to each.
/// Returns a list of (file, line, matched_text) violations.
fn scan_files(
    dirs: &[&str],
    check: impl Fn(&str, &str, &[&str]) -> Vec<(usize, usize, String)>,
) -> Vec<(String, usize, String)> {
    let mut violations = Vec::new();
    for dir in dirs {
        let base = format!("../../{}", dir);
        for entry in glob::glob(&format!("{base}/**/*.rs")).expect("valid glob") {
            let path = entry.expect("readable entry");
            let path_str = path.display().to_string();

            // Skip test files and test infrastructure
            if path_str.contains("/tests/")
                || path_str.contains("/pbt/")
                || path_str.ends_with("_test.rs")
                || path_str.ends_with("_pbt.rs")
                || path_str.contains("pbt_test")
                || path_str.contains("architecture-tests")
                || path_str.contains("examples/")
                || path_str.contains("integration-tests/")
            {
                continue;
            }

            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let lines: Vec<&str> = source.lines().collect();

            for (start_line, _end_line, text) in check(&path_str, &source, &lines) {
                violations.push((path_str.clone(), start_line, text));
            }
        }
    }
    violations
}

/// Use ast-grep to find all matches of a pattern in source code.
/// Returns (start_line, end_line, matched_text) triples (1-indexed).
fn ast_grep_find(source: &str, pattern: &str) -> Vec<(usize, usize, String)> {
    let lang = rust_lang();
    let root = lang.ast_grep(source);
    root.root()
        .find_all(pattern)
        .map(|m| {
            let start = m.start_pos().line() + 1;
            let end = m.end_pos().line() + 1;
            (start, end, m.text().to_string())
        })
        .collect()
}

/// Check if any line in the match range (or the line above the start) has an ALLOW comment.
fn has_allow(lines: &[&str], start_1indexed: usize, end_1indexed: usize, tag: &str) -> bool {
    let marker = format!("ALLOW({tag})");
    let start_idx = start_1indexed.saturating_sub(1); // 0-indexed

    // Check the line above the match start
    if start_idx > 0 {
        if let Some(l) = lines.get(start_idx - 1) {
            if l.contains(&marker) {
                return true;
            }
        }
    }

    // Check every line in the match range
    for idx in start_idx..end_1indexed {
        if let Some(l) = lines.get(idx) {
            if l.contains(&marker) {
                return true;
            }
        }
    }
    false
}

/// Universal patterns that are always acceptable and don't need `ALLOW` comments.
/// These are so common and well-understood that annotating each one would be noise.
fn is_universally_allowed_ok(file: &str, text: &str) -> bool {
    let t = text.trim();
    // writeln!/write! on String buffers (infallible — String::write_fmt never fails)
    t.contains("writeln!") || t.contains("write!")
    // OnceLock::set() / slot.set() (Err means already set — expected race)
        || t.contains(".set(")
    // tx.send() (Err means no receivers — valid lifecycle)
        || t.contains(".send(")
    // env vars — missing is expected, not an error
        || t.contains("env::var")
    // DI optional resolution — service may not be registered
        || t.contains("try_resolve")
    // build.rs / proc-macro code — compile-time, different rules
        || file.contains("build.rs") || file.contains("holon-macros/")
    // theme hex parsing — cosmetic
        || file.contains("theme.rs")
}

/// Universal patterns for filter_map+ok that don't need `ALLOW` comments.
fn is_universally_allowed_filter_map_ok(file: &str, text: &str) -> bool {
    // build.rs / proc-macro — compile-time code
    file.contains("build.rs") || file.contains("holon-macros/")
}

fn format_violations(violations: &[(String, usize, String)]) -> String {
    let mut out = String::new();
    for (file, line, text) in violations {
        let text_oneline = text.replace('\n', " ");
        let text_short = if text_oneline.len() > 120 {
            format!("{}...", &text_oneline[..120])
        } else {
            text_oneline
        };
        out.push_str(&format!("  {}:{}: {}\n", file, line, text_short));
    }
    out
}

// ============================================================================
// Tests
// ============================================================================

const SCAN_DIRS: &[&str] = &["crates", "frontends"];

#[test]
fn no_result_dot_ok_in_production_code() {
    let violations = scan_files(SCAN_DIRS, |file, source, lines| {
        ast_grep_find(source, "$EXPR.ok()")
            .into_iter()
            .filter(|(_, _, text)| !is_universally_allowed_ok(file, text))
            .filter(|(start, end, _)| !has_allow(lines, *start, *end, "ok"))
            .collect()
    });

    assert!(
        violations.is_empty(),
        "\nFound .ok() calls that silently discard errors. \
         Use `?`, `.context()`, or `.expect()` instead.\n\
         To suppress, add `// ALLOW(ok): <reason>` on the same or preceding line.\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_filter_map_ok_in_production_code() {
    let violations = scan_files(SCAN_DIRS, |file, source, lines| {
        ast_grep_find(source, "$ITER.filter_map($CLOSURE)")
            .into_iter()
            .filter(|(_, _, text)| text.contains(".ok()"))
            .filter(|(start, end, _)| !has_allow(lines, *start, *end, "filter_map_ok"))
            .filter(|(_, _, text)| !is_universally_allowed_filter_map_ok(file, text))
            .collect()
    });

    assert!(
        violations.is_empty(),
        "\nFound filter_map(|x| ...ok()) that silently drops errors from iterators. \
         Use `.map(...).collect::<Result<Vec<_>, _>>()?` instead.\n\
         To suppress, add `// ALLOW(filter_map_ok): <reason>` on the same or preceding line.\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_unwrap_or_default_on_deserialize() {
    let violations = scan_files(SCAN_DIRS, |_file, source, lines| {
        let mut results = Vec::new();
        results.extend(ast_grep_find(
            source,
            "serde_json::from_str($X).unwrap_or_default()",
        ));
        results.extend(ast_grep_find(
            source,
            "serde_json::from_str($X).ok().unwrap_or_default()",
        ));
        results.extend(ast_grep_find(
            source,
            "serde_json::from_value($X).unwrap_or_default()",
        ));
        results
            .into_iter()
            .filter(|(start, end, _)| !has_allow(lines, *start, *end, "unwrap_or_default"))
            .collect()
    });

    assert!(
        violations.is_empty(),
        "\nFound serde_json deserialization with unwrap_or_default() — \
         corrupt data will silently become empty defaults. \
         Use `.expect()` or `?` instead.\n\
         To suppress, add `// ALLOW(unwrap_or_default): <reason>`.\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_catch_unwind_at_debug_level() {
    let violations = scan_files(SCAN_DIRS, |_file, source, _lines| {
        let mut results = Vec::new();
        for (i, line) in source.lines().enumerate() {
            if line.contains("catch_unwind") {
                let window: String = source
                    .lines()
                    .skip(i)
                    .take(15)
                    .collect::<Vec<_>>()
                    .join("\n");
                if (window.contains("tracing::debug!(") || window.contains("debug!("))
                    && (window.contains("panic") || window.contains("Caught"))
                {
                    results.push((i + 1, i + 1, line.trim().to_string()));
                }
            }
        }
        results
    });

    assert!(
        violations.is_empty(),
        "\nFound catch_unwind with debug!-level logging of panics. \
         Swallowed panics must be logged at error! level.\n\n{}",
        format_violations(&violations)
    );
}
