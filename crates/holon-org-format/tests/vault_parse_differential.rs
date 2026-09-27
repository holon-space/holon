//! Dumps what the parser reads from every org file of a vault: per block its
//! title, body, tags and drawer properties, one JSON line each. Two dumps taken
//! with two builds of this crate diff into the exact set of items a parser
//! change moves.
//!
//! Ignored by default (needs a vault). The vault is only read:
//!
//! ```text
//! HOLON_VAULT_SIM=/path/to/vault HOLON_VAULT_DUMP=/tmp/parse.jsonl \
//!   cargo test -p holon-org-format --test vault_parse_differential -- --ignored
//! ```
//!
//! `write_rules_census` counts, without printing any content, the ids and
//! keys of a vault that the write rules (`DrawerId`, `ValueCarrier::key`)
//! would refuse.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use holon_api::EntityUri;
use holon_org_format::OrgBlockExt;
use holon_org_format::parse_org_file;

/// Every org file under `root`. `hidden` also walks dot-directories other
/// than version-control stores.
fn org_files(root: &Path, hidden: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
            .flatten()
        {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && (!hidden || name == ".git" || name == ".jj") {
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
    let files = org_files(&root, false);
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
            serde_json::json!({
                "file": rel,
                "file_drawer": file_drawer,
                "doc_id": parsed.document.id.as_str(),
                "text": parsed.document.content,
                "blank_lines": parsed.document.get_property("_blank_lines"),
            })
        )
        .unwrap();
        for (index, block) in parsed.blocks.iter().enumerate() {
            let props: BTreeMap<String, String> = block.drawer_properties().into_iter().collect();
            let line = serde_json::json!({
                "file": rel,
                "index": index,
                "title": block.content.lines().next().unwrap_or(""),
                "body": block.content.split_once('\n').map(|(_, body)| body),
                "tags": block.tags.to_vec(),
                "props": props,
                "marks": block.marks,
                "blank_lines": block.get_property("_blank_lines"),
            });
            writeln!(sink, "{line}").unwrap();
            blocks += 1;
        }
    }
    sink.flush().unwrap();
    eprintln!("dumped {} files, {blocks} blocks", files.len());
    assert!(blocks > 0, "the vault parsed into no blocks");
}

#[test]
#[ignore = "needs HOLON_VAULT_SIM"]
fn write_rules_census() {
    use holon_org_format::DrawerId;
    use holon_org_format::ValueCarrier;

    let root = PathBuf::from(std::env::var("HOLON_VAULT_SIM").expect("HOLON_VAULT_SIM"));
    let files = org_files(&root, true);
    assert!(!files.is_empty(), "no org files under {}", root.display());
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        for line in source.lines() {
            if let Some(id) = line.trim().strip_prefix("#+ID:") {
                *counts.entry("#+ID: lines").or_default() += 1;
                if EntityUri::schemed(id.trim()).is_some() {
                    *counts.entry("#+ID: lines that name a scheme").or_default() += 1;
                }
            }
        }
        let parsed = match parse_org_file(path, &source, &EntityUri::no_parent(), &root) {
            Ok(p) => p,
            Err(_) => {
                *counts.entry("files refused by the parser").or_default() += 1;
                continue;
            }
        };
        *counts.entry("files").or_default() += 1;
        for block in std::iter::once(&parsed.document).chain(&parsed.blocks) {
            if !block.id.is_block() {
                *counts.entry("ids with a non-block scheme").or_default() += 1;
                continue;
            }
            *counts.entry("block ids").or_default() += 1;
            if DrawerId::parse(block.id.id()).is_err() {
                *counts.entry("block ids DrawerId refuses").or_default() += 1;
            }
        }
        for block in &parsed.blocks {
            let carrier = if block.content_type == holon_api::ContentType::Source {
                ValueCarrier::HeaderArg
            } else {
                ValueCarrier::HeadlineDrawer
            };
            for key in block.drawer_properties().keys() {
                *counts.entry("block keys").or_default() += 1;
                if carrier.key(key).is_err() {
                    *counts
                        .entry("block keys their carrier refuses")
                        .or_default() += 1;
                }
            }
        }
        for (key, value) in
            holon_org_format::parser::parse_file_drawer_from_content(&source).unwrap_or_default()
        {
            *counts.entry("file-drawer keys").or_default() += 1;
            if key.eq_ignore_ascii_case("ID") {
                if !value.is_empty() && DrawerId::parse(&value).is_err() {
                    *counts
                        .entry("file-drawer ids DrawerId refuses")
                        .or_default() += 1;
                }
            } else if ValueCarrier::FileDrawer.key(&key).is_err() {
                *counts
                    .entry("file-drawer keys FileDrawer refuses")
                    .or_default() += 1;
            }
        }
    }
    eprintln!("write-rules census: {counts:?}");
}
