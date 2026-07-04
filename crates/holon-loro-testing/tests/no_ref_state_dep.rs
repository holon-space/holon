//! Plan §4 constraint 1 guard: `holon-loro-testing` must reference `Ref*` only
//! through `holon-pbt-core` capability traits — never the central concrete
//! `ReferenceState`, and never depend on `holon-integration-tests` (a
//! dependency cycle AND coupling co-location to the ref monolith). Fails loud
//! (grep over this crate's own CODE — comments stripped — plus its manifest).

use std::fs;
use std::path::Path;

/// Drop line comments so a doc/comment mention of the blocked type (e.g. the
/// module doc explaining the Phase-1a blocker) is not a false positive; only a
/// real code reference should trip the guard.
fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn walk_rs(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk_rs(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push((
                path.display().to_string(),
                fs::read_to_string(&path).unwrap(),
            ));
        }
    }
}

#[test]
fn no_concrete_reference_state_reach() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mut files = Vec::new();
    walk_rs(&Path::new(manifest_dir).join("src"), &mut files);

    for (path, src) in &files {
        assert!(
            !strip_comments(src).contains("ReferenceState"),
            "{path} references the concrete central `ReferenceState` in code; co-located \
             invariants must read `Ref*` capability traits only (plan §4)."
        );
    }

    let manifest = fs::read_to_string(Path::new(manifest_dir).join("Cargo.toml")).unwrap();
    assert!(
        !manifest.contains("holon-integration-tests"),
        "holon-loro-testing must NOT depend on holon-integration-tests (dependency cycle + \
         ref-monolith coupling)."
    );
}
