//! Ingest → write-back over a REAL vault must not touch a single byte.
//!
//! Every synthetic corpus in this repo is written by someone who already knows
//! which shapes are interesting. A live vault is not: it carries the shapes
//! nobody thought to generate — protective marks over raw link syntax, links
//! whose labels contain identifiers, prose that happens to look like markup.
//! Three rounds of review missed two data-destroying regressions that this
//! simulation surfaces immediately.
//!
//! Ignored by default (needs a vault). Point it at a COPY:
//!
//! ```text
//! HOLON_VAULT_SIM=/path/to/vault-copy \
//!   cargo test -p holon-org-format --test vault_writeback_stability -- --ignored --nocapture
//! ```

use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

fn org_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "org") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// One write-back pass: parse the file, render the parsed blocks back, and
/// return the bytes. This is the store→disk half of the sync loop.
fn write_back(path: &Path, source: &str, root: &Path) -> anyhow::Result<String> {
    let parsed = parse_org_file(path, source, &EntityUri::no_parent(), root)?;
    Ok(OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        path,
        &parsed.document.id,
    ))
}

fn changed_lines(before: &str, after: &str) -> Vec<(usize, String, String)> {
    let b: Vec<&str> = before.lines().collect();
    let a: Vec<&str> = after.lines().collect();
    let mut out = Vec::new();
    for i in 0..b.len().max(a.len()) {
        let lb = b.get(i).copied().unwrap_or("<missing>");
        let la = a.get(i).copied().unwrap_or("<missing>");
        if lb != la {
            out.push((i + 1, lb.to_string(), la.to_string()));
        }
    }
    out
}

/// Files that already moved under write-back BEFORE any inline-markup work,
/// with the reason. Measured by A/B: with content emission reduced to raw
/// passthrough (pre-task-#67 behavior) these same lines still differ, so they
/// are not caused by the quoting.
///
/// The one entry is `:PROPERTIES:` drawer KEY ORDER —
/// `format_properties_drawer` sorts keys while the file on disk holds them in
/// insertion order. A different subsystem and a separate fix.
const KNOWN_PRE_EXISTING_CHURN: &[(&str, usize)] =
    &[("Agents/citrix/citrix-STX.BROWSER_AGENT.org", 3)];

fn pre_existing_allowance(path: &Path) -> usize {
    KNOWN_PRE_EXISTING_CHURN
        .iter()
        .find(|(suffix, _)| path.to_string_lossy().ends_with(suffix))
        .map(|(_, lines)| *lines)
        .unwrap_or(0)
}

#[test]
#[ignore = "needs a vault: set HOLON_VAULT_SIM to a COPY of one"]
fn vault_is_byte_stable_under_writeback() {
    let root = PathBuf::from(
        std::env::var("HOLON_VAULT_SIM").expect("set HOLON_VAULT_SIM to a vault COPY"),
    );
    let files = org_files(&root);
    assert!(!files.is_empty(), "no .org files under {}", root.display());

    let mut unstable = 0usize;
    let mut unparseable = 0usize;
    let mut total_changed_lines = 0usize;
    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => panic!("reading {}: {e}", path.display()),
        };
        let rendered = match write_back(path, &source, &root) {
            Ok(r) => r,
            Err(e) => {
                // A file that no longer parses is a different defect class;
                // count and report it rather than folding it into stability.
                unparseable += 1;
                println!("UNPARSEABLE {}: {e}", path.display());
                continue;
            }
        };
        let diff = changed_lines(&source, &rendered);
        if !diff.is_empty() {
            let allowed = pre_existing_allowance(path);
            let counted = diff.len().saturating_sub(allowed);
            let tag = if counted == 0 { "KNOWN" } else { "UNSTABLE" };
            unstable += usize::from(counted > 0);
            total_changed_lines += counted;
            println!("{tag} {} ({} lines)", path.display(), diff.len());
            for (line, before, after) in diff.iter().take(6) {
                println!("  L{line}\n    disk    {before:?}\n    written {after:?}");
            }
        }
    }
    println!(
        "\nvault write-back: {} files, {unstable} unstable ({total_changed_lines} changed lines), \
         {unparseable} unparseable",
        files.len()
    );
    assert_eq!(
        total_changed_lines,
        0,
        "write-back must not change a single vault line ({unstable} of {} files moved)",
        files.len()
    );
}
