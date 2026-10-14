//! The #167 contract docs name executor-side symbols in backticks, not as
//! intra-doc links, because the declaring crate does not depend on the crate
//! that owns them. Rustdoc therefore cannot catch the drift; this test does.
//!
//! The scan set is keyed on the contract SYMBOLS, not on a file list: a root
//! may be a directory, and `every_contract_symbol_has_its_doc_in_the_scan_set`
//! fails when a symbol's doc moves out of the scanned roots.

use std::path::Path;
use std::path::PathBuf;

/// The files and directories whose backticked forward references must resolve.
/// A directory root covers every `.rs` file under it, so a new contract module
/// is scanned the day it is created.
const CONTRACT_DOC_ROOTS: [&str; 4] = [
    "crates/holon-api/src/user_transition.rs",
    "crates/holon-net/src/uft.rs",
    "crates/holon-net/src/uft",
    "crates/holon-net/src/exec.rs",
];

/// The cross-crate symbols the contract documents. Their declaring file must
/// lie inside [`CONTRACT_DOC_ROOTS`].
const CONTRACT_SYMBOLS: [&str; 3] = ["work_occurrence", "ask_hash", "lower"];

/// The crates that declare the contract and its executor side.
const CONTRACT_CRATES: [&str; 2] = ["holon-api", "holon-net"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the manifest sits at <root>/crates/<crate>")
        .to_path_buf()
}

fn src_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    for entry in std::fs::read_dir(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    {
        let entry = entry.expect("a readable directory entry").path();
        if entry.is_dir() || entry.extension().is_some_and(|ext| ext == "rs") {
            src_files(&entry, out);
        }
    }
}

fn scan_set(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for doc_root in CONTRACT_DOC_ROOTS {
        let path = root.join(doc_root);
        assert!(
            path.exists(),
            "contract doc root {doc_root} does not exist — update CONTRACT_DOC_ROOTS"
        );
        src_files(&path, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The backticked code spans of `text`'s doc comments that look like a call
/// through a path (`a::b(..)`), reduced to their path.
fn forward_references(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("///") || line.starts_with("//!"))
        .flat_map(|line| {
            line.split('`')
                .skip(1)
                .step_by(2)
                .filter(|span| span.contains("::") && span.contains('('))
                .map(|span| {
                    span.split('(')
                        .next()
                        .expect("split always yields one part")
                        .to_string()
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `Ok` when `path` resolves: its first segment names a workspace crate or a
/// module of `from_crate`, and its last segment is a `fn` in that crate.
fn resolve(root: &Path, from_crate: &str, path: &str) -> Result<(), String> {
    let segments: Vec<&str> = path.split("::").collect();
    let first = segments[0];
    let last = segments[segments.len() - 1];
    let as_crate = root.join("crates").join(first.replace('_', "-"));
    let owner = if as_crate.is_dir() {
        as_crate
    } else {
        let own = root.join("crates").join(from_crate);
        if own.join("src").join(format!("{first}.rs")).is_file()
            || own.join("src").join(first).is_dir()
        {
            own
        } else {
            return Err(format!(
                "`{path}`: `{first}` is neither a workspace crate nor a module of `{from_crate}` \
                 — name the owning crate so a reader can follow the reference"
            ));
        }
    };
    let src = owner.join("src");
    let mut files = Vec::new();
    src_files(&src, &mut files);
    let needle = format!("fn {last}(");
    if files.iter().any(|file| read(file).contains(&needle)) {
        return Ok(());
    }
    Err(format!("`{path}`: no `{needle}` under {}", src.display()))
}

/// The crate a scanned file belongs to, from its `crates/<crate>/` prefix.
fn owning_crate(root: &Path, file: &Path) -> String {
    file.strip_prefix(root.join("crates"))
        .unwrap_or_else(|e| panic!("{} is not under crates/: {e}", file.display()))
        .components()
        .next()
        .expect("a path under crates/ has a first component")
        .as_os_str()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn every_forward_reference_in_the_contract_names_a_real_symbol() {
    let root = workspace_root();
    let mut checked = 0usize;
    let mut unresolved = Vec::new();
    for file in scan_set(&root) {
        let from_crate = owning_crate(&root, &file);
        for reference in forward_references(&read(&file)) {
            checked += 1;
            if let Err(why) = resolve(&root, &from_crate, &reference) {
                unresolved.push(format!("{}: {why}", file.display()));
            }
        }
    }
    assert!(
        unresolved.is_empty(),
        "forward references that name no real symbol:\n{}",
        unresolved.join("\n")
    );
    assert!(
        checked > 0,
        "the scanner found no backticked `a::b(..)` reference under {CONTRACT_DOC_ROOTS:?} — \
         either the docs changed shape or the scanner is broken"
    );
}

#[test]
fn every_contract_symbol_has_its_doc_in_the_scan_set() {
    let root = workspace_root();
    let scanned = scan_set(&root);
    let mut declarations = Vec::new();
    for crate_name in CONTRACT_CRATES {
        src_files(
            &root.join("crates").join(crate_name).join("src"),
            &mut declarations,
        );
    }
    let mut missing = Vec::new();
    for symbol in CONTRACT_SYMBOLS {
        let needle = format!("pub fn {symbol}(");
        let declaring: Vec<&PathBuf> = declarations
            .iter()
            .filter(|file| read(file).contains(&needle))
            .collect();
        assert!(
            !declaring.is_empty(),
            "no `{needle}` in {CONTRACT_CRATES:?} — the contract symbol was renamed or removed, \
             so CONTRACT_SYMBOLS is stale"
        );
        for file in declaring {
            if !scanned.contains(file) {
                missing.push(format!(
                    "the doc of `{symbol}` lives in {}, which no root in CONTRACT_DOC_ROOTS covers",
                    file.display()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "contract symbols whose doc the scanner never reads:\n{}",
        missing.join("\n")
    );
}
