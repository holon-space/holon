//! Architecture rules — thin wrapper around `archlint`.
//!
//! All rule logic lives in `archlint/` at the repo root (Python + ast-grep
//! YAML + ripgrep TOML smells). This test just shells out to it so the rules
//! also run from `cargo test --workspace` (CI integration).
//!
//! See `devlog/2026-05-05-archlint-prototype.md` for the rule catalogue and
//! parity matrix. To suppress a rule on a specific line, add
//! `// ALLOW(<tag>): <reason>` (the accepted tag is documented in each
//! rule's diagnostic message).

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root above crates/holon-architecture-tests")
        .to_path_buf()
}

#[test]
fn archlint_all_passes() {
    let root = repo_root();
    let archlint = root.join("archlint").join("archlint");
    assert!(
        archlint.exists(),
        "archlint script not found at {} — repo layout broken",
        archlint.display(),
    );

    let output = Command::new(&archlint)
        .arg("--all")
        .current_dir(&root)
        .output()
        .expect("failed to spawn archlint");

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "archlint --all reported architecture violations.\n\
             Exit code: {:?}\n\n\
             === stderr ===\n{}\n\
             === stdout ===\n{}\n",
            output.status.code(),
            stderr,
            stdout,
        );
    }
}

/// Regression guard for the hakari `traversal-excludes` on `gpui`
/// (see `.config/hakari.toml`).
///
/// `gpui` is a dev-only, `test-support`-flavoured dependency of
/// `frontends/gpui`. Before the exclude, hakari feature-unification promoted it
/// (plus the `proptest` git fork it drags) into `workspace-hack` as a *normal*
/// dependency, pulling the whole GPUI framework and `proptest` into the
/// production build graphs of `holon-tui`, `holon-mcp` and the storage
/// adapters. This asserts `proptest` is absent from those prod (`-e normal`)
/// graphs so a future `cargo hakari generate` can't silently re-introduce it.
#[test]
fn no_proptest_in_prod_graph() {
    let root = repo_root();
    for pkg in ["holon-tui", "holon-mcp"] {
        let output = Command::new(env!("CARGO"))
            .args(["tree", "-p", pkg, "-e", "normal", "-i", "proptest"])
            .current_dir(&root)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn `cargo tree` for {pkg}: {e}"));

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Presence signals:
        //  * an inverted tree rooted at `proptest v…` on stdout, or
        //  * cargo's "specification `proptest` is ambiguous" — emitted only
        //    when >1 `proptest` source is reachable in this graph.
        // Absence signals (both fine): empty stdout with a "nothing to print"
        // warning, or "did not match any packages".
        let linked = stdout.contains("proptest v");
        let ambiguous = stderr.contains("is ambiguous");
        assert!(
            !linked && !ambiguous,
            "`proptest` is back in the {pkg} production (`-e normal`) build graph.\n\
             This means the hakari gpui `traversal-excludes` regressed (likely a \n\
             `cargo hakari generate` that re-added gpui to workspace-hack).\n\
             Re-apply the exclude in `.config/hakari.toml` and regenerate.\n\n\
             === cargo tree -p {pkg} -e normal -i proptest ===\n\
             --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}\n",
        );
    }
}
