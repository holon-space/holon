//! **Guard: no `*.proptest-regressions` seed file exists in the repo tree.**
//!
//! Proptest's default `failure_persistence` auto-writes a seed file next to
//! the failing test on every red run. Every `Config`/`ProptestConfig`
//! construction in this repo sets `failure_persistence: None` to suppress
//! that, and `.gitignore` excludes the pattern — but a future suite that
//! forgets the field would otherwise silently re-introduce seed-file churn.
//! Permanent regression guards belong in the hand-authored replay corpus
//! (`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`),
//! not an auto-accumulated seed file.
//!
//! Walks the filesystem (not `git`/`jj`) so the guard holds regardless of
//! VCS state — a jj workspace's git index can lag its working copy.

use std::path::Path;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("archlint").is_dir() && dir.join("Cargo.toml").is_file() {
            return dir;
        }
        assert!(
            dir.pop(),
            "walked past filesystem root looking for the repo root"
        );
    }
}

fn is_offender(name: &str) -> bool {
    name.ends_with(".proptest-regressions")
}

fn walk(dir: &Path, offenders: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            if name == "proptest-regressions" {
                for txt in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                    if txt.path().extension().is_some_and(|e| e == "txt") {
                        offenders.push(txt.path());
                    }
                }
                continue;
            }
            walk(&path, offenders);
        } else if is_offender(&name) {
            offenders.push(path);
        }
    }
}

#[test]
fn no_proptest_regressions_files_exist() {
    let root = repo_root();
    let mut offenders = Vec::new();
    walk(&root, &mut offenders);
    assert!(
        offenders.is_empty(),
        "proptest auto-persisted seed file(s) found on disk:\n{}\n\
         Set `failure_persistence: None` on the offending suite's Config and \
         `rm` the file — permanent regressions belong in \
         hand-authored-regressions/keystone.jsonl, not an auto-written seed file.",
        offenders
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
