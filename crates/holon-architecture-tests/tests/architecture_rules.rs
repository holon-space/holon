//! Architecture boundary tests.
//!
//! These tests enforce structural rules from docs/Architecture.md:
//! - Crate dependency boundaries
//! - No platform-specific code in shared layers
//! - Parse, Don't Validate (no scattered string matching)
//! - No block_on in async contexts
//!
//! Suppress with `// ALLOW(<tag>): <reason>` comment on the same line or line above.
//!
//! Run via: just arch-test

use std::collections::HashMap;

/// Read a file relative to the architecture-tests crate root (which is in crates/).
fn read_file(path: &str) -> Option<String> {
    let full = format!("../../{}", path);
    std::fs::read_to_string(&full).ok()
}

/// Scan .rs files, returning (file, line, text) for each match.
fn scan_rs_files(
    dirs: &[&str],
    skip: &[&str],
    check: impl Fn(&str, &str, &[&str]) -> Vec<(usize, String)>,
) -> Vec<(String, usize, String)> {
    let mut violations = Vec::new();
    for dir in dirs {
        let base = format!("../../{}", dir);
        for entry in glob::glob(&format!("{base}/**/*.rs")).expect("valid glob") {
            let path = entry.expect("readable entry");
            let path_str = path.display().to_string();

            if skip.iter().any(|s| path_str.contains(s))
                || path_str.contains("/tests/")
                || path_str.contains("/pbt/")
                || path_str.ends_with("_test.rs")
                || path_str.ends_with("_pbt.rs")
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

            for (line, text) in check(&path_str, &source, &lines) {
                violations.push((path_str.clone(), line, text));
            }
        }
    }
    violations
}

fn has_allow(lines: &[&str], line_1indexed: usize, tag: &str) -> bool {
    let marker = format!("ALLOW({tag})");
    let idx = line_1indexed.saturating_sub(1);
    if let Some(l) = lines.get(idx) {
        if l.contains(&marker) {
            return true;
        }
    }
    if idx > 0 {
        if let Some(l) = lines.get(idx - 1) {
            if l.contains(&marker) {
                return true;
            }
        }
    }
    false
}

fn format_violations(violations: &[(String, usize, String)]) -> String {
    violations
        .iter()
        .map(|(file, line, text)| {
            let short = if text.len() > 120 {
                format!("{}...", &text[..120])
            } else {
                text.clone()
            };
            format!("  {}:{}: {}", file, line, short)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn orgmode_no_direct_loro_or_turso_imports() {
    let violations = scan_rs_files(
        &["crates/holon-orgmode/src"],
        &["di.rs"], // DI wiring layer is exempt — it's the boundary
        |_file, _source, lines| {
            let mut results = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                if (line.contains("use loro") || line.contains("loro::"))
                    && !has_allow(lines, i + 1, "loro")
                {
                    results.push((i + 1, line.trim().to_string()));
                }
                if (line.contains("use turso") || line.contains("turso::"))
                    && !has_allow(lines, i + 1, "turso")
                {
                    results.push((i + 1, line.trim().to_string()));
                }
            }
            results
        },
    );

    assert!(
        violations.is_empty(),
        "\nholon-orgmode must not import Loro or Turso directly (docs/Architecture.md §Crate Responsibilities).\n\
         Use traits from traits.rs; wire implementations in di.rs.\n\
         To suppress: `// ALLOW(loro): <reason>` or `// ALLOW(turso): <reason>`\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn frontend_cargo_no_provider_deps() {
    // Frontends should not depend directly on provider crates (holon-orgmode, holon-todoist).
    // Exception: holon-mcp-client is borderline (MCP frontend uses it), mcp frontend is special.
    let forbidden_deps = ["holon-orgmode", "holon-todoist"];
    // MCP frontend is exempt — it's a power-user tool that wires everything.
    // Flutter is exempt for now — it does DI wiring inline (legacy).
    let exempt_frontends = ["frontends/mcp", "frontends/flutter"];

    let cargo_files = [
        "frontends/gpui/Cargo.toml",
        "frontends/tui/Cargo.toml",
        "frontends/ply/Cargo.toml",
        "frontends/mcp/Cargo.toml",
        "frontends/flutter/rust/Cargo.toml",
    ];

    let mut violations = Vec::new();
    for cargo_path in &cargo_files {
        if exempt_frontends.iter().any(|e| cargo_path.starts_with(e)) {
            continue;
        }
        if let Some(content) = read_file(cargo_path) {
            for dep in &forbidden_deps {
                if content.contains(dep) {
                    violations.push((
                        cargo_path.to_string(),
                        0,
                        format!("depends on provider crate `{dep}`"),
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "\nFrontends should depend on holon-api + holon-frontend, not provider crates.\n\
         Provider crates (holon-orgmode, holon-todoist) should be wired via DI, not imported directly.\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_raw_sql_in_frontends() {
    let violations = scan_rs_files(
        &["frontends"],
        &[
            "frontends/mcp", // MCP frontend intentionally exposes raw DB access
        ],
        |_file, _source, lines| {
            let mut results = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let upper = line.to_uppercase();
                if (upper.contains("\"SELECT ")
                    || upper.contains("\"INSERT ")
                    || upper.contains("\"CREATE TABLE")
                    || upper.contains("\"DROP ")
                    || upper.contains("\"ALTER "))
                    && !has_allow(lines, i + 1, "sql")
                {
                    results.push((i + 1, line.trim().to_string()));
                }
            }
            results
        },
    );

    assert!(
        violations.is_empty(),
        "\nFrontends must not contain raw SQL (docs/Architecture.md: 'frontend is a pure render engine').\n\
         Move SQL to the backend (holon crate) and expose via BackendEngine methods.\n\
         MCP frontend is exempt. To suppress: `// ALLOW(sql): <reason>`\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn frontend_crate_no_platform_imports() {
    let platform_markers = [
        ("gpui::", "GPUI"),
        ("flutter_rust_bridge::", "Flutter"),
        ("dioxus::", "Dioxus"),
        ("ratatui::", "Ratatui"),
        ("crossterm::", "Crossterm"),
        ("winit::", "Winit"),
    ];

    let violations = scan_rs_files(
        &["crates/holon-frontend/src"],
        &[],
        |_file, _source, lines| {
            let mut results = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                for (marker, name) in &platform_markers {
                    if line.contains(marker) && !has_allow(lines, i + 1, "platform") {
                        results.push((
                            i + 1,
                            format!("imports platform-specific {name}: {}", line.trim()),
                        ));
                    }
                }
            }
            results
        },
    );

    assert!(
        violations.is_empty(),
        "\nholon-frontend must be platform-agnostic (docs/Architecture.md §MVVM Pattern).\n\
         Platform-specific code belongs in frontends/*, not in the shared ViewModel layer.\n\
         To suppress: `// ALLOW(platform): <reason>`\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_scattered_string_matching() {
    // Find match arms on .as_str() and group by the arm values.
    // If the same set of string arms appears in 3+ files, it should be an enum.
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::SupportLang;

    let lang: SupportLang = "rs".parse().unwrap();

    let mut arm_sets: HashMap<String, Vec<String>> = HashMap::new();

    let base = "../../crates";
    for entry in glob::glob(&format!("{base}/**/*.rs")).expect("valid glob") {
        let path = entry.expect("readable entry");
        let path_str = path.display().to_string();
        if path_str.contains("/tests/")
            || path_str.contains("architecture-tests")
            || path_str.contains("examples/")
        {
            continue;
        }

        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Find match expressions on .as_str()
        let root = lang.ast_grep(&source);
        for m in root.root().find_all("match $EXPR.as_str() { $$$ARMS }") {
            let text = m.text().to_string();
            // Extract string literal arms (the "foo" => parts)
            let mut arms: Vec<String> = Vec::new();
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(quote_start) = trimmed.find('"') {
                    if let Some(quote_end) = trimmed[quote_start + 1..].find('"') {
                        let arm_str = &trimmed[quote_start + 1..quote_start + 1 + quote_end];
                        arms.push(arm_str.to_string());
                    }
                }
            }
            if arms.len() >= 3 {
                arms.sort();
                let key = arms.join(",");
                arm_sets.entry(key).or_default().push(path_str.clone());
            }
        }
    }

    // Flag arm sets that appear in 3+ different files
    let mut violations = Vec::new();
    for (arms, files) in &arm_sets {
        let unique_files: Vec<&String> = {
            let mut seen = std::collections::HashSet::new();
            files.iter().filter(|f| seen.insert(f.as_str())).collect()
        };
        if unique_files.len() >= 3 {
            violations.push((
                unique_files[0].clone(),
                0,
                format!(
                    "match arms [{}] appear in {} files — should be an enum. Files: {}",
                    arms,
                    unique_files.len(),
                    unique_files
                        .iter()
                        .map(|f| f.rsplit('/').next().unwrap_or(f))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "\nParse, Don't Validate: match str.as_str() with the same arms in 3+ files\n\
         suggests the string should be a typed enum (docs/Architecture.md / CLAUDE.md §Parse Don't Validate).\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_underscore_prefixed_params() {
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::SupportLang;

    let lang: SupportLang = "rs".parse().unwrap();

    let violations = scan_rs_files(&["crates", "frontends"], &[], |_file, source, lines| {
        let root = lang.ast_grep(source);
        root.root()
            .dfs()
            .filter(|node| node.kind() == "parameter")
            .filter_map(|node| {
                let pattern = node.field("pattern")?;
                let name = pattern.text();
                if name.len() > 1
                    && name.starts_with('_')
                    && name.as_bytes()[1].is_ascii_alphabetic()
                {
                    let line = node.start_pos().line() + 1;
                    if !has_allow(lines, line, "unused_param") {
                        return Some((line, format!("parameter `{name}`")));
                    }
                }
                None
            })
            .collect()
    });

    assert!(
        violations.is_empty(),
        "\nFound underscore-prefixed function parameters that suppress unused-variable warnings.\n\
         Either use the parameter or remove it. If required by a trait, use `_` (bare discard).\n\
         To suppress: `// ALLOW(unused_param): <reason>`\n\n{}",
        format_violations(&violations)
    );
}

#[test]
fn no_block_on_in_async_context() {
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::SupportLang;

    let lang: SupportLang = "rs".parse().unwrap();

    let mut violations = Vec::new();

    for dir in &["crates", "frontends"] {
        let base = format!("../../{}", dir);
        for entry in glob::glob(&format!("{base}/**/*.rs")).expect("valid glob") {
            let path = entry.expect("readable entry");
            let path_str = path.display().to_string();
            if path_str.contains("/tests/")
                || path_str.contains("architecture-tests")
                || path_str.contains("examples/")
                || path_str.contains("integration-tests/")
                || path_str.ends_with("_pbt.rs")
                || path_str.contains("pbt_test")
                || path_str.contains("_test.rs")
                || path_str.contains("pbt_infrastructure")
                // Frontend main.rs / lib.rs uses block_on as the entry point — not inside async
                || path_str.ends_with("/main.rs")
                || (path_str.contains("frontends/") && path_str.ends_with("/lib.rs"))
            {
                continue;
            }

            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let lines: Vec<&str> = source.lines().collect();

            // Find block_on calls inside async fn bodies
            let root = lang.ast_grep(&source);
            for m in root.root().find_all("$RT.block_on($FUTURE)") {
                let line = m.start_pos().line() + 1;
                if has_allow(&lines, line, "block_on") {
                    continue;
                }

                // Check if this is inside an async fn by scanning backwards
                let idx = line.saturating_sub(1);
                let preceding = &lines[..idx.min(lines.len())];
                let in_async = preceding
                    .iter()
                    .rev()
                    .take(50) // look back at most 50 lines
                    .any(|l| {
                        l.contains("async fn")
                            || l.contains("async move")
                            || l.contains("tokio::spawn")
                    });

                if in_async {
                    violations.push((
                        path_str.clone(),
                        line,
                        lines.get(idx).unwrap_or(&"").trim().to_string(),
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "\nFound block_on() inside async context — this causes deadlocks.\n\
         Use `.await` instead, or restructure to avoid sync/async bridge.\n\
         To suppress: `// ALLOW(block_on): <reason>`\n\n{}",
        format_violations(&violations)
    );
}

/// Forbids reintroducing the global GPUI/Loro `FocusRegistry`-style cell
/// (`Arc<RwLock<HashMap<String, Entity<_>>>>`) at app/model level. The
/// per-block reactive primitives (`UiState.focused_block`,
/// `services.watch_editor_cursor`, `LocalEntityScope::get_or_create`)
/// replace it; see the FocusRegistry-removal handoff in MEMORY.md.
///
/// Suppress with `// ALLOW(global_registry): <reason>`.
#[test]
fn no_global_entity_registries() {
    let needle = "Arc<RwLock<HashMap<String, Entity<";
    let alt = "Arc<std::sync::RwLock<HashMap<String, Entity<";

    let violations = scan_rs_files(&["crates", "frontends"], &[], |_file, _source, lines| {
        let mut hits = Vec::new();
        for (i, raw) in lines.iter().enumerate() {
            let line = i + 1;
            if (raw.contains(needle) || raw.contains(alt))
                && !has_allow(lines, line, "global_registry")
            {
                hits.push((line, raw.trim().to_string()));
            }
        }
        hits
    });

    assert!(
        violations.is_empty(),
        "\nFound global mutable entity registry — exactly the `FocusRegistry`-style \
         pattern that was removed.\n\
         Decentralize via per-row reactive primitives instead \
         (`UiState.focused_block`, `LocalEntityScope::get_or_create`, ...).\n\
         To suppress: `// ALLOW(global_registry): <reason>`\n\n{}",
        format_violations(&violations)
    );
}

/// Forbids `as_string()` / `as_string_owned()` on values pulled from columns
/// declared `#[jsonb]`. CDC delivers jsonb columns as `Value::Array` /
/// `Value::Json` / `Value::Null`, never `Value::String`, so the call always
/// returns `None` and silently misclassifies rows. This is the bug class that
/// caused the BulkExternalAdd page-overcount race (May 2026).
///
/// Suppress with `// ALLOW(jsonb_as_string): <reason>` — only legitimate when
/// the field name collides with a non-jsonb column on a different entity.
#[test]
fn no_as_string_on_jsonb_columns() {
    use ast_grep_core::tree_sitter::LanguageExt;
    use ast_grep_language::SupportLang;

    let lang: SupportLang = "rs".parse().unwrap();

    // Harvest jsonb field names by scanning for `#[jsonb]\n    pub <name>:`.
    let mut jsonb_fields: std::collections::BTreeSet<String> = Default::default();
    for entry in glob::glob("../../crates/**/*.rs").expect("valid glob") {
        let path = entry.expect("readable entry");
        let path_str = path.display().to_string();
        if path_str.contains("architecture-tests") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = source.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.trim_start().starts_with("#[jsonb]") {
                continue;
            }
            // Next non-attribute line should be `pub <name>: <type>,`
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_start().starts_with("#[") {
                j += 1;
            }
            let Some(decl) = lines.get(j) else { continue };
            let decl = decl.trim_start();
            let body = decl.strip_prefix("pub ").unwrap_or(decl);
            if let Some(colon) = body.find(':') {
                let name = body[..colon].trim().to_string();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    jsonb_fields.insert(name);
                }
            }
        }
    }

    assert!(
        !jsonb_fields.is_empty(),
        "Expected to discover at least one #[jsonb] field; harvest is broken"
    );

    let violations = scan_rs_files(&["crates", "frontends"], &[], |_file, source, lines| {
        let root = lang.ast_grep(source);
        let mut hits = Vec::new();
        // Patterns like `.get("KEY").and_then(|...| ...as_string())` and
        // chained `.get("KEY").something.as_string()`.
        let patterns = [
            "$ROW.get($KEY).and_then($CB)",
            "$ROW.get($KEY).map($CB)",
            "$ROW.get($KEY).map_or($DEFAULT, $CB)",
            "$ROW.get($KEY).and_then(|v| v.as_string())",
            "$ROW.get($KEY).and_then(|v| v.as_string_owned())",
        ];
        for pattern in patterns {
            for m in root.root().find_all(pattern) {
                let text = m.text();
                // Must reference as_string in the same expression
                if !text.contains("as_string") {
                    continue;
                }
                let key_node = match m.get_env().get_match("KEY") {
                    Some(n) => n,
                    None => continue,
                };
                let key_text = key_node.text();
                let stripped = key_text.trim_matches('"');
                if !jsonb_fields.contains(stripped) {
                    continue;
                }
                let line = m.start_pos().line() + 1;
                if has_allow(lines, line, "jsonb_as_string") {
                    continue;
                }
                hits.push((line, format!("get({}) + as_string", key_text)));
            }
        }
        hits
    });

    assert!(
        violations.is_empty(),
        "\nFound `as_string()`/`as_string_owned()` on a `#[jsonb]` column.\n\
         CDC delivers jsonb values as Value::Array / Value::Json / Value::Null —\n\
         never Value::String — so the call always returns None and silently\n\
         misclassifies rows. Parse the JSON shape explicitly instead.\n\
         jsonb fields detected: {:?}\n\
         To suppress: `// ALLOW(jsonb_as_string): <reason>`\n\n{}",
        jsonb_fields,
        format_violations(&violations)
    );
}

/// Forbids `HANDOFF_*.md` files at the holon repo root.
///
/// Root-level HANDOFF docs rot: they go stale, lose context, and duplicate
/// project memory. Per AC-7 (MVP Definition.org), handoff knowledge belongs
/// as topic-anchored sub-blocks in holon-pkm, tagged `:handoff:active:`, e.g.
///   holon-pkm/Projects/Holon/Engine Foundations.org
///   holon-pkm/Projects/Holon/Frontends/GPUI.org
///   holon-pkm/Projects/Holon/Frontends/TUI.org
///
/// Topic-anchored blocks are queryable, taggable, and live alongside the work
/// they describe — root HANDOFF files are none of those things.
#[test]
fn no_handoff_md_at_repo_root() {
    // CARGO_MANIFEST_DIR is crates/holon-architecture-tests — two levels up is the repo root.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR must have at least two ancestor directories");

    let offenders: Vec<String> = std::fs::read_dir(repo_root)
        .expect("failed to read repo root directory")
        .filter_map(|entry| {
            let entry = entry.expect("failed to read directory entry");
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if name_str.starts_with("HANDOFF_") && name_str.ends_with(".md") {
                Some(name_str)
            } else {
                None
            }
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "\nFound HANDOFF_*.md files at the repo root — these are forbidden (AC-7).\n\
         \n\
         Offending files:\n{}\n\
         \n\
         Where they belong: add a sub-block under the relevant topic doc in\n\
         holon-pkm/Projects/Holon/ (e.g. Engine Foundations.org, Frontends/GPUI.org,\n\
         Frontends/TUI.org, ...) tagged :handoff:active:.\n\
         \n\
         Rationale: HANDOFF docs at the repo root rot — they get stale, lose context,\n\
         and duplicate project memory. Topic-anchored sub-blocks are queryable,\n\
         taggable, and live alongside the work they describe.",
        offenders
            .iter()
            .map(|f| format!("  - {}", f))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
