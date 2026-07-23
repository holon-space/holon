//! Vault shape-profile extractor.
//!
//! Walks a vault directory of `.org` files locally and emits an ANONYMIZED
//! structural profile (counts + distributions only, ZERO strings from the
//! vault). The emitted JSON is the environment-parity input the keystone +
//! windowed PBT generators read (`HOLON_PBT_SHAPE_PROFILE`).
//!
//! Run:
//!   cargo run -p holon-tools --bin vault-shape-profile -- \
//!     --vault /path/to/vault --out profile.json
//!
//! The vault is READ-ONLY input; nothing is written back to it.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use holon_api::entity_uri::EntityUri;
use holon_api::inline_mark::InlineMark;
use holon_api::vault_shape::BucketHistogram;
use holon_api::vault_shape::Histogram;
use holon_api::vault_shape::VAULT_SHAPE_SCHEMA_VERSION;
use holon_api::vault_shape::VaultShapeProfile;
use holon_org_format::parser::parse_doc_id;
use holon_org_format::parser::parse_org_file;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(about = "Emit an anonymized structural profile of a vault (counts only)")]
struct Args {
    /// Vault root directory to walk (read-only).
    #[arg(long)]
    vault: PathBuf,
    /// Output path for the profile JSON. Prints to stdout when omitted.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let profile = extract_profile(&args.vault)?;
    let json = profile.to_json();
    match args.out {
        Some(path) => {
            std::fs::write(&path, &json)
                .with_context(|| format!("writing profile to {}", path.display()))?;
            eprintln!(
                "wrote profile ({} files) to {}",
                profile.file_count,
                path.display()
            );
        }
        None => print!("{json}"),
    }
    Ok(())
}

/// Accumulator kept separate from the emitted profile so distinct-value SETS
/// (tag/property keys) stay internal — only their `.len()` is ever emitted, so
/// no vault string escapes.
#[derive(Default)]
struct Acc {
    file_count: u64,
    companion_pair_count: u64,
    files_without_docid: u64,
    total_blocks: u64,
    idless_blocks: u64,
    blocks_per_file: Histogram,
    depth: Histogram,
    sibling_count: Histogram,
    content_length: BucketHistogram,
    links_per_block: Histogram,
    tag_usage: u64,
    property_usage: u64,
    distinct_tags: BTreeSet<String>,
    distinct_properties: BTreeSet<String>,
}

impl Acc {
    fn finish(self) -> VaultShapeProfile {
        let ratio = |num: u64, den: u64| {
            if den == 0 {
                0.0
            } else {
                num as f64 / den as f64
            }
        };
        VaultShapeProfile {
            schema_version: VAULT_SHAPE_SCHEMA_VERSION,
            file_count: self.file_count,
            companion_pair_count: self.companion_pair_count,
            idless_file_ratio: ratio(self.files_without_docid, self.file_count),
            idless_block_ratio: ratio(self.idless_blocks, self.total_blocks),
            blocks_per_file: self.blocks_per_file,
            depth: self.depth,
            sibling_count: self.sibling_count,
            content_length: self.content_length,
            links_per_block: self.links_per_block,
            tag_usage: self.tag_usage,
            distinct_tag_count: self.distinct_tags.len() as u64,
            property_usage: self.property_usage,
            distinct_property_count: self.distinct_properties.len() as u64,
        }
    }
}

/// Walk `root` and build the structural profile. Deterministic (files walked in
/// sorted order; every distribution is a sorted `BTreeMap`).
pub fn extract_profile(root: &Path) -> Result<VaultShapeProfile> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing vault root {}", root.display()))?;
    let mut acc = Acc::default();

    for entry in WalkDir::new(&root).sort_by_file_name() {
        let entry = entry?;
        let path = entry.path();
        // Companion pairs: a directory that has a same-name `.org` beside it.
        if entry.file_type().is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let companion = path.with_file_name(format!("{name}.org"));
                if companion.is_file() {
                    acc.companion_pair_count += 1;
                }
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        ingest_file(&mut acc, path, &content, &root);
    }

    Ok(acc.finish())
}

fn ingest_file(acc: &mut Acc, path: &Path, content: &str, root: &Path) {
    acc.file_count += 1;
    if parse_doc_id(content).is_none() {
        acc.files_without_docid += 1;
    }

    let parent_dir = EntityUri::file(
        path.parent()
            .and_then(|p| p.strip_prefix(root).ok()) // ALLOW(ok): outside-root path yields empty parent-dir id, never used in stats
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
            .as_str(),
    );
    let parsed = match parse_org_file(path, content, &parent_dir, root) {
        Ok(p) => p,
        // A single malformed file must not abort the whole vault scan; count it
        // as an empty file and move on (disclosed via stderr).
        Err(e) => {
            eprintln!("skip (parse error) {}: {e:#}", path.display());
            acc.blocks_per_file.record(0);
            return;
        }
    };

    acc.blocks_per_file.record(parsed.blocks.len() as u32);
    acc.total_blocks += parsed.blocks.len() as u64;
    acc.idless_blocks += parsed.headlines_needing_ids.len() as u64;

    // Depth + sibling counts from the parent chain. The document block is the
    // root (depth 0); a block whose parent is the document is depth 1.
    let doc_id = parsed.document.id.to_string();
    let mut parent_of: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut children_of: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for b in &parsed.blocks {
        parent_of.insert(b.id.to_string(), b.parent_id.to_string());
        *children_of.entry(b.parent_id.to_string()).or_insert(0) += 1;
    }
    for b in &parsed.blocks {
        acc.depth
            .record(depth_of(&b.id.to_string(), &doc_id, &parent_of));

        acc.content_length.record(b.content.chars().count() as u32);

        let link_count = b
            .marks
            .as_ref()
            .map(|marks| {
                marks
                    .iter()
                    .filter(|m| matches!(m.mark, InlineMark::Link { .. }))
                    .count() as u32
            })
            .unwrap_or(0);
        acc.links_per_block.record(link_count);

        for tag in b.tags.iter() {
            if tag == "Page" {
                continue;
            }
            acc.tag_usage += 1;
            acc.distinct_tags.insert(tag.clone());
        }
        for key in b.properties.keys() {
            if key == "ID" {
                continue;
            }
            acc.property_usage += 1;
            acc.distinct_properties.insert(key.clone());
        }
    }
    // Sibling fan-out: every parent that actually has children (includes the
    // document root, i.e. the file's top-level fan-out).
    for count in children_of.values() {
        acc.sibling_count.record(*count);
    }
}

/// Depth of `id` measured from the document root. Walks the parent chain,
/// capped so a malformed cycle can't loop forever.
fn depth_of(id: &str, doc_id: &str, parent_of: &std::collections::HashMap<String, String>) -> u32 {
    let mut depth = 0u32;
    let mut cur = id.to_string();
    while cur != *doc_id {
        depth += 1;
        match parent_of.get(&cur) {
            Some(parent) if *parent != cur => cur = parent.clone(),
            // Parent not in this file (or self-loop): stop; it sits directly
            // under whatever root we could resolve.
            _ => break,
        }
        if depth > 64 {
            break;
        }
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny SYNTHETIC vault in a temp dir (never derived from any real
    /// vault) and assert the extracted shape matches the known topology.
    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn extracts_known_synthetic_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // File A: doc id, two top-level headings, one nested under the first.
        write(
            root,
            "a.org",
            "#+ID: doc-a\n#+TITLE: A\n\
             * First :work:\n:PROPERTIES:\n:ID: a1\n:PRIORITY: A\n:END:\n\
             ** Child of first\n:PROPERTIES:\n:ID: a2\n:END:\n\
             * Second\n:PROPERTIES:\n:ID: a3\n:END:\n",
        );
        // File B: NO doc id, one heading WITHOUT an :ID: (idless block).
        write(root, "b.org", "* Lonely heading\nbody text here\n");
        // Companion: dir `Area/` next to `Area.org`.
        write(
            root,
            "Area.org",
            "#+ID: doc-area\n* In area\n:PROPERTIES:\n:ID: ar1\n:END:\n",
        );
        write(
            root,
            "Area/note.org",
            "#+ID: doc-note\n* Note heading\n:PROPERTIES:\n:ID: n1\n:END:\n",
        );

        let p = extract_profile(root).unwrap();

        assert_eq!(p.schema_version, VAULT_SHAPE_SCHEMA_VERSION);
        assert_eq!(p.file_count, 4, "a, b, Area, Area/note");
        assert_eq!(p.companion_pair_count, 1, "Area/ has Area.org companion");

        // b.org has no #+ID → 1 of 4 files idless.
        assert!(
            (p.idless_file_ratio - 0.25).abs() < 1e-9,
            "{}",
            p.idless_file_ratio
        );

        // b.org's single heading lacks :ID: → at least one idless block.
        assert!(p.idless_block_ratio > 0.0);

        // a.org has depth-2 (the nested child). Max depth must reach >=2.
        assert!(p.depth.max_value() >= 2, "depth {:?}", p.depth.counts);

        // a.org: :work: tag once; a1 has :PRIORITY: property once.
        assert!(p.tag_usage >= 1);
        assert!(p.property_usage >= 1);
        assert!(p.distinct_tag_count >= 1);

        // Blocks-per-file distribution has been populated for all 4 files.
        assert_eq!(p.blocks_per_file.total(), 4);

        // JSON is deterministic + carries no obvious vault strings.
        let json = p.to_json();
        assert!(!json.contains("Lonely"), "no content leaked: {json}");
        assert!(!json.contains("doc-a"), "no id leaked");
        assert!(!json.contains("work"), "no tag string leaked");
        // Byte-stable across two extractions of the same vault.
        assert_eq!(json, extract_profile(root).unwrap().to_json());
    }
}
