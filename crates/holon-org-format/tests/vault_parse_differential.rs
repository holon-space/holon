//! Dumps what the parser reads from every org file of a vault: per block its
//! title, tags and drawer properties, one JSON line each. Two dumps taken
//! with two builds of this crate diff into the exact set of items a parser
//! change moves.
//!
//! Ignored by default (needs a vault). The vault is only read:
//!
//! ```text
//! HOLON_VAULT_SIM=/path/to/vault HOLON_VAULT_DUMP=/tmp/parse.jsonl \
//!   cargo test -p holon-org-format --test vault_parse_differential -- --ignored
//! ```

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_org_format::OrgBlockExt;
use holon_org_format::parse_org_file;

fn org_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .flatten()
        {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
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

#[test]
#[ignore = "needs HOLON_VAULT_SIM and HOLON_VAULT_DUMP"]
fn dump_parsed_vault() {
    let root = PathBuf::from(std::env::var("HOLON_VAULT_SIM").expect("HOLON_VAULT_SIM"));
    let out = PathBuf::from(std::env::var("HOLON_VAULT_DUMP").expect("HOLON_VAULT_DUMP"));
    let mut sink = std::io::BufWriter::new(
        std::fs::File::create(&out).unwrap_or_else(|e| panic!("create {}: {e}", out.display())),
    );
    let files = org_files(&root);
    assert!(!files.is_empty(), "no org files under {}", root.display());
    let mut blocks = 0usize;
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap().display().to_string();
        let source = std::fs::read_to_string(path).unwrap();
        let parsed = match parse_org_file(path, &source, &EntityUri::no_parent(), &root) {
            Ok(p) => p,
            Err(e) => {
                let line = serde_json::json!({ "file": rel, "error": format!("{e:#}") });
                writeln!(sink, "{line}").unwrap();
                continue;
            }
        };
        let file_drawer: BTreeMap<String, String> =
            holon_org_format::parser::parse_file_drawer_from_content(&source)
                .unwrap_or_default()
                .into_iter()
                .collect();
        writeln!(
            sink,
            "{}",
            serde_json::json!({ "file": rel, "file_drawer": file_drawer })
        )
        .unwrap();
        for (index, block) in parsed.blocks.iter().enumerate() {
            let props: BTreeMap<String, String> = block.drawer_properties().into_iter().collect();
            let line = serde_json::json!({
                "file": rel,
                "index": index,
                "title": block.content.lines().next().unwrap_or(""),
                "tags": block.tags.to_vec(),
                "props": props,
            });
            writeln!(sink, "{line}").unwrap();
            blocks += 1;
        }
    }
    sink.flush().unwrap();
    eprintln!("dumped {} files, {blocks} blocks", files.len());
    assert!(blocks > 0, "the vault parsed into no blocks");
}
