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
            "archlint --all reported architecture violations.\nExit code: {:?}\n\n=== stderr \
             ===\n{}\n=== stdout ===\n{}\n",
            output.status.code(),
            stderr,
            stdout,
        );
    }
}
