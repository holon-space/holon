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

use std::path::Path;
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

const LATENCY_TARGET_MARKER: &str = r#"target: "holon_latency""#;

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable source dir") {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `holon_latency` events must be emitted at INFO or above.
///
/// The turso fork's `workspace-hack` enables `tracing/release_max_level_info`,
/// which feature-unifies across the whole graph: a `debug!`/`trace!` callsite
/// is compiled OUT of every release binary. A `holon_latency` event below INFO
/// therefore cannot reach `LatencySloLayer` — nor the log — in the release
/// build Martin dogfoods, whatever `HOLON_LATENCY_SLO` says. Default log
/// volume is held by the `holon_latency` EnvFilter directive in
/// `holon_frontend::logging`, not by the callsite level.
#[test]
fn latency_events_are_emitted_above_the_release_level_ceiling() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    rust_sources(&root.join("frontends"), &mut files);

    let mut offenders = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).expect("readable rust source");
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains(LATENCY_TARGET_MARKER) || line.trim_start().starts_with("//") {
                continue;
            }
            let window = lines[i.saturating_sub(3)..=i].join("\n");
            let sub_info = ["debug!(", "trace!(", "Level::DEBUG", "Level::TRACE"];
            if sub_info.iter().any(|m| window.contains(m)) {
                offenders.push(format!("{}:{}", file.display(), i + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these `holon_latency` events are emitted below INFO, so \
         `tracing/release_max_level_info` (from the turso workspace-hack) \
         compiles them out of every release build and the latency-SLO oracle \
         goes silent there:\n{}\n",
        offenders.join("\n"),
    );
}

/// Every `EnvFilter` in the workspace must suppress `holon_latency`.
///
/// The stage events are INFO, and `holon=info` prefix-matches `holon_latency`,
/// so a self-built filter silently turns a normal run into hundreds of timing
/// lines. The one definition that gets this right is
/// `holon_frontend::logging::env_filter_with_default`; a site that cannot call
/// it must name the target in its own spec, or carry
/// `ALLOW(latency-filter): <reason>`.
#[test]
fn latency_target_is_suppressed_by_every_filter_builder() {
    let root = repo_root();
    let owner = root.join("crates/holon-frontend/src/logging.rs");
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    rust_sources(&root.join("frontends"), &mut files);

    let discharges = [
        "env_filter_with_default",
        "logging::env_filter",
        "holon_latency",
        "ALLOW(latency-filter)",
    ];
    let mut offenders = Vec::new();
    for file in files.iter().filter(|f| **f != owner) {
        // Scope: subscribers whose output a user sees (`src/`, `examples/`). A
        // `tests/` binary's filter governs only that test's own output.
        if file.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let src = std::fs::read_to_string(file).expect("readable rust source");
        if !src.contains("EnvFilter::") {
            continue;
        }
        if !discharges.iter().any(|d| src.contains(d)) {
            offenders.push(file.display().to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "these files build their own `EnvFilter` without suppressing the \
         `holon_latency` target, so the INFO stage events land in their \
         default output — route them through \
         `holon_frontend::logging::env_filter_with_default`:\n{}\n",
        offenders.join("\n"),
    );
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
        //  * cargo's "specification `proptest` is ambiguous" — emitted only when >1
        //    `proptest` source is reachable in this graph.
        // Absence signals (both fine): empty stdout with a "nothing to print"
        // warning, or "did not match any packages".
        let linked = stdout.contains("proptest v");
        let ambiguous = stderr.contains("is ambiguous");
        assert!(
            !linked && !ambiguous,
            "`proptest` is back in the {pkg} production (`-e normal`) build graph.\nThis means \
             the hakari gpui `traversal-excludes` regressed (likely a \n`cargo hakari generate` \
             that re-added gpui to workspace-hack).\nRe-apply the exclude in \
             `.config/hakari.toml` and regenerate.\n\n=== cargo tree -p {pkg} -e normal -i \
             proptest ===\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}\n",
        );
    }
}
